package store

import (
	"context"
	"errors"
	"path/filepath"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

func TestNamedAgentCreateAdoptRetireAndPermanentReservation(t *testing.T) {
	ctx := context.Background()
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	for _, name := range []string{"self", "human", "Upper", "-fred", "fred-", "a_b"} {
		if _, err := s.CreateNamedAgent(ctx, name, ""); err == nil {
			t.Fatalf("invalid name %q succeeded", name)
		}
	}
	legacy := resolveAgent(t, s, "codex", "legacy-thread", "/repo")
	agent, err := s.CreateNamedAgent(ctx, "fred", legacy.ID)
	if err != nil || agent.MailboxID != legacy.ID || agent.Name != "fred" {
		t.Fatalf("adopt = %#v, %v", agent, err)
	}
	if _, err := s.CreateNamedAgent(ctx, "jane", legacy.ID); !errors.Is(err, domain.ErrMailboxNamed) {
		t.Fatalf("second adoption = %v", err)
	}
	if err := s.RetireNamedAgent(ctx, "fred"); err != nil {
		t.Fatal(err)
	}
	retired, err := s.GetNamedAgent(ctx, "fred")
	if err != nil || !retired.Retired {
		t.Fatalf("retired = %#v, %v", retired, err)
	}
	if _, err := s.CreateNamedAgent(ctx, "fred", ""); !errors.Is(err, domain.ErrAgentRetired) {
		t.Fatalf("reuse = %v", err)
	}
}

func TestNamedAgentRetainsHistoricalSessionsAndSelectedSession(t *testing.T) {
	ctx := context.Background()
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	agent, err := s.CreateNamedAgent(ctx, "fred", "")
	if err != nil {
		t.Fatal(err)
	}
	for _, sessionID := range []string{"thread-one", "thread-two"} {
		selected, err := s.SelectNamedAgentSession(ctx, "fred", model.SessionIdentity{Harness: "codex", ExternalSessionID: sessionID}, model.RepositoryContext{Directory: "/repo/" + sessionID})
		if err != nil {
			t.Fatal(err)
		}
		if selected.CurrentSessionID != sessionID {
			t.Fatalf("selected session = %q", selected.CurrentSessionID)
		}
	}
	var bindings int
	if err := s.db.QueryRow(`SELECT count(*) FROM harness_bindings WHERE mailbox_id=?`, agent.MailboxID).Scan(&bindings); err != nil || bindings != 2 {
		t.Fatalf("bindings = %d, %v", bindings, err)
	}
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	rebuilt, err := s.GetNamedAgent(ctx, "fred")
	if err != nil || rebuilt.CurrentSessionID != "thread-two" {
		t.Fatalf("rebuilt = %#v, %v", rebuilt, err)
	}
}

func TestNamedAgentOwnershipConflictExpiryRenewalAndRebuild(t *testing.T) {
	ctx := context.Background()
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	now := time.Date(2026, 8, 22, 12, 0, 0, 0, time.UTC)
	s.now = func() time.Time { return now }
	if _, err := s.CreateNamedAgent(ctx, "fred", ""); err != nil {
		t.Fatal(err)
	}
	owned, err := s.AcquireNamedAgent(ctx, "fred", "owner-one", 30*time.Second)
	if err != nil || !owned.Active {
		t.Fatalf("acquire = %#v, %v", owned, err)
	}
	if _, err := s.AcquireNamedAgent(ctx, "fred", "owner-two", 30*time.Second); !errors.Is(err, domain.ErrAgentOwned) {
		t.Fatalf("competing acquire = %v", err)
	}
	now = now.Add(10 * time.Second)
	if _, err := s.RenewNamedAgent(ctx, "fred", "owner-one", 30*time.Second); err != nil {
		t.Fatal(err)
	}
	if _, err := s.SelectNamedAgentSession(ctx, "fred", model.SessionIdentity{Harness: "codex", ExternalSessionID: "thread"}, model.RepositoryContext{Directory: "/repo"}); err != nil {
		t.Fatal(err)
	}
	afterRebuild, err := s.GetNamedAgent(ctx, "fred")
	if err != nil || !afterRebuild.Active {
		t.Fatalf("lease after rebuild = %#v, %v", afterRebuild, err)
	}
	now = now.Add(31 * time.Second)
	if _, err := s.AcquireNamedAgent(ctx, "fred", "owner-two", 30*time.Second); err != nil {
		t.Fatalf("expired takeover = %v", err)
	}
	if err := s.ReleaseNamedAgent(ctx, "fred", "owner-two"); err != nil {
		t.Fatal(err)
	}
	offline, err := s.GetNamedAgent(ctx, "fred")
	if err != nil || offline.Active {
		t.Fatalf("release = %#v, %v", offline, err)
	}
}

func TestNamedAgentOwnershipInvalidatesOnlyOnPresenceTransitions(t *testing.T) {
	ctx := context.Background()
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	now := time.Date(2026, 8, 22, 12, 0, 0, 0, time.UTC)
	s.now = func() time.Time { return now }
	if _, err := s.CreateNamedAgent(ctx, "fred", ""); err != nil {
		t.Fatal(err)
	}
	var changes []domain.Invalidation
	s.SetChangeObserver(func(change domain.Invalidation) { changes = append(changes, change) })
	if _, err := s.AcquireNamedAgent(ctx, "fred", "owner", 30*time.Second); err != nil {
		t.Fatal(err)
	}
	now = now.Add(10 * time.Second)
	if _, err := s.RenewNamedAgent(ctx, "fred", "owner", 30*time.Second); err != nil {
		t.Fatal(err)
	}
	if len(changes) != 1 || len(changes[0].Topics) != 1 || changes[0].Topics[0] != domain.TopicAgents {
		t.Fatalf("acquire/renew changes = %#v", changes)
	}
	if err := s.ReleaseNamedAgent(ctx, "fred", "owner"); err != nil {
		t.Fatal(err)
	}
	if len(changes) != 2 || changes[1].Topics[0] != domain.TopicAgents {
		t.Fatalf("release changes = %#v", changes)
	}
}

func TestClaimUnthreadedOnlyExcludesReplies(t *testing.T) {
	ctx := context.Background()
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	agent := resolveAgent(t, s, "codex", "thread", "/repo")
	human, _ := s.HumanMailbox(ctx)
	original := message("019c0000-0000-7000-8000-000000000201", agent.ID, human.ID, "output")
	if err := s.Create(ctx, original); err != nil {
		t.Fatal(err)
	}
	reply := message("019c0000-0000-7000-8000-000000000202", human.ID, agent.ID, "old reply")
	if err := s.Reply(ctx, original.ID, reply); err != nil {
		t.Fatal(err)
	}
	root := message("019c0000-0000-7000-8000-000000000203", human.ID, agent.ID, "new root")
	if err := s.Create(ctx, root); err != nil {
		t.Fatal(err)
	}
	claimed, err := s.Claim(ctx, domain.Claim{RecipientMailboxID: agent.ID, UnthreadedOnly: true}, "owner")
	if err != nil || claimed.ID != root.ID {
		t.Fatalf("claim = %#v, %v", claimed, err)
	}
	exact, err := s.Claim(ctx, domain.Claim{ReplyTo: original.ID, RecipientMailboxID: agent.ID}, "exact")
	if err != nil || exact.ID != reply.ID {
		t.Fatalf("exact claim = %#v, %v", exact, err)
	}
}
