package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/agenthelp"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

func testApp(t *testing.T, input string) (*App, *bytes.Buffer) {
	t.Helper()
	out := new(bytes.Buffer)
	return &App{
		In: strings.NewReader(input), Out: out, ErrOut: new(bytes.Buffer),
		Getwd:  func() (string, error) { return "/work/repo", nil },
		Getenv: func(string) string { return "" },
		Open: func(path string) (store.Store, error) {
			initializeTestIdentity(t, path)
			return store.Open(path)
		},
		ReadPassword: func(string) ([]byte, error) { return []byte("test password"), nil },
		RepoContext: func(_ context.Context, directory string) model.RepositoryContext {
			return model.RepositoryContext{Directory: directory, GitCommonDir: "/work/main/.git", RemoteIdentity: "origin: team/repo", Worktree: "/work/repo", Branch: "main"}
		},
	}, out
}

func initializeTestIdentity(t *testing.T, database string) {
	t.Helper()
	key, err := identity.KeyPath(database)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Load(key); errors.Is(err, identity.ErrNotInitialized) {
		if _, err := identity.Initialize(key, nil); err != nil {
			t.Fatal(err)
		}
	}
}

func openTestStore(t *testing.T, database string) *store.SQLite {
	t.Helper()
	initializeTestIdentity(t, database)
	s, err := store.Open(database)
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func TestAgentsPrintsEmbeddedInstructionsWithoutOpeningStore(t *testing.T) {
	a, out := testApp(t, "")
	a.Open = func(string) (store.Store, error) { t.Fatal("agents opened store"); return nil, nil }
	if err := a.Run(context.Background(), []string{"agents"}); err != nil {
		t.Fatal(err)
	}
	if out.String() != agenthelp.Text {
		t.Fatal("agents output differs from source")
	}
	if strings.Contains(out.String(), "export HQ_SESSION") || strings.Contains(out.String(), "same session ID") {
		t.Fatal("agent help asks agents to manage session IDs")
	}
}

func TestBareCommandUsesTUIWhenInteractive(t *testing.T) {
	a, _ := testApp(t, "")
	a.IsTTY = func() bool { return true }
	called := false
	a.RunTUI = func(context.Context, store.Store, io.Reader, io.Writer) error { called = true; return nil }
	if err := a.Run(context.Background(), []string{"--db", filepath.Join(t.TempDir(), "hq.db")}); err != nil {
		t.Fatal(err)
	}
	if !called {
		t.Fatal("bare interactive command did not run TUI")
	}
}

func TestBareCommandListsOnlyOpenHumanInboxInDirectory(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	s := openTestStore(t, db)
	human, _ := s.HumanMailbox(ctx)
	agent, _ := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "a"}, model.RepositoryContext{Directory: "/work/repo"})
	items := []model.Message{
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", "/work/repo", agent.ID, human.ID, "archived"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", "/other", agent.ID, human.ID, "wrong dir"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d63", "/work/repo", human.ID, agent.ID, "sent"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d64", "/work/repo", agent.ID, human.ID, "open inbox"),
	}
	for _, item := range items {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Archive(ctx, items[0].ID); err != nil {
		t.Fatal(err)
	}
	s.Close()
	a, out := testApp(t, "")
	a.IsTTY = func() bool { return false }
	if err := a.Run(ctx, []string{"--db", db}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "open inbox") || strings.Contains(out.String(), "archived") || strings.Contains(out.String(), "sent") {
		t.Fatalf("bare output = %q", out.String())
	}
}

func TestDetectedHarnessesAskWithoutSetup(t *testing.T) {
	for env, harness := range map[string]string{"CODEX_THREAD_ID": "codex", "CLAUDE_CODE_SESSION_ID": "claude-code", "PI_SESSION_ID": "pi"} {
		t.Run(harness, func(t *testing.T) {
			db := filepath.Join(t.TempDir(), "hq.db")
			a, out := testApp(t, "")
			a.Getenv = func(name string) string {
				if name == env {
					return "session"
				}
				return ""
			}
			if err := a.Run(context.Background(), []string{"--db", db, "ask", "hello"}); err != nil {
				t.Fatal(err)
			}
			id := strings.TrimSpace(out.String())
			a, _ = testApp(t, "")
			if err := a.Run(context.Background(), []string{"--db", db, "answer", id, "reply"}); err != nil {
				t.Fatal(err)
			}
			a, out = testApp(t, "")
			a.Getenv = func(name string) string {
				if name == env {
					return "session"
				}
				return ""
			}
			if err := a.Run(context.Background(), []string{"--db", db, "wait", "--timeout", "1s", id}); err != nil {
				t.Fatal(err)
			}
			if out.String() != "reply\n" {
				t.Fatalf("wait = %q", out.String())
			}
			s := openTestStore(t, db)
			m, err := s.Get(context.Background(), id)
			if err != nil {
				t.Fatal(err)
			}
			if !strings.HasPrefix(m.SenderLabel, harness+":") {
				t.Fatalf("sender = %q", m.SenderLabel)
			}
			unsolicited := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d69", "/elsewhere", model.HumanMailboxID, m.SenderMailboxID, "poll me")
			if err := s.Create(context.Background(), unsolicited); err != nil {
				t.Fatal(err)
			}
			s.Close()
			a, out = testApp(t, "")
			a.Getenv = func(name string) string {
				if name == env {
					return "session"
				}
				return ""
			}
			if err := a.Run(context.Background(), []string{"--db", db, "poll"}); err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(out.String(), "poll me") {
				t.Fatalf("poll = %q", out.String())
			}
		})
	}
}

func TestAskAnswerWaitAndOwnership(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "one"})
	if err := a.Run(ctx, []string{"--db", db, "ask", "Choose a port"}); err != nil {
		t.Fatal(err)
	}
	id := strings.TrimSpace(out.String())
	a, _ = testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "answer", id, "8080"}); err != nil {
		t.Fatal(err)
	}
	a, _ = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "two"})
	if err := a.Run(ctx, []string{"--db", db, "wait", "--timeout", "1s", id}); err == nil || !strings.Contains(err.Error(), "another agent mailbox") {
		t.Fatalf("foreign wait = %v", err)
	}
	a, out = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "one"})
	if err := a.Run(ctx, []string{"--db", db, "wait", "--timeout", "1s", id}); err != nil {
		t.Fatal(err)
	}
	if out.String() != "8080\n" {
		t.Fatalf("reply = %q", out.String())
	}
}

func TestPollIsolatedBySessionAndWorksAcrossDirectory(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	s := openTestStore(t, db)
	human, _ := s.HumanMailbox(ctx)
	one, _ := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "one"}, model.RepositoryContext{Directory: "/old"})
	two, _ := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "two"}, model.RepositoryContext{Directory: "/old"})
	for _, item := range []model.Message{
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", "/old", human.ID, one.ID, "for one"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", "/old", human.ID, two.ID, "for two"),
	} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	s.Close()
	a, out := testApp(t, "")
	a.Getwd = func() (string, error) { return "/new", nil }
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "one"})
	if err := a.Run(ctx, []string{"--db", db, "poll"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "for one") || strings.Contains(out.String(), "for two") {
		t.Fatalf("poll = %q", out.String())
	}
}

func TestExplicitAndEnvironmentOverridesSelectCustomMailbox(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"HQ_SESSION": "shared", "CODEX_THREAD_ID": "codex"})
	if err := a.Run(ctx, []string{"--db", db, "ask", "from env"}); err != nil {
		t.Fatal(err)
	}
	firstID := strings.TrimSpace(out.String())
	a, out = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "other"})
	if err := a.Run(ctx, []string{"--db", db, "ask", "--session", "shared", "explicit"}); err != nil {
		t.Fatal(err)
	}
	secondID := strings.TrimSpace(out.String())
	s := openTestStore(t, db)
	defer s.Close()
	first, _ := s.Get(ctx, firstID)
	second, _ := s.Get(ctx, secondID)
	if first.SenderMailboxID != second.SenderMailboxID || !strings.HasPrefix(first.SenderLabel, "custom:") {
		t.Fatalf("mailboxes differ: %#v %#v", first, second)
	}
	if err := s.Create(ctx, message("0198c7ec-73b0-7cc3-a5f7-e31c77140d68", "/other", model.HumanMailboxID, first.SenderMailboxID, "custom inbox")); err != nil {
		t.Fatal(err)
	}
	s.Close()
	a, out = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "wrong"})
	if err := a.Run(ctx, []string{"--db", db, "poll", "--session", "shared"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "custom inbox") {
		t.Fatalf("custom poll = %q", out.String())
	}
}

func TestListArchiveModes(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "one"})
	if err := a.Run(ctx, []string{"--db", db, "ask", "open"}); err != nil {
		t.Fatal(err)
	}
	openID := strings.TrimSpace(out.String())
	a, out = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "one"})
	if err := a.Run(ctx, []string{"--db", db, "ask", "closed"}); err != nil {
		t.Fatal(err)
	}
	closedID := strings.TrimSpace(out.String())
	a, _ = testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "cancel", closedID}); err != nil {
		t.Fatal(err)
	}
	checks := []struct {
		args               []string
		hasOpen, hasClosed bool
	}{
		{[]string{"list"}, true, false},
		{[]string{"list", "--archived"}, false, true},
		{[]string{"list", "--all"}, true, true},
	}
	for _, check := range checks {
		a, out = testApp(t, "")
		args := append([]string{"--db", db}, check.args...)
		if err := a.Run(ctx, args); err != nil {
			t.Fatal(err)
		}
		if strings.Contains(out.String(), openID) != check.hasOpen || strings.Contains(out.String(), closedID) != check.hasClosed {
			t.Fatalf("%v = %q", check.args, out.String())
		}
	}
	a, _ = testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "list", "--archived", "--all"}); err == nil {
		t.Fatal("conflicting flags succeeded")
	}
}

func TestMailboxesFindsCandidatesWithoutClaiming(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, _ := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "old"})
	if err := a.Run(ctx, []string{"--db", db, "ask", "old"}); err != nil {
		t.Fatal(err)
	}
	a, out := testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "mailboxes"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "codex:") {
		t.Fatalf("mailboxes = %q", out.String())
	}
}

func TestAskReadsBodyFromStdinAndGetIsDirect(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, "Message from a pipe\n")
	a.Getenv = envMap(map[string]string{"PI_SESSION_ID": "pi"})
	if err := a.Run(context.Background(), []string{"--db", db, "ask", "--json"}); err != nil {
		t.Fatal(err)
	}
	var sent model.Message
	if err := json.Unmarshal(out.Bytes(), &sent); err != nil {
		t.Fatal(err)
	}
	a, out = testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", db, "get", sent.ID}); err != nil {
		t.Fatal(err)
	}
	var got model.Message
	if err := json.Unmarshal(out.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Body != "Message from a pipe" {
		t.Fatalf("message = %#v", got)
	}
}

func TestPollEmpty(t *testing.T) {
	a, _ := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "empty"})
	err := a.Run(context.Background(), []string{"--db", filepath.Join(t.TempDir(), "hq.db"), "poll"})
	if !errors.Is(err, ErrNoMessages) {
		t.Fatalf("poll = %v", err)
	}
}

func TestIdentityInitBackupResetAndImport(t *testing.T) {
	dir := t.TempDir()
	database := filepath.Join(dir, "state", "hq.db")
	backup := filepath.Join(dir, "identity-backup.json")
	a, out := testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "identity", "init"}); err != nil {
		t.Fatal(err)
	}
	initOutput := out.String()
	if !strings.Contains(initOutput, "installation:") || !strings.Contains(initOutput, "npub:") || strings.Contains(initOutput, "secret_key") {
		t.Fatalf("identity init output = %q", initOutput)
	}
	a, _ = testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "identity", "export", backup}); err != nil {
		t.Fatal(err)
	}
	a, _ = testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "identity", "reset"}); err == nil {
		t.Fatal("unconfirmed identity reset succeeded")
	}
	if err := a.Run(context.Background(), []string{"--db", database, "identity", "reset", "--yes"}); err != nil {
		t.Fatal(err)
	}
	a, out = testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "identity", "import", backup}); err != nil {
		t.Fatal(err)
	}
	if out.String() != initOutput {
		t.Fatalf("restored identity changed\ninit: %q\nimport: %q", initOutput, out.String())
	}
}

func TestPeerAndMailboxCommands(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "share"})
	if err := a.Run(context.Background(), []string{"--db", database, "ask", "hello"}); err != nil {
		t.Fatal(err)
	}
	s := openTestStore(t, database)
	mailboxes, err := s.FindMailboxes(context.Background(), model.RepositoryContext{Directory: "/work/repo"})
	if err != nil || len(mailboxes) != 1 {
		t.Fatalf("mailboxes = %#v, %v", mailboxes, err)
	}
	s.Close()
	peerID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	npub, err := identity.EncodePublicKey(event.MustSecretKeyFromHex("42").PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	a, _ = testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "peer", "add", "--name", "server", "--relay", "wss://relay.example", peerID, npub}); err != nil {
		t.Fatal(err)
	}
	a, out := testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "peer", "list"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "trusted\tserver") {
		t.Fatalf("peer list = %q", out.String())
	}
	a, _ = testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "mailbox", "share", mailboxes[0].ID, peerID}); err != nil {
		t.Fatal(err)
	}
	if err := a.Run(context.Background(), []string{"--db", database, "mailbox", "revoke", mailboxes[0].ID, peerID}); err != nil {
		t.Fatal(err)
	}
	if err := a.Run(context.Background(), []string{"--db", database, "peer", "distrust", peerID}); err != nil {
		t.Fatal(err)
	}
}

func TestRelayCommands(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	initializeTestIdentity(t, database)
	if err := a.Run(context.Background(), []string{"--db", database, "relay", "add", "wss://relay.example"}); err != nil {
		t.Fatal(err)
	}
	a, out := testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "relay", "list"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "wss://relay.example\tread=true\twrite=true\tauth=true") {
		t.Fatalf("relay list = %q", out.String())
	}
	a, _ = testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "relay", "remove", "wss://relay.example"}); err != nil {
		t.Fatal(err)
	}
}

func envMap(values map[string]string) func(string) string {
	return func(name string) string { return values[name] }
}

func message(id, directory, sender, recipient, body string) model.Message {
	return model.Message{ID: id, Context: model.RepositoryContext{Directory: directory}, SenderMailboxID: sender, RecipientMailboxID: recipient, Body: body, CreatedAt: time.Now().UTC()}
}
