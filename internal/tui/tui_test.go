package tui

import (
	"context"
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/textarea"
	tea "charm.land/bubbletea/v2"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/repoctx"
	"github.com/wbbradley/hq/internal/store"
)

func TestRefreshPreservesActiveDraft(t *testing.T) {
	q1 := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d61", Prompt: "First"}
	q2 := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d62", Prompt: "Second"}
	editor := textarea.New()
	editor.SetValue("unfinished answer")
	m := app{
		questions: []model.Question{q1, q2},
		answering: true,
		answerID:  q1.ID,
		answerQ:   q1,
		editor:    editor,
	}

	updated, _ := m.Update(loadedMsg{pending: []model.Question{q2}})
	got := updated.(app)
	if got.editor.Value() != "unfinished answer" {
		t.Fatalf("draft = %q", got.editor.Value())
	}
	if got.answerID != q1.ID || got.answerQ.ID != q1.ID {
		t.Fatalf("answer target changed to %q", got.answerID)
	}
}

func TestHistoryViewShowsPastAnswer(t *testing.T) {
	response := "Use the small API."
	pending := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d61", Status: model.StatusPending, Prompt: "Pending"}
	answered := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d62", Status: model.StatusAnswered, Prompt: "Past question", Response: &response}
	m := app{pending: []model.Question{pending}, history: []model.Question{answered}}
	m.setQuestions()

	updated, _ := m.Update(tea.KeyPressMsg{Code: 'h', Text: "h"})
	got := updated.(app)
	if !got.historyOn || len(got.questions) != 1 || got.questions[0].ID != answered.ID {
		t.Fatalf("history questions = %#v", got.questions)
	}
	view := got.View().Content
	for _, want := range []string{"History", "Past question", response} {
		if !strings.Contains(view, want) {
			t.Fatalf("history view missing %q: %q", want, view)
		}
	}
}

func TestRepositoryContextIgnoresStaleResultsAndShowsUnavailable(t *testing.T) {
	q := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d61", Directory: "/repo", Prompt: "Question"}
	m := app{questions: []model.Question{q}, contextID: q.ID, branch: "loading"}
	updated, cmd := m.Update(branchMsg{question: q, branch: "feature", err: nil})
	m = updated.(app)
	if m.branch != "feature" || cmd == nil {
		t.Fatalf("branch = %q, cmd = %#v", m.branch, cmd)
	}
	updated, cmd = m.Update(remotesMsg{
		question: q,
		branch:   "feature",
		remotes:  []repoctx.Remote{{Name: "origin", Display: "wbbradley/hq"}},
	})
	m = updated.(app)
	if m.remotes != "origin: wbbradley/hq" || cmd == nil {
		t.Fatalf("remotes = %q, cmd = %#v", m.remotes, cmd)
	}
	updated, _ = m.Update(pullMsg{questionID: q.ID, err: repoctx.ErrUnavailable})
	m = updated.(app)
	if m.pull != "[gh unavailable]" {
		t.Fatalf("pull = %q", m.pull)
	}
	updated, _ = m.Update(branchMsg{question: model.Question{ID: "stale"}, branch: "wrong", err: errors.New("stale")})
	if got := updated.(app).branch; got != "feature" {
		t.Fatalf("stale branch changed context to %q", got)
	}
	view := m.View().Content
	if strings.Index(view, "origin: wbbradley/hq") > strings.Index(view, "[gh unavailable]") {
		t.Fatalf("remote appears after pull status: %q", view)
	}
}

func TestSubmitUsesDraftQuestionAfterQueueRefresh(t *testing.T) {
	s, err := store.Open(filepath.Join(t.TempDir(), "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	ctx := context.Background()
	q1 := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d61", Directory: "/repo", SessionID: "run", Prompt: "First", CreatedAt: time.Now().UTC()}
	q2 := model.Question{ID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d62", Directory: "/repo", SessionID: "run", Prompt: "Second", CreatedAt: time.Now().UTC().Add(time.Millisecond)}
	for _, q := range []model.Question{q1, q2} {
		if err := s.Create(ctx, q); err != nil {
			t.Fatal(err)
		}
	}
	editor := textarea.New()
	editor.SetValue("answer for first")
	m := app{ctx: ctx, store: s, questions: []model.Question{q2}, answerID: q1.ID, answerQ: q1, answering: true, editor: editor}
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd == nil {
		t.Fatal("enter did not submit the draft")
	}
	msg := cmd().(answeredMsg)
	if msg.err != nil {
		t.Fatal(msg.err)
	}
	got1, err := s.Get(ctx, q1.ID)
	if err != nil {
		t.Fatal(err)
	}
	got2, err := s.Get(ctx, q2.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got1.Response == nil || *got1.Response != "answer for first" {
		t.Fatalf("first response = %#v", got1.Response)
	}
	if got2.Status != model.StatusPending {
		t.Fatalf("second status = %q", got2.Status)
	}
}

func TestShiftEnterAndCtrlJInsertNewlines(t *testing.T) {
	editor := textarea.New()
	editor.KeyMap.InsertNewline = key.NewBinding(key.WithKeys("shift+enter", "ctrl+j"))
	editor.SetValue("first")
	editor.Focus()
	m := app{answering: true, answerID: "question", editor: editor}

	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter, Mod: tea.ModShift})
	m = updated.(app)
	if m.editor.Value() != "first\n" {
		t.Fatalf("shift+enter value = %q", m.editor.Value())
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'j', Mod: tea.ModCtrl})
	m = updated.(app)
	if m.editor.Value() != "first\n\n" {
		t.Fatalf("ctrl+j value = %q", m.editor.Value())
	}
}

func TestRefreshSchedulesLoadAndNextRefresh(t *testing.T) {
	m := app{}
	_, cmd := m.Update(refreshMsg{})
	if cmd == nil {
		t.Fatal("refresh did not schedule commands")
	}
}
