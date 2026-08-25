package store

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

func TestHarnessActivityCoalescesReplayAndNotifiesMaterialChanges(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	var changes []domain.Invalidation
	s.SetChangeObserver(func(change domain.Invalidation) { changes = append(changes, change) })
	first := domain.HarnessActivity{
		MailboxID: "agent-mailbox", Harness: "home-built", SessionID: "session-1", OperationID: "operation-1",
		Kind: domain.HarnessActivityPlan, Body: "first plan", OccurredAt: time.Unix(100, 0),
	}
	if err := s.UpsertHarnessActivity(ctx, first); err != nil {
		t.Fatal(err)
	}
	if err := s.UpsertHarnessActivity(ctx, first); err != nil {
		t.Fatal(err)
	}
	updated := first
	updated.Body = "final plan"
	updated.OccurredAt = time.Unix(101, 0)
	if err := s.UpsertHarnessActivity(ctx, updated); err != nil {
		t.Fatal(err)
	}
	activities, err := s.ListHarnessActivities(ctx, domain.HarnessActivityFilter{MailboxID: first.MailboxID})
	if err != nil {
		t.Fatal(err)
	}
	if len(activities) != 1 || activities[0].Body != updated.Body || !activities[0].OccurredAt.Equal(updated.OccurredAt) {
		t.Fatalf("activities = %#v", activities)
	}
	if len(changes) != 2 {
		t.Fatalf("material activity changes = %#v", changes)
	}
	for _, change := range changes {
		if len(change.Topics) != 1 || change.Topics[0] != domain.TopicActivities {
			t.Fatalf("activity change = %#v", change)
		}
	}
}

func TestHarnessActivityBoundsUTF8AndRetainsRecentProgress(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	command := domain.HarnessActivity{
		MailboxID: "agent-mailbox", Harness: "home-built", SessionID: "bounded-session", OperationID: "operation-command",
		Kind: domain.HarnessActivityCommand, ItemID: "command-1", Status: domain.HarnessActivityCompleted,
		Title: strings.Repeat("title", 500), Body: strings.Repeat("界", domain.HarnessActivityCommandBodyBytes), OccurredAt: time.Unix(1, 0),
	}
	if err := s.UpsertHarnessActivity(ctx, command); err != nil {
		t.Fatal(err)
	}
	for index := 0; index < domain.HarnessActivityProgressRetained+5; index++ {
		activity := domain.HarnessActivity{
			MailboxID: command.MailboxID, Harness: command.Harness, SessionID: command.SessionID, OperationID: "operation-progress",
			Kind: domain.HarnessActivityProgress, ItemID: fmt.Sprintf("progress-%03d", index),
			Body: strings.Repeat("p", domain.HarnessActivityProgressBytes+1), OccurredAt: time.Unix(int64(index+2), 0),
		}
		activity.ItemID = strings.TrimSpace(activity.ItemID)
		if err := s.UpsertHarnessActivity(ctx, activity); err != nil {
			t.Fatal(err)
		}
	}
	activities, err := s.ListHarnessActivities(ctx, domain.HarnessActivityFilter{MailboxID: command.MailboxID, Harness: command.Harness, SessionID: command.SessionID})
	if err != nil {
		t.Fatal(err)
	}
	progress := 0
	for _, activity := range activities {
		if activity.Kind == domain.HarnessActivityCommand {
			if len(activity.Title) > domain.HarnessActivityTitleBytes || len(activity.Body) > domain.HarnessActivityCommandBodyBytes || !utf8.ValidString(activity.Body) || !activity.Truncated {
				t.Fatalf("bounded command = %#v", activity)
			}
		}
		if activity.Kind == domain.HarnessActivityProgress {
			progress++
			if len(activity.Body) != domain.HarnessActivityProgressBytes || !activity.Truncated {
				t.Fatalf("bounded progress = %#v", activity)
			}
		}
	}
	if progress != domain.HarnessActivityProgressRetained {
		t.Fatalf("retained progress = %d", progress)
	}
}

func TestHarnessActivitySurvivesRestartAndProjectionRebuild(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, path)
	activity := domain.HarnessActivity{
		MailboxID: "local-mailbox", Harness: "fake", SessionID: "restart-session", OperationID: "restart-operation",
		Kind: domain.HarnessActivityOperation, Status: domain.HarnessActivityFailed, Body: "failed safely", OccurredAt: time.Unix(200, 0),
	}
	if err := s.UpsertHarnessActivity(context.Background(), activity); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	s = openStore(t, path)
	if err := s.Rebuild(context.Background()); err != nil {
		t.Fatal(err)
	}
	activities, err := s.ListHarnessActivities(context.Background(), domain.HarnessActivityFilter{MailboxID: activity.MailboxID})
	if err != nil || len(activities) != 1 || activities[0].Status != domain.HarnessActivityFailed {
		t.Fatalf("activities after restart/rebuild = %#v, %v", activities, err)
	}
}

func TestSchemaVersionTwentySixMigratesLocalActivityProjection(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, path)
	canonical := tableCount(t, s, "canonical_events")
	if _, err := s.db.Exec(`DROP TABLE harness_activities; PRAGMA user_version = 26`); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	s = openStore(t, path)
	if canonical != tableCount(t, s, "canonical_events") {
		t.Fatal("activity migration rebuilt or changed canonical state")
	}
	if err := s.UpsertHarnessActivity(context.Background(), domain.HarnessActivity{
		MailboxID: "migrated", Harness: "fake", SessionID: "session", OperationID: "operation",
		Kind: domain.HarnessActivityOperation, Status: domain.HarnessActivityRunning,
	}); err != nil {
		t.Fatal(err)
	}
}

func TestHarnessActivityDoesNotEnterMessageOrCanonicalState(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	mailbox, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "fake", ExternalSessionID: "message-session"}, model.RepositoryContext{Directory: "/work"})
	if err != nil {
		t.Fatal(err)
	}
	message := model.Message{
		ID: "019c0000-0000-7000-8000-000000000701", SenderMailboxID: mailbox.ID, RecipientMailboxID: model.HumanMailboxID,
		Body: "actionable message", CreatedAt: time.Unix(300, 0),
	}
	if err := s.Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	canonicalBefore, outboxBefore, receiptsBefore := tableCount(t, s, "canonical_events"), tableCount(t, s, "outbox"), tableCount(t, s, "mutation_receipts")
	before, err := s.ListConversations(ctx, model.ConversationFilter{Limit: 20})
	if err != nil {
		t.Fatal(err)
	}
	activity := domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "fake", SessionID: "message-session", OperationID: "operation-1",
		Kind: domain.HarnessActivityTool, ItemID: "tool-1", Status: domain.HarnessActivityCompleted, Title: "search", Body: "done", OccurredAt: time.Unix(301, 0),
	}
	if err := s.UpsertHarnessActivity(ctx, activity); err != nil {
		t.Fatal(err)
	}
	after, err := s.ListConversations(ctx, model.ConversationFilter{Limit: 20})
	if err != nil {
		t.Fatal(err)
	}
	if canonicalBefore != tableCount(t, s, "canonical_events") || outboxBefore != tableCount(t, s, "outbox") || receiptsBefore != tableCount(t, s, "mutation_receipts") {
		t.Fatal("activity entered canonical event, relay outbox, or signed mutation state")
	}
	if len(before.Conversations) != len(after.Conversations) || before.Conversations[0].OpenCount != after.Conversations[0].OpenCount {
		t.Fatalf("activity changed conversation counts: %#v -> %#v", before, after)
	}
	reply := model.Message{ID: "019c0000-0000-7000-8000-000000000702", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: mailbox.ID, Body: "reply", CreatedAt: time.Unix(302, 0)}
	if err := s.Reply(ctx, message.ID, reply); err != nil {
		t.Fatal(err)
	}
	if err := s.Archive(ctx, activity.ItemID); err == nil {
		t.Fatal("activity item unexpectedly became an archive target")
	}
}

func TestHarnessActivityValidation(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	base := domain.HarnessActivity{MailboxID: "mailbox", Harness: "fake", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityTool, ItemID: "tool", Title: "search", Status: domain.HarnessActivityCompleted}
	for _, invalid := range []domain.HarnessActivity{
		{},
		{MailboxID: "mailbox", Harness: "fake", SessionID: "session", OperationID: "operation", Kind: "unknown"},
		{MailboxID: "mailbox", Harness: "fake", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityProgress},
		{MailboxID: "mailbox", Harness: "fake", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityOperation},
	} {
		if err := s.UpsertHarnessActivity(context.Background(), invalid); err == nil {
			t.Fatalf("invalid activity succeeded: %#v", invalid)
		}
	}
	if err := s.UpsertHarnessActivity(context.Background(), base); err != nil {
		t.Fatal(err)
	}
	if _, err := s.ListHarnessActivities(context.Background(), domain.HarnessActivityFilter{}); err == nil {
		t.Fatal("activity query without mailbox succeeded")
	}
}

func tableCount(t *testing.T, s *SQLite, table string) int {
	t.Helper()
	var count int
	if err := s.db.QueryRow(`SELECT count(*) FROM ` + table).Scan(&count); err != nil {
		t.Fatal(err)
	}
	return count
}
