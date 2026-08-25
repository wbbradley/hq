package store

import (
	"context"
	"encoding/base64"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

func TestConversationEntriesPageMixedCanonicalOrderAndRebuild(t *testing.T) {
	s := openStore(t, t.TempDir()+"/hq.db")
	ctx := context.Background()
	mailbox, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "provider", ExternalSessionID: "session"}, model.RepositoryContext{Directory: "/work"})
	if err != nil {
		t.Fatal(err)
	}
	correlation := func(operation, item string) model.MessageCorrelation {
		return model.MessageCorrelation{Provider: "provider", SessionID: "session", OperationID: operation, ItemID: item}
	}
	first := model.Message{
		ID: "019c0000-0000-7000-8000-000000000801", SenderMailboxID: mailbox.ID, RecipientMailboxID: model.HumanMailboxID,
		Body: "first", Correlation: correlation("operation-1", "output-1"), Presentation: model.PresentationUpdate, CreatedAt: time.Unix(100, 0),
	}
	if err := s.Create(ctx, first); err != nil {
		t.Fatal(err)
	}
	progress := canonicalHarnessActivity(domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "provider", SessionID: "session", OperationID: "operation-1", ItemID: "progress",
		Kind: domain.HarnessActivityProgress, Body: "working", OccurredAt: time.Unix(101, 0),
	})
	if err := s.UpsertHarnessActivity(ctx, progress); err != nil {
		t.Fatal(err)
	}
	second := model.Message{
		ID: "019c0000-0000-7000-8000-000000000802", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: mailbox.ID,
		Body: "second", Correlation: correlation("operation-1", "input-2"), Presentation: model.PresentationNotice, CreatedAt: time.Unix(102, 0),
	}
	if err := s.Create(ctx, second); err != nil {
		t.Fatal(err)
	}
	tool := canonicalHarnessActivity(domain.HarnessActivity{
		MailboxID: mailbox.ID, Harness: "provider", SessionID: "session", OperationID: "operation-1", ItemID: "tool",
		Kind: domain.HarnessActivityTool, Status: domain.HarnessActivityCompleted, Title: "inspect", Body: "done", OccurredAt: time.Unix(103, 0),
	})
	tool.Sequence = 2
	if err := s.UpsertHarnessActivity(ctx, tool); err != nil {
		t.Fatal(err)
	}
	key := model.ConversationKey{CounterpartyMailboxID: mailbox.ID, HarnessProvider: "provider", HarnessSessionID: "session"}
	firstPage, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 2})
	if err != nil || len(firstPage.Entries) != 2 || firstPage.NextCursor == "" {
		t.Fatalf("first entry page = %#v, %v", firstPage, err)
	}
	secondPage, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 2, Cursor: firstPage.NextCursor})
	if err != nil || len(secondPage.Entries) != 2 || secondPage.NextCursor != "" {
		t.Fatalf("second entry page = %#v, %v", secondPage, err)
	}
	entries := append(append([]domain.ConversationEntry(nil), firstPage.Entries...), secondPage.Entries...)
	if entries[0].Kind != domain.ConversationEntryMessage || entries[0].Message.Body != "first" || entries[1].Kind != domain.ConversationEntryActivity || entries[1].Activity.Body != "working" || entries[2].Kind != domain.ConversationEntryMessage || entries[2].Message.Body != "second" || entries[3].Kind != domain.ConversationEntryActivity || entries[3].Activity.Title != "inspect" {
		t.Fatalf("mixed entries = %#v", entries)
	}
	for index, entry := range entries {
		if !entry.Valid() || index > 0 && entry.DisplayOrder <= entries[index-1].DisplayOrder {
			t.Fatalf("entry %d order/shape = %#v", index, entry)
		}
	}
	legacy, err := s.ListConversationHistory(ctx, model.ConversationHistoryFilter{Key: key})
	if err != nil || len(legacy.Messages) != 2 || legacy.Messages[0].ID != first.ID || legacy.Messages[1].ID != second.ID {
		t.Fatalf("legacy history = %#v, %v", legacy, err)
	}
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	rebuilt, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Limit: 10})
	if err != nil || !reflect.DeepEqual(rebuilt.Entries, entries) {
		t.Fatalf("rebuilt entries = %#v, %v; want %#v", rebuilt.Entries, err, entries)
	}
	if _, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Cursor: "malformed"}); err == nil {
		t.Fatal("malformed unified cursor succeeded")
	}
	for _, malformed := range []string{
		`{"event_id":"` + strings.Repeat("a", 64) + `"}`,
		`{"display_order":0,"event_id":"` + strings.Repeat("A", 64) + `"}`,
		`{"display_order":0,"event_id":"` + strings.Repeat("z", 64) + `"}`,
		`{"display_order":0,"event_id":"` + strings.Repeat("a", 64) + `","extra":true}`,
		`{"display_order":0,"event_id":"` + strings.Repeat("a", 64) + `"} {}`,
	} {
		cursor := base64.RawURLEncoding.EncodeToString([]byte(malformed))
		if _, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: key, Cursor: cursor}); err == nil {
			t.Fatalf("non-strict unified cursor succeeded: %s", malformed)
		}
	}
}

func TestConversationEntriesIsolateProviderAndThreadFallback(t *testing.T) {
	s := openStore(t, t.TempDir()+"/hq.db")
	ctx := context.Background()
	mailbox, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "provider", ExternalSessionID: "shared"}, model.RepositoryContext{Directory: "/work"})
	if err != nil {
		t.Fatal(err)
	}
	message := model.Message{
		ID: "019c0000-0000-7000-8000-000000000811", SenderMailboxID: mailbox.ID, RecipientMailboxID: model.HumanMailboxID, Body: "provider message",
		Correlation: model.MessageCorrelation{Provider: "provider", SessionID: "shared", OperationID: "operation"}, CreatedAt: time.Unix(110, 0),
	}
	if err := s.Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	activity := canonicalHarnessActivity(domain.HarnessActivity{MailboxID: mailbox.ID, Harness: "other", SessionID: "shared", OperationID: "operation", Kind: domain.HarnessActivityPlan, Body: "other provider", OccurredAt: time.Unix(111, 0)})
	if err := s.UpsertHarnessActivity(ctx, activity); err != nil {
		t.Fatal(err)
	}
	providerEntries, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: model.ConversationKey{CounterpartyMailboxID: mailbox.ID, HarnessProvider: "provider", HarnessSessionID: "shared"}})
	if err != nil || len(providerEntries.Entries) != 1 || providerEntries.Entries[0].Kind != domain.ConversationEntryMessage {
		t.Fatalf("provider-isolated entries = %#v, %v", providerEntries, err)
	}
	otherEntries, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: model.ConversationKey{CounterpartyMailboxID: mailbox.ID, HarnessProvider: "other", HarnessSessionID: "shared"}})
	if err != nil || len(otherEntries.Entries) != 1 || otherEntries.Entries[0].Kind != domain.ConversationEntryActivity {
		t.Fatalf("other-provider entries = %#v, %v", otherEntries, err)
	}

	unthreaded := model.Message{ID: "019c0000-0000-7000-8000-000000000812", SenderMailboxID: mailbox.ID, RecipientMailboxID: model.HumanMailboxID, Body: "thread fallback", CreatedAt: time.Unix(112, 0)}
	if err := s.Create(ctx, unthreaded); err != nil {
		t.Fatal(err)
	}
	conversations, err := s.ListConversations(ctx, model.ConversationFilter{IncludeSent: true, Limit: 20})
	if err != nil {
		t.Fatal(err)
	}
	var threadKey model.ConversationKey
	for _, summary := range conversations.Conversations {
		if summary.Latest.ID == unthreaded.ID {
			threadKey = summary.Key
		}
	}
	threadEntries, err := s.ListConversationEntries(ctx, model.ConversationHistoryFilter{Key: threadKey})
	if err != nil || len(threadEntries.Entries) != 1 || threadEntries.Entries[0].Kind != domain.ConversationEntryMessage {
		t.Fatalf("thread-fallback entries = %#v, %v", threadEntries, err)
	}
}
