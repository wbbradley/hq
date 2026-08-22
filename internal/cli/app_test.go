package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/agenthelp"
	"github.com/wbbradley/hq/internal/codexbridge"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/hqclient"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/node"
	"github.com/wbbradley/hq/internal/store"
	"github.com/wbbradley/hq/internal/syncer"
)

type testDomainStore struct{ *store.SQLite }

func (*testDomainStore) Synchronize(context.Context) error { return nil }

type updatingTestStore struct {
	*testDomainStore
	updates domain.ClientUpdates
}

func (s *updatingTestStore) Updates() domain.ClientUpdates { return s.updates }

func testApp(t *testing.T, input string) (*App, *bytes.Buffer) {
	t.Helper()
	out := new(bytes.Buffer)
	return &App{
		In: strings.NewReader(input), Out: out, ErrOut: new(bytes.Buffer),
		Getwd:  func() (string, error) { return "/work/repo", nil },
		Getenv: func(string) string { return "" },
		Open: func(_ context.Context, path string) (domain.Store, error) {
			initializeTestIdentity(t, path)
			database, err := store.Open(path)
			if err != nil {
				return nil, err
			}
			return &testDomainStore{SQLite: database}, nil
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

func TestCodexCommandBuildsBridgeOptions(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	var received codexbridge.Options
	a.RunCodexBridge = func(_ context.Context, options codexbridge.Options) error {
		received = options
		return nil
	}
	if err := a.Run(context.Background(), []string{"--no-sync", "--db", database, "codex", "--cwd", "child", "--resume", "thread-42", "--yolo", "continue", "working"}); err != nil {
		t.Fatal(err)
	}
	if received.Directory != "/work/repo/child" || received.ResumeThreadID != "thread-42" || received.InitialPrompt != "continue working" {
		t.Fatalf("options = %#v", received)
	}
	if received.Repository.Directory != "/work/repo/child" || received.Store == nil || received.Stderr != a.ErrOut || received.Sync != nil || received.LedgerPath != database+".codexbridge.json" || !received.Yolo {
		t.Fatalf("dependencies = %#v", received)
	}
}

func TestCodexCommandDefaultsToCurrentDirectoryAndEnablesSync(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	a.Synchronize = func(context.Context, domain.Store) error { return nil }
	var received codexbridge.Options
	a.RunCodexBridge = func(_ context.Context, options codexbridge.Options) error {
		received = options
		return options.Sync(context.Background())
	}
	if err := a.Run(context.Background(), []string{"--db", database, "codex"}); err != nil {
		t.Fatal(err)
	}
	if received.Directory != "/work/repo" || received.InitialPrompt != "" || received.Sync == nil || received.Yolo {
		t.Fatalf("options = %#v", received)
	}
}

func TestCodexHelpDoesNotOpenStore(t *testing.T) {
	var outputs []string
	for _, args := range [][]string{{"codex", "--help"}, {"help", "codex"}} {
		a, out := testApp(t, "")
		a.Open = func(context.Context, string) (domain.Store, error) {
			t.Fatal("Codex help opened the store")
			return nil, nil
		}
		if err := a.Run(context.Background(), args); err != nil {
			t.Fatal(err)
		}
		outputs = append(outputs, out.String())
	}
	if outputs[0] != outputs[1] || outputs[0] != codexUsage {
		t.Fatalf("help outputs differ:\n%s\n---\n%s", outputs[0], outputs[1])
	}
	for _, required := range []string{"Codex CLI v0.148.0", "--resume THREAD_ID", "--yolo", "disables approvals and sandboxing", "<database>.codexbridge.json", "Secret-marked", "one bridge process"} {
		if !strings.Contains(outputs[0], required) {
			t.Fatalf("Codex help is missing %q", required)
		}
	}
}

func TestGlobalHelpIncludesCodexSynopsisAndRejectsUnknownTopic(t *testing.T) {
	a, out := testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) { t.Fatal("help opened store"); return nil, nil }
	if err := a.Run(context.Background(), []string{"help"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "codex [--cwd PATH] [--resume THREAD_ID] [--yolo] [INITIAL PROMPT...]") {
		t.Fatalf("global help = %q", out.String())
	}
	a, _ = testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) {
		t.Fatal("invalid help opened store")
		return nil, nil
	}
	err := a.Run(context.Background(), []string{"help", "future"})
	if err == nil || !strings.Contains(err.Error(), "topic codex") {
		t.Fatalf("error = %v", err)
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
	a.Open = func(context.Context, string) (domain.Store, error) { t.Fatal("agents opened store"); return nil, nil }
	if err := a.Run(context.Background(), []string{"agents"}); err != nil {
		t.Fatal(err)
	}
	if out.String() != agenthelp.Text {
		t.Fatal("agents output differs from source")
	}
	if strings.Contains(out.String(), "export HQ_SESSION") || strings.Contains(out.String(), "same session ID") {
		t.Fatal("agent help asks agents to manage session IDs")
	}
	if !strings.Contains(out.String(), "reply=$(hq ask") || !strings.Contains(out.String(), "message_id=$(hq send") || !strings.Contains(out.String(), "Do not add a timeout") {
		t.Fatal("agent help does not teach blocking ask and asynchronous send")
	}
	if strings.Contains(out.String(), "wait --timeout") {
		t.Fatal("agent help encourages routine wait timeouts")
	}
	if strings.Contains(out.String(), "hq codex") {
		t.Fatal("embedded agent help tells an existing agent to launch the bridge")
	}
}

func TestAgentsPrintsFocusedTopicWithoutOpeningStore(t *testing.T) {
	a, out := testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) { t.Fatal("agents opened store"); return nil, nil }
	if err := a.Run(context.Background(), []string{"agents", "sync-semantics"}); err != nil {
		t.Fatal(err)
	}
	want, ok := agenthelp.Topic("sync-semantics")
	if !ok {
		t.Fatal("sync-semantics topic is missing")
	}
	if out.String() != want {
		t.Fatal("agents topic output differs from source")
	}
}

func TestAgentsRejectsUnknownTopic(t *testing.T) {
	a, _ := testApp(t, "")
	err := a.Run(context.Background(), []string{"agents", "unknown"})
	if err == nil || !strings.Contains(err.Error(), "unknown agents topic") {
		t.Fatalf("error = %v", err)
	}
}

func TestBareCommandUsesTUIWhenInteractive(t *testing.T) {
	a, _ := testApp(t, "")
	a.IsTTY = func() bool { return true }
	called := false
	a.RunTUI = func(context.Context, domain.Store, io.Reader, io.Writer) error { called = true; return nil }
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
			if err := a.Run(context.Background(), []string{"--db", db, "send", "hello"}); err != nil {
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

func TestAskWaitsForReplyByDefault(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	initializeTestIdentity(t, database)
	askStore := openTestStore(t, database)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	a, out := testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) { return &testDomainStore{SQLite: askStore}, nil }
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "blocking-ask"})
	done := make(chan error, 1)
	go func() {
		done <- a.Run(ctx, []string{"--no-sync", "--db", database, "ask", "--interval", "5ms", "Choose a port"})
	}()

	var question model.Message
	deadline := time.Now().Add(time.Second)
	for question.ID == "" && time.Now().Before(deadline) {
		select {
		case err := <-done:
			t.Fatalf("ask returned before saving its question: %v", err)
		default:
		}
		messages, err := askStore.List(ctx, model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 1})
		if err != nil {
			t.Fatal(err)
		}
		if len(messages) == 1 {
			question = messages[0]
			break
		}
		time.Sleep(5 * time.Millisecond)
	}
	if question.ID == "" {
		t.Fatal("ask did not save its question")
	}
	select {
	case err := <-done:
		t.Fatalf("ask returned before a reply: %v", err)
	default:
	}

	reply := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d70", "/work/repo", model.HumanMailboxID, question.SenderMailboxID, "8080")
	if err := askStore.Reply(ctx, question.ID, reply); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-ctx.Done():
		t.Fatal("ask did not return the reply")
	}
	if out.String() != "8080\n" {
		t.Fatalf("ask output = %q", out.String())
	}
}

func TestAskTimeoutReportsSavedQuestionID(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "timed-ask"})

	err := a.Run(context.Background(), []string{"--no-sync", "--db", database, "ask", "--timeout", "20ms", "--interval", "5ms", "Need an answer"})
	if !errors.Is(err, context.DeadlineExceeded) || out.Len() != 0 {
		t.Fatalf("ask error = %v, output = %q", err, out.String())
	}
	parts := strings.Fields(err.Error())
	if len(parts) < 2 {
		t.Fatalf("ask error lacks question ID: %v", err)
	}
	s := openTestStore(t, database)
	defer s.Close()
	question, getErr := s.Get(context.Background(), strings.TrimSuffix(parts[1], ":"))
	if getErr != nil || question.Body != "Need an answer" {
		t.Fatalf("saved question = %#v, %v", question, getErr)
	}
}

func TestAskAnswerWaitAndOwnership(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "one"})
	if err := a.Run(ctx, []string{"--db", db, "send", "Choose a port"}); err != nil {
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
	if err := a.Run(ctx, []string{"--db", db, "send", "from env"}); err != nil {
		t.Fatal(err)
	}
	firstID := strings.TrimSpace(out.String())
	a, out = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "other"})
	if err := a.Run(ctx, []string{"--db", db, "send", "--session", "shared", "explicit"}); err != nil {
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
	if err := a.Run(ctx, []string{"--db", db, "send", "open"}); err != nil {
		t.Fatal(err)
	}
	openID := strings.TrimSpace(out.String())
	a, out = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "one"})
	if err := a.Run(ctx, []string{"--db", db, "send", "closed"}); err != nil {
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
	if err := a.Run(ctx, []string{"--db", db, "send", "old"}); err != nil {
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
	if err := a.Run(context.Background(), []string{"--db", db, "send", "--json"}); err != nil {
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
	if err := a.Run(context.Background(), []string{"--db", database, "send", "hello"}); err != nil {
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

func TestDomainSynchronizationAndNoSync(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "foreground"})
	calls := 0
	a.Synchronize = func(context.Context, domain.Store) error {
		calls++
		return nil
	}
	if err := a.Run(context.Background(), []string{"--db", database, "send", "sync me"}); err != nil {
		t.Fatal(err)
	}
	if calls != 1 || len(strings.TrimSpace(out.String())) != 36 {
		t.Fatalf("foreground sync calls=%d stdout=%q", calls, out.String())
	}
	a, _ = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "offline"})
	a.Synchronize = func(context.Context, domain.Store) error {
		t.Fatal("--no-sync ran sync")
		return nil
	}
	if err := a.Run(context.Background(), []string{"--no-sync", "--db", database, "send", "offline"}); err != nil {
		t.Fatal(err)
	}
}

func TestForegroundRelayFailureKeepsLocalSuccess(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "failure"})
	a.Synchronize = func(context.Context, domain.Store) error { return errors.New("relay offline") }
	if err := a.Run(context.Background(), []string{"--db", database, "send", "keep local"}); err != nil {
		t.Fatal(err)
	}
	if len(strings.TrimSpace(out.String())) != 36 || !strings.Contains(a.ErrOut.(*bytes.Buffer).String(), "message saved; relay sync pending") {
		t.Fatalf("stdout=%q stderr=%q", out.String(), a.ErrOut.(*bytes.Buffer).String())
	}
	s := openTestStore(t, database)
	defer s.Close()
	message, err := s.Get(context.Background(), strings.TrimSpace(out.String()))
	if err != nil || message.Body != "keep local" {
		t.Fatalf("saved message = %#v, %v", message, err)
	}
}

func TestDaemonCLICommands(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) {
		t.Fatal("daemon run opened SQLite in the CLI")
		return nil, nil
	}
	runs := 0
	a.RunDaemon = func(_ context.Context, got string) error {
		runs++
		if got != database {
			t.Fatalf("daemon database = %q", got)
		}
		return nil
	}
	if err := a.Run(context.Background(), []string{"--db", database, "daemon", "run"}); err != nil || runs != 1 {
		t.Fatalf("daemon runs=%d err=%v", runs, err)
	}

	a, out := testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) {
		t.Fatal("daemon status opened SQLite")
		return nil, nil
	}
	a.DaemonStatus = func(string) (string, error) { return "running test", nil }
	if err := a.Run(context.Background(), []string{"--db", database, "daemon", "status"}); err != nil || out.String() != "running test\n" {
		t.Fatalf("daemon status stdout=%q err=%v", out.String(), err)
	}
	a, _ = testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) {
		t.Fatal("daemon stop opened SQLite")
		return nil, nil
	}
	stopped := false
	a.StopDaemon = func(string) error { stopped = true; return nil }
	if err := a.Run(context.Background(), []string{"--db", database, "daemon", "stop"}); err != nil || !stopped {
		t.Fatalf("daemon stop stopped=%t err=%v", stopped, err)
	}
	restarted := false
	a.RestartDaemon = func(string) error { restarted = true; return nil }
	if err := a.Run(context.Background(), []string{"--db", database, "daemon", "restart"}); err != nil || !restarted {
		t.Fatalf("daemon restart restarted=%t err=%v", restarted, err)
	}
}

func TestNormalCommandsOpenDomainClientButLifecycleAndIdentityDoNot(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "autostart"})
	opens := 0
	defaultOpen := a.Open
	a.Open = func(ctx context.Context, got string) (domain.Store, error) {
		opens++
		if got != database {
			t.Fatalf("open database = %q", got)
		}
		return defaultOpen(ctx, got)
	}
	if err := a.Run(context.Background(), []string{"--no-sync", "--db", database, "send", "start node"}); err != nil {
		t.Fatal(err)
	}
	if opens != 1 {
		t.Fatalf("normal command opens = %d", opens)
	}

	a, _ = testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) {
		t.Fatal("daemon lifecycle command opened a domain client")
		return nil, nil
	}
	a.DaemonStatus = func(string) (string, error) { return "running", nil }
	if err := a.Run(context.Background(), []string{"--db", database, "daemon", "status"}); err != nil {
		t.Fatal(err)
	}

	a, _ = testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) {
		t.Fatal("identity command opened a domain client")
		return nil, nil
	}
	if err := a.Run(context.Background(), []string{"--db", filepath.Join(t.TempDir(), "identity.db"), "identity", "init"}); err != nil {
		t.Fatal(err)
	}
}

func TestOrdinaryCommandPrintsConnectionDiagnostic(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	a.Open = func(context.Context, string) (domain.Store, error) {
		initializeTestIdentity(t, database)
		databaseStore, err := store.Open(database)
		if err != nil {
			return nil, err
		}
		return &updatingTestStore{
			testDomainStore: &testDomainStore{SQLite: databaseStore},
			updates:         domain.ClientUpdates{Initial: domain.ConnectionUpdate{Diagnostic: "restart the local HQ node"}},
		}, nil
	}
	if err := a.Run(context.Background(), []string{"--no-sync", "--db", database, "list"}); err != nil {
		t.Fatal(err)
	}
	if diagnostic := a.ErrOut.(*bytes.Buffer).String(); diagnostic != "hq: restart the local HQ node\n" {
		t.Fatalf("connection diagnostic = %q", diagnostic)
	}
}

func TestAskReceivesLiveNodeReplyThroughSubscription(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	initializeTestIdentity(t, database)
	nodeContext, stopNode := context.WithCancel(context.Background())
	nodeDone := make(chan error, 1)
	go func() { nodeDone <- node.Run(nodeContext, database) }()
	defer func() {
		stopNode()
		select {
		case err := <-nodeDone:
			if err != nil {
				t.Errorf("stop test node: %v", err)
			}
		case <-time.After(2 * time.Second):
			t.Error("test node did not stop")
		}
	}()
	paths, err := syncer.ResolveRuntimePaths(database)
	if err != nil {
		t.Fatal(err)
	}
	readyDeadline := time.Now().Add(2 * time.Second)
	for {
		if _, statErr := os.Stat(paths.Socket); statErr == nil {
			break
		}
		if time.Now().After(readyDeadline) {
			t.Fatal("test node did not create its socket")
		}
		time.Sleep(10 * time.Millisecond)
	}
	application := New()
	output := new(bytes.Buffer)
	application.In = strings.NewReader("")
	application.Out = output
	application.ErrOut = new(bytes.Buffer)
	application.Getwd = func() (string, error) { return "/work/repo", nil }
	application.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "subscribed-ask"})
	application.RepoContext = func(context.Context, string) model.RepositoryContext {
		return model.RepositoryContext{Directory: "/work/repo"}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	done := make(chan error, 1)
	go func() {
		done <- application.Run(ctx, []string{"--no-sync", "--db", database, "ask", "--interval", "1h", "Live reply?"})
	}()

	replier, err := hqclient.Open(ctx, database)
	if err != nil {
		t.Fatal(err)
	}
	defer replier.Close()
	var question model.Message
	deadline := time.Now().Add(2 * time.Second)
	for question.ID == "" && time.Now().Before(deadline) {
		messages, listErr := replier.List(ctx, model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 10})
		if listErr != nil {
			t.Fatal(listErr)
		}
		for _, candidate := range messages {
			if candidate.Body == "Live reply?" {
				question = candidate
				break
			}
		}
		if question.ID == "" {
			time.Sleep(10 * time.Millisecond)
		}
	}
	if question.ID == "" {
		t.Fatal("subscribed ask did not publish its question")
	}
	reply := message("019c0000-0000-7000-8000-000000000301", "/work/repo", model.HumanMailboxID, question.SenderMailboxID, "Immediately")
	started := time.Now()
	if err := replier.Reply(ctx, question.ID, reply); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("subscribed ask waited for its one-hour repair interval")
	}
	if elapsed := time.Since(started); elapsed > time.Second || output.String() != "Immediately\n" {
		t.Fatalf("subscribed ask elapsed=%s output=%q", elapsed, output.String())
	}
}

func TestWaitRunsBoundedSyncAndSyncFailureKeepsLocalSuccess(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "waiting"})
	if err := a.Run(context.Background(), []string{"--no-sync", "--db", database, "send", "wait for me"}); err != nil {
		t.Fatal(err)
	}
	messageID := strings.TrimSpace(out.String())
	a, _ = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "waiting"})
	syncCalls := 0
	a.Synchronize = func(context.Context, domain.Store) error { syncCalls++; return nil }
	err := a.Run(context.Background(), []string{"--db", database, "wait", "--timeout", "20ms", "--interval", "5ms", messageID})
	if !errors.Is(err, context.DeadlineExceeded) || syncCalls == 0 {
		t.Fatalf("wait error=%v sync calls=%d", err, syncCalls)
	}
	a, out = testApp(t, "")
	a.Getenv = envMap(map[string]string{"CODEX_THREAD_ID": "failed-sync"})
	a.Synchronize = func(context.Context, domain.Store) error { return errors.New("node sync unavailable") }
	if err := a.Run(context.Background(), []string{"--db", database, "send", "sync can fail"}); err != nil {
		t.Fatal(err)
	}
	if len(strings.TrimSpace(out.String())) != 36 || !strings.Contains(a.ErrOut.(*bytes.Buffer).String(), "relay sync pending") {
		t.Fatalf("failed wake stdout=%q stderr=%q", out.String(), a.ErrOut.(*bytes.Buffer).String())
	}
}

func TestStatusCommandWording(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, "")
	if err := a.Run(context.Background(), []string{"--db", database, "status"}); err != nil {
		t.Fatal(err)
	}
	if out.String() != "queued=0 relay_accepted=0 rejected=0 unresolved=0 unsupported=0 staged=0 quarantined=0 account_members=1 pending_account_fanout=0 invalid_account_traffic=0 revoked_device_traffic=0\n" {
		t.Fatalf("status = %q", out.String())
	}
}

func TestHumanAccountCLIShowInviteJoinAndDevices(t *testing.T) {
	ctx := context.Background()
	root := t.TempDir()
	creatorDB := filepath.Join(root, "creator", "hq.db")
	invitedDB := filepath.Join(root, "invited", "hq.db")
	creator := openTestStore(t, creatorDB)
	creatorAccount, err := creator.HumanAccount(ctx)
	if err != nil {
		t.Fatal(err)
	}
	invited := openTestStore(t, invitedDB)
	invitedID, invitedKey := invited.InstallationIdentity()
	npub, err := identity.EncodePublicKey(invitedKey)
	if err != nil {
		t.Fatal(err)
	}
	if err := creator.Close(); err != nil {
		t.Fatal(err)
	}
	if err := invited.Close(); err != nil {
		t.Fatal(err)
	}

	app, output := testApp(t, "")
	app.Hostname = func() (string, error) { return "desktop", nil }
	if err := app.Run(ctx, []string{"--no-sync", "--db", creatorDB, "human", "invite", "--relay", "ws://relay.lan:7447", invitedID, npub}); err != nil {
		t.Fatal(err)
	}
	var bundle store.PairingBundle
	if err := json.Unmarshal(output.Bytes(), &bundle); err != nil {
		t.Fatalf("invite output = %q: %v", output.String(), err)
	}
	if bundle.AccountID != creatorAccount.ID || bundle.TargetLabel != "desktop" {
		t.Fatalf("invite = %#v", bundle)
	}
	invitePath := filepath.Join(root, "pairing.json")
	if err := os.WriteFile(invitePath, output.Bytes(), 0o600); err != nil {
		t.Fatal(err)
	}

	app, _ = testApp(t, "")
	if err := app.Run(ctx, []string{"--no-sync", "--db", invitedDB, "human", "join", invitePath}); err != nil {
		t.Fatal(err)
	}
	app, output = testApp(t, "")
	if err := app.Run(ctx, []string{"--db", invitedDB, "human", "show", "--json"}); err != nil {
		t.Fatal(err)
	}
	var joined store.HumanAccount
	if err := json.Unmarshal(output.Bytes(), &joined); err != nil || joined.ID != creatorAccount.ID || joined.Creator {
		t.Fatalf("joined account = %#v, %v", joined, err)
	}
	app, output = testApp(t, "")
	if err := app.Run(ctx, []string{"--db", invitedDB, "human", "devices"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(output.String(), invitedID+"\tactive\tdesktop") {
		t.Fatalf("devices = %q", output.String())
	}
}

func envMap(values map[string]string) func(string) string {
	return func(name string) string { return values[name] }
}

func message(id, directory, sender, recipient, body string) model.Message {
	return model.Message{ID: id, Context: model.RepositoryContext{Directory: directory}, SenderMailboxID: sender, RecipientMailboxID: recipient, Body: body, CreatedAt: time.Now().UTC()}
}
