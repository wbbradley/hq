package store

import (
	"context"
	"database/sql"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/model"
	_ "modernc.org/sqlite"
)

func TestSQLiteConfigurationAndSchema(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state", "hq.db")
	s := openStore(t, path)
	checks := map[string]string{"PRAGMA journal_mode": "wal", "PRAGMA synchronous": "2", "PRAGMA foreign_keys": "1", "PRAGMA trusted_schema": "0", "PRAGMA integrity_check": "ok", "PRAGMA user_version": "3"}
	for query, want := range checks {
		var got string
		if err := s.db.QueryRow(query).Scan(&got); err != nil {
			t.Fatalf("%s: %v", query, err)
		}
		if got != want {
			t.Errorf("%s = %q, want %q", query, got, want)
		}
	}
	for _, table := range []string{"mailboxes", "harness_bindings", "mailbox_contexts", "messages"} {
		var strict int
		if err := s.db.QueryRow(`SELECT strict FROM pragma_table_list WHERE name = ?`, table).Scan(&strict); err != nil {
			t.Fatal(err)
		}
		if strict != 1 {
			t.Fatalf("%s strict = %d", table, strict)
		}
	}
	var pk int
	if err := s.db.QueryRow(`SELECT pk FROM pragma_table_info('messages') WHERE name = 'id'`).Scan(&pk); err != nil {
		t.Fatal(err)
	}
	if pk != 1 {
		t.Fatalf("message id pk = %d", pk)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("database mode = %o", info.Mode().Perm())
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
	if _, err := s.db.Exec(`INSERT INTO mailboxes(id, kind, created_at, last_seen_at) VALUES ('0198c7ec-73b0-7cc3-a5f7-e31c77140d61', 'agent', ?, ?), ('0198c7ec-73b0-7cc3-a5f7-e31c77140d62', 'agent', ?, ?)`, now, now, now, now); err != nil {
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
	if err := s.Archive(ctx, outbound.ID); !errors.Is(err, ErrAlreadyHandled) {
		t.Fatalf("archive agent message = %v", err)
	}
}

func TestVersionTwoDataIsDestroyed(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	db, err := sql.Open("sqlite", path)
	if err != nil {
		t.Fatal(err)
	}
	_, err = db.Exec(`CREATE TABLE mailboxes(directory TEXT); CREATE TABLE messages(body TEXT); INSERT INTO messages VALUES ('old'); PRAGMA user_version = 2`)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	s := openStore(t, path)
	var count int
	if err := s.db.QueryRow(`SELECT count(*) FROM messages`).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if count != 0 {
		t.Fatalf("old messages left = %d", count)
	}
	var version int
	if err := s.db.QueryRow(`PRAGMA user_version`).Scan(&version); err != nil {
		t.Fatal(err)
	}
	if version != 3 {
		t.Fatalf("user_version = %d", version)
	}
}

func openStore(t *testing.T, path string) *SQLite {
	t.Helper()
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
