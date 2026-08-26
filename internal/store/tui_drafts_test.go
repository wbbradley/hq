package store

import (
	"context"
	"errors"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

func TestTUIDraftRoundTripOptimisticVersionAndRestart(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, database)
	ctx := context.Background()
	var canonicalBefore int
	if err := s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&canonicalBefore); err != nil {
		t.Fatal(err)
	}
	draft := domain.TUIDraft{
		ID: "019c0000-0000-7000-8000-000000000901", Body: "unsent", RecipientMailboxID: "019c0000-0000-7000-8000-000000000902",
		RecipientLabel: "alice", RecipientNamed: true, RecipientAddress: model.MessageAddress{MailboxID: "019c0000-0000-7000-8000-000000000902", Kind: model.MailboxAgent, Label: "alice"},
		Repository: model.RepositoryContext{Directory: "/repo", Branch: "drafts"},
		Activation: &domain.ProjectActivationIntent{ProjectID: "019c0000-0000-7000-8000-000000000903", AgentName: "alice", Harness: "codex", Directory: "/repo"},
	}
	stored, err := s.PutTUIDraft(ctx, draft)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Version != 1 || stored.CreatedAt.IsZero() || stored.UpdatedAt.IsZero() {
		t.Fatalf("stored draft = %#v", stored)
	}
	stale := stored
	stored.Body = "edited"
	updated, err := s.PutTUIDraft(ctx, stored)
	if err != nil || updated.Version != 2 || updated.Body != "edited" || !reflect.DeepEqual(updated.Activation, draft.Activation) {
		t.Fatalf("updated draft = %#v, %v", updated, err)
	}
	if _, err := s.PutTUIDraft(ctx, stale); !errors.Is(err, domain.ErrTUIDraftConflict) {
		t.Fatalf("stale update = %v", err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	s, err = Open(database)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	drafts, err := s.ListTUIDrafts(ctx)
	if err != nil || len(drafts) != 1 || drafts[0].ID != updated.ID || drafts[0].Version != 2 || drafts[0].Repository != draft.Repository {
		t.Fatalf("reopened drafts = %#v, %v", drafts, err)
	}
	var canonicalAfter int
	if err := s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&canonicalAfter); err != nil {
		t.Fatal(err)
	}
	if canonicalAfter != canonicalBefore {
		t.Fatalf("unsigned draft changed canonical count %d→%d", canonicalBefore, canonicalAfter)
	}
	if err := s.DeleteTUIDraft(ctx, updated.ID, 1); !errors.Is(err, domain.ErrTUIDraftConflict) {
		t.Fatalf("stale delete = %v", err)
	}
	if err := s.DeleteTUIDraft(ctx, updated.ID, 2); err != nil {
		t.Fatal(err)
	}
}

func TestOpenAddsLocalDraftTableToExistingSchema33(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, database)
	if _, err := s.db.Exec(`DROP TABLE tui_drafts`); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	reopened, err := Open(database)
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	draft, err := reopened.PutTUIDraft(context.Background(), domain.TUIDraft{
		ID: "019c0000-0000-7000-8000-000000000904", Body: "same-schema upgrade", RecipientMailboxID: "recipient",
	})
	if err != nil || draft.Version != 1 {
		t.Fatalf("draft after schema-33 reopen = %#v, %v", draft, err)
	}
}

func TestSubmitTUIDraftAtomicallyCreatesAndConsumes(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "draft-submit", "/repo")
	draft, err := s.PutTUIDraft(ctx, domain.TUIDraft{ID: "019c0000-0000-7000-8000-000000000911", Body: "send once", RecipientMailboxID: agent.ID, RecipientLabel: agent.Label, Repository: model.RepositoryContext{Directory: "/repo"}})
	if err != nil {
		t.Fatal(err)
	}
	submitted, err := s.SubmitTUIDraft(ctx, draft.ID, draft.Version)
	if err != nil || submitted.MessageID != draft.ID {
		t.Fatalf("submission = %#v, %v", submitted, err)
	}
	message, err := s.Get(ctx, draft.ID)
	if err != nil || message.Body != "send once" || message.ID != draft.ID {
		t.Fatalf("submitted message = %#v, %v", message, err)
	}
	drafts, err := s.ListTUIDrafts(ctx)
	if err != nil || len(drafts) != 0 {
		t.Fatalf("drafts after submit = %#v, %v", drafts, err)
	}
}

func TestSubmitTUIDraftFailureRetainsDraftAndMessageRollback(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "draft-failure", "/repo")
	draft, err := s.PutTUIDraft(ctx, domain.TUIDraft{ID: "019c0000-0000-7000-8000-000000000912", Body: "retain me", RecipientMailboxID: agent.ID, Repository: model.RepositoryContext{Directory: "/repo"}})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.db.Exec(`CREATE TRIGGER fail_draft_message BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'injected draft submit failure'); END`); err != nil {
		t.Fatal(err)
	}
	if _, err := s.SubmitTUIDraft(ctx, draft.ID, draft.Version); err == nil {
		t.Fatal("injected submit failure succeeded")
	}
	if _, err := s.Get(ctx, draft.ID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("rolled-back draft message = %v", err)
	}
	drafts, err := s.ListTUIDrafts(ctx)
	if err != nil || len(drafts) != 1 || drafts[0].ID != draft.ID || drafts[0].Version != draft.Version {
		t.Fatalf("retained drafts = %#v, %v", drafts, err)
	}
}

func TestSubmitTUIDraftAtomicallyCreatesProjectInput(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "draft project"})
	if err != nil {
		t.Fatal(err)
	}
	draft, err := s.PutTUIDraft(ctx, domain.TUIDraft{
		ID: "019c0000-0000-7000-8000-000000000913", Body: "durable project input",
		RecipientMailboxID: project.MailboxID, RecipientLabel: project.Name,
		RecipientAddress: model.MessageAddress{MailboxID: project.MailboxID, Kind: model.MailboxProject, Label: project.Name},
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.SubmitTUIDraft(ctx, draft.ID, draft.Version); err != nil {
		t.Fatal(err)
	}
	message, err := s.Get(ctx, draft.ID)
	if err != nil || message.Purpose != model.MessagePurposeProjectInput || message.RecipientMailboxID != project.MailboxID {
		t.Fatalf("project draft message = %#v, %v", message, err)
	}
}
