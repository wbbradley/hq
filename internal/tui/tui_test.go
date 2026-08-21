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
	"charm.land/lipgloss/v2"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/repoctx"
	"github.com/wbbradley/hq/internal/store"
)

const testAgentID = "0198c7ec-73b0-7cc3-a5f7-e31c77140d60"

type testDomainStore struct{ *store.SQLite }

func (*testDomainStore) Synchronize(context.Context) error { return nil }

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

func TestSyncCompletionPreservesActiveDraft(t *testing.T) {
	item := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d65", testAgentID, model.HumanMailboxID, "Question")
	editor := textarea.New()
	editor.SetValue("draft survives")
	editor.Focus()
	m := app{messages: []model.Message{item}, answering: true, answerID: item.ID, answerQ: item, editor: editor}
	updated, cmd := m.Update(syncMsg{})
	got := updated.(app)
	if got.editor.Value() != "draft survives" || !got.editor.Focused() || got.answerQ.ID != item.ID || cmd == nil {
		t.Fatalf("sync changed draft: %#v", got)
	}
}

func TestInvalidationReloadPreservesActiveDraftAndRearms(t *testing.T) {
	item := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d68", testAgentID, model.HumanMailboxID, "Question")
	editor := textarea.New()
	editor.SetValue("draft survives live update")
	changes := make(chan domain.Invalidation, 1)
	m := app{
		ctx: context.Background(), messages: []model.Message{item}, answering: true,
		answerID: item.ID, answerQ: item, editor: editor, changes: changes,
	}
	changes <- domain.Invalidation{Revision: 2, Topics: []domain.ChangeTopic{domain.TopicMessages}}
	if msg := m.waitInvalidation()(); msg != (invalidatedMsg{}) {
		t.Fatalf("invalidation message = %#v", msg)
	}
	updated, cmd := m.Update(invalidatedMsg{})
	got := updated.(app)
	if cmd == nil || got.editor.Value() != "draft survives live update" || got.answerQ.ID != item.ID {
		t.Fatalf("invalidation changed draft or did not schedule reload: %#v", got)
	}
}

func TestConnectionDiagnosticsArePersistentAndIncompatibilityBlocks(t *testing.T) {
	drift := app{connection: domain.ConnectionUpdate{Diagnostic: "restart the local HQ node"}}
	if view := drift.View().Content; !strings.Contains(view, "restart the local HQ node") || strings.Contains(view, "then reopen the TUI") {
		t.Fatalf("drift view = %q", view)
	}
	blocked := app{connection: domain.ConnectionUpdate{Diagnostic: "upgrade this HQ client", Blocking: true}}
	view := blocked.View().Content
	if !strings.Contains(view, "upgrade this HQ client") || !strings.Contains(view, "then reopen the TUI") {
		t.Fatalf("blocked view = %q", view)
	}
	updated, cmd := blocked.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	if cmd != nil || updated.(app).answering {
		t.Fatal("incompatible connection accepted an interactive command")
	}
}

func TestDeliveryLabels(t *testing.T) {
	message := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d66", model.HumanMailboxID, testAgentID, "sent")
	wants := map[string]string{"queued": " [sending]", "relay-accepted": " [sent]", "peer-received": " [peer received]", "rejected": " [rejected]", "local": ""}
	for state, want := range wants {
		message.DeliveryState = state
		if got := deliveryLabel(message); got != want {
			t.Fatalf("delivery label %q = %q; want %q", state, got, want)
		}
	}
}

func TestStatusViewIsSeparateAndToggleable(t *testing.T) {
	received := time.Date(2026, 8, 9, 12, 0, 0, 0, time.UTC)
	m := app{network: store.NetworkStatus{Queued: 2, RelayAccepted: 3, Rejected: 1, Relays: []store.RelayHealth{{URL: "wss://relay.test", LastEvent: &received}}}}
	if strings.Contains(m.View().Content, "Relay status") {
		t.Fatal("status crowded the default view")
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'v', Text: "v"})
	m = updated.(app)
	if view := m.View().Content; !strings.Contains(view, "Relay status") || !strings.Contains(view, "queued 2") || !strings.Contains(view, "relay accepted 3") || !strings.Contains(view, "rejected 1") || !strings.Contains(view, "last receive 2026-08-09T12:00:00Z") {
		t.Fatalf("status view = %q", view)
	}
}

func TestMessageDetailWrapsToTerminalWidth(t *testing.T) {
	item := message(
		"0198c7ec-73b0-7cc3-a5f7-e31c77140d67",
		testAgentID,
		model.HumanMailboxID,
		"Which feature should come next for the cellular automata ASCII art generator?",
	)
	item.Details = "RLE support shares patterns with other tools.\nCell-age trails add richer terminal art and keep the tail-marker visible."
	m := app{messages: []model.Message{item}, width: 40}

	view := m.View().Content
	inDetailPanel := false
	foundDetailPanel := false
	for lineNumber, line := range strings.Split(view, "\n") {
		if strings.Contains(line, "╭") {
			inDetailPanel = true
			foundDetailPanel = true
		}
		if inDetailPanel && lipgloss.Width(line) > m.width {
			t.Fatalf("detail panel line %d width = %d; want at most %d: %q", lineNumber+1, lipgloss.Width(line), m.width, line)
		}
		if inDetailPanel && strings.Contains(line, "╰") {
			break
		}
	}
	if !foundDetailPanel {
		t.Fatal("detail panel not found")
	}
	if !strings.Contains(view, "tail-marker") {
		t.Fatalf("wrapped detail lost text: %q", view)
	}
}

func TestShortMessageDetailKeepsNaturalWidth(t *testing.T) {
	item := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d68", testAgentID, model.HumanMailboxID, "Short")
	m := app{messages: []model.Message{item}, width: 100}

	for _, line := range strings.Split(m.View().Content, "\n") {
		if strings.Contains(line, "╭") {
			if width := lipgloss.Width(line); width >= m.width {
				t.Fatalf("short detail panel width = %d; want natural width below %d", width, m.width)
			}
			return
		}
	}
	t.Fatal("detail panel not found")
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
	item.SourceDeviceLabel = "desktop"
	item.SenderInstallationID = "0198c7ec-73b0-7cc3-a5f7-e31c77140d01"
	m := app{messages: []model.Message{item}, contextID: item.ID}
	updated, _ := m.Update(branchMsg{message: item, branch: "feature"})
	m = updated.(app)
	updated, _ = m.Update(remotesMsg{message: item, branch: "feature", remotes: []repoctx.Remote{{Name: "origin", Display: "wbbradley/hq"}}})
	m = updated.(app)
	updated, _ = m.Update(pullMsg{questionID: item.ID, err: repoctx.ErrUnavailable})
	m = updated.(app)
	view := m.View().Content
	if !strings.Contains(view, "source desktop") || strings.Contains(view, item.SenderInstallationID) {
		t.Fatalf("source context missing: %q", view)
	}
	remoteAt, pullAt := strings.Index(view, "origin: wbbradley/hq"), strings.Index(view, "[gh unavailable]")
	if remoteAt < 0 || pullAt < 0 || remoteAt > pullAt {
		t.Fatalf("context order: %q", view)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'i', Text: "i"})
	m = updated.(app)
	if view = m.View().Content; !strings.Contains(view, "sender installation ID: "+item.SenderInstallationID) {
		t.Fatalf("expanded source context missing: %q", view)
	}
	updated, _ = m.Update(branchMsg{message: model.Message{ID: "stale"}, branch: "wrong", err: errors.New("stale")})
	if updated.(app).branch != "feature" {
		t.Fatal("stale context replaced branch")
	}
}

func TestMessagePresentationKindsAndFriendlyLabels(t *testing.T) {
	kinds := []struct {
		kind  string
		badge string
	}{
		{kind: "final-answer", badge: "[final answer]"},
		{kind: "update", badge: "[update]"},
		{kind: "status", badge: "[status]"},
		{kind: "notice", badge: "[notice]"},
	}
	messages := make([]model.Message, 0, len(kinds))
	for i, test := range kinds {
		item := message(string(rune('a'+i)), testAgentID, model.HumanMailboxID, test.kind)
		item.Context.Directory = "/work/repo"
		item.Details = "Kind: " + test.kind
		messages = append(messages, item)
	}
	view := (app{messages: messages}).View().Content
	if !strings.Contains(view, "codex · repo") || strings.Contains(view, "codex:0198c7ec") {
		t.Fatalf("friendly mailbox label missing: %q", view)
	}
	for _, test := range kinds {
		if !strings.Contains(view, test.badge) {
			t.Fatalf("badge %q missing: %q", test.badge, view)
		}
	}
	legacy := message("legacy", testAgentID, model.HumanMailboxID, "old final")
	legacy.Details = "Phase: final_answer"
	if got := presentationKind(legacy); got != "final-answer" {
		t.Fatalf("legacy kind = %q", got)
	}
}

func TestTechnicalDetailsAreHiddenUntilExpanded(t *testing.T) {
	replyTo := "reply-id-opaque"
	item := message("message-id-opaque", testAgentID, model.HumanMailboxID, "Approval needed")
	item.Context.Directory = "/work/repo"
	item.EventID = "event-id-opaque"
	item.ThreadID = "thread-event-id-opaque"
	item.SenderInstallationID = "sender-installation-opaque"
	item.RecipientInstallationID = "recipient-installation-opaque"
	item.ReplyTo = &replyTo
	item.Details = "Kind: update\nCodex thread: codex-thread-opaque\nCodex turn: turn-opaque\nHQ message: hq-opaque\n\nChoose one:\n- accept\n- decline"
	m := app{messages: []model.Message{item}}

	view := m.View().Content
	for _, hidden := range []string{item.ID, item.EventID, item.ThreadID, item.SenderInstallationID, item.RecipientInstallationID, replyTo, "codex-thread-opaque", "turn-opaque", "hq-opaque", "Kind: update"} {
		if strings.Contains(view, hidden) {
			t.Fatalf("collapsed view exposed %q: %q", hidden, view)
		}
	}
	if !strings.Contains(view, "Choose one:") || !strings.Contains(view, "- accept") || !strings.Contains(view, "technical details hidden · press i to show") {
		t.Fatalf("collapsed view lost human details: %q", view)
	}

	updated, _ := m.Update(tea.KeyPressMsg{Code: 'i', Text: "i"})
	view = updated.(app).View().Content
	for _, shown := range []string{item.ID, item.EventID, item.ThreadID, item.SenderInstallationID, item.RecipientInstallationID, replyTo, "codex-thread-opaque", "turn-opaque", "hq-opaque", "Kind: update"} {
		if !strings.Contains(view, shown) {
			t.Fatalf("expanded view omitted %q: %q", shown, view)
		}
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

func TestDArchivesSelectionWithoutReply(t *testing.T) {
	s, ctx, agent := openStore(t)
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d69", agent.ID, model.HumanMailboxID, "No reply needed")
	if err := s.Create(ctx, inbound); err != nil {
		t.Fatal(err)
	}
	m := app{ctx: ctx, store: s, messages: []model.Message{inbound}, editor: textarea.New()}
	_, cmd := m.Update(tea.KeyPressMsg{Code: 'd', Text: "d"})
	if cmd == nil {
		t.Fatal("d did not archive")
	}
	if msg := cmd().(archivedMsg); msg.err != nil {
		t.Fatal(msg.err)
	}
	archived, err := s.Get(ctx, inbound.ID)
	if err != nil {
		t.Fatal(err)
	}
	if archived.ArchivedAt == nil {
		t.Fatal("message was not archived")
	}
	replies, err := s.List(ctx, model.Filter{ReplyTo: inbound.ID})
	if err != nil {
		t.Fatal(err)
	}
	if len(replies) != 0 {
		t.Fatalf("replies = %#v", replies)
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

func TestPasteInsertsTextIntoActiveDraft(t *testing.T) {
	editor := textarea.New()
	editor.SetValue("Before ")
	editor.Focus()
	m := app{answering: true, answerID: "id", editor: editor}

	updated, _ := m.Update(tea.PasteMsg{Content: "voice-to-text"})
	m = updated.(app)

	if m.editor.Value() != "Before voice-to-text" {
		t.Fatalf("value = %q", m.editor.Value())
	}
}

func TestPasteIsIgnoredOutsideActiveDraft(t *testing.T) {
	editor := textarea.New()
	editor.SetValue("unchanged")
	m := app{editor: editor}

	updated, _ := m.Update(tea.PasteMsg{Content: "voice-to-text"})
	m = updated.(app)

	if m.editor.Value() != "unchanged" {
		t.Fatalf("value = %q", m.editor.Value())
	}
}

func TestRepairSchedulesNextRepair(t *testing.T) {
	_, cmd := (app{}).Update(repairMsg{})
	if cmd == nil {
		t.Fatal("repair did not schedule commands")
	}
}

func openStore(t *testing.T) (*testDomainStore, context.Context, model.Mailbox) {
	t.Helper()
	database := filepath.Join(t.TempDir(), "hq.db")
	keyPath, err := identity.KeyPath(database)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(database)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	ctx := context.Background()
	agent, err := s.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "test"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	return &testDomainStore{SQLite: s}, ctx, agent
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
