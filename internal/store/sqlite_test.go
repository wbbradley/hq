package store

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

func TestSQLiteConfiguration(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state", "hq.db")
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })

	checks := map[string]string{
		"PRAGMA journal_mode":    "wal",
		"PRAGMA synchronous":     "2",
		"PRAGMA foreign_keys":    "1",
		"PRAGMA trusted_schema":  "0",
		"PRAGMA integrity_check": "ok",
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
	var strict int
	if err := s.db.QueryRow(`SELECT strict FROM pragma_table_list WHERE name = 'questions'`).Scan(&strict); err != nil {
		t.Fatal(err)
	}
	if strict != 1 {
		t.Fatalf("questions strict = %d", strict)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("database mode = %o", info.Mode().Perm())
	}
}

func TestQuestionLifecycle(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	ctx := context.Background()
	q := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d65", Directory: "/repo", SessionID: "run-1", Prompt: "Ship?", CreatedAt: time.Now().UTC()}
	if err := s.Create(ctx, q); err != nil {
		t.Fatal(err)
	}
	if err := s.Answer(ctx, q.ID, "yes"); err != nil {
		t.Fatal(err)
	}
	claimed, err := s.ClaimAnswer(ctx, q.ID, "consumer-1")
	if err != nil {
		t.Fatal(err)
	}
	if claimed.Response == nil || *claimed.Response != "yes" {
		t.Fatalf("response = %#v", claimed.Response)
	}
	if _, err := s.ClaimAnswer(ctx, q.ID, "consumer-2"); !errors.Is(err, ErrClaimed) {
		t.Fatalf("second claim error = %v", err)
	}
	if err := s.CompleteAnswer(ctx, q.ID, "consumer-1"); err != nil {
		t.Fatal(err)
	}
	got, err := s.Get(ctx, q.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.CompletedAt == nil {
		t.Fatal("completed_at is nil")
	}
}

func TestListFiltersScope(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	ctx := context.Background()
	for i, session := range []string{"one", "two"} {
		q := model.Question{ID: []string{"0198c7ec-73b0-7cc3-a5f7-e31c77140d65", "0198c7ec-73b0-7cc3-a5f7-e31c77140d66"}[i], Directory: "/repo", SessionID: session, Prompt: "Question", CreatedAt: time.Now().UTC().Add(time.Duration(i) * time.Millisecond)}
		if err := s.Create(ctx, q); err != nil {
			t.Fatal(err)
		}
	}
	questions, err := s.List(ctx, model.Filter{Directory: "/repo", SessionID: "two", Status: model.StatusPending})
	if err != nil {
		t.Fatal(err)
	}
	if len(questions) != 1 || questions[0].SessionID != "two" {
		t.Fatalf("questions = %#v", questions)
	}
}
