package store

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

func TestHarnessActivityCoalescesReplayAndNotifiesMaterialChanges(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	mailbox := harnessActivityMailbox(t, s, "session-1")
	var changes []domain.Invalidation
	s.SetChangeObserver(func(change domain.Invalidation) { changes = append(changes, change) })
	first := domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "home-built", SessionID: "session-1", OperationID: "operation-1",
		Kind: domain.HarnessActivityPlan, Body: "first plan", OccurredAt: time.Unix(100, 0),
	}
	first = canonicalHarnessActivity(first)
	if err := s.UpsertHarnessActivity(ctx, first); err != nil {
		t.Fatal(err)
	}
	canonicalAfterFirst := tableCount(t, s, "canonical_events")
	if err := s.UpsertHarnessActivity(ctx, first); err != nil {
		t.Fatal(err)
	}
	if tableCount(t, s, "canonical_events") != canonicalAfterFirst {
		t.Fatal("identical activity replay appended another canonical event")
	}
	updated := first
	updated.Body = "final plan"
	updated.Sequence++
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
		if !slices.Contains(change.Topics, domain.TopicActivities) {
			t.Fatalf("activity change = %#v", change)
		}
	}
}

func TestHarnessActivityBoundsUTF8AndRetainsRecentProgress(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	mailbox := harnessActivityMailbox(t, s, "bounded-session")
	command := domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "home-built", SessionID: "bounded-session", OperationID: "operation-command",
		Kind: domain.HarnessActivityCommand, ItemID: "command-1", Status: domain.HarnessActivityCompleted,
		Title: strings.Repeat("title", 500), Body: strings.Repeat("界", domain.HarnessActivityCommandBodyBytes), OccurredAt: time.Unix(1, 0),
	}
	command = canonicalHarnessActivity(command)
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
		activity = canonicalHarnessActivity(activity)
		activity.Sequence = uint64(index + 2)
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
	canonicalCount := tableCount(t, s, "canonical_events")
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	rebuilt, err := s.ListHarnessActivities(ctx, domain.HarnessActivityFilter{MailboxID: command.MailboxID, Harness: command.Harness, SessionID: command.SessionID})
	if err != nil || len(rebuilt) != len(activities) {
		t.Fatalf("rebuilt bounded activities = %d, %v", len(rebuilt), err)
	}
	if tableCount(t, s, "canonical_events") != canonicalCount {
		t.Fatal("projection retention deleted canonical progress events")
	}
}

func TestHarnessActivityDynamicallyFitsEscapedSignedWire(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	mailbox := harnessActivityMailbox(t, s, "escaped-session")
	activity := canonicalHarnessActivity(domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "home-built", SessionID: "escaped-session", OperationID: "operation",
		Kind: domain.HarnessActivityPlan, Body: strings.Repeat("\x00", domain.HarnessActivityBodyBytes), OccurredAt: time.Unix(10, 0),
	})
	if err := s.UpsertHarnessActivity(context.Background(), activity); err != nil {
		t.Fatal(err)
	}
	activities, err := s.ListHarnessActivities(context.Background(), domain.HarnessActivityFilter{MailboxID: mailbox.ID})
	if err != nil || len(activities) != 1 {
		t.Fatalf("fitted activity = %#v, %v", activities, err)
	}
	if !activities[0].Truncated || len(activities[0].Body) >= len(activity.Body) || !utf8.ValidString(activities[0].Body) {
		t.Fatalf("dynamic fitting result = %#v", activities[0])
	}
	var raw []byte
	if err := s.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_id=?`, activities[0].EventID).Scan(&raw); err != nil {
		t.Fatal(err)
	}
	if len(raw) > event.MaxWireBytes || event.Inspect(raw).Status != event.StatusProjected {
		t.Fatalf("signed wire size/status = %d/%s", len(raw), event.Inspect(raw).Status)
	}
}

func TestHarnessActivityAccountFanoutConvergesAcrossDevices(t *testing.T) {
	ctx := context.Background()
	creator := openStore(t, filepath.Join(t.TempDir(), "creator", "hq.db"))
	desktop := openStore(t, filepath.Join(t.TempDir(), "desktop", "hq.db"))
	creatorID, _ := creator.InstallationIdentity()
	desktopID, desktopKey := desktop.InstallationIdentity()
	const relay = "wss://activity.relay.test"
	bundle, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: desktopID, SignerKeyID: desktopKey, Name: "desktop", Relays: []string{relay}})
	if err != nil {
		t.Fatal(err)
	}
	rawBundle, _ := json.Marshal(bundle)
	if err := desktop.JoinHumanInvite(ctx, rawBundle); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{canonicalEventByTypeAndInstallation(t, desktop, event.TypeHumanDeviceAccept, desktopID)}); err != nil {
		t.Fatal(err)
	}
	mailbox := harnessActivityMailbox(t, creator, "fanout-session")
	activity := canonicalHarnessActivity(domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "home-built", SessionID: "fanout-session", OperationID: "operation",
		Kind: domain.HarnessActivityTool, ItemID: "tool", Status: domain.HarnessActivityCompleted,
		Title: "inspect", Body: "done", OccurredAt: time.Unix(20, 0),
	})
	if err := creator.UpsertHarnessActivity(ctx, activity); err != nil {
		t.Fatal(err)
	}
	local, err := creator.ListHarnessActivities(ctx, domain.HarnessActivityFilter{InstallationID: creatorID, MailboxID: mailbox.ID})
	if err != nil || len(local) != 1 {
		t.Fatalf("local activity = %#v, %v", local, err)
	}
	if prepared, err := creator.PrepareOutbound(ctx, 100); err != nil || prepared == 0 {
		t.Fatalf("prepared wrappers = %d, %v", prepared, err)
	}
	jobs, err := creator.RelayJobs(ctx, relay, 100, time.Now().UTC())
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, job := range jobs {
		if job.CanonicalEventID == local[0].EventID {
			found = true
		}
		if _, err := desktop.ReceiveGiftWrap(ctx, job.ExactGiftWrapBytes, relay, time.Now().UTC()); err != nil {
			t.Fatalf("receive account activity wrapper %s: %v", job.CanonicalEventID, err)
		}
	}
	if !found {
		t.Fatalf("activity event %s absent from account outbox jobs %#v", local[0].EventID, jobs)
	}
	remote, err := desktop.ListHarnessActivities(ctx, domain.HarnessActivityFilter{InstallationID: creatorID, MailboxID: mailbox.ID})
	if err != nil || len(remote) != 1 || remote[0].EventID != local[0].EventID || remote[0].AudienceAccountID == "" || remote[0].AudienceAccountID != local[0].AudienceAccountID || remote[0].Correlation != local[0].Correlation || remote[0].DisplayOrder != local[0].DisplayOrder {
		t.Fatalf("remote activity = %#v, %v; local=%#v", remote, err, local)
	}
}

func TestHarnessActivitySurvivesRestartAndProjectionRebuild(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, path)
	mailbox := harnessActivityMailbox(t, s, "restart-session")
	activity := domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "fake", SessionID: "restart-session", OperationID: "restart-operation",
		Kind: domain.HarnessActivityOperation, Status: domain.HarnessActivityFailed, Body: "failed safely", OccurredAt: time.Unix(200, 0),
	}
	activity = canonicalHarnessActivity(activity)
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
	if err != nil || len(activities) != 1 || activities[0].Status != domain.HarnessActivityFailed || activities[0].EventID == "" || activities[0].InstallationID == "" || activities[0].RuntimeID != activity.RuntimeID || activities[0].Sequence != activity.Sequence || activities[0].Correlation != activity.Correlation {
		t.Fatalf("activities after restart/rebuild = %#v, %v", activities, err)
	}
}

func TestSchemaVersionThirtyDiscardsUnsignedActivityProjection(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, path)
	canonical := tableCount(t, s, "canonical_events")
	if _, err := s.db.Exec(`DROP TABLE harness_activities;
CREATE TABLE harness_activities (mailbox_id TEXT NOT NULL,harness TEXT NOT NULL,session_id TEXT NOT NULL,operation_id TEXT NOT NULL,kind TEXT NOT NULL,item_id TEXT NOT NULL,status TEXT NOT NULL,title TEXT NOT NULL,body TEXT NOT NULL,truncated INTEGER NOT NULL,occurred_at INTEGER NOT NULL,PRIMARY KEY(harness,session_id,operation_id,kind,item_id)) STRICT;
INSERT INTO harness_activities VALUES ('legacy','fake','session','operation','operation-status','','running','','',0,1);
PRAGMA user_version = 30`); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	s = openStore(t, path)
	if canonical != tableCount(t, s, "canonical_events") {
		t.Fatal("activity migration rebuilt or changed canonical state")
	}
	if tableCount(t, s, "harness_activities") != 0 {
		t.Fatal("unsigned activity survived migration")
	}
	mailbox := harnessActivityMailbox(t, s, "session")
	activity := canonicalHarnessActivity(domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "fake", SessionID: "session", OperationID: "operation",
		Kind: domain.HarnessActivityOperation, Status: domain.HarnessActivityRunning, OccurredAt: time.Unix(1, 0),
	})
	if err := s.UpsertHarnessActivity(context.Background(), activity); err != nil {
		t.Fatal(err)
	}
}

func TestHarnessActivityEntersCanonicalStateWithoutChangingMessageBehavior(t *testing.T) {
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
	activity = canonicalHarnessActivity(activity)
	if err := s.UpsertHarnessActivity(ctx, activity); err != nil {
		t.Fatal(err)
	}
	after, err := s.ListConversations(ctx, model.ConversationFilter{Limit: 20})
	if err != nil {
		t.Fatal(err)
	}
	if canonicalBefore+1 != tableCount(t, s, "canonical_events") || outboxBefore != tableCount(t, s, "outbox") || receiptsBefore != tableCount(t, s, "mutation_receipts") {
		t.Fatal("activity did not enter only the expected canonical state")
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
	mailbox := harnessActivityMailbox(t, s, "session")
	base := canonicalHarnessActivity(domain.HarnessActivity{MailboxID: mailbox.ID, Harness: "fake", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityTool, ItemID: "tool", Title: "search", Status: domain.HarnessActivityCompleted, OccurredAt: time.Unix(1, 0)})
	for _, invalid := range []domain.HarnessActivity{
		{},
		canonicalHarnessActivity(domain.HarnessActivity{MailboxID: mailbox.ID, Harness: "fake", SessionID: "session", OperationID: "operation", Kind: "unknown", OccurredAt: time.Unix(1, 0)}),
		canonicalHarnessActivity(domain.HarnessActivity{MailboxID: mailbox.ID, Harness: "fake", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityProgress, OccurredAt: time.Unix(1, 0)}),
		canonicalHarnessActivity(domain.HarnessActivity{MailboxID: mailbox.ID, Harness: "fake", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityOperation, OccurredAt: time.Unix(1, 0)}),
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

func harnessActivityMailbox(t *testing.T, s *SQLite, sessionID string) model.Mailbox {
	t.Helper()
	mailbox, err := s.ResolveMailbox(context.Background(), model.SessionIdentity{Harness: "home-built", ExternalSessionID: sessionID}, model.RepositoryContext{Directory: "/work"})
	if err != nil {
		t.Fatal(err)
	}
	return mailbox
}

func canonicalHarnessActivity(activity domain.HarnessActivity) domain.HarnessActivity {
	activity.Correlation = model.MessageCorrelation{Provider: activity.Harness, SessionID: activity.SessionID, OperationID: activity.OperationID, ItemID: activity.ItemID}
	activity.RuntimeID = "runtime-" + activity.SessionID
	activity.Sequence = 1
	return activity
}

func tableCount(t *testing.T, s *SQLite, table string) int {
	t.Helper()
	var count int
	if err := s.db.QueryRow(`SELECT count(*) FROM ` + table).Scan(&count); err != nil {
		t.Fatal(err)
	}
	return count
}
