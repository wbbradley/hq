package tui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"slices"
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
	runtime  domain.HarnessRuntime
	launches []domain.HarnessLaunchRequest
}

type pagedHistoryStore struct {
	domain.Store
	calls int
}

func (s *pagedHistoryStore) ListConversationHistory(_ context.Context, filter model.ConversationHistoryFilter) (model.MessagePage, error) {
	s.calls++
	if filter.Cursor == "" {
		messages := make([]model.Message, 200)
		for index := range messages {
			messages[index] = message(fmt.Sprintf("message-%03d", index), testAgentID, model.HumanMailboxID, fmt.Sprintf("body-%03d", index))
		}
		return model.MessagePage{Messages: messages, NextCursor: "next-page"}, nil
	}
	return model.MessagePage{Messages: []model.Message{message("message-200", testAgentID, model.HumanMailboxID, "body-200")}}, nil
}

type pagedEntryStore struct {
	domain.Store
	calls   int
	filters []model.ConversationHistoryFilter
}

func (s *pagedEntryStore) ListConversationEntries(_ context.Context, filter model.ConversationHistoryFilter) (domain.ConversationEntryPage, error) {
	s.calls++
	s.filters = append(s.filters, filter)
	messageEntry := func(index int) domain.ConversationEntry {
		item := message(fmt.Sprintf("entry-message-%03d", index), testAgentID, model.HumanMailboxID, fmt.Sprintf("entry-body-%03d", index))
		return domain.ConversationEntry{Kind: domain.ConversationEntryMessage, EventID: fmt.Sprintf("%064x", index+1), DisplayOrder: index, Message: &item}
	}
	if filter.Cursor == "" {
		entries := make([]domain.ConversationEntry, 200)
		for index := range entries {
			entries[index] = messageEntry(index)
		}
		return domain.ConversationEntryPage{Entries: entries, NextCursor: "next-entry-page"}, nil
	}
	return domain.ConversationEntryPage{Entries: []domain.ConversationEntry{messageEntry(200)}}, nil
}

type outboundCaptureStore struct {
	domain.Store
	agent           domain.NamedAgent
	created         model.Message
	replied         model.Message
	repliedOriginal string
}

type worktreeCaptureStore struct {
	domain.Store
	request domain.ProjectWorktreeRequest
	project domain.Project
}

func (s *worktreeCaptureStore) ProvisionProjectWorktree(_ context.Context, request domain.ProjectWorktreeRequest) (domain.Project, error) {
	s.request = request
	return s.project, nil
}

func (s *outboundCaptureStore) GetNamedAgent(context.Context, string) (domain.NamedAgent, error) {
	return s.agent, nil
}

func (s *outboundCaptureStore) Create(_ context.Context, created model.Message) error {
	s.created = created
	return nil
}

func (s *outboundCaptureStore) Reply(_ context.Context, originalID string, reply model.Message) error {
	s.repliedOriginal = originalID
	s.replied = reply
	return nil
}

func (s *runtimeTestStore) LaunchHarnessAgent(_ context.Context, request domain.HarnessLaunchRequest) (domain.HarnessRuntime, error) {
	copyRequest := request
	copyRequest.Environment = append([]string(nil), request.Environment...)
	s.launches = append(s.launches, copyRequest)
	s.runtime = domain.HarnessRuntime{AgentName: request.AgentName, Harness: request.Harness, SessionID: request.SessionID, Directory: request.Directory, Phase: domain.HarnessRuntimeRunning}
	if s.runtime.SessionID == "" {
		s.runtime.SessionID = "thread-new"
	}
	return s.runtime, nil
}

func (s *runtimeTestStore) StopHarnessAgent(_ context.Context, name string) (domain.HarnessRuntime, error) {
	s.runtime = domain.HarnessRuntime{AgentName: name, Phase: domain.HarnessRuntimeOffline}
	return s.runtime, nil
}

func (s *runtimeTestStore) HarnessAgentRuntime(context.Context, string) (domain.HarnessRuntime, error) {
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

func TestStaleReloadCannotReplaceNewerConversationSnapshot(t *testing.T) {
	previous := message("previous-snapshot", testAgentID, model.HumanMailboxID, "Previous reply")
	latest := message("latest-snapshot", testAgentID, model.HumanMailboxID, "Latest reply")
	setMessageSemantics(&previous, "Harness provider: codex\nHarness session: reload-thread\nHarness operation: previous-turn")
	setMessageSemantics(&latest, "Harness provider: codex\nHarness session: reload-thread\nHarness operation: latest-turn")
	latest.CreatedAt = previous.CreatedAt.Add(time.Second)
	m := app{inbox: []model.Message{previous}, loadGeneration: 2, width: 80, height: 24}
	m.setMessages()
	m.reconcileMessageViewport(false)

	updated, _ := m.Update(loadedMsg{generation: 2, inbox: []model.Message{latest, previous}})
	m = updated.(app)
	if group, found := m.detailGroup(); !found || group.latest().ID != latest.ID {
		t.Fatalf("newest snapshot was not applied: %#v", group)
	}

	updated, _ = m.Update(loadedMsg{generation: 1, inbox: []model.Message{previous}})
	m = updated.(app)
	if group, found := m.detailGroup(); !found || group.latest().ID != latest.ID || m.messageLiveAnchorID != latest.ID {
		t.Fatalf("stale snapshot replaced latest conversation state: group=%#v anchor=%q", group, m.messageLiveAnchorID)
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
	group := groupMessages(m.messages)[0]
	view := m.renderGroupPanel(group, m.width)
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

func TestConversationLoadShowsCompleteBidirectionalHistory(t *testing.T) {
	s, ctx, agent := openStore(t)
	created := time.Now().UTC().Add(-time.Minute)
	first := message("0198c7ec-73b0-7cc3-a5f7-e31c77140df1", agent.ID, model.HumanMailboxID, "first inbound")
	first.CreatedAt = created
	setMessageSemantics(&first, "Harness provider: codex\nHarness session: history-thread\nHarness operation: turn-one")
	human := message("0198c7ec-73b0-7cc3-a5f7-e31c77140df2", model.HumanMailboxID, agent.ID, "human response")
	human.CreatedAt = created.Add(time.Second)
	setMessageSemantics(&human, "Harness provider: codex\nHarness session: history-thread\nHarness operation: turn-one")
	second := message("0198c7ec-73b0-7cc3-a5f7-e31c77140df3", agent.ID, model.HumanMailboxID, "second inbound")
	second.CreatedAt = created.Add(2 * time.Second)
	setMessageSemantics(&second, "Harness provider: codex\nHarness session: history-thread\nHarness operation: turn-two")
	for _, item := range []model.Message{first, human, second} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Archive(ctx, first.ID); err != nil {
		t.Fatal(err)
	}
	m := app{ctx: ctx, store: s, editor: textarea.New(), markdown: newMessageMarkdownRenderer(nil), width: 100, height: 32}
	loaded := m.load().(loadedMsg)
	updated, _ := m.Update(loaded)
	m = updated.(app)
	groups := m.visibleGroups()
	if len(groups) != 1 || len(groups[0].messages) != 3 {
		t.Fatalf("conversation groups = %#v", groups)
	}
	for index, body := range []string{"first inbound", "human response", "second inbound"} {
		if groups[0].messages[index].Body != body {
			t.Fatalf("history[%d] = %q; want %q", index, groups[0].messages[index].Body, body)
		}
	}
	if view := m.View().Content; !strings.Contains(view, "You →") || !strings.Contains(view, "first inbound") {
		t.Fatalf("bidirectional history view = %q", view)
	}
}

func TestConversationReloadPreservesStableSelection(t *testing.T) {
	firstMessage := message("selection-first", testAgentID, model.HumanMailboxID, "first")
	setMessageSemantics(&firstMessage, "Harness provider: codex\nHarness session: first-thread")
	secondMessage := message("selection-second", testAgentID, model.HumanMailboxID, "second")
	setMessageSemantics(&secondMessage, "Harness provider: codex\nHarness session: second-thread")
	first := model.ConversationSummary{Key: conversationKeyForMessage(firstMessage), Latest: firstMessage}
	second := model.ConversationSummary{Key: conversationKeyForMessage(secondMessage), Latest: secondMessage}
	m := app{conversations: []model.ConversationSummary{first, second}, conversationMode: true, cursor: 1}
	m.setMessages()
	selectedKey := m.selectedGroupKey()
	updated, _ := m.Update(loadedMsg{conversations: []model.ConversationSummary{second, first}, histories: map[string][]model.Message{selectedKey: {secondMessage}}})
	m = updated.(app)
	group, ok := m.groupAtCursor()
	if !ok || group.key != selectedKey {
		t.Fatalf("selection moved after reorder: cursor=%d group=%#v want=%q", m.cursor, group, selectedKey)
	}
}

func TestConversationHistoryLoaderExhaustsPages(t *testing.T) {
	store := &pagedHistoryStore{}
	m := app{ctx: context.Background(), store: store}
	key := model.ConversationKey{CounterpartyMailboxID: testAgentID, HarnessProvider: "codex", HarnessSessionID: "thread"}
	messages, err := m.loadAllConversationHistory(key)
	if err != nil || len(messages) != 201 || store.calls != 2 || messages[200].Body != "body-200" {
		t.Fatalf("paged history = %d messages / %d calls / %v", len(messages), store.calls, err)
	}
}

func TestUnifiedConversationLoaderExhaustsPagesAndDerivesCompatibilitySlices(t *testing.T) {
	store := &pagedEntryStore{}
	m := app{ctx: context.Background(), store: store}
	key := model.ConversationKey{CounterpartyMailboxID: testAgentID, HarnessProvider: "codex", HarnessSessionID: "thread"}
	entries, err := m.loadAllConversationEntries(key)
	if err != nil || len(entries) != 201 || store.calls != 2 || entries[200].Message.Body != "entry-body-200" {
		t.Fatalf("paged entries = %d entries / %d calls / %v", len(entries), store.calls, err)
	}
	if store.filters[0].Limit != 200 || store.filters[0].Key != key || store.filters[1].Cursor != "next-entry-page" {
		t.Fatalf("entry filters = %#v", store.filters)
	}

	activity := domain.HarnessActivity{EventID: strings.Repeat("a", 64), MailboxID: testAgentID, Harness: "codex", SessionID: "thread", OperationID: "operation", Kind: domain.HarnessActivityPlan, Body: "plan"}
	mixed := []domain.ConversationEntry{entries[0], {Kind: domain.ConversationEntryActivity, EventID: activity.EventID, DisplayOrder: 1, Activity: &activity}}
	messages, activities := splitConversationEntries(mixed)
	if len(messages) != 1 || messages[0].Body != "entry-body-000" || len(activities) != 1 || activities[0].Body != "plan" {
		t.Fatalf("derived slices = %#v / %#v", messages, activities)
	}
}

func TestConversationDetailRefreshUsesUnifiedEntriesAsAuthoritativeHistory(t *testing.T) {
	store := &pagedEntryStore{}
	key := model.ConversationKey{CounterpartyMailboxID: testAgentID, HarnessProvider: "codex", HarnessSessionID: "thread"}
	stableKey := conversationKeyString(key)
	m := app{
		ctx: context.Background(), store: store, conversationMode: true,
		conversations:  []model.ConversationSummary{{Key: key, Latest: message("summary-latest", testAgentID, model.HumanMailboxID, "summary")}},
		entryHistories: make(map[string][]domain.ConversationEntry), histories: make(map[string][]model.Message), activities: make(map[string][]domain.HarnessActivity),
	}
	command := m.loadConversationHistory(key)
	if command == nil {
		t.Fatal("unloaded unified conversation did not schedule a detail read")
	}
	loaded := command().(historyLoadedMsg)
	if loaded.err != nil || len(loaded.entries) != 201 || len(loaded.messages) != 201 || store.calls != 2 {
		t.Fatalf("unified detail result = %#v, calls=%d", loaded, store.calls)
	}
	updated, _ := m.Update(loaded)
	m = updated.(app)
	group, found := m.groupByKey(stableKey)
	if !found || !group.entriesLoaded || len(group.entries) != 201 || len(group.messages) != 201 || len(group.activities) != 0 {
		t.Fatalf("authoritative detail group = %#v", group)
	}
	if m.loadConversationHistory(key) != nil {
		t.Fatal("loaded unified conversation scheduled a duplicate read")
	}
}

func TestNewNamedAgentMessageRecordsCurrentCodexThread(t *testing.T) {
	store := &outboundCaptureStore{agent: domain.NamedAgent{Name: "fred", MailboxID: testAgentID, Harness: "codex", CurrentSessionID: "current-codex-thread"}}
	editor := textarea.New()
	editor.SetValue("hello")
	m := app{ctx: context.Background(), store: store, editor: editor, answering: true, composeTo: testAgentID, composeName: "fred", composeNamed: true}
	result := m.answer().(answeredMsg)
	if result.err != nil || store.created.Correlation != (model.MessageCorrelation{Provider: "codex", SessionID: "current-codex-thread"}) || store.created.Details != "" {
		t.Fatalf("created message = %#v, result=%#v", store.created, result)
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
	view = m.renderGroupPanel(groupMessages(m.messages)[0], m.width)
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
		item.Presentation = model.PresentationKind(test.kind)
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
	setMessageSemantics(&legacy, "Phase: final_answer")
	if got := presentationKind(legacy); got != "final-answer" {
		t.Fatalf("legacy kind = %q", got)
	}
}

func TestTypedMailboxAddressDisplayIsExhaustive(t *testing.T) {
	context := model.RepositoryContext{Directory: "/work/repo"}
	tests := []struct {
		name    string
		address model.MessageAddress
		want    string
	}{
		{name: "human", address: model.MessageAddress{Kind: model.MailboxHuman, Label: "wrong"}, want: "human"},
		{name: "unnamed agent", address: model.MessageAddress{Kind: model.MailboxAgent, Label: "codex", Harness: "codex"}, want: "codex · repo"},
		{name: "named agent", address: model.MessageAddress{Kind: model.MailboxAgent, Label: "alice", Harness: "codex", Name: "alice"}, want: "alice"},
		{name: "project", address: model.MessageAddress{Kind: model.MailboxProject, Label: "TUI Work"}, want: "TUI Work"},
		{name: "remote", address: model.MessageAddress{Kind: model.MailboxRemote, Label: "silver"}, want: "silver"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := displayMessageAddress(test.address, "fallback", context); got != test.want {
				t.Fatalf("display = %q, want %q", got, test.want)
			}
		})
	}
}

func TestInboxRowsUseOnlyNeededSpaceAfterAgentName(t *testing.T) {
	item := message("compact-row", testAgentID, model.HumanMailboxID, "Working on it")
	item.Context.Directory = "/work/repo"
	setMessageSemantics(&item, "Kind: update")
	view := ansi.Strip((app{messages: []model.Message{item}, width: 100, height: 24}).View().Content)
	if !strings.Contains(view, "codex · repo [update] Working on it") {
		t.Fatalf("inbox row retained a fixed-width agent column: %q", view)
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
	item.Presentation = model.PresentationUpdate
	item.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "codex-thread-opaque", OperationID: "turn-opaque"}
	item.TechnicalSections = []model.TechnicalSection{{Namespace: "vendor.experimental", Fields: []model.TechnicalField{
		{Key: "opaque_key", Label: "Opaque label", Value: "opaque-value"},
		{Key: "second_key", Value: "second-value"},
	}}}
	item.Details = "Kind: human heading\nHarness session: human example\nProject assignment: human note\n\nChoose one:\n- accept\n- decline"
	m := app{messages: []model.Message{item}, width: 100, height: 80}

	view := m.View().Content
	for _, hidden := range []string{item.ID, item.EventID, item.ThreadID, item.SenderInstallationID, item.RecipientInstallationID, replyTo, "codex-thread-opaque", "turn-opaque", "vendor.experimental", "Opaque label", "opaque-value", "second_key", "second-value"} {
		if strings.Contains(view, hidden) {
			t.Fatalf("collapsed view exposed %q: %q", hidden, view)
		}
	}
	for _, human := range []string{"Kind: human heading", "Harness session: human example", "Project assignment: human note", "Choose one:", "- accept"} {
		if !strings.Contains(view, human) {
			t.Fatalf("collapsed view lost human detail %q: %q", human, view)
		}
	}
	if !strings.Contains(view, "technical details hidden · press i to show") {
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
	for _, shown := range []string{item.ID, item.EventID, item.ThreadID, item.SenderInstallationID, item.RecipientInstallationID, replyTo, "codex-thread-opaque", "turn-opaque", "hq.message.identifiers", "hq.message.correlation", "vendor.experimental", "Opaque label", "opaque-value", "second_key", "second-value"} {
		if !strings.Contains(view, shown) {
			t.Fatalf("expanded view omitted %q: %q", shown, view)
		}
	}
	if namespaceIndex, labelIndex, keyIndex := strings.Index(view, "vendor.experimental"), strings.Index(view, "Opaque label: opaque-value"), strings.Index(view, "second_key: second-value"); namespaceIndex < 0 || namespaceIndex > labelIndex || labelIndex > keyIndex {
		t.Fatalf("technical section order changed: %q", view)
	}
}

func TestTechnicalMetadataCannotChangeMessageBehavior(t *testing.T) {
	created := time.Date(2026, 8, 25, 12, 0, 0, 0, time.UTC)
	question := message("question", testAgentID, model.HumanMailboxID, "Choose")
	question.CreatedAt = created
	question.Correlation = model.MessageCorrelation{Provider: "home-built", SessionID: "session", OperationID: "operation", RequestID: "request"}
	question.TechnicalSections = []model.TechnicalSection{{Namespace: "vendor.before", Fields: []model.TechnicalField{{Key: "before", Label: "Before", Value: "one"}}}}
	final := message("final", testAgentID, model.HumanMailboxID, "Done")
	final.CreatedAt = created.Add(time.Second)
	final.Presentation = model.PresentationFinalAnswer
	final.Correlation = model.MessageCorrelation{Provider: "home-built", SessionID: "session", OperationID: "operation"}
	final.TechnicalSections = []model.TechnicalSection{{Namespace: "vendor.before", Fields: []model.TechnicalField{{Key: "before", Label: "Before", Value: "two"}}}}

	before := groupMessages([]model.Message{final, question})
	question.TechnicalSections = []model.TechnicalSection{{Namespace: "unrecognized.changed", Fields: []model.TechnicalField{{Key: "renamed", Label: "Renamed", Value: "different"}}}}
	final.TechnicalSections = []model.TechnicalSection{{Namespace: "also.changed", Fields: []model.TechnicalField{{Key: "final_answer", Label: "Looks behavioral", Value: "no"}}}}
	after := groupMessages([]model.Message{final, question})

	if len(before) != 1 || len(after) != 1 || before[0].key != after[0].key {
		t.Fatalf("technical metadata changed conversation grouping: before=%#v after=%#v", before, after)
	}
	if actionUnitKey(before[0].messages[0]) != actionUnitKey(after[0].messages[0]) || groupPresentationKind(before[0]) != "final-answer" || groupPresentationKind(after[0]) != "final-answer" {
		t.Fatalf("technical metadata changed action or presentation: before=%#v after=%#v", before, after)
	}
	if replyTarget(before[0]).ID != question.ID || replyTarget(after[0]).ID != question.ID {
		t.Fatalf("technical metadata changed reply target: before=%q after=%q", replyTarget(before[0]).ID, replyTarget(after[0]).ID)
	}
}

func TestTypedCorrelationIsOnlyTUIHarnessBehaviorSource(t *testing.T) {
	created := time.Date(2026, 8, 25, 13, 0, 0, 0, time.UTC)
	flatFirst := message("flat-first", testAgentID, model.HumanMailboxID, "First")
	flatFirst.ThreadID = "causal-one"
	flatFirst.CreatedAt = created
	flatFirst.HarnessProvider = "deprecated"
	flatFirst.HarnessSessionID = "same-flat-session"
	flatFirst.HarnessOperationID = "same-flat-operation"
	flatSecond := message("flat-second", testAgentID, model.HumanMailboxID, "Second")
	flatSecond.ThreadID = "causal-two"
	flatSecond.CreatedAt = created.Add(time.Second)
	flatSecond.HarnessProvider = flatFirst.HarnessProvider
	flatSecond.HarnessSessionID = flatFirst.HarnessSessionID
	flatSecond.HarnessOperationID = flatFirst.HarnessOperationID

	if groups := groupMessages([]model.Message{flatSecond, flatFirst}); len(groups) != 2 {
		t.Fatalf("flat-only compatibility fields merged causal conversations: %#v", groups)
	}
	if got := actionUnitKey(flatFirst); got != "thread:"+flatFirst.ThreadID {
		t.Fatalf("flat-only operation selected action unit %q", got)
	}

	typed := model.MessageCorrelation{Provider: "typed", SessionID: "typed-session", OperationID: "typed-operation"}
	typedFirst, typedSecond := flatFirst, flatSecond
	typedFirst.Correlation, typedSecond.Correlation = typed, typed
	typedFirst.HarnessProvider, typedSecond.HarnessProvider = "conflict-one", "conflict-two"
	typedFirst.HarnessSessionID, typedSecond.HarnessSessionID = "flat-one", "flat-two"
	typedFirst.HarnessOperationID, typedSecond.HarnessOperationID = "operation-one", "operation-two"
	groups := groupMessages([]model.Message{typedSecond, typedFirst})
	if len(groups) != 1 || actionUnitKey(typedFirst) != "operation:"+typed.OperationID || actionUnitKey(typedSecond) != "operation:"+typed.OperationID {
		t.Fatalf("typed correlation did not override conflicting compatibility fields: %#v", groups)
	}

	capture := &outboundCaptureStore{}
	editor := textarea.New()
	editor.SetValue("Reply")
	m := app{ctx: context.Background(), store: capture, answering: true, answerID: flatFirst.ID, answerQ: flatFirst, editor: editor}
	result := m.answer().(answeredMsg)
	if result.err != nil || !result.sent || capture.repliedOriginal != flatFirst.ID {
		t.Fatalf("flat-only reply failed: result=%#v original=%q", result, capture.repliedOriginal)
	}
	if !capture.replied.Correlation.Empty() || capture.replied.HarnessProvider != "" || capture.replied.HarnessSessionID != "" || capture.replied.HarnessOperationID != "" {
		t.Fatalf("flat-only compatibility fields leaked into reply: %#v", capture.replied)
	}
}

func TestMessagePanelCombinesKindAndSenderInBorder(t *testing.T) {
	item := message("message-id", testAgentID, model.HumanMailboxID, "Working on it")
	item.Context.Directory = "/work/repo"
	setMessageSemantics(&item, "Kind: update")
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

	setMessageSemantics(&item, "Kind: final-answer")
	view = (app{messages: []model.Message{item}}).View().Content
	if !strings.Contains(view, "[a final answer from codex · repo]") || strings.Contains(view, "From: codex · repo") {
		t.Fatalf("final-answer border title: %q", view)
	}
}

func TestFinalAnswerPanelKeepsFinalAnswerSenderAfterHumanReply(t *testing.T) {
	answer := message("answer-id", "project-mailbox", model.HumanMailboxID, "The answer")
	answer.SenderLabel = "alice · TUI Work"
	answer.SenderAddress = model.MessageAddress{MailboxID: "project-mailbox", Kind: model.MailboxProject, Label: "alice · TUI Work"}
	answer.ThreadID = "project-conversation"
	setMessageSemantics(&answer, "Kind: final-answer")
	reply := message("reply-id", model.HumanMailboxID, "project-mailbox", "A follow-up")
	reply.SenderLabel = "silver"
	reply.RecipientLabel = "TUI Work"
	reply.RecipientAddress = model.MessageAddress{MailboxID: "project-mailbox", Kind: model.MailboxProject, Label: "TUI Work"}
	reply.ThreadID = answer.ThreadID
	reply.CreatedAt = answer.CreatedAt.Add(time.Second)
	view := (app{messages: []model.Message{answer, reply}}).View().Content
	if !strings.Contains(view, "[a final answer from alice · TUI Work]") || strings.Contains(view, "[a final answer from silver]") || !strings.Contains(view, "You → TUI Work") {
		t.Fatalf("replied final-answer panel attribution = %q", view)
	}
}

func TestMessagePanelRendersOnlyBodiesAsMarkdown(t *testing.T) {
	item := message("message-id", testAgentID, model.HumanMailboxID, "Body with **bold text**")
	setMessageSemantics(&item, "Kind: update\nVisible **detail markers**")
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
	setMessageSemantics(&item, "Kind: update")
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
	setMessageSemantics(&first, "Kind: update\nHarness provider: codex\nHarness session: thread\nHarness operation: turn")
	second := message("second", testAgentID, model.HumanMailboxID, "Second *italic part*")
	second.CreatedAt = created.Add(time.Second)
	setMessageSemantics(&second, "Kind: final-answer\nHarness provider: codex\nHarness session: thread\nHarness operation: turn")
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

func TestReplyEditsReuseRenderedConversationPanel(t *testing.T) {
	item := message("cached-panel", testAgentID, model.HumanMailboxID, "Original body")
	editor := textarea.New()
	editor.SetValue("first draft")
	m := app{
		messages: []model.Message{item}, groups: groupMessages([]model.Message{item}),
		answering: true, answerID: item.ID, answerGroupKey: messageGroupKey(item), answerQ: item,
		editor: editor, paneFocus: focusReply, width: 100, height: 30,
		markdown: newMessageMarkdownRenderer(func(body, _ string, _ int) (string, error) { return body, nil }),
	}
	m.View()
	firstCache := m.markdown.groupCache
	if firstCache == nil {
		t.Fatal("initial view did not cache the rendered conversation")
	}

	m.editor.SetValue("edited draft")
	m.View()
	if m.markdown.groupCache != firstCache {
		t.Fatal("reply-only edit rebuilt the unchanged conversation panel")
	}

	m.groups[0].messages[0].Body = "Changed message body"
	view := m.View().Content
	if m.markdown.groupCache == firstCache || !strings.Contains(view, "Changed message body") {
		t.Fatalf("message change reused stale conversation panel: %q", view)
	}
}

func BenchmarkReplyEditorViewLongConversation(b *testing.B) {
	created := time.Date(2026, 8, 23, 12, 0, 0, 0, time.Local)
	messages := make([]model.Message, 0, 120)
	for index := range 120 {
		item := message(fmt.Sprintf("benchmark-message-%03d", index), testAgentID, model.HumanMailboxID, strings.Repeat("A representative message body with enough text to wrap. ", 12))
		item.CreatedAt = created.Add(time.Duration(index) * time.Second)
		item.Correlation = model.MessageCorrelation{Provider: "codex", SessionID: "benchmark-thread", OperationID: fmt.Sprintf("turn-%03d", index)}
		messages = append(messages, item)
	}
	editor := textarea.New()
	editor.SetValue("A reply in progress")
	editor.Focus()
	m := app{
		messages: messages, groups: groupMessages(messages), answering: true,
		answerID: messages[len(messages)-1].ID, answerGroupKey: messageGroupKey(messages[0]), answerQ: messages[len(messages)-1],
		editor: editor, paneFocus: focusReply, width: 120, height: 40,
		markdown: newMessageMarkdownRenderer(func(body, _ string, _ int) (string, error) { return body, nil }),
	}
	m.resizeEditor()
	m.View()
	b.ReportAllocs()
	b.ResetTimer()
	for range b.N {
		m.View()
	}
}

func TestTurnMessagesCoalesceAndRefreshDuringDraft(t *testing.T) {
	created := time.Date(2026, 8, 21, 15, 4, 5, 0, time.Local)
	question := message("question", testAgentID, model.HumanMailboxID, "Which approach?")
	question.CreatedAt = created
	setMessageSemantics(&question, "Harness provider: codex\nHarness session: thread-1\nHarness operation: turn-1\nHarness request: request-1")
	update := message("update", testAgentID, model.HumanMailboxID, "First update")
	update.CreatedAt = created.Add(time.Second)
	setMessageSemantics(&update, "Kind: update\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
	final := message("final", testAgentID, model.HumanMailboxID, "Finished")
	final.CreatedAt = created.Add(2 * time.Second)
	setMessageSemantics(&final, "Kind: final-answer\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")

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
	setMessageSemantics(&late, "Kind: update\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
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
	if layout.inboxHeight != 4 || layout.messageWidth != 120 || layout.replyWidth != 120 || layout.messageHeight != 8 || layout.replyHeight != 7 {
		t.Fatalf("vertical layout = %#v", layout)
	}
	for _, test := range []struct {
		height      int
		replyHeight int
	}{{20, 7}, {60, 10}, {80, 13}, {100, 16}} {
		got := responsivePaneLayout(160, test.height, true)
		if got.replyHeight != test.replyHeight {
			t.Fatalf("%d-row reply height = %d; want %d", test.height, got.replyHeight, test.replyHeight)
		}
	}
	item := message("message", testAgentID, model.HumanMailboxID, "Body")
	setMessageSemantics(&item, "Kind: update\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
	editor := textarea.New()
	editor.Focus()
	m := app{
		messages: []model.Message{item}, answering: true, answerID: item.ID,
		answerGroupKey: messageGroupKey(item), answerQ: item, editor: editor, width: 120, height: 30,
	}
	view := m.View().Content
	lines := strings.Split(view, "\n")
	viewLayout := responsivePaneLayout(m.width, m.height, true)
	if !strings.Contains(lines[viewLayout.inboxHeight+viewLayout.messageHeight], "[Replying to ") {
		t.Fatalf("reply pane was not rendered below message pane: %q", view)
	}
	for _, line := range lines {
		if strings.Count(line, "╭") > 1 {
			t.Fatalf("panes rendered side by side: %q", line)
		}
	}
}

func TestInboxAndMessagePanesShowScrollbarsOnlyWhenScrollable(t *testing.T) {
	messages := make([]model.Message, 0, 12)
	for index := range 12 {
		item := message(fmt.Sprintf("scroll-row-%02d", index), testAgentID, model.HumanMailboxID, fmt.Sprintf("message %02d", index))
		item.ThreadID = item.ID
		item.CreatedAt = item.CreatedAt.Add(time.Duration(index) * time.Second)
		messages = append(messages, item)
	}
	m := app{messages: messages, width: 80, height: 40, paneFocus: focusInbox}
	inbox := m.renderInboxPane(m.width, responsivePaneLayout(m.width, m.height, false).inboxHeight)
	if !strings.Contains(inbox, "█") || !strings.Contains(inbox, "░") {
		t.Fatalf("scrollable inbox omitted scrollbar: %q", inbox)
	}

	long := message("long-scroll-message", testAgentID, model.HumanMailboxID, strings.Repeat("message line\n", 30))
	m = app{messages: []model.Message{long}, width: 80, height: 24, paneFocus: focusMessage}
	view := m.View().Content
	layout := responsivePaneLayout(m.width, m.height, false)
	messagePane := strings.Join(strings.Split(view, "\n")[layout.inboxHeight:layout.inboxHeight+layout.messageHeight], "\n")
	if !strings.Contains(messagePane, "█") || !strings.Contains(messagePane, "░") {
		t.Fatalf("scrollable message pane omitted scrollbar: %q", messagePane)
	}

	short := message("short-message", testAgentID, model.HumanMailboxID, "short")
	m = app{messages: []model.Message{short}, width: 80, height: 24}
	if strings.Contains(m.View().Content, "█") || strings.Contains(m.View().Content, "░") {
		t.Fatalf("non-scrollable panes showed a scrollbar: %q", m.View().Content)
	}
}

func TestResponsiveViewFitsTerminalWithVerticalPanes(t *testing.T) {
	item := message("message", testAgentID, model.HumanMailboxID, "Body")
	setMessageSemantics(&item, "Kind: update\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
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
		if strings.Count(lines[layout.inboxHeight], "╭") != 1 || !strings.Contains(lines[layout.inboxHeight+layout.messageHeight], "[Replying to ") {
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
	topView := m.View().Content
	topLines := strings.Split(topView, "\n")[responsivePaneLayout(m.width, m.height, false).inboxHeight:]
	if top := strings.Join(topLines, "\n"); !strings.Contains(top, "line-01") || strings.Contains(top, "line-20") {
		t.Fatalf("message pane did not anchor at oldest open content: %q", top)
	}
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyPgDown})
	m = updated.(app)
	lowerView := m.View().Content
	lowerLines := strings.Split(lowerView, "\n")[responsivePaneLayout(m.width, m.height, false).inboxHeight:]
	if lower := strings.Join(lowerLines, "\n"); !strings.Contains(lower, "line-08") || strings.Contains(lower, "line-01") {
		t.Fatalf("page-down did not scroll within message fixture: %q", lower)
	}
	if lipgloss.Height(lowerView) != m.height {
		t.Fatalf("scrolled view height = %d; want %d", lipgloss.Height(lowerView), m.height)
	}
}

func TestMessagePaneOppositeDirectionMovesImmediatelyAfterTopBoundary(t *testing.T) {
	var body strings.Builder
	for index := 1; index <= 30; index++ {
		fmt.Fprintf(&body, "line-%02d\n", index)
	}
	item := message("scroll-boundary", testAgentID, model.HumanMailboxID, strings.TrimSpace(body.String()))
	m := app{messages: []model.Message{item}, width: 80, height: 24, paneFocus: focusMessage}
	for range 8 {
		updated, _ := m.Update(tea.KeyPressMsg{Code: 'k', Text: "k"})
		m = updated.(app)
	}
	if m.messageScroll != 0 {
		t.Fatalf("top-boundary keys accumulated scroll state: %#v", m)
	}
	before := m.View().Content
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'j', Text: "j"})
	m = updated.(app)
	after := m.View().Content
	if m.messageScroll != 1 || !m.messageScrollManual || before == after {
		t.Fatalf("first j after top boundary did not move: scroll=%d manual=%t", m.messageScroll, m.messageScrollManual)
	}
}

func TestMessagePaneKeepsOldestOpenActionVisibleAfterArchivedHistory(t *testing.T) {
	archived := message("archived-history", testAgentID, model.HumanMailboxID, strings.Repeat("archived ancestor\n", 20))
	setMessageSemantics(&archived, "Harness provider: codex\nHarness session: anchor-thread\nHarness operation: old-turn")
	archivedAt := time.Now().UTC()
	archived.ArchivedAt = &archivedAt
	open := message("open-history", testAgentID, model.HumanMailboxID, "oldest open action")
	open.CreatedAt = archived.CreatedAt.Add(time.Second)
	setMessageSemantics(&open, "Harness provider: codex\nHarness session: anchor-thread\nHarness operation: open-turn")
	m := app{messages: []model.Message{archived, open}, width: 80, height: 24}
	view := m.View().Content
	messageView := strings.Join(strings.Split(view, "\n")[responsivePaneLayout(m.width, m.height, false).inboxHeight:], "\n")
	if !strings.Contains(messageView, "oldest open action") {
		t.Fatalf("message pane did not keep open action visible: %q", messageView)
	}
}

func TestMessagePaneAnchorsNewReplyAfterArchivedMessagesInSameTurn(t *testing.T) {
	created := time.Date(2026, 8, 23, 15, 4, 5, 0, time.Local)
	read := message("read-reply", testAgentID, model.HumanMailboxID, strings.Repeat("already read\n", 18))
	read.CreatedAt = created
	setMessageSemantics(&read, "Harness provider: codex\nHarness session: anchor-thread\nHarness operation: shared-turn")
	archivedAt := created.Add(time.Second)
	read.ArchivedAt = &archivedAt

	human := message("human-response", model.HumanMailboxID, testAgentID, "My response")
	human.CreatedAt = created.Add(2 * time.Second)
	setMessageSemantics(&human, "Harness provider: codex\nHarness session: anchor-thread\nHarness operation: shared-turn")

	unread := message("unread-reply", testAgentID, model.HumanMailboxID, "New reply")
	unread.CreatedAt = created.Add(3 * time.Second)
	setMessageSemantics(&unread, "Harness provider: codex\nHarness session: anchor-thread\nHarness operation: shared-turn")

	m := app{messages: []model.Message{read, human, unread}, width: 80, height: 24}
	m.groups = groupMessages(m.messages)
	group, found := m.detailGroup()
	if !found {
		t.Fatal("message group not found")
	}
	layout := responsivePaneLayout(m.width, m.height, false)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	want := messagePaneMaxStart(rendered.panel, layout.messageHeight)
	for _, span := range rendered.spans {
		if span.messageID == unread.ID {
			want = min(want, span.start)
			break
		}
	}
	if got := automaticMessageStart(group, rendered, layout.messageHeight, ""); got != want || got == 0 {
		t.Fatalf("automatic start = %d; want unread reply start %d", got, want)
	}
	messageView := strings.Join(strings.Split(m.View().Content, "\n")[layout.inboxHeight:], "\n")
	if !strings.Contains(messageView, "New reply") || strings.Count(messageView, "already read") >= 18 {
		t.Fatalf("message pane did not prioritize the new reply: %q", messageView)
	}
}

func TestMessagePaneClearsStaleAnchorWhenOnlyConversationReappears(t *testing.T) {
	created := time.Date(2026, 8, 23, 18, 4, 5, 0, time.Local)
	previous := message("previous-reply", testAgentID, model.HumanMailboxID, strings.Repeat("previous reply\n", 20))
	previous.CreatedAt = created
	setMessageSemantics(&previous, "Harness provider: codex\nHarness session: reappearing-thread\nHarness operation: previous-turn")
	archivedAt := created.Add(time.Second)
	previous.ArchivedAt = &archivedAt
	human := message("human-reply", model.HumanMailboxID, testAgentID, "My reply")
	human.CreatedAt = created.Add(2 * time.Second)
	setMessageSemantics(&human, "Harness provider: codex\nHarness session: reappearing-thread\nHarness operation: previous-turn")
	latest := message("latest-reply", testAgentID, model.HumanMailboxID, strings.Repeat("latest reply\n", 12))
	latest.CreatedAt = created.Add(3 * time.Second)
	setMessageSemantics(&latest, "Harness provider: codex\nHarness session: reappearing-thread\nHarness operation: latest-turn")
	key := conversationKeyForMessage(latest)
	stableKey := conversationKeyString(key)

	m := app{
		conversationMode: true, histories: map[string][]model.Message{}, width: 80, height: 24,
		messageViewportKey: stableKey, messageScrollManual: true, messageAnchorID: previous.ID,
	}
	m.setMessages()
	updated, _ := m.Update(loadedMsg{
		conversations: []model.ConversationSummary{{Key: key, Latest: latest, OldestOpen: &latest}},
		histories:     map[string][]model.Message{stableKey: {previous, human, latest}},
	})
	m = updated.(app)
	group, found := m.detailGroup()
	if !found {
		t.Fatal("automatically selected conversation not found")
	}
	layout := responsivePaneLayout(m.width, m.height, false)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	want := messagePaneMaxStart(rendered.panel, layout.messageHeight)
	for _, span := range rendered.spans {
		if span.messageID == latest.ID {
			want = min(want, span.start)
			break
		}
	}
	if m.messageScrollManual || m.messageLiveAnchorID != latest.ID || m.messageScroll != want {
		t.Fatalf("reappearing conversation retained stale anchor: scroll=%d want=%d manual=%t live=%q", m.messageScroll, want, m.messageScrollManual, m.messageLiveAnchorID)
	}
}

func TestMessagePaneAdvancesToNewLiveMessagePastEarlierOpenMessages(t *testing.T) {
	created := time.Date(2026, 8, 23, 16, 4, 5, 0, time.Local)
	previous := message("previous-open", testAgentID, model.HumanMailboxID, strings.Repeat("previous update\n", 18))
	previous.CreatedAt = created
	setMessageSemantics(&previous, "Harness provider: codex\nHarness session: live-thread\nHarness operation: shared-turn")
	key := conversationKeyForMessage(previous)
	stableKey := conversationKeyString(key)
	m := app{
		conversations:    []model.ConversationSummary{{Key: key, Latest: previous, OldestOpen: &previous}},
		conversationMode: true,
		histories:        map[string][]model.Message{stableKey: {previous}},
		width:            80,
		height:           24,
	}
	m.setMessages()
	m.reconcileMessageViewport(false)
	m.scrollMessagePane(3)
	initial := m.messageScroll
	if !m.messageScrollManual {
		t.Fatal("fixture did not establish a manual scroll anchor")
	}

	latest := message("latest-open", testAgentID, model.HumanMailboxID, "Latest update")
	latest.CreatedAt = created.Add(time.Second)
	setMessageSemantics(&latest, "Harness provider: codex\nHarness session: live-thread\nHarness operation: shared-turn")
	updated, _ := m.Update(loadedMsg{
		conversations: []model.ConversationSummary{{Key: key, Latest: latest, OldestOpen: &previous}},
		histories:     map[string][]model.Message{stableKey: {previous, latest}},
	})
	m = updated.(app)
	if m.messageLiveAnchorID != latest.ID || m.messageScroll <= initial || m.messageScrollManual {
		t.Fatalf("live anchor = %q at %d (manual=%t); want %q after %d", m.messageLiveAnchorID, m.messageScroll, m.messageScrollManual, latest.ID, initial)
	}
	layout := responsivePaneLayout(m.width, m.height, false)
	messageView := strings.Join(strings.Split(m.View().Content, "\n")[layout.inboxHeight:], "\n")
	if !strings.Contains(messageView, "Latest update") || strings.Count(messageView, "previous update") >= 18 {
		t.Fatalf("message pane did not advance to latest update: %q", messageView)
	}
}

func TestMessagePaneRestoresUnreadBoundaryAfterRestartAndMultipleReplies(t *testing.T) {
	created := time.Date(2026, 8, 23, 17, 4, 5, 0, time.Local)
	makeTurnMessage := func(id, body string, offset time.Duration, sender, recipient string) model.Message {
		item := message(id, sender, recipient, body)
		item.CreatedAt = created.Add(offset)
		setMessageSemantics(&item, "Harness provider: codex\nHarness session: restart-thread")
		return item
	}
	first := makeTurnMessage("first-agent", strings.Repeat("first response\n", 10), 0, testAgentID, model.HumanMailboxID)
	firstReply := makeTurnMessage("first-human", "First human reply", time.Second, model.HumanMailboxID, testAgentID)
	second := makeTurnMessage("second-agent", strings.Repeat("second response\n", 10), 2*time.Second, testAgentID, model.HumanMailboxID)
	secondReply := makeTurnMessage("second-human", "Second human reply", 3*time.Second, model.HumanMailboxID, testAgentID)
	latest := makeTurnMessage("latest-agent", "Latest response after restart", 4*time.Second, testAgentID, model.HumanMailboxID)

	m := app{messages: []model.Message{first, firstReply, second, secondReply, latest}, width: 80, height: 24}
	m.groups = groupMessages(m.messages)
	group, found := m.detailGroup()
	if !found {
		t.Fatal("message group not found")
	}
	layout := responsivePaneLayout(m.width, m.height, false)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	want := messagePaneMaxStart(rendered.panel, layout.messageHeight)
	for _, span := range rendered.spans {
		if span.messageID == latest.ID {
			want = min(want, span.start)
			break
		}
	}
	if got := automaticMessageStart(group, rendered, layout.messageHeight, ""); got != want {
		t.Fatalf("restart start = %d; want latest post-reply boundary %d", got, want)
	}
	messageView := strings.Join(strings.Split(m.View().Content, "\n")[layout.inboxHeight:], "\n")
	if !strings.Contains(messageView, "Latest response after restart") || strings.Contains(messageView, "first response") {
		t.Fatalf("restarted pane returned to already-read content: %q", messageView)
	}
}

func TestMessagePaneUsesNewestContentWhenConversationHasNoOpenWork(t *testing.T) {
	first := message("archived-first", testAgentID, model.HumanMailboxID, strings.Repeat("old history\n", 20))
	second := message("archived-latest", testAgentID, model.HumanMailboxID, "newest archived content")
	second.CreatedAt = first.CreatedAt.Add(time.Second)
	archivedAt := time.Now().UTC()
	first.ArchivedAt, second.ArchivedAt = &archivedAt, &archivedAt
	m := app{messages: []model.Message{first, second}, width: 80, height: 24}
	view := m.View().Content
	messageView := strings.Join(strings.Split(view, "\n")[responsivePaneLayout(m.width, m.height, false).inboxHeight:], "\n")
	if !strings.Contains(messageView, "newest archived content") || strings.Contains(messageView, "old history") {
		t.Fatalf("archived-only conversation did not open at newest content: %q", messageView)
	}
}

func TestManualMessageAnchorSurvivesEarlierLiveHistory(t *testing.T) {
	current := message("current-message", testAgentID, model.HumanMailboxID, strings.Repeat("current line\n", 24))
	setMessageSemantics(&current, "Harness provider: codex\nHarness session: live-anchor\nHarness operation: current-turn")
	conversationKey := conversationKeyForMessage(current)
	stableKey := conversationKeyString(conversationKey)
	m := app{
		conversations: []model.ConversationSummary{{Key: conversationKey, Latest: current}}, conversationMode: true,
		histories: map[string][]model.Message{stableKey: {current}}, width: 80, height: 24, paneFocus: focusMessage,
	}
	m.setMessages()
	m.reconcileMessageViewport(false)
	m.scrollMessagePane(5)
	anchorID, anchorOffset := m.messageAnchorID, m.messageAnchorOffset
	earlier := message("earlier-message", testAgentID, model.HumanMailboxID, strings.Repeat("earlier line\n", 12))
	earlier.CreatedAt = current.CreatedAt.Add(-time.Minute)
	setMessageSemantics(&earlier, "Harness provider: codex\nHarness session: live-anchor\nHarness operation: earlier-turn")
	updated, _ := m.Update(historyLoadedMsg{key: stableKey, messages: []model.Message{earlier, current}})
	m = updated.(app)
	if !m.messageScrollManual || m.messageAnchorID != anchorID || m.messageAnchorOffset != anchorOffset || m.messageScroll <= 5 {
		t.Fatalf("manual anchor was not preserved across earlier history: scroll=%d anchor=%q+%d", m.messageScroll, m.messageAnchorID, m.messageAnchorOffset)
	}
}

func TestManualMessageAnchorSurvivesResizeReflow(t *testing.T) {
	item := message("resize-anchor", testAgentID, model.HumanMailboxID, strings.Repeat("several words that wrap differently ", 40))
	m := app{messages: []model.Message{item}, groups: groupMessages([]model.Message{item}), markdown: newMessageMarkdownRenderer(nil), editor: textarea.New(), width: 48, height: 24, paneFocus: focusMessage}
	m.reconcileMessageViewport(false)
	m.scrollMessagePane(6)
	anchorID, anchorOffset := m.messageAnchorID, m.messageAnchorOffset
	updated, _ := m.Update(tea.WindowSizeMsg{Width: 100, Height: 28})
	m = updated.(app)
	group, _ := m.detailGroup()
	rendered := m.renderGroupPanelLayout(group, responsivePaneLayout(m.width, m.height, m.answering).messageWidth)
	layout := responsivePaneLayout(m.width, m.height, m.answering)
	maximum := messagePaneMaxStart(rendered.panel, layout.messageHeight)
	expectedScroll := maximum
	for _, span := range rendered.spans {
		if span.messageID == anchorID {
			expectedScroll = min(maximum, span.start+anchorOffset)
			break
		}
	}
	if !m.messageScrollManual || m.messageAnchorID != anchorID || m.messageScroll != expectedScroll || m.messageScroll > maximum {
		t.Fatalf("manual resize anchor = scroll %d, %q+%d; want scroll %d for %q", m.messageScroll, m.messageAnchorID, m.messageAnchorOffset, expectedScroll, anchorID)
	}
}

func TestAutomaticMessageAnchorAdvancesAndRestoresWithOpenState(t *testing.T) {
	first := message("first-open", testAgentID, model.HumanMailboxID, strings.Repeat("first action\n", 12))
	setMessageSemantics(&first, "Harness provider: codex\nHarness session: state-anchor\nHarness operation: first-turn")
	second := message("second-open", testAgentID, model.HumanMailboxID, "second action")
	second.CreatedAt = first.CreatedAt.Add(time.Second)
	setMessageSemantics(&second, "Harness provider: codex\nHarness session: state-anchor\nHarness operation: second-turn")
	m := app{messages: []model.Message{first, second}, width: 80, height: 24}
	m.groups = groupMessages(m.messages)
	m.reconcileMessageViewport(false)
	initial := m.messageScroll
	archivedAt := time.Now().UTC()
	m.messages[0].ArchivedAt = &archivedAt
	m.groups = groupMessages(m.messages)
	m.reconcileMessageViewport(true)
	advanced := m.messageScroll
	if advanced <= initial || m.messageScrollManual {
		t.Fatalf("automatic anchor did not advance: initial=%d advanced=%d", initial, advanced)
	}
	m.messages[0].ArchivedAt = nil
	m.groups = groupMessages(m.messages)
	m.reconcileMessageViewport(true)
	if m.messageScroll != initial {
		t.Fatalf("automatic anchor did not restore: got=%d want=%d", m.messageScroll, initial)
	}
}

func TestRenderedMessageSpansTrackWrappedMessages(t *testing.T) {
	first := message("wrapped-first", testAgentID, model.HumanMailboxID, strings.Repeat("wrapped words ", 20))
	setMessageSemantics(&first, "Harness provider: codex\nHarness session: span-thread\nHarness operation: first-turn")
	second := message("wrapped-second", testAgentID, model.HumanMailboxID, "second")
	second.CreatedAt = first.CreatedAt.Add(time.Second)
	setMessageSemantics(&second, "Harness provider: codex\nHarness session: span-thread\nHarness operation: second-turn")
	m := app{markdown: newMessageMarkdownRenderer(nil)}
	group := groupMessages([]model.Message{first, second})[0]
	narrow := m.renderGroupPanelLayout(group, 40)
	wide := m.renderGroupPanelLayout(group, 100)
	if len(narrow.spans) != 2 || narrow.spans[0].messageID != first.ID || narrow.spans[1].messageID != second.ID || narrow.spans[0].end > narrow.spans[1].start {
		t.Fatalf("message spans = %#v", narrow.spans)
	}
	if narrow.spans[0].end <= wide.spans[0].end {
		t.Fatalf("wrapping did not expand first span: narrow=%#v wide=%#v", narrow.spans[0], wide.spans[0])
	}
}

func TestMessageScrollingRemainsBoundedInSmallTerminal(t *testing.T) {
	item := message("small-scroll", testAgentID, model.HumanMailboxID, strings.Repeat("small terminal line\n", 20))
	m := app{messages: []model.Message{item}, editor: textarea.New(), width: 38, height: 8, paneFocus: focusMessage}
	for range 50 {
		updated, _ := m.Update(tea.KeyPressMsg{Code: 'j', Text: "j"})
		m = updated.(app)
	}
	group, _ := m.detailGroup()
	layout := responsivePaneLayout(m.width, m.height, false)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	maximum := messagePaneMaxStart(rendered.panel, layout.messageHeight)
	if m.messageScroll < 0 || m.messageScroll > maximum || lipgloss.Height(m.View().Content) != m.height {
		t.Fatalf("small-terminal scroll=%d maximum=%d height=%d", m.messageScroll, maximum, lipgloss.Height(m.View().Content))
	}
}

func TestMessagePaneStopsAtLastFullViewport(t *testing.T) {
	var body strings.Builder
	for index := 1; index <= 30; index++ {
		fmt.Fprintf(&body, "line-%02d\n", index)
	}
	item := message("bottom-boundary", testAgentID, model.HumanMailboxID, strings.TrimSpace(body.String()))
	m := app{messages: []model.Message{item}, width: 80, height: 24, paneFocus: focusMessage}
	for range 100 {
		updated, _ := m.Update(tea.KeyPressMsg{Code: 'j', Text: "j"})
		m = updated.(app)
	}
	group, _ := m.detailGroup()
	layout := responsivePaneLayout(m.width, m.height, false)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	maximum := messagePaneMaxStart(rendered.panel, layout.messageHeight)
	if m.messageScroll != maximum {
		t.Fatalf("bottom scroll=%d; want last full viewport start %d", m.messageScroll, maximum)
	}
	before := m.View().Content
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'j', Text: "j"})
	m = updated.(app)
	if m.messageScroll != maximum || m.View().Content != before {
		t.Fatalf("scroll advanced past bottom: scroll=%d maximum=%d", m.messageScroll, maximum)
	}
}

func TestFitRenderedPaneFromTopClampsToLastFullViewport(t *testing.T) {
	content := strings.Join([]string{"line-01", "line-02", "line-03", "line-04", "line-05", "line-06"}, "\n")
	rendered := renderMessagePanel(content, 40, "[message]", "", false)
	fitted := fitRenderedPaneFromTop(rendered, 40, 6, 100, false)
	lines := strings.Split(ansi.Strip(fitted), "\n")
	if len(lines) != 6 || !strings.Contains(lines[len(lines)-2], "line-06") || strings.Contains(fitted, "line-01") || strings.Contains(fitted, "line-02") {
		t.Fatalf("last full viewport was not preserved: %q", fitted)
	}
}

func TestTabAndShiftTabCyclePaneFocus(t *testing.T) {
	selected := message("selected", testAgentID, model.HumanMailboxID, "Selected message")
	m := app{messages: []model.Message{selected}, editor: textarea.New(), width: 80, height: 24}
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
	if m.paneFocus != focusMessage || !borderUses(view, "[HQ · Inbox]", "59") || strings.Contains(view, "focused") {
		t.Fatalf("first tab focus = %v", m.paneFocus)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	view = m.View().Content
	if m.paneFocus != focusReply || !m.answering || m.answerID != selected.ID || !borderUses(view, "[Replying to ", "63") || strings.Contains(view, "focused") {
		t.Fatalf("second tab state: focus=%v answering=%v answerID=%q", m.paneFocus, m.answering, m.answerID)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyTab, Mod: tea.ModShift})
	m = updated.(app)
	if m.paneFocus != focusMessage {
		t.Fatalf("shift-tab focus = %v", m.paneFocus)
	}
}

func TestTabIntoReplyWithoutSelectionOpensRecipientPicker(t *testing.T) {
	m := app{editor: textarea.New(), paneFocus: focusMessage, width: 80, height: 24}

	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	if m.paneFocus != focusReply || !m.pickingRecipient || m.answering {
		t.Fatalf("tab into reply without selection = %#v", m)
	}
	if view := m.View().Content; !strings.Contains(view, "[project · choose project work or direct recipient]") {
		t.Fatalf("recipient picker was not rendered: %q", view)
	}
}

func TestReplyHintsDescribeContextualComposeKeys(t *testing.T) {
	withoutSelection := (app{editor: textarea.New(), width: 100, height: 24}).View().Content
	if !strings.Contains(withoutSelection, "Press Tab or n to choose a recipient for a new message.") || strings.Contains(withoutSelection, "Enter to reply") {
		t.Fatalf("new-message hint is not contextual: %q", withoutSelection)
	}

	selected := message("selected", testAgentID, model.HumanMailboxID, "Selected message")
	withSelection := (app{messages: []model.Message{selected}, editor: textarea.New(), width: 100, height: 24}).View().Content
	if !strings.Contains(withSelection, "Press Tab or Enter to reply to the selected turn, or n for a new message.") {
		t.Fatalf("reply hint is not contextual: %q", withSelection)
	}
}

func TestComposePanePutsActionAndStyledAgentNameInBorder(t *testing.T) {
	newMessage := app{
		answering: true, composeTo: "alice-id", composeName: "alice", editor: textarea.New(),
		paneFocus: focusReply,
	}
	view := newMessage.renderReplyPane(80)
	if !strings.Contains(ansi.Strip(view), "[New message to alice]") || !strings.Contains(view, titleStyle.Render("alice")) {
		t.Fatalf("new-message border title = %q", view)
	}
	if strings.Count(ansi.Strip(view), "New message to alice") != 1 {
		t.Fatalf("new-message title was duplicated in the pane body: %q", view)
	}

	reply := app{
		answering: true,
		answerQ:   message("selected", "alice-id", model.HumanMailboxID, "Selected message"),
		agents:    []domain.NamedAgent{{Name: "alice", MailboxID: "alice-id"}},
		editor:    textarea.New(), paneFocus: focusReply,
	}
	view = reply.renderReplyPane(80)
	if !strings.Contains(ansi.Strip(view), "[Replying to alice]") || !strings.Contains(view, titleStyle.Render("alice")) {
		t.Fatalf("reply border title = %q", view)
	}
	if strings.Count(ansi.Strip(view), "Replying to alice") != 1 || strings.Contains(view, "Reply to this turn") {
		t.Fatalf("reply title was duplicated in the pane body: %q", view)
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

func TestLeavingEmptyComposerDoesNotCreateDraft(t *testing.T) {
	question := message("question", testAgentID, model.HumanMailboxID, "Question")
	for name, m := range map[string]app{
		"reply": {
			messages: []model.Message{question}, answering: true, answerID: question.ID,
			answerGroupKey: messageGroupKey(question), answerQ: question, activeDraftKey: messageGroupKey(question),
			editor: textarea.New(), paneFocus: focusReply,
		},
		"new message": {
			answering: true, activeDraftKey: "draft:new", composeTo: "alice-id", composeName: "alice",
			editor: textarea.New(), paneFocus: focusReply,
		},
	} {
		t.Run(name, func(t *testing.T) {
			m.editor.SetValue(" \n\t")
			updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
			m = updated.(app)
			if m.answering || m.paneFocus != focusInbox || len(m.drafts) != 0 {
				t.Fatalf("empty composer was retained as a draft: %#v", m)
			}
		})
	}
}

func TestEmptyingExistingDraftDeletesItWhenLeavingComposer(t *testing.T) {
	draft := messageDraft{key: "draft:new", body: "saved", composeTo: "alice-id", composeName: "alice"}
	m := app{
		drafts: map[string]messageDraft{draft.key: draft}, editor: textarea.New(),
		paneFocus: focusInbox, width: 80, height: 24,
	}
	m.resumeDraft(draft)
	m.editor.SetValue("")

	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	if m.answering || len(m.drafts) != 0 || len(m.visibleGroups()) != 0 || m.cursor != 0 {
		t.Fatalf("emptied saved draft was not deleted: %#v", m)
	}
}

func TestPageKeysApplyToFocusedPane(t *testing.T) {
	messages := make([]model.Message, 0, 10)
	for i := range 10 {
		item := message(fmt.Sprintf("message-%02d", i), testAgentID, model.HumanMailboxID, strings.Repeat(fmt.Sprintf("Body %02d\n", i), 30))
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
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyPgDown})
	m = updated.(app)
	if m.messageScroll == 0 {
		t.Fatal("message page-down did not change message scroll")
	}
	previousScroll := m.messageScroll
	m.paneFocus = focusReply
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyPgUp})
	m = updated.(app)
	if m.messageScroll != previousScroll {
		t.Fatalf("reply page-up changed message scroll from %d to %d", previousScroll, m.messageScroll)
	}
}

func TestControlDUPageTheFocusedPane(t *testing.T) {
	messages := make([]model.Message, 0, 10)
	for index := range 10 {
		item := message(fmt.Sprintf("control-page-%02d", index), testAgentID, model.HumanMailboxID, strings.Repeat(fmt.Sprintf("Body %02d\n", index), 30))
		item.CreatedAt = time.Unix(int64(100-index), 0)
		messages = append(messages, item)
	}
	controlDown := tea.KeyPressMsg{Code: 'd', Mod: tea.ModCtrl}
	controlUp := tea.KeyPressMsg{Code: 'u', Mod: tea.ModCtrl}
	m := app{messages: messages, width: 80, height: 24, paneFocus: focusInbox}

	updated, _ := m.Update(controlDown)
	m = updated.(app)
	if m.cursor == 0 {
		t.Fatal("ctrl+d did not page the inbox down")
	}
	updated, _ = m.Update(controlUp)
	m = updated.(app)
	if m.cursor != 0 {
		t.Fatalf("ctrl+u did not page the inbox up: cursor=%d", m.cursor)
	}

	m.paneFocus = focusMessage
	m.reconcileMessageViewport(false)
	messageStart := m.messageScroll
	updated, _ = m.Update(controlDown)
	m = updated.(app)
	if m.messageScroll <= messageStart {
		t.Fatal("ctrl+d did not page the message pane down")
	}
	updated, _ = m.Update(controlUp)
	m = updated.(app)
	if m.messageScroll != messageStart {
		t.Fatalf("ctrl+u did not page the message pane up: scroll=%d want=%d", m.messageScroll, messageStart)
	}

	editor := textarea.New()
	editor.SetHeight(4)
	editor.SetValue(strings.Repeat("reply line\n", 20))
	editor.MoveToBegin()
	editor.Focus()
	m = app{answering: true, editor: editor, paneFocus: focusReply, width: 80, height: 24}
	updated, _ = m.Update(controlDown)
	m = updated.(app)
	if m.editor.Line() == 0 {
		t.Fatal("ctrl+d did not page the reply pane down")
	}
	updated, _ = m.Update(controlUp)
	m = updated.(app)
	if m.editor.Line() != 0 {
		t.Fatalf("ctrl+u did not page the reply pane up: line=%d", m.editor.Line())
	}
}

func TestReplyAndNewMessageUseMailboxID(t *testing.T) {
	s, ctx, agent := openStore(t)
	inbound := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d61", agent.ID, model.HumanMailboxID, "Question")
	inbound.SenderLabel = agent.Label
	setMessageSemantics(&inbound, "Harness provider: codex\nHarness session: thread-1\nHarness operation: turn-1\nHarness request: request-1")
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
	if len(replies) != 1 || replies[0].RecipientMailboxID != agent.ID || replies[0].Body != "Answer" || replies[0].Correlation != (model.MessageCorrelation{Provider: "codex", SessionID: "thread-1", OperationID: "turn-1", RequestID: "request-1"}) || replies[0].Details != "" {
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
	runtimeStore := &runtimeTestStore{testDomainStore: base, runtime: domain.HarnessRuntime{AgentName: "fred", Phase: domain.HarnessRuntimeOffline}}
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
	updated, _ = m.Update(tea.KeyPressMsg{Code: 'y', Text: "y"})
	m = updated.(app)
	if !m.agentManager.yolo || !strings.Contains(m.renderAgentManager(), "Codex YOLO: ON") {
		t.Fatalf("YOLO switch = %#v", m.agentManager)
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
	if len(runtimeStore.launches) != 1 || runtimeStore.launches[0].Harness != "codex" || runtimeStore.launches[0].SessionID != "thread-one" || runtimeStore.launches[0].Directory != directoryOne || strings.Join(runtimeStore.launches[0].Environment, "|") != "PATH=/tui/bin|TOKEN=transient" || !providerYolo(runtimeStore.launches[0].ProviderOptions) {
		t.Fatalf("resume request = %#v", runtimeStore.launches)
	}
	if m.cursor != 3 || m.messageScroll != 2 || m.editor.Value() != "preserved draft" {
		t.Fatalf("agent manager changed inbox state: %#v", m)
	}

	m.agentManager.stage, m.agentManager.cursor = chooseRuntimeSession, 0
	m.agentManager.runtime = domain.HarnessRuntime{AgentName: "fred", Harness: "codex", SessionID: "thread-one", Directory: directoryOne, Phase: domain.HarnessRuntimeRunning}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.agentManager.stage != enterRuntimeHarness || m.agentManager.harness != "codex" {
		t.Fatalf("new session harness stage = %#v", m.agentManager)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.agentManager.stage != enterRuntimeDirectory {
		t.Fatalf("new session directory stage = %v", m.agentManager.stage)
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
	if len(runtimeStore.launches) != 2 || !runtimeStore.launches[1].ConfirmSwitch || runtimeStore.launches[1].Action != domain.HarnessSessionNew || runtimeStore.launches[1].Directory != directoryTwo || !providerYolo(runtimeStore.launches[1].ProviderOptions) {
		t.Fatalf("new-thread request = %#v", runtimeStore.launches)
	}
}

func TestAgentManagerUsesConfiguredYoloDefault(t *testing.T) {
	m := app{editor: textarea.New(), defaultYolo: true}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'g', Text: "g"})
	m = updated.(app)
	if !m.managingAgents || !m.agentManager.yolo {
		t.Fatalf("agent manager did not use configured YOLO default: %#v", m.agentManager)
	}
}

func TestAgentManagerRenamesThreadWithoutSelectingOrLaunchingIt(t *testing.T) {
	base, ctx, mailbox := openStore(t)
	directory := t.TempDir()
	if _, err := base.CreateNamedAgent(ctx, "fred", mailbox.ID); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"thread-one", "thread-two"} {
		if _, err := base.SelectNamedAgentSession(ctx, "fred", model.SessionIdentity{Harness: "codex", ExternalSessionID: id}, model.RepositoryContext{Directory: directory + "/" + id}); err != nil {
			t.Fatal(err)
		}
	}
	agent, _ := base.GetNamedAgent(ctx, "fred")
	sessions, _ := base.ListNamedAgentSessions(ctx, "fred")
	m := app{ctx: ctx, store: &runtimeTestStore{testDomainStore: base}, managingAgents: true, editor: textarea.New(), agentManager: agentManager{stage: chooseRuntimeSession, agent: agent, sessions: sessions}}
	for index, session := range sessions {
		if session.SessionID == "thread-one" {
			m.agentManager.cursor = index + 1
		}
	}
	updated, cmd := m.Update(tea.KeyPressMsg{Code: 'r', Text: "r"})
	m = updated.(app)
	if cmd != nil || m.agentManager.stage != enterThreadName {
		t.Fatalf("rename stage = %v, cmd=%v", m.agentManager.stage, cmd)
	}
	for _, character := range "Build auth" {
		updated, _ = m.Update(tea.KeyPressMsg{Code: character, Text: string(character)})
		m = updated.(app)
	}
	updated, cmd = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if cmd == nil {
		t.Fatal("rename did not run asynchronously")
	}
	updated, _ = m.Update(cmd())
	m = updated.(app)
	renamed, err := base.ListNamedAgentSessions(ctx, "fred")
	if err != nil {
		t.Fatal(err)
	}
	var found domain.AgentSession
	for _, session := range renamed {
		if session.SessionID == "thread-one" {
			found = session
		}
	}
	if found.ThreadName != "Build auth" || found.Current || agent.CurrentSessionID != "thread-two" || len(m.store.(*runtimeTestStore).launches) != 0 {
		t.Fatalf("rename result = %#v, agent=%#v", found, agent)
	}
	view := m.renderAgentManager()
	if !strings.Contains(view, "Build auth") || !strings.Contains(view, "thread-one") || !strings.Contains(view, filepath.Base(directory)) {
		t.Fatalf("renamed session details missing: %q", view)
	}
}

func TestThreadNameAnnotatesTypedCorrelationDetails(t *testing.T) {
	m := app{threadSessions: map[string]domain.AgentSession{"codex\x00thread-123": {Harness: "codex", SessionID: "thread-123", ThreadName: "Fix login", Context: model.RepositoryContext{Directory: "/repo"}}}}
	details := m.technicalIdentifiers(model.Message{Correlation: model.MessageCorrelation{Provider: "codex", SessionID: "thread-123"}})
	if !strings.Contains(details, "provider: codex\nsession ID: Fix login (thread-123)") {
		t.Fatalf("typed correlation details = %q", details)
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

func TestClosedProjectComposeGuidesAgentThreadAndDirectory(t *testing.T) {
	s, ctx, _ := openStore(t)
	if _, err := s.CreateNamedAgent(ctx, "alice", ""); err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	project, err := s.CreateProject(ctx, domain.CreateProjectRequest{Name: "guided", Paths: []domain.ProjectPathInput{{DisplayPath: directory}}})
	if err != nil {
		t.Fatal(err)
	}
	agent, err := s.GetNamedAgent(ctx, "alice")
	if err != nil {
		t.Fatal(err)
	}
	m := app{ctx: ctx, store: s, projects: []domain.Project{project}, agents: []domain.NamedAgent{agent}, editor: textarea.New(), launchDirectory: directory}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.projectSetup == nil || m.projectSetup.stage != chooseProjectAgent {
		t.Fatalf("project setup = %#v", m.projectSetup)
	}
	updated, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if cmd == nil {
		t.Fatal("agent selection did not load project threads")
	}
	updated, _ = m.Update(cmd())
	m = updated.(app)
	if m.projectSetup == nil || m.projectSetup.stage != enterProjectHarness || m.projectSetup.harness != "codex" {
		t.Fatalf("harness setup = %#v", m.projectSetup)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.projectSetup.stage != chooseProjectThread {
		t.Fatalf("session setup = %#v", m.projectSetup)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.projectSetup.stage != enterProjectDirectory || m.projectSetup.directory != directory {
		t.Fatalf("directory setup = %#v", m.projectSetup)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if !m.answering || m.composeActivation == nil || m.composeActivation.projectID != project.ID || m.composeActivation.agentName != "alice" || m.composeActivation.harness != "codex" || m.composeActivation.action != domain.HarnessSessionNew {
		t.Fatalf("guided compose = %#v", m.composeActivation)
	}
}

func TestNewProjectComposeCollectsHomeBriefAndResources(t *testing.T) {
	s, ctx, _ := openStore(t)
	if _, err := s.CreateNamedAgent(ctx, "alice", ""); err != nil {
		t.Fatal(err)
	}
	agent, _ := s.GetNamedAgent(ctx, "alice")
	account, _ := s.HumanAccount(ctx)
	devices, _ := s.HumanDevices(ctx)
	directory := t.TempDir()
	t.Setenv("HQ_PROJECT_PATH", directory)
	m := app{ctx: ctx, store: s, agents: []domain.NamedAgent{agent}, account: account, devices: devices, editor: textarea.New(), launchDirectory: directory}
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'n', Text: "n"})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.projectSetup == nil || m.projectSetup.stage != enterProjectName {
		t.Fatalf("new project setup = %#v", m.projectSetup)
	}
	for _, r := range "new work" {
		updated, _ = m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
		m = updated.(app)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	for _, r := range "brief" {
		updated, _ = m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
		m = updated.(app)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	for _, r := range "$HQ_PROJECT_PATH" {
		updated, _ = m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
		m = updated.(app)
	}
	updated, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if cmd == nil {
		t.Fatal("project creation command missing")
	}
	updated, _ = m.Update(cmd())
	m = updated.(app)
	if m.projectSetup == nil || m.projectSetup.stage != chooseProjectAgent || m.projectSetup.project.Name != "new work" || m.projectSetup.project.Brief != "brief" || len(m.projectSetup.project.Resources) != 1 || m.projectSetup.project.Resources[0].DisplayLocator != directory {
		t.Fatalf("created setup = %#v", m.projectSetup)
	}
}

func TestProjectPathsExpandAgainstClientEnvironment(t *testing.T) {
	home := t.TempDir()
	base := t.TempDir()
	explicit := filepath.Join(t.TempDir(), "from-env")
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("HQ_PROJECT_PATH", explicit)
	m := app{launchDirectory: base}
	paths, err := m.expandProjectPaths("~/home-project, $HQ_PROJECT_PATH, relative-project")
	if err != nil {
		t.Fatal(err)
	}
	want := []string{filepath.Join(home, "home-project"), explicit, filepath.Join(base, "relative-project")}
	if !slices.Equal(paths, want) {
		t.Fatalf("expanded paths = %#v, want %#v", paths, want)
	}
}

func TestProjectSetupTextStagesShowCursor(t *testing.T) {
	setup := &projectComposeSetup{name: "name", brief: "brief", pathsText: "/path", worktreeRepository: "/repo", worktreeBase: "HEAD", worktreeDestination: "/worktree", worktreeBranch: "branch", query: "agent", directory: "/cwd"}
	m := app{projectSetup: setup, width: 90, height: 30}
	for _, stage := range []projectSetupStage{enterProjectName, enterProjectBrief, enterProjectPaths, enterWorktreeRepository, enterWorktreeBase, enterWorktreeDestination, enterWorktreeBranch, chooseProjectAgent, enterProjectDirectory} {
		setup.stage = stage
		if rendered := m.renderProjectSetup(90, 30); !strings.Contains(rendered, "▏") {
			t.Fatalf("stage %v did not render an input cursor: %q", stage, rendered)
		}
	}
}

func TestNewProjectComposeOffersDaemonWorktreeProvisioning(t *testing.T) {
	repository := filepath.Join(t.TempDir(), "repo")
	destination := filepath.Join(t.TempDir(), "worktree")
	additional := filepath.Join(t.TempDir(), "extra")
	project := domain.Project{ID: "019c0000-0000-7000-8000-000000000391", HomeInstallation: "019c0000-0000-7000-8000-000000000392", MailboxID: "019c0000-0000-7000-8000-000000000393", Name: "worktree", Lifecycle: domain.ProjectClosed}
	store := &worktreeCaptureStore{project: project}
	setup := &projectComposeSetup{stage: enterProjectPaths, name: "worktree", home: project.HomeInstallation, pathsText: additional}
	m := app{ctx: context.Background(), store: store, projectSetup: setup, agents: []domain.NamedAgent{{Name: "alice", Idle: true}}, editor: textarea.New()}
	updated, _ := m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	m = updated.(app)
	if m.projectSetup.stage != enterWorktreeRepository {
		t.Fatalf("worktree stage = %v", m.projectSetup.stage)
	}
	enter := func(value string) {
		for _, r := range value {
			updated, _ = m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
			m = updated.(app)
		}
		updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
		m = updated.(app)
	}
	enter(repository)
	// HEAD is prefilled.
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	enter(destination)
	for _, r := range "feature" {
		updated, _ = m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
		m = updated.(app)
	}
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if m.projectSetup.stage != chooseWorktreePrimary {
		t.Fatalf("primary stage = %v", m.projectSetup.stage)
	}
	updated, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(app)
	if cmd == nil {
		t.Fatal("worktree provisioning command missing")
	}
	updated, _ = m.Update(cmd())
	m = updated.(app)
	if store.request.Repository != repository || store.request.Destination != destination || store.request.Branch != "feature" || !slices.Equal(store.request.AdditionalPaths, []domain.ProjectPathInput{{DisplayPath: additional}}) || m.projectSetup.stage != chooseProjectAgent {
		t.Fatalf("worktree request = %#v setup=%#v", store.request, m.projectSetup)
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
	if len(lines) > 12 || !strings.Contains(view, "project") || !strings.Contains(view, "choose project") {
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
	setMessageSemantics(&first, "Kind: update\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
	final := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d73", agent.ID, model.HumanMailboxID, "Final answer")
	final.CreatedAt = first.CreatedAt.Add(time.Second)
	setMessageSemantics(&final, "Kind: final-answer\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
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

func TestArchiveTargetsOnlyOldestOpenActionUnit(t *testing.T) {
	s, ctx, agent := openStore(t)
	firstUpdate := message("0198c7ec-73b0-7cc3-a5f7-e31c77140e01", agent.ID, model.HumanMailboxID, "first update")
	setMessageSemantics(&firstUpdate, "Harness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
	firstFinal := message("0198c7ec-73b0-7cc3-a5f7-e31c77140e02", agent.ID, model.HumanMailboxID, "first final")
	firstFinal.CreatedAt = firstUpdate.CreatedAt.Add(time.Second)
	setMessageSemantics(&firstFinal, "Harness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
	secondTurn := message("0198c7ec-73b0-7cc3-a5f7-e31c77140e03", agent.ID, model.HumanMailboxID, "second turn")
	secondTurn.CreatedAt = firstUpdate.CreatedAt.Add(2 * time.Second)
	setMessageSemantics(&secondTurn, "Harness provider: codex\nHarness session: thread-1\nHarness operation: turn-2")
	for _, item := range []model.Message{firstUpdate, firstFinal, secondTurn} {
		if err := s.Create(ctx, item); err != nil {
			t.Fatal(err)
		}
	}
	group := groupMessages([]model.Message{secondTurn, firstFinal, firstUpdate})[0]
	result := (app{ctx: ctx, store: s}).archiveGroup(group)().(archivedMsg)
	if result.err != nil || len(result.messageIDs) != 2 {
		t.Fatalf("archive result = %#v", result)
	}
	for _, item := range []model.Message{firstUpdate, firstFinal} {
		got, err := s.Get(ctx, item.ID)
		if err != nil || got.ArchivedAt == nil {
			t.Fatalf("oldest unit message = %#v, %v", got, err)
		}
	}
	remaining, err := s.Get(ctx, secondTurn.ID)
	if err != nil || remaining.ArchivedAt != nil {
		t.Fatalf("newer unit was archived: %#v, %v", remaining, err)
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
	setMessageSemantics(&first, "Kind: update\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
	second := message("0198c7ec-73b0-7cc3-a5f7-e31c77140d71", agent.ID, model.HumanMailboxID, "Second")
	second.CreatedAt = first.CreatedAt.Add(time.Second)
	setMessageSemantics(&second, "Kind: final-answer\nHarness provider: codex\nHarness session: thread-1\nHarness operation: turn-1")
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
	senderAddress := model.MessageAddress{MailboxID: sender, Kind: model.MailboxAgent, Label: "codex", Harness: "codex"}
	recipientAddress := model.MessageAddress{MailboxID: recipient, Kind: model.MailboxAgent, Label: "codex", Harness: "codex"}
	if sender == model.HumanMailboxID {
		senderLabel = "human"
		senderAddress = model.MessageAddress{MailboxID: sender, Kind: model.MailboxHuman, Label: "human"}
	}
	if recipient == model.HumanMailboxID {
		recipientLabel = "human"
		recipientAddress = model.MessageAddress{MailboxID: recipient, Kind: model.MailboxHuman, Label: "human"}
	}
	return model.Message{ID: id, Context: model.RepositoryContext{Directory: "/repo"}, SenderMailboxID: sender, RecipientMailboxID: recipient, SenderLabel: senderLabel, RecipientLabel: recipientLabel, SenderAddress: senderAddress, RecipientAddress: recipientAddress, Body: body, CreatedAt: time.Now().UTC()}
}

func setMessageSemantics(message *model.Message, legacy string) {
	var visible []string
	for _, line := range strings.Split(legacy, "\n") {
		trimmed := strings.TrimSpace(line)
		value := func(prefix string) (string, bool) {
			raw, found := strings.CutPrefix(trimmed, prefix)
			return strings.TrimSpace(raw), found
		}
		switch {
		case strings.HasPrefix(trimmed, "Kind:"):
			kind, _ := value("Kind:")
			message.Presentation = model.PresentationKind(kind)
		case trimmed == "Phase: final_answer":
			message.Presentation = model.PresentationFinalAnswer
		case strings.HasPrefix(trimmed, "Phase:"):
			phase, _ := value("Phase:")
			message.TechnicalSections = []model.TechnicalSection{{Namespace: "hq.legacy.harness", Fields: []model.TechnicalField{{Key: "phase", Label: "Phase", Value: phase}}}}
		case strings.HasPrefix(trimmed, "Harness provider:"):
			message.Correlation.Provider, _ = value("Harness provider:")
		case strings.HasPrefix(trimmed, "Harness session:"):
			message.Correlation.SessionID, _ = value("Harness session:")
		case strings.HasPrefix(trimmed, "Harness operation:"):
			message.Correlation.OperationID, _ = value("Harness operation:")
		case strings.HasPrefix(trimmed, "Harness item:"):
			message.Correlation.ItemID, _ = value("Harness item:")
		case strings.HasPrefix(trimmed, "Harness request:"):
			message.Correlation.RequestID, _ = value("Harness request:")
		case strings.HasPrefix(trimmed, "HQ message:"), strings.HasPrefix(trimmed, "HQ mailbox:"):
		default:
			visible = append(visible, line)
		}
	}
	message.HarnessProvider = message.Correlation.Provider
	message.HarnessSessionID = message.Correlation.SessionID
	message.HarnessOperationID = message.Correlation.OperationID
	message.Details = strings.TrimSpace(strings.Join(visible, "\n"))
}
