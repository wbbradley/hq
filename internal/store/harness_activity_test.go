package store

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"reflect"
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
	entries, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: model.ConversationKey{
		CounterpartyMailboxID: first.MailboxID, HarnessProvider: first.Harness, HarnessSessionID: first.SessionID,
	}})
	if err != nil || len(entries.Entries) != 1 || entries.Entries[0].Kind != domain.ConversationEntryActivity || entries.Entries[0].Activity.Body != updated.Body || entries.Entries[0].EventID != entries.Entries[0].Activity.EventID {
		t.Fatalf("coalesced unified entries = %#v, %v", entries, err)
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
	entryFilter := model.ConversationHistoryFilter{Key: model.ConversationKey{
		CounterpartyMailboxID: command.MailboxID, HarnessProvider: command.Harness, HarnessSessionID: command.SessionID,
	}, Limit: 200}
	var entryProgress, entryCommands int
	for {
		page, listErr := s.ListConversationEntries(ctx, entryFilter)
		if listErr != nil {
			t.Fatal(listErr)
		}
		for _, entry := range page.Entries {
			if entry.Activity.Kind == domain.HarnessActivityProgress {
				entryProgress++
			}
			if entry.Activity.Kind == domain.HarnessActivityCommand {
				entryCommands++
			}
		}
		if page.NextCursor == "" {
			break
		}
		entryFilter.Cursor = page.NextCursor
	}
	if entryProgress != domain.HarnessActivityProgressRetained || entryCommands != 1 {
		t.Fatalf("unified retained activity = %d progress / %d commands", entryProgress, entryCommands)
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
	root := t.TempDir()
	creatorPath, desktopPath := filepath.Join(root, "creator", "hq.db"), filepath.Join(root, "desktop", "hq.db")
	creator := openStore(t, creatorPath)
	desktop := openStore(t, desktopPath)
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
	correlation := model.MessageCorrelation{Provider: "home-built", SessionID: "fanout-session", OperationID: "operation"}
	first := model.Message{
		ID: "019c0000-0000-7000-8000-000000000901", SenderMailboxID: mailbox.ID, RecipientMailboxID: model.HumanMailboxID,
		Body: "first synchronized message", Details: "visible details", Presentation: model.PresentationUpdate,
		Correlation: correlation, TechnicalSections: []model.TechnicalSection{{Namespace: "test.fanout", Fields: []model.TechnicalField{{Key: "source", Value: "creator"}}}},
		CreatedAt: time.Unix(19, 0),
	}
	if err := creator.Create(ctx, first); err != nil {
		t.Fatal(err)
	}
	activity := canonicalHarnessActivity(domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "home-built", SessionID: "fanout-session", OperationID: "operation",
		Kind: domain.HarnessActivityTool, ItemID: "tool", Status: domain.HarnessActivityCompleted,
		Title: "inspect", Body: "done", OccurredAt: time.Unix(20, 0),
	})
	if err := creator.UpsertHarnessActivity(ctx, activity); err != nil {
		t.Fatal(err)
	}
	second := model.Message{
		ID: "019c0000-0000-7000-8000-000000000902", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: mailbox.ID,
		Body: "second synchronized message", Presentation: model.PresentationNotice,
		Correlation: correlation, CreatedAt: time.Unix(21, 0),
	}
	if err := creator.Create(ctx, second); err != nil {
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
	key := model.ConversationKey{CounterpartyMailboxID: mailbox.ID, HarnessProvider: correlation.Provider, HarnessSessionID: correlation.SessionID}
	wantEntries, err := creator.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 20})
	if err != nil || len(wantEntries.Entries) != 3 || wantEntries.Entries[0].Message == nil || wantEntries.Entries[1].Activity == nil || wantEntries.Entries[2].Message == nil {
		t.Fatalf("local mixed conversation = %#v, %v", wantEntries, err)
	}
	wantLegacy, err := creator.ListConversationHistory(ctx, model.ConversationHistoryFilter{Key: key, Limit: 20})
	if err != nil || len(wantLegacy.Messages) != 2 {
		t.Fatalf("local legacy conversation = %#v, %v", wantLegacy, err)
	}
	var changes []domain.Invalidation
	desktop.SetChangeObserver(func(change domain.Invalidation) { changes = append(changes, change) })
	found := false
	var activityJob *RelayJob
	for index := len(jobs) - 1; index >= 0; index-- {
		job := jobs[index]
		if job.CanonicalEventID == local[0].EventID {
			found = true
			copyJob := job
			activityJob = &copyJob
		}
		if _, err := desktop.ReceiveGiftWrap(ctx, job.ExactGiftWrapBytes, relay, time.Now().UTC()); err != nil {
			t.Fatalf("receive account activity wrapper %s: %v", job.CanonicalEventID, err)
		}
	}
	if !found {
		t.Fatalf("activity event %s absent from account outbox jobs %#v", local[0].EventID, jobs)
	}
	changesBeforeDuplicate := len(changes)
	if duplicate, err := desktop.ReceiveGiftWrap(ctx, activityJob.ExactGiftWrapBytes, relay, time.Now().UTC()); err != nil || duplicate.Status != "duplicate-wrapper" || len(changes) != changesBeforeDuplicate {
		t.Fatalf("duplicate activity wrapper = %#v, %v; changes %d -> %d", duplicate, err, changesBeforeDuplicate, len(changes))
	}
	remote, err := desktop.ListHarnessActivities(ctx, domain.HarnessActivityFilter{InstallationID: creatorID, MailboxID: mailbox.ID})
	if err != nil || len(remote) != 1 || remote[0].EventID != local[0].EventID || remote[0].AudienceAccountID == "" || remote[0].AudienceAccountID != local[0].AudienceAccountID || remote[0].Correlation != local[0].Correlation || remote[0].DisplayOrder != local[0].DisplayOrder {
		t.Fatalf("remote activity = %#v, %v; local=%#v", remote, err, local)
	}
	gotEntries, err := desktop.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 20})
	if err != nil {
		t.Fatal(err)
	}
	assertConversationEntryPagesConverge(t, gotEntries, wantEntries)
	gotLegacy, err := desktop.ListConversationHistory(ctx, model.ConversationHistoryFilter{Key: key, Limit: 20})
	if err != nil {
		t.Fatal(err)
	}
	assertConversationMessagesConverge(t, gotLegacy.Messages, wantLegacy.Messages)
	if gotLegacy.NextCursor != wantLegacy.NextCursor {
		t.Fatalf("remote legacy cursor = %q; want %q", gotLegacy.NextCursor, wantLegacy.NextCursor)
	}
	if err := creator.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	if err := desktop.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	if rebuilt, err := desktop.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 20}); err != nil {
		t.Fatal(err)
	} else {
		assertConversationEntryPagesConverge(t, rebuilt, wantEntries)
	}
	if err := creator.Close(); err != nil {
		t.Fatal(err)
	}
	if err := desktop.Close(); err != nil {
		t.Fatal(err)
	}
	creator, desktop = openStore(t, creatorPath), openStore(t, desktopPath)
	for label, database := range map[string]*SQLite{"creator": creator, "desktop": desktop} {
		restarted, restartErr := database.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 20})
		if restartErr != nil {
			t.Fatal(restartErr)
		}
		t.Run(label+" restarted projection", func(t *testing.T) { assertConversationEntryPagesConverge(t, restarted, wantEntries) })
	}
}

func assertConversationEntryPagesConverge(t *testing.T, got, want domain.ConversationEntryPage) {
	t.Helper()
	if got.NextCursor != want.NextCursor || len(got.Entries) != len(want.Entries) {
		t.Fatalf("conversation entry page shape = %d/%q; want %d/%q", len(got.Entries), got.NextCursor, len(want.Entries), want.NextCursor)
	}
	for index := range want.Entries {
		gotEntry, wantEntry := got.Entries[index], want.Entries[index]
		if gotEntry.Kind != wantEntry.Kind || gotEntry.EventID != wantEntry.EventID || gotEntry.DisplayOrder != wantEntry.DisplayOrder {
			t.Fatalf("conversation entry %d identity/order = %#v; want %#v", index, gotEntry, wantEntry)
		}
		if gotEntry.Kind == domain.ConversationEntryActivity {
			if !reflect.DeepEqual(gotEntry.Activity, wantEntry.Activity) {
				t.Fatalf("conversation activity %d = %#v; want %#v", index, gotEntry.Activity, wantEntry.Activity)
			}
			continue
		}
		assertConversationMessagesConverge(t, []model.Message{*gotEntry.Message}, []model.Message{*wantEntry.Message})
	}
}

func assertConversationMessagesConverge(t *testing.T, got, want []model.Message) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("conversation messages = %d; want %d", len(got), len(want))
	}
	normalize := func(message model.Message) model.Message {
		message.DeliveryState = ""
		message.RecipientInstallationID = ""
		message.SenderLabel, message.SourceDeviceLabel, message.RecipientLabel = "", "", ""
		message.SenderAddress, message.RecipientAddress = model.MessageAddress{}, model.MessageAddress{}
		return message
	}
	for index := range want {
		if normalizedGot, normalizedWant := normalize(got[index]), normalize(want[index]); !reflect.DeepEqual(normalizedGot, normalizedWant) {
			gotJSON, _ := json.MarshalIndent(normalizedGot, "", "  ")
			wantJSON, _ := json.MarshalIndent(normalizedWant, "", "  ")
			t.Fatalf("conversation message %d = %s; want %s", index, gotJSON, wantJSON)
		}
	}
}

func TestRevokedDeviceHarnessActivityGiftWrapFailsClosed(t *testing.T) {
	ctx := context.Background()
	root := t.TempDir()
	creator := openStore(t, filepath.Join(root, "creator", "hq.db"))
	revoked := openStore(t, filepath.Join(root, "revoked", "hq.db"))
	revokedID, revokedKey := revoked.InstallationIdentity()
	bundle, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: revokedID, SignerKeyID: revokedKey, Name: "revoked device"})
	if err != nil {
		t.Fatal(err)
	}
	rawBundle, _ := json.Marshal(bundle)
	if err := revoked.JoinHumanInvite(ctx, rawBundle); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{canonicalEventByTypeAndInstallation(t, revoked, event.TypeHumanDeviceAccept, revokedID)}); err != nil {
		t.Fatal(err)
	}
	mailbox := harnessActivityMailbox(t, revoked, "revoked-session")
	if err := creator.RevokeHumanDevice(ctx, revokedID); err != nil {
		t.Fatal(err)
	}
	revocation := canonicalEventByType(t, creator, event.TypeHumanDeviceRevoke)
	if err := revoked.AppendCanonical(ctx, []event.SignedEvent{revocation}); err != nil {
		t.Fatal(err)
	}
	payload, err := event.MarshalPayload(event.HarnessActivityPayload{
		Correlation: model.MessageCorrelation{Provider: "home-built", SessionID: "revoked-session", OperationID: "operation", ItemID: "progress"},
		Kind:        domain.HarnessActivityProgress, Status: domain.HarnessActivityRunning, Body: "must not project",
		OccurredAt: time.Unix(100, 0).UnixMilli(), RuntimeID: "revoked-runtime", Sequence: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	late, err := revoked.signer.Sign(ctx, event.Content{
		Schema: event.Schema2, Type: event.TypeHarnessActivity, Sender: revoked.localAddress(mailbox.ID),
		Audience: &event.Audience{HumanAccountID: bundle.AccountID}, Parents: []string{revocation.ID()},
		Scope: event.ScopeAccountAddressed, Payload: payload,
	}, time.Unix(100, 0))
	if err != nil {
		t.Fatal(err)
	}
	_, creatorKey := creator.InstallationIdentity()
	wrapper, err := revoked.WireCodec(nil, nil).Wrap(late, creatorKey)
	if err != nil {
		t.Fatal(err)
	}
	key := model.ConversationKey{CounterpartyMailboxID: mailbox.ID, HarnessProvider: "home-built", HarnessSessionID: "revoked-session"}
	beforeConversations, err := creator.ListConversations(ctx, model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Limit: 20})
	if err != nil {
		t.Fatal(err)
	}
	canonicalBefore := tableCount(t, creator, "canonical_events")
	deliveryBefore := tableCount(t, creator, "delivery_facts")
	if _, err := creator.ReceiveGiftWrap(ctx, wrapper.ExactWire, "wss://activity.relay.test", time.Now().UTC()); err == nil {
		t.Fatal("revoked device activity projected through gift wrap")
	}
	if tableCount(t, creator, "canonical_events") != canonicalBefore || tableCount(t, creator, "delivery_facts") != deliveryBefore || tableCount(t, creator, "harness_activities") != 0 {
		t.Fatal("revoked activity changed canonical, delivery, or activity projections")
	}
	entries, err := creator.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 20})
	if err != nil || len(entries.Entries) != 0 {
		t.Fatalf("revoked activity conversation = %#v, %v", entries, err)
	}
	afterConversations, err := creator.ListConversations(ctx, model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Limit: 20})
	if err != nil || !reflect.DeepEqual(afterConversations, beforeConversations) {
		t.Fatalf("revoked activity changed summaries: %#v -> %#v, %v", beforeConversations, afterConversations, err)
	}
	status, err := creator.NetworkStatus(ctx)
	if err != nil || status.RevokedDeviceTraffic != 1 || status.Quarantined == 0 {
		t.Fatalf("revoked activity network status = %#v, %v", status, err)
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
