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
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

func testApp(t *testing.T, input string) (*App, *bytes.Buffer) {
	t.Helper()
	out := new(bytes.Buffer)
	return &App{
		In: inputReader(input), Out: out, ErrOut: new(bytes.Buffer),
		Getwd:  func() (string, error) { return "/work/repo", nil },
		Getenv: func(string) string { return "" },
		Open:   func(path string) (store.Store, error) { return store.Open(path) },
	}, out
}

func inputReader(value string) io.Reader { return strings.NewReader(value) }

func TestAgentsPrintsEmbeddedInstructionsWithoutOpeningStore(t *testing.T) {
	a, out := testApp(t, "")
	a.Open = func(string) (store.Store, error) { t.Fatal("agents opened store"); return nil, nil }
	if err := a.Run(context.Background(), []string{"agents"}); err != nil {
		t.Fatal(err)
	}
	if out.String() != agenthelp.Text {
		t.Fatal("agents output differs from source")
	}
}

func TestBareCommandUsesTUIWhenInteractive(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	a, _ := testApp(t, "")
	a.IsTTY = func() bool { return true }
	called := false
	a.RunTUI = func(context.Context, store.Store, io.Reader, io.Writer) error { called = true; return nil }
	if err := a.Run(context.Background(), []string{"--db", db}); err != nil {
		t.Fatal(err)
	}
	if !called {
		t.Fatal("bare interactive command did not run TUI")
	}
}

func TestBareCommandListsOpenHumanInbox(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	s, err := store.Open(db)
	if err != nil {
		t.Fatal(err)
	}
	messages := []model.Message{
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", "/work/repo", "agent-a", model.HumanSession, "include"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", "/other/repo", "agent-a", model.HumanSession, "wrong dir"),
		message("0198c7ec-73b0-7cc3-a5f7-e31c77140d63", "/work/repo", model.HumanSession, "agent-a", "sent"),
	}
	for _, m := range messages {
		if err := s.Create(ctx, m); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Archive(ctx, messages[0].ID); err != nil {
		t.Fatal(err)
	}
	open := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d64", "/work/repo", "agent-b", model.HumanSession, "open inbox")
	if err := s.Create(ctx, open); err != nil {
		t.Fatal(err)
	}
	s.Close()
	a, out := testApp(t, "")
	a.IsTTY = func() bool { return false }
	if err := a.Run(ctx, []string{"--db", db}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "open inbox") || strings.Contains(out.String(), "include") || strings.Contains(out.String(), "sent") {
		t.Fatalf("bare output = %q", out.String())
	}
}

func TestAskAnswerWait(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "ask", "--session", "agent-1", "Choose", "a", "port"}); err != nil {
		t.Fatal(err)
	}
	originalID := strings.TrimSpace(out.String())
	a, _ = testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "answer", originalID, "8080"}); err != nil {
		t.Fatal(err)
	}
	a, out = testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "wait", "--timeout", "1s", originalID}); err != nil {
		t.Fatal(err)
	}
	if out.String() != "8080\n" {
		t.Fatalf("answer = %q", out.String())
	}
	s, err := store.Open(db)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	replies, err := s.List(ctx, model.Filter{ReplyTo: originalID})
	if err != nil {
		t.Fatal(err)
	}
	if len(replies) != 1 || replies[0].CompletedAt == nil {
		t.Fatalf("replies = %#v", replies)
	}
}

func TestPollDeliversUnsolicitedMailboxMessage(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	s, err := store.Open(db)
	if err != nil {
		t.Fatal(err)
	}
	m := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", "/work/repo", model.HumanSession, "agent-1", "Please check the logs")
	if err := s.Create(ctx, m); err != nil {
		t.Fatal(err)
	}
	s.Close()
	a, out := testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "poll", "--session", "agent-1"}); err != nil {
		t.Fatal(err)
	}
	if out.String() != m.ID+"\tPlease check the logs\n" {
		t.Fatalf("poll = %q", out.String())
	}
	a, _ = testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "poll", "--session", "agent-1"}); !errors.Is(err, ErrNoMessages) {
		t.Fatalf("second poll = %v", err)
	}
}

func TestWaitStopsWhenInboundMessageWasArchived(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "ask", "--session", "agent-1", "Need a reply"}); err != nil {
		t.Fatal(err)
	}
	id := strings.TrimSpace(out.String())
	a, _ = testApp(t, "")
	if err := a.Run(ctx, []string{"--db", db, "cancel", id}); err != nil {
		t.Fatal(err)
	}
	a, _ = testApp(t, "")
	err := a.Run(ctx, []string{"--db", db, "wait", "--timeout", "1s", id})
	if err == nil || !strings.Contains(err.Error(), "archived without a reply") {
		t.Fatalf("wait = %v", err)
	}
}

func TestAskReadsBodyFromStdin(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, "Message from a pipe\n")
	if err := a.Run(context.Background(), []string{"--db", db, "ask", "--session", "pipe", "--json"}); err != nil {
		t.Fatal(err)
	}
	var m model.Message
	if err := json.Unmarshal(out.Bytes(), &m); err != nil {
		t.Fatal(err)
	}
	if m.Body != "Message from a pipe" || m.RecipientSession != model.HumanSession {
		t.Fatalf("message = %#v", m)
	}
}

func TestHumanSessionIsReservedForHumanCommands(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	tests := [][]string{
		{"--db", db, "ask", "--session", model.HumanSession, "hello"},
		{"--db", db, "poll", "--session", model.HumanSession},
	}
	for _, args := range tests {
		a, _ := testApp(t, "")
		err := a.Run(context.Background(), args)
		if err == nil || !strings.Contains(err.Error(), "reserved") {
			t.Fatalf("Run(%v) = %v", args, err)
		}
	}
}

func TestInferredSessionPrefersHQSession(t *testing.T) {
	a, _ := testApp(t, "")
	a.Getenv = func(name string) string {
		return map[string]string{"HQ_SESSION": "env", "CODEX_THREAD_ID": "codex"}[name]
	}
	if got := a.inferredSession(); got != "env" {
		t.Fatalf("session = %q", got)
	}
}

func message(id, directory, sender, recipient, body string) model.Message {
	return model.Message{ID: id, Directory: directory, SenderSession: sender, RecipientSession: recipient, Body: body, CreatedAt: time.Now().UTC()}
}
