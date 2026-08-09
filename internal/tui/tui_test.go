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

const testAgentID = "0198c7ec-73b0-7cc3-a5f7-e31c77140d60"

func TestRefreshPreservesActiveDraft(t *testing.T) {
	m1 := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", testAgentID, model.HumanMailboxID, "First")
	m2 := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", testAgentID, model.HumanMailboxID, "Second")
	editor := textarea.New()
	editor.SetValue("unfinished")
	m := app{messages: []model.Message{m1, m2}, answering: true, answerID: m1.ID, answerQ: m1, editor: editor}
	updated, _ := m.Update(loadedMsg{inbox: []model.Message{m2}})
	got := updated.(app)
	if got.editor.Value() != "unfinished" || got.answerQ.ID != m1.ID {
		t.Fatalf("draft changed: %#v", got)
	}
}

func TestSentAndArchivedAreIndependent(t *testing.T) {
	inbox := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", testAgentID, model.HumanMailboxID, "Inbox")
	sent := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d62", model.HumanMailboxID, testAgentID, "Sent")
	archived := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d63", testAgentID, model.HumanMailboxID, "Archived")
	archivedSent := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d64", model.HumanMailboxID, testAgentID, "Archived sent")
	now := time.Now().UTC()
	archived.ArchivedAt = &now
	archivedSent.ArchivedAt = &now
	m := app{inbox: []model.Message{inbox}, sent: []model.Message{sent, archivedSent}, archived: []model.Message{archived}}
	m.setMessages()
	if len(m.messages) != 1 {
		t.Fatalf("default messages = %#v", m.messages)
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 's', Text: "s"})
	m = updated.(app)
	if len(m.messages) != 2 || !m.showSent || m.showArchived {
		t.Fatalf("sent mode = %#v", m)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	m = updated.(app)
	if len(m.messages) != 4 || !m.showSent || !m.showArchived {
		t.Fatalf("combined mode = %#v", m)
	}
}

func TestRepositoryContextShowsRemotesBeforePullState(t *testing.T) {
	item := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", testAgentID, model.HumanMailboxID, "Question")
	m := app{messages: []model.Message{item}, contextID: item.ID}
	updated, _ := m.Update(branchMsg{message: item, branch: "feature"})
	m = updated.(app)
	updated, _ = m.Update(remotesMsg{message: item, branch: "feature", remotes: []repoctx.Remote{{Name: "origin", Display: "wbbradley/hq"}}})
	m = updated.(app)
	updated, _ = m.Update(pullMsg{questionID: item.ID, err: repoctx.ErrUnavailable})
	m = updated.(app)
	view := m.View().Content
	remoteAt, pullAt := strings.Index(view, "origin: wbbradley/hq"), strings.Index(view, "[gh unavailable]")
	if remoteAt < 0 || pullAt < 0 || remoteAt > pullAt {
		t.Fatalf("context order: %q", view)
	}
	updated, _ = m.Update(branchMsg{message: model.Message{ID: "stale"}, branch: "wrong", err: errors.New("stale")})
	if updated.(app).branch != "feature" {
		t.Fatal("stale context replaced branch")
	}
}

func TestReplyAndNewMessageUseMailboxID(t *testing.T) {
	s, ctx, agent := openStore(t)
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", agent.ID, model.HumanMailboxID, "Question")
	inbound.SenderLabel = agent.Label
	if err := s.Create(ctx, inbound); err != nil {
		t.Fatal(err)
	}
	editor := textarea.New()
	editor.SetValue("Answer")
	m := app{ctx: ctx, store: s, messages: []model.Message{inbound}, answering: true, answerID: inbound.ID, answerQ: inbound, editor: editor}
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd == nil {
		t.Fatal("enter did not submit")
	}
	if msg := cmd().(answeredMsg); msg.err != nil {
		t.Fatal(msg.err)
	}
	replies, err := s.List(ctx, model.Filter{ReplyTo: inbound.ID})
	if err != nil {
		t.Fatal(err)
	}
	if len(replies) != 1 || replies[0].RecipientMailboxID != agent.ID || replies[0].Body != "Answer" {
		t.Fatalf("replies = %#v", replies)
	}

	now := time.Now().UTC()
	inbound.ArchivedAt = &now
	m = app{ctx: ctx, store: s, messages: []model.Message{inbound}, editor: textarea.New()}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	m = updated.(app)
	if !m.answering || m.composeTo != agent.ID {
		t.Fatalf("compose state = %#v", m)
	}
	m.editor.SetValue("More detail")
	if msg := m.answer().(answeredMsg); msg.err != nil {
		t.Fatal(msg.err)
	}
	sent, err := s.List(ctx, model.Filter{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.ID})
	if err != nil {
		t.Fatal(err)
	}
	if len(sent) != 2 || sent[1].ReplyTo != nil || sent[1].Body != "More detail" {
		t.Fatalf("sent = %#v", sent)
	}
}

func TestShiftEnterAndCtrlJInsertNewlines(t *testing.T) {
	editor := textarea.New()
	editor.KeyMap.InsertNewline = key.NewBinding(key.WithKeys("shift+enter", "ctrl+j"))
	editor.SetValue("first")
	editor.Focus()
	m := app{answering: true, answerID: "id", editor: editor}
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter, Mod: tea.ModShift})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'j', Mod: tea.ModCtrl})
	m = updated.(app)
	if m.editor.Value() != "first\n\n" {
		t.Fatalf("value = %q", m.editor.Value())
	}
}

func TestRefreshSchedulesNextRefresh(t *testing.T) {
	_, cmd := (app{}).Update(refreshMsg{})
	if cmd == nil {
		t.Fatal("refresh did not schedule commands")
	}
}

func openStore(t *testing.T) (*store.SQLite, context.Context, model.Mailbox) {
	t.Helper()
	s, err := store.Open(filepath.Join(t.TempDir(), "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	ctx := context.Background()
	agent, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "test"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	return s, ctx, agent
}

func message(id, sender, recipient, body string) model.Message {
	senderLabel, recipientLabel := "codex:0198c7ec", "codex:0198c7ec"
	if sender == model.HumanMailboxID {
		senderLabel = "human"
	}
	if recipient == model.HumanMailboxID {
		recipientLabel = "human"
	}
	return model.Message{ID: id, Context: model.RepositoryContext{Directory: "/repo"}, SenderMailboxID: sender, RecipientMailboxID: recipient, SenderLabel: senderLabel, RecipientLabel: recipientLabel, Body: body, CreatedAt: time.Now().UTC()}
}
