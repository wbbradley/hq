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

func TestAgentsPrintsEmbeddedInstructionsWithoutOpeningStore(t *testing.T) {
	out := new(bytes.Buffer)
	a := &App{
		In:     strings.NewReader(""),
		Out:    out,
		ErrOut: new(bytes.Buffer),
		Getwd:  func() (string, error) { return "/work/repo", nil },
		Getenv: func(string) string { return "" },
		Open: func(string) (store.Store, error) {
			t.Fatal("agents opened the store")
			return nil, nil
		},
	}
	if err := a.Run(context.Background(), []string{"agents"}); err != nil {
		t.Fatal(err)
	}
	if out.String() != agenthelp.Text {
		t.Fatal("agents output differs from the embedded source")
	}
}

func testApp(t *testing.T, db string, input string) (*App, *bytes.Buffer) {
	t.Helper()
	out := new(bytes.Buffer)
	a := &App{
		In:     strings.NewReader(input),
		Out:    out,
		ErrOut: new(bytes.Buffer),
		Getwd:  func() (string, error) { return "/work/repo", nil },
		Getenv: func(string) string { return "" },
		Open: func(path string) (store.Store, error) {
			return store.Open(path)
		},
	}
	return a, out
}

func TestBareCommandUsesTUIWhenInteractive(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	called := false
	a, _ := testApp(t, db, "")
	a.IsTTY = func() bool { return true }
	a.RunTUI = func(context.Context, store.Store, io.Reader, io.Writer) error {
		called = true
		return nil
	}
	if err := a.Run(context.Background(), []string{"--db", db}); err != nil {
		t.Fatal(err)
	}
	if !called {
		t.Fatal("bare interactive command did not run the TUI")
	}
}

func TestBareCommandListsPendingQuestionsInInferredScope(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	s, err := store.Open(db)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now().UTC()
	questions := []model.Question{
		{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d61", Directory: "/work/repo", SessionID: "codex-run", Prompt: "include", CreatedAt: now},
		{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d62", Directory: "/work/repo", SessionID: "other-run", Prompt: "wrong session", CreatedAt: now.Add(time.Millisecond)},
		{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d63", Directory: "/other/repo", SessionID: "codex-run", Prompt: "wrong dir", CreatedAt: now.Add(2 * time.Millisecond)},
		{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d64", Directory: "/work/repo", SessionID: "codex-run", Prompt: "answered", CreatedAt: now.Add(3 * time.Millisecond)},
	}
	for _, q := range questions {
		if err := s.Create(ctx, q); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Answer(ctx, questions[3].ID, "done"); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}

	a, out := testApp(t, db, "")
	a.IsTTY = func() bool { return false }
	a.Getenv = func(name string) string {
		if name == "CODEX_THREAD_ID" {
			return "codex-run"
		}
		return ""
	}
	if err := a.Run(ctx, []string{"--db", db}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "include") {
		t.Fatalf("bare output misses pending question: %q", out.String())
	}
	for _, excluded := range []string{"wrong session", "wrong dir", "answered"} {
		if strings.Contains(out.String(), excluded) {
			t.Fatalf("bare output contains %q: %q", excluded, out.String())
		}
	}
}

func TestInferredSessionPrefersHQSession(t *testing.T) {
	a, _ := testApp(t, "", "")
	a.Getenv = func(name string) string {
		return map[string]string{"HQ_SESSION": "explicit-env", "CODEX_THREAD_ID": "codex-run"}[name]
	}
	if got := a.inferredSession(); got != "explicit-env" {
		t.Fatalf("inferred session = %q", got)
	}
}

func TestAskAnswerWait(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "ask", "--session", "agent-1", "Choose", "a", "port"}); err != nil {
		t.Fatal(err)
	}
	id := strings.TrimSpace(out.String())
	if len(id) != 36 {
		t.Fatalf("ID = %q", id)
	}

	a, _ = testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "answer", id, "8080"}); err != nil {
		t.Fatal(err)
	}

	a, out = testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "wait", "--timeout", "1s", id}); err != nil {
		t.Fatal(err)
	}
	if out.String() != "8080\n" {
		t.Fatalf("answer = %q", out.String())
	}

	a, out = testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "get", id}); err != nil {
		t.Fatal(err)
	}
	var q model.Question
	if err := json.Unmarshal(out.Bytes(), &q); err != nil {
		t.Fatal(err)
	}
	if q.CompletedAt == nil {
		t.Fatal("wait did not complete the answer")
	}
}

func TestPollUsesDirectoryAndSessionScope(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	ctx := context.Background()
	a, out := testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "ask", "--session", "wanted", "Ready?"}); err != nil {
		t.Fatal(err)
	}
	id := strings.TrimSpace(out.String())
	a, _ = testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "answer", id, "yes"}); err != nil {
		t.Fatal(err)
	}

	a, _ = testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "poll", "--session", "other"}); !errors.Is(err, ErrNoAnswers) {
		t.Fatalf("wrong session poll error = %v", err)
	}

	a, out = testApp(t, db, "")
	if err := a.Run(ctx, []string{"--db", db, "poll", "--session", "wanted"}); err != nil {
		t.Fatal(err)
	}
	if out.String() != id+"\tyes\n" {
		t.Fatalf("poll output = %q", out.String())
	}
}

func TestAskReadsPromptFromStdin(t *testing.T) {
	db := filepath.Join(t.TempDir(), "hq.db")
	a, out := testApp(t, db, "Question from a pipe\n")
	if err := a.Run(context.Background(), []string{"--db", db, "ask", "--session", "pipe"}); err != nil {
		t.Fatal(err)
	}
	id := strings.TrimSpace(out.String())
	a, out = testApp(t, db, "")
	if err := a.Run(context.Background(), []string{"--db", db, "get", id}); err != nil {
		t.Fatal(err)
	}
	var q model.Question
	if err := json.Unmarshal(out.Bytes(), &q); err != nil {
		t.Fatal(err)
	}
	if q.Prompt != "Question from a pipe" {
		t.Fatalf("prompt = %q", q.Prompt)
	}
}
