package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"path/filepath"
	"strings"
	"testing"

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
		Open: func(path string) (store.Store, error) {
			return store.Open(path)
		},
	}
	return a, out
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
