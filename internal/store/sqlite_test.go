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

	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	_ "modernc.org/sqlite"
)

func TestSQLiteConfigurationAndSchema(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state", "hq.db")
	s := openStore(t, path)
	checks := map[string]string{"PRAGMA journal_mode": "wal", "PRAGMA synchronous": "2", "PRAGMA foreign_keys": "1", "PRAGMA trusted_schema": "0", "PRAGMA integrity_check": "ok", "PRAGMA user_version": "7"}
	for query, want := range checks {
		var got string
		if err := s.db.QueryRow(query).Scan(&got); err != nil {
			t.Fatalf("%s: %v", query, err)
		}
		if got != want {
			t.Errorf("%s = %q, want %q", query, got, want)
		}
	}
	for _, table := range []string{"canonical_events", "causal_edges", "projection_checkpoint", "mailboxes", "harness_bindings", "mailbox_contexts", "messages", "threads", "peers", "mailbox_shares", "human_accounts", "human_account_devices", "human_account_default", "outbox", "relays", "outbound_relay_attempts", "inbound_wrappers", "relay_sync_state", "inbound_staging", "quarantine"} {
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
	if version != 7 {
		t.Fatalf("user_version = %d", version)
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
	content := event.Content{Type: event.TypeMessage, InstallationID: peerInstallation, Sender: &event.MailboxAddress{InstallationID: peerInstallation, MailboxID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d12"}, Recipient: &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: model.HumanMailboxID}, Scope: event.ScopePeerAddressed, Payload: payload}
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
	makeEvent := func(messageID, body string) event.SignedEvent {
		payload, _ := event.MarshalPayload(event.TextPayload{MessageID: messageID, Body: body, Context: &event.RepositoryContext{Directory: "/repo"}})
		content := event.Content{Type: event.TypeMessage, Sender: &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: model.HumanMailboxID}, Recipient: &event.MailboxAddress{InstallationID: peerInstallation, MailboxID: peerMailbox}, Scope: event.ScopePeerAddressed, Payload: payload}
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

func TestPeerTrustDistrustAndMailboxShareAreSigned(t *testing.T) {
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
	if err := s.db.QueryRow(`SELECT active FROM mailbox_shares WHERE mailbox_id=? AND peer_installation_id=?`, agent.ID, peerID).Scan(&active); err != nil || active != 1 {
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
	if err := s.db.QueryRow(`SELECT active FROM mailbox_shares WHERE mailbox_id=? AND peer_installation_id=?`, agent.ID, peerID).Scan(&active); err != nil || active != 0 {
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
