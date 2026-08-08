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

func TestSQLiteConfiguration(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state", "hq.db")
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	checks := map[string]string{
		"PRAGMA journal_mode": "wal", "PRAGMA synchronous": "2", "PRAGMA foreign_keys": "1",
		"PRAGMA trusted_schema": "0", "PRAGMA integrity_check": "ok",
	}
	for query, want := range checks {
		var got string
		if err := s.db.QueryRow(query).Scan(&got); err != nil {
			t.Fatalf("%s: %v", query, err)
		}
		if got != want {
			t.Errorf("%s = %q, want %q", query, got, want)
		}
	}
	for _, table := range []string{"mailboxes", "messages"} {
		var strict int
		if err := s.db.QueryRow(`SELECT strict FROM pragma_table_list WHERE name = ?`, table).Scan(&strict); err != nil {
			t.Fatal(err)
		}
		if strict != 1 {
			t.Fatalf("%s strict = %d", table, strict)
		}
	}
	rows, err := s.db.Query(`SELECT name, pk FROM pragma_table_info('messages') WHERE pk > 0 ORDER BY pk`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var primaryKey []string
	for rows.Next() {
		var name string
		var position int
		if err := rows.Scan(&name, &position); err != nil {
			t.Fatal(err)
		}
		primaryKey = append(primaryKey, name)
	}
	if strings.Join(primaryKey, ",") != "directory,recipient_session,id" {
		t.Fatalf("message primary key = %v", primaryKey)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("database mode = %o", info.Mode().Perm())
	}
}

func TestMessageReplyAndDeliveryLifecycle(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	ctx := context.Background()
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", "agent-1", model.HumanSession, "Which port?")
	if err := s.Create(ctx, inbound); err != nil {
		t.Fatal(err)
	}
	replyTo := inbound.ID
	reply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", model.HumanSession, "agent-1", "8080")
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
	claimed, err := s.Claim(ctx, Claim{ReplyTo: inbound.ID, Directory: "/repo", RecipientSession: "agent-1"}, "consumer")
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

func TestUnsolicitedMessageAndMailboxFilters(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	ctx := context.Background()
	old := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", "agent-1", model.HumanSession, "old inbox")
	newer := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", model.HumanSession, "agent-1", "unsolicited")
	newer.CreatedAt = old.CreatedAt.Add(time.Second)
	for _, m := range []model.Message{old, newer} {
		if err := s.Create(ctx, m); err != nil {
			t.Fatal(err)
		}
	}
	archived := false
	inbox, err := s.List(ctx, model.Filter{RecipientSession: model.HumanSession, Archived: &archived})
	if err != nil {
		t.Fatal(err)
	}
	if len(inbox) != 1 || inbox[0].Body != "old inbox" {
		t.Fatalf("inbox = %#v", inbox)
	}
	sent, err := s.List(ctx, model.Filter{SenderSession: model.HumanSession, NewestFirst: true})
	if err != nil {
		t.Fatal(err)
	}
	if len(sent) != 1 || sent[0].Body != "unsolicited" {
		t.Fatalf("sent = %#v", sent)
	}
	if err := s.Archive(ctx, newer.ID); !errors.Is(err, ErrAlreadyHandled) {
		t.Fatalf("archive agent mailbox message = %v", err)
	}
}

func TestLegacyQuestionsArePreservedButNotMigrated(t *testing.T) {
	path := filepath.Join(t.TempDir(), "hq.db")
	db, err := sql.Open("sqlite", path)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`CREATE TABLE questions(id TEXT); INSERT INTO questions VALUES ('old')`); err != nil {
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	var value string
	if err := s.db.QueryRow(`SELECT id FROM legacy_questions_v1`).Scan(&value); err != nil {
		t.Fatal(err)
	}
	if value != "old" {
		t.Fatalf("legacy value = %q", value)
	}
	var version int
	if err := s.db.QueryRow(`PRAGMA user_version`).Scan(&version); err != nil {
		t.Fatal(err)
	}
	if version != 2 {
		t.Fatalf("user_version = %d", version)
	}
}

func message(id, sender, recipient, body string) model.Message {
	return model.Message{ID: id, Directory: "/repo", SenderSession: sender, RecipientSession: recipient, Body: body, CreatedAt: time.Now().UTC()}
}
