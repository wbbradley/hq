package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	_ "modernc.org/sqlite"
)

func TestSQLiteConfigurationAndSchema(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state", "hq.db")
	s := openStore(t, path)
	checks := map[string]string{"PRAGMA journal_mode": "wal", "PRAGMA synchronous": "2", "PRAGMA foreign_keys": "1", "PRAGMA trusted_schema": "0", "PRAGMA integrity_check": "ok", "PRAGMA user_version": "33"}
	for query, want := range checks {
		var got string
		if err := s.db.QueryRow(query).Scan(&got); err != nil {
			t.Fatalf("%s: %v", query, err)
		}
		if got != want {
			t.Errorf("%s = %q, want %q", query, got, want)
		}
	}
	for _, table := range []string{"canonical_events", "causal_edges", "projection_checkpoint", "mailboxes", "harness_bindings", "named_agents", "agent_sessions", "agent_ownership", "mailbox_contexts", "messages", "threads", "peers", "mailbox_access", "human_accounts", "human_account_devices", "human_account_default", "outbox", "relays", "outbound_relay_attempts", "inbound_wrappers", "relay_sync_state", "inbound_staging", "quarantine", "mutation_receipts", "change_revision", "projects", "project_events", "resources", "project_resources", "resource_claim_epochs", "resource_health", "project_assignment_epochs", "project_threads", "project_message_acceptances", "project_dispatch_records", "project_dispatch_attempts", "project_activation_operations", "project_runtime_operations", "project_worktree_operations", "agent_retirement_operations", "project_output_provenance", "project_replicas", "project_audit_log", "harness_activities"} {
		var strict int
		if err := s.db.QueryRow(`SELECT strict FROM pragma_table_list WHERE name = ?`, table).Scan(&strict); err != nil {
			t.Fatal(err)
		}
		if strict != 1 {
			t.Fatalf("%s strict = %d", table, strict)
		}
	}
	var canonical int
	if err := s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&canonical); err != nil || canonical != 4 {
		t.Fatalf("bootstrap event count = %d, %v", canonical, err)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("database mode = %o", info.Mode().Perm())
	}
}

func TestOpenSkipsRebuildWhenProjectionIsCurrent(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, database)
	if _, err := s.db.Exec(`UPDATE projection_checkpoint SET rebuilt_at=7 WHERE id=1`); err != nil {
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
	var rebuiltAt int64
	if err := reopened.db.QueryRow(`SELECT rebuilt_at FROM projection_checkpoint WHERE id=1`).Scan(&rebuiltAt); err != nil || rebuiltAt != 7 {
		t.Fatalf("current projection was rebuilt: rebuilt_at=%d, err=%v", rebuiltAt, err)
	}
}

func TestResolveMailboxIsolationContinuityAndContext(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, path)
	ctx := context.Background()
	repoA := model.RepositoryContext{Directory: "/repo/a", GitCommonDir: "/repo/.git", RemoteIdentity: "origin: team/repo", Worktree: "/repo/a", Branch: "one"}
	first, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "same"}, repoA)
	if err != nil {
		t.Fatal(err)
	}
	second, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "pi", ExternalSessionID: "same"}, repoA)
	if err != nil {
		t.Fatal(err)
	}
	if first.ID == second.ID {
		t.Fatal("equal external IDs across harnesses shared a mailbox")
	}
	if first.Label == second.Label {
		t.Fatalf("distinct mailboxes share label %q", first.Label)
	}
	before := first.LastSeen
	time.Sleep(2 * time.Millisecond)
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	s, err = Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	resumed, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "same"}, model.RepositoryContext{Directory: "/other", GitCommonDir: "/repo/.git", Branch: "two"})
	if err != nil {
		t.Fatal(err)
	}
	if resumed.ID != first.ID || !resumed.LastSeen.After(before) {
		t.Fatalf("resumed mailbox = %#v, first = %#v", resumed, first)
	}
	candidates, err := s.FindMailboxes(ctx, model.RepositoryContext{Directory: "/new", GitCommonDir: "/repo/.git"})
	if err != nil {
		t.Fatal(err)
	}
	if len(candidates) != 2 {
		t.Fatalf("candidates = %#v", candidates)
	}
	for _, candidate := range candidates {
		if candidate.Context.Directory == "/new" {
			t.Fatalf("candidate used query context instead of stored context: %#v", candidate)
		}
	}
	third, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "new"}, model.RepositoryContext{Directory: "/new", GitCommonDir: "/repo/.git"})
	if err != nil {
		t.Fatal(err)
	}
	if third.ID == first.ID || third.ID == second.ID {
		t.Fatal("context search reassigned an old mailbox")
	}
	var count int
	if err := s.db.QueryRow(`SELECT count(*) FROM mailbox_contexts WHERE mailbox_id = ?`, first.ID).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if count != 2 {
		t.Fatalf("context history rows = %d", count)
	}
}

func TestHarnessBindingIsUnique(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	now := time.Now().UTC().UnixMilli()
	if _, err := s.db.Exec(`INSERT INTO mailboxes(id, installation_id, kind, label, created_at) VALUES ('0198c7ec-73b0-7cc3-a5f7-e31c77140d61', 'local', 'agent', '', ?), ('0198c7ec-73b0-7cc3-a5f7-e31c77140d62', 'local', 'agent', '', ?)`, now, now); err != nil {
		t.Fatal(err)
	}
	_, err := s.db.Exec(`INSERT INTO harness_bindings(harness, external_session_id, mailbox_id, created_at) VALUES ('codex', 'one', '0198c7ec-73b0-7cc3-a5f7-e31c77140d61', ?)`, now)
	if err != nil {
		t.Fatal(err)
	}
	_, err = s.db.Exec(`INSERT INTO harness_bindings(harness, external_session_id, mailbox_id, created_at) VALUES ('codex', 'one', '0198c7ec-73b0-7cc3-a5f7-e31c77140d62', ?)`, now)
	if err == nil {
		t.Fatal("duplicate harness binding succeeded")
	}
}

func TestConcurrentResolveUsesOneMailbox(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	first := openStore(t, path)
	second, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { second.Close() })
	identity := model.SessionIdentity{Harness: "codex", ExternalSessionID: "shared"}
	repo := model.RepositoryContext{Directory: "/repo"}
	start := make(chan struct{})
	results := make(chan struct {
		mailbox model.Mailbox
		err     error
	}, 2)
	for _, database := range []*SQLite{first, second} {
		go func(database *SQLite) {
			<-start
			mailbox, err := database.ResolveMailbox(context.Background(), identity, repo)
			results <- struct {
				mailbox model.Mailbox
				err     error
			}{mailbox, err}
		}(database)
	}
	close(start)
	one, two := <-results, <-results
	if one.err != nil || two.err != nil {
		t.Fatalf("resolve errors = %v, %v", one.err, two.err)
	}
	if one.mailbox.ID != two.mailbox.ID {
		t.Fatalf("mailboxes = %s, %s", one.mailbox.ID, two.mailbox.ID)
	}
}

func TestMessageReplyAndDeliveryLifecycle(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "one", "/repo")
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", agent.ID, model.HumanMailboxID, "Which port?")
	if err := s.Create(ctx, inbound); err != nil {
		t.Fatal(err)
	}
	replyTo := inbound.ID
	reply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", model.HumanMailboxID, agent.ID, "8080")
	reply.ReplyTo = &replyTo
	if err := s.Reply(ctx, inbound.ID, reply); err != nil {
		t.Fatal(err)
	}
	gotInbound, err := s.Get(ctx, inbound.ID)
	if err != nil {
		t.Fatal(err)
	}
	if gotInbound.ArchivedAt == nil {
		t.Fatal("reply did not archive inbound message")
	}
	claimed, err := s.Claim(ctx, Claim{ReplyTo: inbound.ID, RecipientMailboxID: agent.ID}, "consumer")
	if err != nil {
		t.Fatal(err)
	}
	if claimed.Body != "8080" {
		t.Fatalf("claimed body = %q", claimed.Body)
	}
	if _, err := s.Claim(ctx, Claim{MessageID: reply.ID}, "other"); !errors.Is(err, ErrNotReady) {
		t.Fatalf("second claim = %v", err)
	}
	if err := s.Complete(ctx, reply.ID, "consumer"); err != nil {
		t.Fatal(err)
	}
	gotReply, err := s.Get(ctx, reply.ID)
	if err != nil {
		t.Fatal(err)
	}
	if gotReply.CompletedAt == nil || gotReply.ArchivedAt == nil {
		t.Fatalf("completed reply = %#v", gotReply)
	}
}

func TestClaimCanExcludeReservedReplies(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "reserved-reply", "/repo")
	question := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d71", agent.ID, model.HumanMailboxID, "Approve?")
	if err := s.Create(ctx, question); err != nil {
		t.Fatal(err)
	}
	reply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d72", model.HumanMailboxID, agent.ID, "yes")
	if err := s.Reply(ctx, question.ID, reply); err != nil {
		t.Fatal(err)
	}
	ordinary := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d73", model.HumanMailboxID, agent.ID, "ordinary")
	if err := s.Create(ctx, ordinary); err != nil {
		t.Fatal(err)
	}
	claimed, err := s.Claim(ctx, Claim{RecipientMailboxID: agent.ID, ExcludeReplyTo: []string{question.ID}}, "consumer")
	if err != nil {
		t.Fatal(err)
	}
	if claimed.ID != ordinary.ID {
		t.Fatalf("claimed reserved reply %s instead of ordinary message %s", claimed.ID, ordinary.ID)
	}
	if err := s.Release(ctx, claimed.ID, "consumer"); err != nil {
		t.Fatal(err)
	}
	reserved, err := s.Claim(ctx, Claim{ReplyTo: question.ID, RecipientMailboxID: agent.ID}, "reply-consumer")
	if err != nil || reserved.ID != reply.ID {
		t.Fatalf("reserved reply = %#v, %v", reserved, err)
	}
}

func TestMailboxFilters(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "one", "/repo")
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", agent.ID, model.HumanMailboxID, "inbox")
	outbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", model.HumanMailboxID, agent.ID, "sent")
	for _, item := range []model.Message{inbound, outbound} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	open := false
	inbox, err := s.List(ctx, model.Filter{RecipientMailboxID: model.HumanMailboxID, Archived: &open})
	if err != nil {
		t.Fatal(err)
	}
	if len(inbox) != 1 || inbox[0].SenderLabel != agent.Label {
		t.Fatalf("inbox = %#v", inbox)
	}
	sent, err := s.List(ctx, model.Filter{SenderMailboxID: model.HumanMailboxID})
	if err != nil {
		t.Fatal(err)
	}
	if len(sent) != 1 || sent[0].RecipientLabel != agent.Label {
		t.Fatalf("sent = %#v", sent)
	}
	if sent[0].Purpose != model.MessagePurposeConversation || sent[0].SenderAddress.Kind != model.MailboxHuman || sent[0].RecipientAddress.Kind != model.MailboxAgent || sent[0].RecipientAddress.Harness != "codex" || sent[0].RecipientAddress.Label != "codex" {
		t.Fatalf("typed message projection = %#v", sent[0])
	}
	if err := s.Archive(ctx, outbound.ID); !errors.Is(err, ErrAlreadyHandled) {
		t.Fatalf("archive agent message = %v", err)
	}
	if err := s.Archive(ctx, inbound.ID); err != nil {
		t.Fatal(err)
	}
	if err := s.Restore(ctx, inbound.ID); err != nil {
		t.Fatal(err)
	}
	restored, err := s.Get(ctx, inbound.ID)
	if err != nil || restored.ArchivedAt != nil {
		t.Fatalf("restored message = %#v, %v", restored, err)
	}
	if err := s.Restore(ctx, inbound.ID); !errors.Is(err, ErrAlreadyHandled) {
		t.Fatalf("restore open message = %v", err)
	}
	if err := s.Archive(ctx, inbound.ID); err != nil {
		t.Fatalf("rearchive restored message: %v", err)
	}
}

func TestMessageConversationCorrelationFilters(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	firstAgent := resolveAgent(t, s, "codex", "correlation-one", "/repo")
	secondAgent := resolveAgent(t, s, "codex", "correlation-two", "/repo")
	firstInbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da1", firstAgent.ID, model.HumanMailboxID, "first inbound")
	firstInbound.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "shared-thread", OperationID: "first-turn"}
	firstOutbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da2", model.HumanMailboxID, firstAgent.ID, "first outbound")
	firstOutbound.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "shared-thread", OperationID: "second-turn"}
	secondInbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da3", secondAgent.ID, model.HumanMailboxID, "second inbound")
	secondInbound.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "shared-thread", OperationID: "first-turn"}
	uncorrelated := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da4", firstAgent.ID, model.HumanMailboxID, "uncorrelated")
	for _, item := range []model.Message{firstInbound, firstOutbound, secondInbound, uncorrelated} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}

	got, err := s.List(ctx, model.Filter{HarnessSessionID: "shared-thread", CounterpartyMailboxID: firstAgent.ID, Limit: 10})
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 || got[0].Body != "first inbound" || got[1].Body != "first outbound" {
		t.Fatalf("first conversation = %#v", got)
	}
	for _, item := range got {
		if item.HarnessSessionID != "shared-thread" || item.HarnessOperationID == "" {
			t.Fatalf("projected correlation = %#v", item)
		}
	}

	turn, err := s.List(ctx, model.Filter{HarnessSessionID: "shared-thread", HarnessOperationID: "first-turn", CounterpartyMailboxID: secondAgent.ID})
	if err != nil || len(turn) != 1 || turn[0].ID != secondInbound.ID {
		t.Fatalf("turn filter = %#v, %v", turn, err)
	}
	root, err := s.Get(ctx, uncorrelated.ID)
	if err != nil {
		t.Fatal(err)
	}
	canonical, err := s.List(ctx, model.Filter{ThreadID: root.ThreadID})
	if err != nil || len(canonical) != 1 || canonical[0].ID != uncorrelated.ID {
		t.Fatalf("canonical thread filter = %#v, %v", canonical, err)
	}
}

func TestTypedMessageSemanticsSurviveCreateReplyRestartAndRebuild(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, database)
	ctx := context.Background()
	agent := resolveAgent(t, s, "home-built", "typed-round-trip", "/repo")
	original := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da5", agent.ID, model.HumanMailboxID, "typed original")
	original.Details = "Kind: notice\nHarness session: this stays human text"
	original.Presentation = model.PresentationFinalAnswer
	original.Correlation = model.MessageCorrelation{Provider: "home-built", SessionID: "session-1", OperationID: "operation-1", ItemID: "item-1", RequestID: "request-1"}
	original.TechnicalSections = []model.TechnicalSection{
		{Namespace: "vendor.first", Fields: []model.TechnicalField{{Key: "zeta", Label: "Zeta", Value: "one"}, {Key: "alpha", Value: "two"}}},
		{Namespace: "hq.harness.output", Fields: []model.TechnicalField{{Key: "status", Label: "Status", Value: "completed"}}},
	}
	if err := s.Create(ctx, original); err != nil {
		t.Fatal(err)
	}
	stored, err := s.Get(ctx, original.ID)
	if err != nil {
		t.Fatal(err)
	}
	assertMessageSemantics(t, stored, original)
	filtered, err := s.List(ctx, model.Filter{HarnessProvider: original.Correlation.Provider, HarnessSessionID: original.Correlation.SessionID, HarnessOperationID: original.Correlation.OperationID})
	if err != nil || len(filtered) != 1 || filtered[0].ID != original.ID {
		t.Fatalf("typed correlation filter = %#v, %v", filtered, err)
	}

	reply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da6", model.HumanMailboxID, agent.ID, "typed reply")
	if err := s.Reply(ctx, original.ID, reply); err != nil {
		t.Fatal(err)
	}
	storedReply, err := s.Get(ctx, reply.ID)
	if err != nil {
		t.Fatal(err)
	}
	if storedReply.Correlation != original.Correlation {
		t.Fatalf("inherited reply correlation = %#v, want %#v", storedReply.Correlation, original.Correlation)
	}

	if _, err := s.db.Exec(`UPDATE projection_checkpoint SET event_count=-1 WHERE id=1`); err != nil {
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
	rebuilt, err := reopened.Get(ctx, original.ID)
	if err != nil {
		t.Fatal(err)
	}
	assertMessageSemantics(t, rebuilt, original)
	rebuiltReply, err := reopened.Get(ctx, reply.ID)
	if err != nil || rebuiltReply.Correlation != original.Correlation {
		t.Fatalf("rebuilt reply = %#v, %v", rebuiltReply, err)
	}
}

func TestTypedReplyKeepsExplicitCorrelation(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "home-built", "explicit-reply-correlation", "/repo")
	original := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da7", agent.ID, model.HumanMailboxID, "question")
	original.Correlation = model.MessageCorrelation{Provider: "home-built", SessionID: "session-1", OperationID: "operation-1"}
	if err := s.Create(ctx, original); err != nil {
		t.Fatal(err)
	}
	reply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140da8", model.HumanMailboxID, agent.ID, "answer")
	reply.Correlation = model.MessageCorrelation{Provider: "home-built", SessionID: "session-2", OperationID: "operation-2", ItemID: "item-2", RequestID: "request-2"}
	if err := s.Reply(ctx, original.ID, reply); err != nil {
		t.Fatal(err)
	}
	stored, err := s.Get(ctx, reply.ID)
	if err != nil || stored.Correlation != reply.Correlation {
		t.Fatalf("explicit reply correlation = %#v, %v", stored, err)
	}
}

func assertMessageSemantics(t *testing.T, got, want model.Message) {
	t.Helper()
	if got.Presentation != want.Presentation || got.Correlation != want.Correlation || got.Details != want.Details || got.Purpose != model.NormalizeMessagePurpose(want.Purpose) || got.Context != want.Context || !reflect.DeepEqual(got.TechnicalSections, want.TechnicalSections) {
		t.Fatalf("message semantics\ngot:  %#v\nwant: %#v", got, want)
	}
}

func payloadMessageID(t *testing.T, raw json.RawMessage) string {
	t.Helper()
	var payload event.TextPayload
	if err := json.Unmarshal(raw, &payload); err != nil {
		t.Fatal(err)
	}
	return payload.MessageID
}

func TestConversationSummariesFilterUnionAndPagination(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "conversation-summary", "/repo")
	other := resolveAgent(t, s, "codex", "conversation-summary-other", "/repo")
	created := time.Now().UTC().Add(-time.Minute)
	open := message("0198c7ec-73b0-7cc3-a5f7-e31c77140dc1", agent.ID, model.HumanMailboxID, "open")
	open.CreatedAt = created
	open.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "open-thread", OperationID: "turn-one"}
	openReply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140dc4", model.HumanMailboxID, agent.ID, "open conversation reply")
	openReply.CreatedAt = created.Add(time.Second)
	openReply.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "open-thread", OperationID: "turn-one"}
	sent := message("0198c7ec-73b0-7cc3-a5f7-e31c77140dc2", model.HumanMailboxID, agent.ID, "sent only")
	sent.CreatedAt = created
	sent.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "sent-thread"}
	archived := message("0198c7ec-73b0-7cc3-a5f7-e31c77140dc3", other.ID, model.HumanMailboxID, "archived only")
	archived.CreatedAt = created
	archived.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "archived-thread"}
	for _, item := range []model.Message{open, openReply, sent, archived} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Archive(ctx, archived.ID); err != nil {
		t.Fatal(err)
	}

	defaultPage, err := s.ListConversations(ctx, model.ConversationFilter{})
	if err != nil || len(defaultPage.Conversations) != 1 || defaultPage.Conversations[0].Key.HarnessSessionID != "open-thread" || defaultPage.Conversations[0].OpenCount != 1 {
		t.Fatalf("default conversations = %#v, %v", defaultPage, err)
	}
	first, err := s.ListConversations(ctx, model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Limit: 1})
	if err != nil || len(first.Conversations) != 1 || first.NextCursor == "" {
		t.Fatalf("first conversation page = %#v, %v", first, err)
	}
	second, err := s.ListConversations(ctx, model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Limit: 1, Cursor: first.NextCursor})
	if err != nil || len(second.Conversations) != 1 || second.NextCursor == "" || second.Conversations[0].Key == first.Conversations[0].Key {
		t.Fatalf("second conversation page = %#v, %v", second, err)
	}
	third, err := s.ListConversations(ctx, model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Limit: 1, Cursor: second.NextCursor})
	if err != nil || len(third.Conversations) != 1 || third.NextCursor != "" {
		t.Fatalf("third conversation page = %#v, %v", third, err)
	}
	seen := map[string]bool{
		first.Conversations[0].Key.HarnessSessionID:  true,
		second.Conversations[0].Key.HarnessSessionID: true,
		third.Conversations[0].Key.HarnessSessionID:  true,
	}
	for _, thread := range []string{"open-thread", "sent-thread", "archived-thread"} {
		if !seen[thread] {
			t.Fatalf("paginated union omitted %q: %#v", thread, seen)
		}
	}
	if _, err := s.ListConversations(ctx, model.ConversationFilter{Cursor: "malformed"}); err == nil {
		t.Fatal("malformed conversation cursor was accepted")
	}
}

func TestConversationHistoryIsCompleteChronologicalAndIsolated(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "conversation-history", "/repo")
	other := resolveAgent(t, s, "codex", "conversation-history-other", "/repo")
	created := time.Now().UTC().Add(-time.Minute)
	items := []model.Message{
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140dd1", agent.ID, model.HumanMailboxID, "first inbound"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140dd2", model.HumanMailboxID, agent.ID, "human reply"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140dd3", agent.ID, model.HumanMailboxID, "second inbound"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140dd4", other.ID, model.HumanMailboxID, "other agent"),
	}
	for index := range items {
		items[index].CreatedAt = created.Add(time.Duration(index) * time.Second)
		items[index].Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "shared-thread", OperationID: "turn-" + fmt.Sprint(index)}
		if err := s.Create(ctx, items[index]); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Archive(ctx, items[0].ID); err != nil {
		t.Fatal(err)
	}
	key := model.ConversationKey{CounterpartyMailboxID: agent.ID, HarnessProvider: "codex", HarnessSessionID: "shared-thread"}
	first, err := s.ListConversationHistory(ctx, model.ConversationHistoryFilter{Key: key, Limit: 2})
	if err != nil || len(first.Messages) != 2 || first.NextCursor == "" {
		t.Fatalf("first history page = %#v, %v", first, err)
	}
	second, err := s.ListConversationHistory(ctx, model.ConversationHistoryFilter{Key: key, Limit: 2, Cursor: first.NextCursor})
	if err != nil || len(second.Messages) != 1 || second.NextCursor != "" {
		t.Fatalf("second history page = %#v, %v", second, err)
	}
	got := append(first.Messages, second.Messages...)
	for index, body := range []string{"first inbound", "human reply", "second inbound"} {
		if got[index].Body != body {
			t.Fatalf("history[%d] = %q; want %q", index, got[index].Body, body)
		}
	}
	if got[0].ArchivedAt == nil || got[1].SenderMailboxID != model.HumanMailboxID {
		t.Fatalf("history lost archive/direction state: %#v", got)
	}
	if _, err := s.ListConversationHistory(ctx, model.ConversationHistoryFilter{Key: key, Cursor: "malformed"}); err == nil {
		t.Fatal("malformed history cursor was accepted")
	}
}

func TestConversationHistoryCanonicalFallback(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "conversation-fallback", "/repo")
	question := message("0198c7ec-73b0-7cc3-a5f7-e31c77140de1", agent.ID, model.HumanMailboxID, "question")
	if err := s.Create(ctx, question); err != nil {
		t.Fatal(err)
	}
	reply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140de2", model.HumanMailboxID, agent.ID, "answer")
	if err := s.Reply(ctx, question.ID, reply); err != nil {
		t.Fatal(err)
	}
	root, err := s.Get(ctx, question.ID)
	if err != nil {
		t.Fatal(err)
	}
	page, err := s.ListConversations(ctx, model.ConversationFilter{IncludeSent: true, IncludeArchived: true})
	if err != nil || len(page.Conversations) != 1 || page.Conversations[0].Key.ThreadID != root.ThreadID {
		t.Fatalf("fallback summary = %#v, %v", page, err)
	}
	history, err := s.ListConversationHistory(ctx, model.ConversationHistoryFilter{Key: page.Conversations[0].Key})
	if err != nil || len(history.Messages) != 2 || history.Messages[0].ID != question.ID || history.Messages[1].ID != reply.ID {
		t.Fatalf("fallback history = %#v, %v", history, err)
	}
}

func TestHarnessConversationsNamespaceProviderLocalSessionIDs(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent, err := s.CreateNamedAgent(ctx, "multi-provider", "")
	if err != nil {
		t.Fatal(err)
	}
	for index, provider := range []string{"alpha", "beta"} {
		if _, err := s.SelectNamedAgentSession(ctx, agent.Name, model.SessionIdentity{Harness: provider, ExternalSessionID: "shared-session"}, model.RepositoryContext{Directory: "/repo"}); err != nil {
			t.Fatal(err)
		}
		item := message(fmt.Sprintf("0198c7ec-73b0-7cc3-a5f7-e31c77140d8%d", index), agent.MailboxID, model.HumanMailboxID, provider+" output")
		item.Correlation = model.MessageCorrelation{Provider: provider, SessionID: "shared-session", OperationID: "operation-1"}
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	page, err := s.ListConversations(ctx, model.ConversationFilter{})
	if err != nil || len(page.Conversations) != 2 {
		t.Fatalf("namespaced conversations = %#v, %v", page, err)
	}
	providers := map[string]bool{}
	for _, conversation := range page.Conversations {
		providers[conversation.Key.HarnessProvider] = true
		if conversation.Key.HarnessSessionID != "shared-session" {
			t.Fatalf("conversation key = %#v", conversation.Key)
		}
	}
	if !providers["alpha"] || !providers["beta"] {
		t.Fatalf("conversation providers = %#v", providers)
	}
}

func TestOpenRejectsOlderAndUnversionedDatabases(t *testing.T) {
	for _, test := range []struct {
		name   string
		schema string
		want   string
	}{
		{name: "older version", schema: `CREATE TABLE old_events(id TEXT); PRAGMA user_version=32`, want: "unsupported HQ database schema 32"},
		{name: "unversioned", schema: `CREATE TABLE old_events(id TEXT)`, want: "unsupported unversioned HQ database"},
	} {
		t.Run(test.name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "hq.db")
			initialized := openStore(t, path)
			if err := initialized.Close(); err != nil {
				t.Fatal(err)
			}
			db, err := sql.Open("sqlite", path)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := db.Exec(`PRAGMA user_version=0; ` + test.schema); err != nil {
				t.Fatal(err)
			}
			if err := db.Close(); err != nil {
				t.Fatal(err)
			}
			if _, err := Open(path); err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("open error = %v, want %q", err, test.want)
			}
		})
	}
}

func TestMutationReceiptPersistsResultAndRejectsKeyReuse(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	mutation := domain.Mutation{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d98", Method: "mailbox/resolve", RequestDigest: strings.Repeat("a", 64)}
	ctx := domain.WithMutation(context.Background(), mutation)
	mailbox, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "receipt"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	raw, found, err := s.MutationResult(context.Background(), mutation)
	if err != nil || !found {
		t.Fatalf("mutation result found=%t err=%v", found, err)
	}
	var persisted model.Mailbox
	if err := json.Unmarshal(raw, &persisted); err != nil || persisted.ID != mailbox.ID {
		t.Fatalf("persisted mailbox = %#v, %v", persisted, err)
	}
	conflict := mutation
	conflict.RequestDigest = strings.Repeat("b", 64)
	if _, _, err := s.MutationResult(context.Background(), conflict); err == nil || !strings.Contains(err.Error(), "different request") {
		t.Fatalf("mutation key conflict = %v", err)
	}
}

func TestChangeRevisionAdvancesWithCommittedTopics(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	if revision, err := s.CurrentRevision(ctx); err != nil || revision != 0 {
		t.Fatalf("initial revision = %d, %v", revision, err)
	}
	var changes []domain.Invalidation
	s.SetChangeObserver(func(change domain.Invalidation) { changes = append(changes, change) })
	agent, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "revision"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	message := model.Message{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d97", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.ID, Body: "revision", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, message); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Claim(ctx, Claim{MessageID: message.ID}, "revision-token"); err != nil {
		t.Fatal(err)
	}
	if err := s.Release(ctx, message.ID, "revision-token"); err != nil {
		t.Fatal(err)
	}
	if revision, err := s.CurrentRevision(ctx); err != nil || revision != 4 {
		t.Fatalf("final revision = %d, %v", revision, err)
	}
	if len(changes) != 4 || changes[0].Topics[0] != domain.TopicMessages || changes[2].Topics[0] != domain.TopicMessages {
		t.Fatalf("changes = %#v", changes)
	}
}

func TestOpenRejectsMismatchedIdentityKey(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	s := openStore(t, database)
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	keyPath, err := identity.KeyPath(database)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(keyPath); err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := Open(database); err == nil || !strings.Contains(err.Error(), "does not match") {
		t.Fatalf("mismatched identity open = %v", err)
	}
}

func TestSignedEventLogRebuildAndTransactionRollback(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "signed", "/repo")
	item := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d71", agent.ID, model.HumanMailboxID, "signed question")
	if err := s.Create(ctx, item); err != nil {
		t.Fatal(err)
	}
	rows, err := s.db.Query(`SELECT event_id, raw, reduction_status FROM canonical_events`)
	if err != nil {
		t.Fatal(err)
	}
	count := 0
	for rows.Next() {
		var id, status string
		var raw []byte
		if err := rows.Scan(&id, &raw, &status); err != nil {
			t.Fatal(err)
		}
		inspection := event.Inspect(raw)
		if inspection.Event.ID() != id || !inspection.Event.Nostr.VerifySignature() || status != string(event.StatusProjected) {
			t.Fatalf("canonical row = %s, %s, %#v", id, status, inspection)
		}
		count++
	}
	rows.Close()
	if count < 6 {
		t.Fatalf("canonical row count = %d", count)
	}
	if _, err := s.db.Exec(`DELETE FROM messages`); err != nil {
		t.Fatal(err)
	}
	if err := s.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	if got, err := s.Get(ctx, item.ID); err != nil || got.Body != item.Body {
		t.Fatalf("rebuilt message = %#v, %v", got, err)
	}
	var before int
	if err := s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&before); err != nil {
		t.Fatal(err)
	}
	if _, err := s.db.Exec(`CREATE TRIGGER fail_projection BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL, 'forced projection failure'); END`); err != nil {
		t.Fatal(err)
	}
	bad := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d72", agent.ID, model.HumanMailboxID, "must roll back")
	if err := s.Create(ctx, bad); err == nil {
		t.Fatal("forced projection failure succeeded")
	}
	var after int
	if err := s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&after); err != nil {
		t.Fatal(err)
	}
	if after != before {
		t.Fatalf("failed transaction changed event count from %d to %d", before, after)
	}
}

func TestCanonicalAppendDeduplicatesAndRetainsMissingParent(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "orphan", "/repo")
	var raw []byte
	if err := s.db.QueryRow(`SELECT raw FROM canonical_events ORDER BY event_id LIMIT 1`).Scan(&raw); err != nil {
		t.Fatal(err)
	}
	duplicate := event.Inspect(raw).Event
	var before int
	_ = s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&before)
	if err := s.AppendCanonical(ctx, []event.SignedEvent{duplicate}); err != nil {
		t.Fatal(err)
	}
	var after int
	_ = s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&after)
	if after != before {
		t.Fatalf("duplicate changed event count from %d to %d", before, after)
	}
	missing := strings.Repeat("a", 64)
	payload, _ := event.MarshalPayload(event.TextPayload{MessageID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d77", Body: "orphan answer", Context: &event.RepositoryContext{Directory: "/repo"}})
	content := event.Content{Type: event.TypeAnswer, Sender: &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: model.HumanMailboxID}, Recipient: &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: agent.ID}, ThreadID: missing, Parents: []string{missing}, Scope: event.ScopeInstallationPrivate, Payload: payload}
	orphan, err := s.signer.Sign(ctx, content, time.Now().UTC())
	if err != nil {
		t.Fatal(err)
	}
	if err := s.AppendCanonical(ctx, []event.SignedEvent{orphan}); err != nil {
		t.Fatal(err)
	}
	got, err := s.Get(ctx, "0198c7ec-73b0-7cc3-a5f7-e31c77140d77")
	if err != nil || !got.Incomplete || got.EventID != orphan.ID() {
		t.Fatalf("orphan projection = %#v, %v", got, err)
	}
}

func TestUnknownPeerCannotAppendCanonicalState(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	peerSecret := event.MustSecretKeyFromHex("88")
	peerInstallation := "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	payload, _ := event.MarshalPayload(event.TextPayload{MessageID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d78", Body: "forged", Context: &event.RepositoryContext{Directory: "/repo"}})
	missingAuthority := strings.Repeat("b", 64)
	content := event.Content{Type: event.TypeMessage, InstallationID: peerInstallation, Sender: &event.MailboxAddress{InstallationID: peerInstallation, MailboxID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d12"}, Recipient: &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: model.HumanMailboxID}, Parents: []string{missingAuthority}, Authorities: []string{missingAuthority}, Scope: event.ScopePeerAddressed, Payload: payload}
	forged, err := event.Sign(content, time.Now().UTC(), peerSecret)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.AppendCanonical(context.Background(), []event.SignedEvent{forged}); err == nil {
		t.Fatal("unknown peer changed canonical state")
	}
	if _, err := s.Get(context.Background(), "0198c7ec-73b0-7cc3-a5f7-e31c77140d78"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("unknown peer message = %v", err)
	}
}

func TestPeerAddressedAppendDerivesExactOutboxAtomically(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	peerInstallation := "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	peerMailbox := "0198c7ec-73b0-7cc3-a5f7-e31c77140d12"
	peerSecret := event.MustSecretKeyFromHex("89")
	if err := s.TrustPeer(ctx, Peer{InstallationID: peerInstallation, SignerKeyID: peerSecret.PublicKeyHex()}); err != nil {
		t.Fatal(err)
	}
	grantPayload, _ := event.MarshalPayload(event.MailboxAccessPayload{MailboxID: peerMailbox, GranteeInstallationID: s.signer.InstallationID, GranteeSignerKeyID: s.signer.PublicKey()})
	grant, err := event.Sign(event.Content{Type: event.TypeMailboxAccessGrant, InstallationID: peerInstallation, Sender: &event.MailboxAddress{InstallationID: peerInstallation, MailboxID: model.HumanMailboxID}, Recipient: &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: model.HumanMailboxID}, Scope: event.ScopePeerAddressed, Payload: grantPayload}, time.Now().UTC(), peerSecret)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.AppendCanonical(ctx, []event.SignedEvent{grant}); err != nil {
		t.Fatal(err)
	}
	if _, err := s.db.Exec(`DELETE FROM outbox`); err != nil {
		t.Fatal(err)
	}
	makeEvent := func(messageID, body string) event.SignedEvent {
		payload, _ := event.MarshalPayload(event.TextPayload{MessageID: messageID, Body: body, Context: &event.RepositoryContext{Directory: "/repo"}})
		content := event.Content{Type: event.TypeMessage, Sender: &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: model.HumanMailboxID}, Recipient: &event.MailboxAddress{InstallationID: peerInstallation, MailboxID: peerMailbox}, Parents: []string{grant.ID()}, Authorities: []string{grant.ID()}, Scope: event.ScopePeerAddressed, Payload: payload}
		item, err := s.signer.Sign(ctx, content, time.Now().UTC())
		if err != nil {
			t.Fatal(err)
		}
		return item
	}
	first := makeEvent("0198c7ec-73b0-7cc3-a5f7-e31c77140d79", "remote one")
	if err := s.AppendCanonical(ctx, []event.SignedEvent{first}); err != nil {
		t.Fatal(err)
	}
	if _, err := s.db.Exec(`DELETE FROM outbox WHERE event_id IN (SELECT event_id FROM canonical_events WHERE event_type IN (?, ?, ?))`, event.TypeMailboxAccessGrant, event.TypeMailboxAccessRevoke, event.TypeMailboxAccessObserve); err != nil {
		t.Fatal(err)
	}
	jobs, err := s.PendingOutbox(ctx, 10)
	if err != nil || len(jobs) != 1 || jobs[0].EventID != first.ID() || string(jobs[0].ExactCanonicalBytes) != string(first.Wire) {
		t.Fatalf("outbox = %#v, %v", jobs, err)
	}
	var before int
	_ = s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&before)
	if _, err := s.db.Exec(`CREATE TRIGGER fail_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(FAIL, 'forced outbox failure'); END`); err != nil {
		t.Fatal(err)
	}
	second := makeEvent("0198c7ec-73b0-7cc3-a5f7-e31c77140d80", "remote two")
	if err := s.AppendCanonical(ctx, []event.SignedEvent{second}); err == nil {
		t.Fatal("forced outbox failure succeeded")
	}
	var after int
	_ = s.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&after)
	if after != before {
		t.Fatalf("outbox failure changed event count from %d to %d", before, after)
	}
}

func TestPeerBindingAndMailboxAccessAreSigned(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	agent := resolveAgent(t, s, "codex", "share", "/repo")
	peerID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	peerKey := event.MustSecretKeyFromHex("99").PublicKeyHex()
	if err := s.TrustPeer(ctx, Peer{InstallationID: peerID, SignerKeyID: peerKey, Name: "server", Relays: []string{"wss://relay.example"}}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetMailboxShare(ctx, agent.ID, peerID, true); err != nil {
		t.Fatal(err)
	}
	var active int
	if err := s.db.QueryRow(`SELECT active FROM mailbox_access WHERE mailbox_id=? AND grantee_installation_id=?`, agent.ID, peerID).Scan(&active); err != nil || active != 1 {
		t.Fatalf("active share = %d, %v", active, err)
	}
	if err := s.SetMailboxShare(ctx, agent.ID, peerID, false); err != nil {
		t.Fatal(err)
	}
	if err := s.DistrustPeer(ctx, peerID); err != nil {
		t.Fatal(err)
	}
	peers, err := s.ListPeers(ctx)
	if err != nil || len(peers) != 1 || peers[0].Trusted {
		t.Fatalf("peers = %#v, %v", peers, err)
	}
	if err := s.db.QueryRow(`SELECT active FROM mailbox_access WHERE mailbox_id=? AND grantee_installation_id=?`, agent.ID, peerID).Scan(&active); err != nil || active != 0 {
		t.Fatalf("revoked share = %d, %v", active, err)
	}
}

func TestStagingAndBoundedQuarantine(t *testing.T) {
	s := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	ctx := context.Background()
	now := time.Now().UTC()
	if err := s.Stage(ctx, []byte("retry"), "wss://relay", "event", "key locked", now, now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	for index := 0; index < quarantineMaxRows+5; index++ {
		if err := s.Quarantine(ctx, []byte("bad"), "wss://relay", "event", "bad signature", now.Add(time.Duration(index)*time.Millisecond)); err != nil {
			t.Fatal(err)
		}
	}
	var staged, quarantined int
	if err := s.db.QueryRow(`SELECT count(*) FROM inbound_staging`).Scan(&staged); err != nil {
		t.Fatal(err)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM quarantine`).Scan(&quarantined); err != nil {
		t.Fatal(err)
	}
	if staged != 1 || quarantined != quarantineMaxRows {
		t.Fatalf("staging=%d quarantine=%d", staged, quarantined)
	}
	var quarantineID int64
	if err := s.db.QueryRow(`SELECT id FROM quarantine ORDER BY id LIMIT 1`).Scan(&quarantineID); err != nil {
		t.Fatal(err)
	}
	if err := s.ReevaluateQuarantine(ctx, quarantineID, now); err != nil {
		t.Fatal(err)
	}
	if err := s.db.QueryRow(`SELECT count(*) FROM inbound_staging`).Scan(&staged); err != nil || staged != 2 {
		t.Fatalf("re-evaluated staging=%d, %v", staged, err)
	}
}

func openStore(t *testing.T, path string) *SQLite {
	t.Helper()
	key, err := identity.KeyPath(path)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Load(key); errors.Is(err, identity.ErrNotInitialized) {
		if _, err := identity.Initialize(key, nil); err != nil {
			t.Fatal(err)
		}
	}
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func resolveAgent(t *testing.T, s *SQLite, harness, externalID, directory string) model.Mailbox {
	t.Helper()
	m, err := s.ResolveMailbox(context.Background(), model.SessionIdentity{Harness: harness, ExternalSessionID: externalID}, model.RepositoryContext{Directory: directory})
	if err != nil {
		t.Fatal(err)
	}
	return m
}

func message(id, sender, recipient, body string) model.Message {
	return model.Message{ID: id, Context: model.RepositoryContext{Directory: "/repo"}, SenderMailboxID: sender, RecipientMailboxID: recipient, Body: strings.TrimSpace(body), CreatedAt: time.Now().UTC()}
}
