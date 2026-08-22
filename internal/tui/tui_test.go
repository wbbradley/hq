package tui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/textarea"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/repoctx"
	"github.com/wbbradley/hq/internal/store"
)

const testAgentID = "0198c7ec-73b0-7cc3-a5f7-e31c77140d60"

type testDomainStore struct{ *store.SQLite }

func (*testDomainStore) Synchronize(context.Context) error { return nil }

type runtimeTestStore struct {
	*testDomainStore
	runtime  domain.CodexRuntime
	launches []domain.CodexLaunchRequest
}

func (s *runtimeTestStore) LaunchCodexAgent(_ context.Context, request domain.CodexLaunchRequest) (domain.CodexRuntime, error) {
	copyRequest := request
	copyRequest.Environment = append([]string(nil), request.Environment...)
	s.launches = append(s.launches, copyRequest)
	s.runtime = domain.CodexRuntime{AgentName: request.AgentName, ThreadID: request.SessionID, Directory: request.Directory, Phase: domain.CodexRuntimeRunning}
	if s.runtime.ThreadID == "" {
		s.runtime.ThreadID = "thread-new"
	}
	return s.runtime, nil
}

func (s *runtimeTestStore) StopCodexAgent(_ context.Context, name string) (domain.CodexRuntime, error) {
	s.runtime = domain.CodexRuntime{AgentName: name, Phase: domain.CodexRuntimeOffline}
	return s.runtime, nil
}

func (s *runtimeTestStore) CodexAgentRuntime(context.Context, string) (domain.CodexRuntime, error) {
	return s.runtime, nil
}

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
	m := app{width: 100, height: 40, network: store.NetworkStatus{Queued: 2, RelayAccepted: 3, Rejected: 1, Relays: []store.RelayHealth{{URL: "wss://relay.test", LastEvent: &received}}}}
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

func TestShortMessageDetailFillsAllocatedWidth(t *testing.T) {
	item := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d68", testAgentID, model.HumanMailboxID, "Short")
	m := app{messages: []model.Message{item}, width: 100}

	for _, line := range strings.Split(m.View().Content, "\n") {
		if strings.Contains(line, "╭") {
			if width := lipgloss.Width(line); width != m.width {
				t.Fatalf("short detail panel width = %d; want allocated width %d", width, m.width)
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
	for _, hidden := range []string{"source desktop", "git feature", "origin: wbbradley/hq", "[gh unavailable]", item.Context.Directory, item.SenderInstallationID} {
		if strings.Contains(view, hidden) {
			t.Fatalf("collapsed context exposed %q: %q", hidden, view)
		}
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'i', Text: "i"})
	m = updated.(app)
	view = m.View().Content
	for _, shown := range []string{"source desktop", "git feature", "origin: wbbradley/hq", "[gh unavailable]", item.Context.Directory, "sender installation ID: " + item.SenderInstallationID} {
		if !strings.Contains(view, shown) {
			t.Fatalf("expanded context omitted %q: %q", shown, view)
		}
	}
	remoteAt, pullAt := strings.Index(view, "origin: wbbradley/hq"), strings.Index(view, "[gh unavailable]")
	if remoteAt < 0 || pullAt < 0 || remoteAt > pullAt {
		t.Fatalf("context order: %q", view)
	}
	if !strings.Contains(view, "sender installation ID: "+item.SenderInstallationID) {
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
	view := (app{messages: messages, width: 100, height: 40}).View().Content
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
	m := app{messages: []model.Message{item}, width: 100, height: 80}

	view := m.View().Content
	for _, hidden := range []string{item.ID, item.EventID, item.ThreadID, item.SenderInstallationID, item.RecipientInstallationID, replyTo, "codex-thread-opaque", "turn-opaque", "hq-opaque", "Kind: update"} {
		if strings.Contains(view, hidden) {
			t.Fatalf("collapsed view exposed %q: %q", hidden, view)
		}
	}
	if !strings.Contains(view, "Choose one:") || !strings.Contains(view, "- accept") || !strings.Contains(view, "technical details hidden · press i to show") {
		t.Fatalf("collapsed view lost human details: %q", view)
	}
	for _, line := range strings.Split(view, "\n") {
		if strings.Contains(line, "technical details hidden · press i to show") {
			labelEnd := strings.Index(line, "technical details hidden") + len("technical details hidden · press i to show")
			if !strings.Contains(line, "╰") || lipgloss.Width(line[labelEnd:]) > 3 {
				t.Fatalf("technical-details hint is not right-aligned on bottom border: %q", line)
			}
		}
	}

	updated, _ := m.Update(tea.KeyPressMsg{Code: 'i', Text: "i"})
	view = updated.(app).View().Content
	for _, shown := range []string{item.ID, item.EventID, item.ThreadID, item.SenderInstallationID, item.RecipientInstallationID, replyTo, "codex-thread-opaque", "turn-opaque", "hq-opaque", "Kind: update"} {
		if !strings.Contains(view, shown) {
			t.Fatalf("expanded view omitted %q: %q", shown, view)
		}
	}
}

func TestMessagePanelCombinesKindAndSenderInBorder(t *testing.T) {
	item := message("message-id", testAgentID, model.HumanMailboxID, "Working on it")
	item.Context.Directory = "/work/repo"
	item.Details = "Kind: update"
	view := (app{messages: []model.Message{item}}).View().Content
	lines := strings.Split(view, "\n")
	bodyLine := -1
	titleOnBorder := false
	for i, line := range lines {
		plainLine := ansi.Strip(line)
		switch {
		case strings.Contains(line, "╭") && strings.Contains(line, "[an update from codex · repo]"):
			titleOnBorder = true
		case strings.Contains(plainLine, "Working on it") && strings.Contains(plainLine, "│"):
			bodyLine = i
		}
	}
	if !titleOnBorder || bodyLine < 0 {
		t.Fatalf("message panel did not combine kind and sender in border: %q", view)
	}
	if strings.Contains(lines[bodyLine], "[update]") || strings.Contains(view, "From: codex · repo") {
		t.Fatalf("presentation kind remained inline with body: %q", view)
	}
	for _, line := range lines {
		if strings.Contains(line, "›") && strings.Contains(line, "codex · repo") && strings.Contains(line, "inbox ←") {
			t.Fatalf("incoming row retained inbox arrow: %q", line)
		}
	}

	item.Details = "Kind: final-answer"
	view = (app{messages: []model.Message{item}}).View().Content
	if !strings.Contains(view, "[a final answer from codex · repo]") || strings.Contains(view, "From: codex · repo") {
		t.Fatalf("final-answer border title: %q", view)
	}
}

func TestMessagePanelRendersOnlyBodiesAsMarkdown(t *testing.T) {
	item := message("message-id", testAgentID, model.HumanMailboxID, "Body with **bold text**")
	item.Details = "Kind: update\nVisible **detail markers**"
	m := app{messages: []model.Message{item}, width: 80, height: 30, markdown: newMessageMarkdownRenderer(renderMessageMarkdown)}

	view := m.View().Content
	if strings.Count(view, "**bold text**") != 1 || !strings.Contains(view, "\x1b[1m") {
		t.Fatalf("message body was not rendered as Markdown: %q", view)
	}
	if !strings.Contains(view, "Visible **detail markers**") {
		t.Fatalf("message details were incorrectly rendered as Markdown: %q", view)
	}
}

func TestMarkdownTableFitsResponsiveMessagePane(t *testing.T) {
	item := message("message-id", testAgentID, model.HumanMailboxID, "| Name | Description |\n| --- | --- |\n| alpha | a long value that wraps inside the available cell width |")
	item.Details = "Kind: update"
	for _, width := range []int{119, 120} {
		m := app{messages: []model.Message{item}, width: width, height: 40, markdown: newMessageMarkdownRenderer(renderMessageMarkdown)}
		view := m.View().Content
		if !strings.Contains(view, "alpha") || !strings.Contains(view, "Description") {
			t.Fatalf("%d-column table was not rendered: %q", width, view)
		}
		for lineNumber, line := range strings.Split(view, "\n") {
			if got := lipgloss.Width(line); got > width {
				t.Fatalf("%d-column view line %d width = %d: %q", width, lineNumber+1, got, line)
			}
		}
	}
}

func TestCoalescedMessagePartsRenderMarkdownIndependently(t *testing.T) {
	created := time.Date(2026, 8, 22, 12, 0, 0, 0, time.Local)
	first := message("first", testAgentID, model.HumanMailboxID, "First **bold part**")
	first.CreatedAt = created
	first.Details = "Kind: update\nCodex thread: thread\nCodex turn: turn"
	second := message("second", testAgentID, model.HumanMailboxID, "Second *italic part*")
	second.CreatedAt = created.Add(time.Second)
	second.Details = "Kind: final-answer\nCodex thread: thread\nCodex turn: turn"
	group := groupMessages([]model.Message{second, first})[0]
	m := app{markdown: newMessageMarkdownRenderer(renderMessageMarkdown)}

	panel := m.renderGroupPanel(group, 80)
	if strings.Contains(panel, "**bold part**") || strings.Contains(panel, "*italic part*") {
		t.Fatalf("coalesced bodies retained Markdown markers: %q", panel)
	}
	for _, part := range []model.Message{first, second} {
		if timestamp := part.CreatedAt.Local().Format("Jan 2, 3:04:05 PM"); !strings.Contains(panel, timestamp) {
			t.Fatalf("coalesced panel omitted timestamp %q: %q", timestamp, panel)
		}
	}
	if !strings.Contains(panel, "\x1b[1m") || !strings.Contains(panel, ";3m") {
		t.Fatalf("coalesced bodies omitted independent emphasis: %q", panel)
	}
}

func TestMessageMarkdownCacheResetsWhenPaneWidthChanges(t *testing.T) {
	calls := 0
	renderer := newMessageMarkdownRenderer(func(body, kind string, width int) (string, error) {
		calls++
		return body, nil
	})
	item := message("message", testAgentID, model.HumanMailboxID, "Body")
	m := app{messages: []model.Message{item}, width: 100, height: 30, markdown: renderer, editor: textarea.New()}

	m.View()
	m.View()
	if calls != 1 {
		t.Fatalf("unchanged views rendered %d times; want 1", calls)
	}
	updated, _ := m.Update(tea.WindowSizeMsg{Width: 120, Height: 30})
	m = updated.(app)
	m.View()
	if calls != 2 {
		t.Fatalf("resized pane rendered %d times; want 2", calls)
	}
}

func TestTurnMessagesCoalesceAndRefreshDuringDraft(t *testing.T) {
	created := time.Date(2026, 8, 21, 15, 4, 5, 0, time.Local)
	question := message("question", testAgentID, model.HumanMailboxID, "Which approach?")
	question.CreatedAt = created
	question.Details = "Codex thread: thread-1\nCodex turn: turn-1\nCodex request: request-1"
	update := message("update", testAgentID, model.HumanMailboxID, "First update")
	update.CreatedAt = created.Add(time.Second)
	update.Details = "Kind: update\nCodex thread: thread-1\nCodex turn: turn-1"
	final := message("final", testAgentID, model.HumanMailboxID, "Finished")
	final.CreatedAt = created.Add(2 * time.Second)
	final.Details = "Kind: final-answer\nCodex thread: thread-1\nCodex turn: turn-1"

	m := app{inbox: []model.Message{final, update, question}, editor: textarea.New(), width: 120, height: 30}
	m.setMessages()
	if len(m.groups) != 1 || len(m.messages) != 1 || m.messages[0].ID != final.ID {
		t.Fatalf("turn grouping = %#v, representatives = %#v", m.groups, m.messages)
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if !m.answering || m.answerID != question.ID {
		t.Fatalf("reply target = %q; want correlated question %q", m.answerID, question.ID)
	}
	m.editor.SetValue("draft survives")

	late := message("late", testAgentID, model.HumanMailboxID, "Late update")
	late.CreatedAt = created.Add(3 * time.Second)
	late.Details = "Kind: update\nCodex thread: thread-1\nCodex turn: turn-1"
	updated, _ = m.Update(loadedMsg{inbox: []model.Message{late, final, update, question}})
	m = updated.(app)
	view := m.View().Content
	if m.editor.Value() != "draft survives" || m.answerID != question.ID || !strings.Contains(view, "Late update") {
		t.Fatalf("live turn refresh lost draft, binding, or update: %q", view)
	}
	for _, part := range []model.Message{question, update, final, late} {
		if timestamp := part.CreatedAt.Local().Format("Jan 2, 3:04:05 PM"); !strings.Contains(view, timestamp) {
			t.Fatalf("turn view omitted timestamp %q: %q", timestamp, view)
		}
	}
	if !strings.Contains(view, "[a final answer from codex · repo]") {
		t.Fatalf("coalesced final answer did not determine turn treatment: %q", view)
	}
}

func TestListHeightAndVerticalReplyLayout(t *testing.T) {
	layout := responsivePaneLayout(120, 20, true)
	if layout.inboxHeight != 5 || layout.messageWidth != 120 || layout.replyWidth != 120 || layout.messageHeight != 8 || layout.replyHeight != 6 {
		t.Fatalf("vertical layout = %#v", layout)
	}
	for _, test := range []struct {
		height      int
		replyHeight int
	}{{20, 6}, {60, 9}, {80, 12}, {100, 15}} {
		got := responsivePaneLayout(160, test.height, true)
		if got.replyHeight != test.replyHeight {
			t.Fatalf("%d-row reply height = %d; want %d", test.height, got.replyHeight, test.replyHeight)
		}
	}
	item := message("message", testAgentID, model.HumanMailboxID, "Body")
	item.Details = "Kind: update\nCodex thread: thread-1\nCodex turn: turn-1"
	editor := textarea.New()
	editor.Focus()
	m := app{
		messages: []model.Message{item}, answering: true, answerID: item.ID,
		answerGroupKey: messageGroupKey(item), answerQ: item, editor: editor, width: 120, height: 30,
	}
	view := m.View().Content
	lines := strings.Split(view, "\n")
	viewLayout := responsivePaneLayout(m.width, m.height, true)
	if !strings.Contains(lines[viewLayout.inboxHeight+viewLayout.messageHeight], "[reply]") {
		t.Fatalf("reply pane was not rendered below message pane: %q", view)
	}
	for _, line := range lines {
		if strings.Count(line, "╭") > 1 {
			t.Fatalf("panes rendered side by side: %q", line)
		}
	}
}

func TestResponsiveViewFitsTerminalWithVerticalPanes(t *testing.T) {
	item := message("message", testAgentID, model.HumanMailboxID, "Body")
	item.Details = "Kind: update\nCodex thread: thread-1\nCodex turn: turn-1"
	for _, width := range []int{80, 119, 120, 200} {
		editor := textarea.New()
		editor.Focus()
		m := app{
			messages: []model.Message{item}, answering: true, answerID: item.ID,
			answerGroupKey: messageGroupKey(item), answerQ: item, editor: editor, width: width, height: 24,
		}
		m.resizeEditor()
		view := m.View().Content
		if got := lipgloss.Height(view); got != m.height {
			t.Fatalf("%d-column view height = %d; want %d", width, got, m.height)
		}
		for lineNumber, line := range strings.Split(view, "\n") {
			if got := lipgloss.Width(line); got > width {
				t.Fatalf("%d-column view line %d width = %d: %q", width, lineNumber+1, got, line)
			}
		}
		layout := responsivePaneLayout(width, m.height, true)
		lines := strings.Split(view, "\n")
		if lipgloss.Width(lines[0]) != width || !strings.Contains(lines[0], "[HQ · Inbox") || !strings.Contains(lines[layout.inboxHeight], "[an update from") {
			t.Fatalf("%d-column fixture boundaries: %q", width, view)
		}
		if strings.Count(lines[layout.inboxHeight], "╭") != 1 || !strings.Contains(lines[layout.inboxHeight+layout.messageHeight], "[reply]") {
			t.Fatalf("%d-column panes are not vertically stacked: %q", width, view)
		}
	}
}

func TestMessagePanePageScrollingStaysInsideFixture(t *testing.T) {
	var body strings.Builder
	for i := 1; i <= 20; i++ {
		fmt.Fprintf(&body, "line-%02d\n", i)
	}
	item := message("message", testAgentID, model.HumanMailboxID, strings.TrimSpace(body.String()))
	m := app{messages: []model.Message{item}, width: 80, height: 24, paneFocus: focusMessage}
	bottomView := m.View().Content
	bottomLines := strings.Split(bottomView, "\n")[responsivePaneLayout(m.width, m.height, false).inboxHeight:]
	if bottom := strings.Join(bottomLines, "\n"); !strings.Contains(bottom, "line-20") || strings.Contains(bottom, "line-01") {
		t.Fatalf("message pane did not begin at latest content: %q", bottom)
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyPgUp})
	m = updated.(app)
	upperView := m.View().Content
	upperLines := strings.Split(upperView, "\n")[responsivePaneLayout(m.width, m.height, false).inboxHeight:]
	if upper := strings.Join(upperLines, "\n"); !strings.Contains(upper, "line-08") || strings.Contains(upper, "line-20") {
		t.Fatalf("page-up did not scroll within message fixture: %q", upper)
	}
	if lipgloss.Height(upperView) != m.height {
		t.Fatalf("scrolled view height = %d; want %d", lipgloss.Height(upperView), m.height)
	}
}

func TestTabAndShiftTabCyclePaneFocus(t *testing.T) {
	m := app{editor: textarea.New(), width: 80, height: 24}
	borderUses := func(view, label, color string) bool {
		for _, line := range strings.Split(view, "\n") {
			if strings.Contains(line, label) {
				return strings.Contains(line, "\x1b[38;5;"+color+"m")
			}
		}
		return false
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	view := m.View().Content
	if m.paneFocus != focusMessage || !borderUses(view, "[message]", "63") || !borderUses(view, "[HQ · Inbox]", "59") || strings.Contains(view, "focused") {
		t.Fatalf("first tab focus = %v", m.paneFocus)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	view = m.View().Content
	if m.paneFocus != focusReply || !borderUses(view, "[reply]", "63") || !borderUses(view, "[message]", "59") || strings.Contains(view, "focused") {
		t.Fatalf("second tab focus = %v", m.paneFocus)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyTab, Mod: tea.ModShift})
	m = updated.(app)
	if m.paneFocus != focusMessage {
		t.Fatalf("shift-tab focus = %v", m.paneFocus)
	}
}

func TestLeavingReplyStowsDraftAndRestoresInboxNavigation(t *testing.T) {
	question := message("question", testAgentID, model.HumanMailboxID, "Question")
	other := message("other", testAgentID, model.HumanMailboxID, "Other message")
	question.CreatedAt = time.Unix(20, 0)
	other.CreatedAt = time.Unix(10, 0)
	editor := textarea.New()
	editor.SetValue("unfinished reply")
	editor.Focus()
	m := app{
		messages: []model.Message{question, other}, answering: true, answerID: question.ID,
		answerGroupKey: messageGroupKey(question), answerQ: question, activeDraftKey: messageGroupKey(question),
		editor: editor, paneFocus: focusReply, width: 80, height: 24,
	}

	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	if m.answering || m.paneFocus != focusInbox || len(m.drafts) != 1 {
		t.Fatalf("reply was not stowed: %#v", m)
	}
	if view := m.View().Content; !strings.Contains(view, "DRAFT") || !strings.Contains(view, "unfinished reply") {
		t.Fatalf("reply draft was not shown on its thread: %q", view)
	}

	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	m = updated.(app)
	if view := m.View().Content; !strings.Contains(view, "Other message") {
		t.Fatalf("message pane did not follow inbox navigation: %q", view)
	}

	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if !m.answering || m.paneFocus != focusReply || m.editor.Value() != "unfinished reply" {
		t.Fatalf("reply draft did not resume: %#v", m)
	}
}

func TestLeavingNewMessageCreatesOutboundDraftRow(t *testing.T) {
	editor := textarea.New()
	editor.SetValue("unfinished new message")
	editor.Focus()
	m := app{
		answering: true, activeDraftKey: "draft:new", composeTo: "fred-id", composeName: "fred",
		editor: editor, paneFocus: focusReply, width: 80, height: 24,
	}

	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	view := m.View().Content
	if m.answering || m.paneFocus != focusInbox || !strings.Contains(view, "draft → fred") || !strings.Contains(view, "DRAFT") || !strings.Contains(view, "unfinished new message") {
		t.Fatalf("new-message draft row = %q, state = %#v", view, m)
	}
	updated, _ = m.Update(loadedMsg{inbox: []model.Message{message("incoming", testAgentID, model.HumanMailboxID, "Incoming")}})
	m = updated.(app)
	if group, ok := m.groupAtCursor(); !ok || group.draft == nil || group.draft.body != "unfinished new message" {
		t.Fatalf("reload lost draft selection: %#v", m)
	}

	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if !m.answering || m.composeTo != "fred-id" || m.editor.Value() != "unfinished new message" {
		t.Fatalf("new-message draft did not resume: %#v", m)
	}
}

func TestPageKeysApplyToFocusedPane(t *testing.T) {
	messages := make([]model.Message, 0, 10)
	for i := range 10 {
		item := message(fmt.Sprintf("message-%02d", i), testAgentID, model.HumanMailboxID, fmt.Sprintf("Body %02d", i))
		item.CreatedAt = time.Unix(int64(100-i), 0)
		messages = append(messages, item)
	}
	m := app{messages: messages, width: 80, height: 24, paneFocus: focusInbox}
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyPgDown})
	m = updated.(app)
	if m.cursor == 0 || m.messageScroll != 0 {
		t.Fatalf("inbox page-down changed cursor=%d scroll=%d", m.cursor, m.messageScroll)
	}
	m.paneFocus = focusMessage
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyPgUp})
	m = updated.(app)
	if m.messageScroll == 0 {
		t.Fatal("message page-up did not change message scroll")
	}
	previousScroll := m.messageScroll
	m.paneFocus = focusReply
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyPgUp})
	m = updated.(app)
	if m.messageScroll != previousScroll {
		t.Fatalf("reply page-up changed message scroll from %d to %d", previousScroll, m.messageScroll)
	}
}

func TestReplyAndNewMessageUseMailboxID(t *testing.T) {
	s, ctx, agent := openStore(t)
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", agent.ID, model.HumanMailboxID, "Question")
	inbound.SenderLabel = agent.Label
	inbound.Details = "Codex thread: thread-1\nCodex turn: turn-1\nCodex request: request-1"
	if err := s.Create(ctx, inbound); err != nil {
		t.Fatal(err)
	}
	editor := textarea.New()
	editor.SetValue("Answer")
	m := app{ctx: ctx, store: s, messages: []model.Message{inbound}, answering: true, answerID: inbound.ID, answerQ: inbound, editor: editor, paneFocus: focusReply}
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
	if len(replies) != 1 || replies[0].RecipientMailboxID != agent.ID || replies[0].Body != "Answer" || replies[0].Details != "Codex thread: thread-1\nCodex turn: turn-1" {
		t.Fatalf("replies = %#v", replies)
	}

	now := time.Now().UTC()
	inbound.ArchivedAt = &now
	named, err := s.CreateNamedAgent(ctx, "fred", agent.ID)
	if err != nil {
		t.Fatal(err)
	}
	m = app{ctx: ctx, store: s, messages: []model.Message{inbound}, agents: []domain.NamedAgent{named}, editor: textarea.New()}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'r', Text: "r"})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if !m.answering || m.composeTo != agent.ID || m.composeName != "fred" {
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
	if len(sent) != 2 || sent[1].ReplyTo != nil || sent[1].Body != "More detail" || sent[1].RecipientLabel != "fred" {
		t.Fatalf("sent = %#v", sent)
	}
}

func TestRecipientPickerIsIndependentSearchableAndOrdered(t *testing.T) {
	now := time.Now().UTC()
	m := app{
		agents: []domain.NamedAgent{
			{Name: "bob", MailboxID: "bob-id", LastActiveAt: &now},
			{Name: "retired", MailboxID: "retired-id", Retired: true},
			{Name: "alice", MailboxID: "alice-id", Active: true},
		},
		cursor: 4, messageScroll: 3, showSent: true, editor: textarea.New(), width: 70, height: 18,
	}
	choices := m.recipients()
	if len(choices) != 3 || choices[0].name != "alice" || choices[1].name != "self" || choices[2].name != "bob" {
		t.Fatalf("choices = %#v", choices)
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	m = updated.(app)
	if !m.pickingRecipient || m.answering {
		t.Fatalf("picker state = %#v", m)
	}
	for _, character := range []rune{'b', 'o'} {
		updated, _ = m.Update(tea.KeyPressMsg{Code: character, Text: string(character)})
		m = updated.(app)
	}
	if filtered := m.filteredRecipients(); len(filtered) != 1 || filtered[0].name != "bob" {
		t.Fatalf("filtered = %#v", filtered)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if !m.answering || m.pickingRecipient || m.composeTo != "bob-id" || m.composeName != "bob" || m.paneFocus != focusReply {
		t.Fatalf("composition = %#v", m)
	}

	m.answering = false
	m.pickingRecipient = true
	m.pickerQuery = "alice"
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	m = updated.(app)
	if m.pickingRecipient || m.cursor != 4 || m.messageScroll != 3 || !m.showSent {
		t.Fatalf("escape changed inbox state = %#v", m)
	}
}

func TestAgentManagerResumesHistoryAndConfirmsLiveSwitchWithoutChangingInboxState(t *testing.T) {
	base, ctx, mailbox := openStore(t)
	directoryOne, directoryTwo := t.TempDir(), t.TempDir()
	named, err := base.CreateNamedAgent(ctx, "fred", mailbox.ID)
	if err != nil {
		t.Fatal(err)
	}
	for _, selection := range []struct{ id, directory string }{{"thread-one", directoryOne}, {"thread-two", directoryTwo}} {
		if _, err := base.SelectNamedAgentSession(ctx, "fred", model.SessionIdentity{Harness: "codex", ExternalSessionID: selection.id}, model.RepositoryContext{Directory: selection.directory}); err != nil {
			t.Fatal(err)
		}
	}
	named, _ = base.GetNamedAgent(ctx, "fred")
	runtimeStore := &runtimeTestStore{testDomainStore: base, runtime: domain.CodexRuntime{AgentName: "fred", Phase: domain.CodexRuntimeOffline}}
	editor := textarea.New()
	editor.SetValue("preserved draft")
	m := app{
		ctx: ctx, store: runtimeStore, agents: []domain.NamedAgent{named}, editor: editor,
		cursor: 3, messageScroll: 2, launchDirectory: directoryOne, launchEnvironment: []string{"PATH=/tui/bin", "TOKEN=transient"},
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'g', Text: "g"})
	m = updated.(app)
	updated, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if cmd == nil {
		t.Fatal("agent selection did not load sessions asynchronously")
	}
	updated, _ = m.Update(cmd())
	m = updated.(app)
	if m.agentManager.stage != chooseRuntimeSession || len(m.agentManager.sessions) != 2 {
		t.Fatalf("session chooser = %#v", m.agentManager)
	}
	for index, session := range m.agentManager.sessions {
		if session.SessionID == "thread-one" {
			m.agentManager.cursor = index + 1
		}
	}
	updated, cmd = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if cmd == nil {
		t.Fatal("offline historical resume did not launch")
	}
	updated, _ = m.Update(cmd())
	m = updated.(app)
	if len(runtimeStore.launches) != 1 || runtimeStore.launches[0].SessionID != "thread-one" || runtimeStore.launches[0].Directory != directoryOne || strings.Join(runtimeStore.launches[0].Environment, "|") != "PATH=/tui/bin|TOKEN=transient" {
		t.Fatalf("resume request = %#v", runtimeStore.launches)
	}
	if m.cursor != 3 || m.messageScroll != 2 || m.editor.Value() != "preserved draft" {
		t.Fatalf("agent manager changed inbox state: %#v", m)
	}

	m.agentManager.stage, m.agentManager.cursor = chooseRuntimeSession, 0
	m.agentManager.runtime = domain.CodexRuntime{AgentName: "fred", ThreadID: "thread-one", Directory: directoryOne, Phase: domain.CodexRuntimeRunning}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.agentManager.stage != enterRuntimeDirectory {
		t.Fatalf("new thread stage = %v", m.agentManager.stage)
	}
	m.agentManager.directory = directoryTwo
	updated, cmd = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if cmd != nil || m.agentManager.stage != confirmRuntimeSwitch {
		t.Fatalf("live switch was not confirmed first: stage=%v cmd=%v", m.agentManager.stage, cmd)
	}
	updated, cmd = m.Update(tea.KeyPressMsg{Code: 'y', Text: "y"})
	if cmd == nil {
		t.Fatal("confirmed live switch did not launch asynchronously")
	}
	m = updated.(app)
	_ = cmd()
	if len(runtimeStore.launches) != 2 || !runtimeStore.launches[1].ConfirmSwitch || runtimeStore.launches[1].Action != domain.CodexSessionNew || runtimeStore.launches[1].Directory != directoryTwo {
		t.Fatalf("new-thread request = %#v", runtimeStore.launches)
	}
}

func TestNewMessageToSelfIsRootNoteAndDoesNotArchiveSelection(t *testing.T) {
	s, ctx, agent := openStore(t)
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d81", agent.ID, model.HumanMailboxID, "Keep this visible")
	if err := s.Create(ctx, inbound); err != nil {
		t.Fatal(err)
	}
	m := app{ctx: ctx, store: s, inbox: []model.Message{inbound}, editor: textarea.New()}
	m.setMessages()
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.composeTo != model.HumanMailboxID || m.composeName != "self" {
		t.Fatalf("recipient = %#v", m)
	}
	m.editor.SetValue("remember this")
	result := m.answer().(answeredMsg)
	if result.err != nil || !result.sent {
		t.Fatalf("send = %v, sent=%t", result.err, result.sent)
	}
	notes, err := s.List(ctx, model.Filter{SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: model.HumanMailboxID})
	if err != nil || len(notes) != 1 || notes[0].ReplyTo != nil || notes[0].Body != "remember this" {
		t.Fatalf("notes = %#v, %v", notes, err)
	}
	stillOpen, err := s.Get(ctx, inbound.ID)
	if err != nil || stillOpen.ArchivedAt != nil {
		t.Fatalf("selection archived = %#v, %v", stillOpen, err)
	}
}

func TestRetiredRecipientKeepsDraftAndShowsActionableError(t *testing.T) {
	s, ctx, agent := openStore(t)
	named, err := s.CreateNamedAgent(ctx, "fred", agent.ID)
	if err != nil {
		t.Fatal(err)
	}
	editor := textarea.New()
	m := app{ctx: ctx, store: s, agents: []domain.NamedAgent{named}, editor: editor}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'r', Text: "r"})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	m.editor.SetValue("draft survives retirement")
	if err := s.RetireNamedAgent(ctx, "fred"); err != nil {
		t.Fatal(err)
	}
	result := m.answer().(answeredMsg)
	updated, _ = m.Update(result)
	m = updated.(app)
	if m.answering || !m.pickingRecipient || m.editor.Value() != "draft survives retirement" || m.err == nil || !strings.Contains(m.err.Error(), "choose a recipient again") {
		t.Fatalf("retirement result = %#v", m)
	}
}

func TestAgentReloadPreservesPickerAndComposerState(t *testing.T) {
	editor := textarea.New()
	m := app{pickingRecipient: true, pickerQuery: "r", pickerCursor: 2, cursor: 3, editor: editor}
	updated, _ := m.Update(loadedMsg{agents: []domain.NamedAgent{{Name: "fred", MailboxID: "fred-id"}}})
	m = updated.(app)
	if !m.pickingRecipient || m.pickerQuery != "r" || m.pickerCursor != 0 || m.cursor != 0 {
		t.Fatalf("picker reload = %#v", m)
	}
	m.pickingRecipient = false
	m.answering = true
	m.composeTo, m.composeName, m.composeNamed = "fred-id", "fred", true
	m.editor.SetValue("unfinished")
	updated, _ = m.Update(loadedMsg{agents: nil})
	m = updated.(app)
	if !m.answering || m.composeTo != "fred-id" || m.editor.Value() != "unfinished" {
		t.Fatalf("composer reload = %#v", m)
	}
}

func TestRecipientPickerFitsSmallTerminal(t *testing.T) {
	m := app{agents: []domain.NamedAgent{{Name: "fred", MailboxID: "fred-id"}}, pickingRecipient: true, paneFocus: focusReply, editor: textarea.New(), width: 38, height: 12}
	view := m.View().Content
	lines := strings.Split(view, "\n")
	if len(lines) > 12 || !strings.Contains(view, "recipient") || !strings.Contains(view, "choose a local") {
		t.Fatalf("small picker = %q", view)
	}
}

func TestRecipientPickerShowsThreeChoicesAtMinimumHeight(t *testing.T) {
	m := app{
		agents: []domain.NamedAgent{
			{Name: "alice", MailboxID: "alice-id", Active: true},
			{Name: "bob", MailboxID: "bob-id"},
		},
		pickingRecipient: true,
		paneFocus:        focusReply,
		editor:           textarea.New(),
	}
	view := m.renderRecipientPicker(80, 6)
	for _, name := range []string{"alice", "self", "bob"} {
		if !strings.Contains(view, name) {
			t.Fatalf("minimum-height picker omitted %q: %q", name, view)
		}
	}
}

func TestReplyArchivesEveryVisibleMessageInTurn(t *testing.T) {
	s, ctx, agent := openStore(t)
	first := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d72", agent.ID, model.HumanMailboxID, "First update")
	first.Details = "Kind: update\nCodex thread: thread-1\nCodex turn: turn-1"
	final := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d73", agent.ID, model.HumanMailboxID, "Final answer")
	final.CreatedAt = first.CreatedAt.Add(time.Second)
	final.Details = "Kind: final-answer\nCodex thread: thread-1\nCodex turn: turn-1"
	for _, item := range []model.Message{first, final} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}

	m := app{ctx: ctx, store: s, inbox: []model.Message{final, first}, editor: textarea.New()}
	m.setMessages()
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.answerID != final.ID {
		t.Fatalf("reply target = %q; want final answer %q", m.answerID, final.ID)
	}
	m.editor.SetValue("Thanks")
	result := m.answer().(answeredMsg)
	if result.err != nil || !result.sent {
		t.Fatalf("answer result = %#v", result)
	}

	for _, item := range []model.Message{first, final} {
		archived, err := s.Get(ctx, item.ID)
		if err != nil || archived.ArchivedAt == nil {
			t.Fatalf("turn message %s was not archived: %#v, %v", item.ID, archived, err)
		}
	}
	replies, err := s.List(ctx, model.Filter{ReplyTo: final.ID})
	if err != nil || len(replies) != 1 || replies[0].Body != "Thanks" {
		t.Fatalf("replies = %#v, %v", replies, err)
	}
}

func TestSentReplyClosesDraftWhenTurnArchiveReportsError(t *testing.T) {
	editor := textarea.New()
	editor.SetValue("already sent")
	m := app{answering: true, answerID: "message", answerGroupKey: "turn", editor: editor, paneFocus: focusReply}
	archiveErr := errors.New("archive failed")

	updated, _ := m.Update(answeredMsg{err: archiveErr, sent: true})
	m = updated.(app)
	if m.answering || m.answerID != "" || m.editor.Value() != "" || !errors.Is(m.err, archiveErr) {
		t.Fatalf("sent reply retained a duplicate-able draft: %#v", m)
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

func TestUUndoesTurnArchive(t *testing.T) {
	s, ctx, agent := openStore(t)
	first := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d70", agent.ID, model.HumanMailboxID, "First")
	first.Details = "Kind: update\nCodex thread: thread-1\nCodex turn: turn-1"
	second := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d71", agent.ID, model.HumanMailboxID, "Second")
	second.CreatedAt = first.CreatedAt.Add(time.Second)
	second.Details = "Kind: final-answer\nCodex thread: thread-1\nCodex turn: turn-1"
	for _, item := range []model.Message{first, second} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	m := app{ctx: ctx, store: s, inbox: []model.Message{second, first}, editor: textarea.New()}
	m.setMessages()
	updated, cmd := m.Update(tea.KeyPressMsg{Code: 'd', Text: "d"})
	if cmd == nil {
		t.Fatal("d did not schedule grouped archive")
	}
	m = updated.(app)
	archiveResult := cmd().(archivedMsg)
	if archiveResult.err != nil || len(archiveResult.messageIDs) != 2 {
		t.Fatalf("archive result = %#v", archiveResult)
	}
	updated, _ = m.Update(archiveResult)
	m = updated.(app)
	if len(m.undoStack) != 1 || !strings.Contains(m.undoNotice, "press u to undo") {
		t.Fatalf("undo state after archive = %#v, %q", m.undoStack, m.undoNotice)
	}
	updated, cmd = m.Update(tea.KeyPressMsg{Code: 'u', Text: "u"})
	if cmd == nil {
		t.Fatal("u did not schedule restore")
	}
	m = updated.(app)
	restoreResult := cmd().(restoredMsg)
	if restoreResult.err != nil {
		t.Fatal(restoreResult.err)
	}
	updated, _ = m.Update(restoreResult)
	m = updated.(app)
	if len(m.undoStack) != 0 || !strings.Contains(m.undoNotice, "restored 2") {
		t.Fatalf("undo state after restore = %#v, %q", m.undoStack, m.undoNotice)
	}
	for _, item := range []model.Message{first, second} {
		restored, err := s.Get(ctx, item.ID)
		if err != nil || restored.ArchivedAt != nil {
			t.Fatalf("message %s was not restored: %#v, %v", item.ID, restored, err)
		}
	}
}

func TestShiftEnterAndCtrlJInsertNewlines(t *testing.T) {
	editor := textarea.New()
	editor.KeyMap.InsertNewline = key.NewBinding(key.WithKeys("shift+enter", "ctrl+j"))
	editor.SetValue("first")
	editor.Focus()
	m := app{answering: true, answerID: "id", editor: editor, paneFocus: focusReply}
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
	m := app{answering: true, answerID: "id", editor: editor, paneFocus: focusReply}

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
