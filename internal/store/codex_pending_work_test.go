package store

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

func TestListCodexPendingWorkReturnsOnlyRunnableDurableTargets(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	directDirectory := t.TempDir()
	direct, err := s.CreateNamedAgent(ctx, "direct", "")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.SelectNamedAgentSession(ctx, direct.Name, model.SessionIdentity{Harness: "codex", ExternalSessionID: "direct-thread"}, model.RepositoryContext{Directory: directDirectory, Branch: "main"}); err != nil {
		t.Fatal(err)
	}
	directMessage := model.Message{ID: "019c0000-0000-7000-8000-000000000501", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: direct.MailboxID, Body: "direct work", CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, directMessage); err != nil {
		t.Fatal(err)
	}

	if _, err := s.CreateNamedAgent(ctx, "project", ""); err != nil {
		t.Fatal(err)
	}
	projectDirectory := t.TempDir()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "pending project", Open: true, Paths: []domain.ProjectPathInput{{DisplayPath: projectDirectory}}})
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.AssignProject(ctx, project.ID, project.HeadEventID, "project")
	if err != nil {
		t.Fatal(err)
	}
	project, err = s.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "project-thread", LaunchDirectory: projectDirectory})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Create(ctx, model.Message{ID: "019c0000-0000-7000-8000-000000000502", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: "project work", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}

	work, err := s.ListCodexPendingWork(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(work) != 2 {
		t.Fatalf("pending work = %#v", work)
	}
	if work[0].Kind != domain.CodexPendingDirect || work[0].AgentName != direct.Name || work[0].SessionID != "direct-thread" || work[0].Repository.Directory != directDirectory || work[0].Repository.Branch != "main" {
		t.Fatalf("direct pending work = %#v", work[0])
	}
	if work[1].Kind != domain.CodexPendingProject || work[1].ProjectID != project.ID || work[1].AssignmentID != project.Assignment.ID || work[1].ProjectThreadID != project.Assignment.SelectedThreadID || work[1].SessionID != "project-thread" || work[1].Repository.Directory != projectDirectory {
		t.Fatalf("project pending work = %#v", work[1])
	}

	project, err = s.GetProject(ctx, project.ID)
	if err != nil {
		t.Fatal(err)
	}
	closing, err := s.BeginCloseProject(ctx, project.ID, project.HeadEventID)
	if err != nil {
		t.Fatal(err)
	}
	work, err = s.ListCodexPendingWork(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(work) != 1 || work[0].Kind != domain.CodexPendingDirect {
		t.Fatalf("closing project remained runnable: %#v (closing=%#v)", work, closing)
	}

	claimed, err := s.Claim(ctx, domain.Claim{MessageID: directMessage.ID, RecipientMailboxID: direct.MailboxID}, "owner")
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Complete(ctx, claimed.ID, "owner"); err != nil {
		t.Fatal(err)
	}
	work, err = s.ListCodexPendingWork(ctx)
	if err != nil || len(work) != 0 {
		t.Fatalf("completed/non-runnable work = %#v, %v", work, err)
	}
}
