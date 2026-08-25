package tui

import (
	"slices"
	"strings"
	"testing"
	"time"

	"charm.land/bubbles/v2/textarea"
	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

func TestFakeProviderActivityRendersChronologicallyAsCollapsedAndExpandedCards(t *testing.T) {
	started := time.Date(2026, 8, 24, 12, 0, 0, 0, time.UTC)
	first := message("first-message", testAgentID, model.HumanMailboxID, "First message")
	first.CreatedAt = started
	final := message("final-message", testAgentID, model.HumanMailboxID, "Final answer")
	final.CreatedAt = started.Add(9 * time.Second)
	setMessageSemantics(&final, "Kind: final-answer")
	activities := fakeTimelineActivities(started.Add(time.Second))
	group := messageGroup{key: "fake-conversation", messages: []model.Message{first, final}, activities: activities}
	m := app{groups: []messageGroup{group}, messages: []model.Message{final}, editor: textarea.New(), width: 100, height: 100, paneFocus: focusMessage, markdown: newMessageMarkdownRenderer(nil)}

	rendered := m.renderGroupPanelLayout(group, 100)
	if len(rendered.spans) != 2 || len(rendered.activitySpans) != 7 {
		t.Fatalf("actionable/non-actionable spans = %#v / %#v", rendered.spans, rendered.activitySpans)
	}
	collapsed := ansi.Strip(rendered.panel)
	if !(strings.Index(collapsed, "First message") < strings.Index(collapsed, "▸ OPERATION STATUS") && strings.Index(collapsed, "▸ PROGRESS") < strings.Index(collapsed, "Final answer")) {
		t.Fatalf("timeline order = %q", collapsed)
	}
	for _, label := range []string{"OPERATION STATUS", "PLAN", "DIFF", "COMMAND", "FILE CHANGE", "TOOL CALL", "PROGRESS"} {
		if !strings.Contains(collapsed, "▸ "+label) {
			t.Fatalf("collapsed timeline omitted %s: %q", label, collapsed)
		}
	}
	if strings.Contains(collapsed, "second plan line") || !strings.Contains(collapsed, "FAILED") || !strings.Contains(collapsed, "[truncated]") {
		t.Fatalf("collapsed disclosure = %q", collapsed)
	}
	if kind := groupPresentationKind(group); kind != "final-answer" {
		t.Fatalf("activity changed final-answer presentation to %q", kind)
	}

	m.expandedActivities = make(map[string]bool, len(activities))
	for _, activity := range activities {
		m.expandedActivities[activityExpansionKey(activity)] = true
	}
	expanded := ansi.Strip(m.renderGroupPanel(m.groups[0], 100))
	if !strings.Contains(expanded, "▾ PLAN") || !strings.Contains(expanded, "second plan line") || !strings.Contains(expanded, "[content truncated]") {
		t.Fatalf("expanded timeline = %q", expanded)
	}
}

func TestHarnessActivitiesUseMutedText(t *testing.T) {
	for _, kind := range []domain.HarnessActivityKind{
		domain.HarnessActivityOperation,
		domain.HarnessActivityPlan,
		domain.HarnessActivityProgress,
		domain.HarnessActivityCommand,
		domain.HarnessActivityDiff,
		domain.HarnessActivityFile,
		domain.HarnessActivityTool,
	} {
		activity := domain.HarnessActivity{Kind: kind, Title: "activity title", Body: "activity body", Status: domain.HarnessActivityFailed}
		for _, expanded := range []bool{false, true} {
			rendered := (app{}).renderHarnessActivity(activity, 80, expanded)
			label := strings.ToUpper(strings.ReplaceAll(string(kind), "-", " "))
			content := "activity body"
			if expanded || kind == domain.HarnessActivityCommand || kind == domain.HarnessActivityFile || kind == domain.HarnessActivityTool {
				content = "activity title"
			}
			if !strings.Contains(rendered, "\x1b[38;5;241m▸ "+label) && !strings.Contains(rendered, "\x1b[38;5;241m▾ "+label) {
				t.Fatalf("%s expanded=%t title was not muted: %q", kind, expanded, rendered)
			}
			if !strings.Contains(rendered, "\x1b[38;5;241m"+content) || strings.Contains(rendered, "\x1b[38;5;212m") || strings.Contains(rendered, "\x1b[38;5;196m") {
				t.Fatalf("%s expanded=%t styling = %q", kind, expanded, rendered)
			}
		}
	}
}

func TestTypedConversationEntriesRenderCanonicalOrderInsteadOfTimestamps(t *testing.T) {
	started := time.Date(2026, 8, 24, 12, 0, 0, 0, time.UTC)
	first := message("canonical-first", testAgentID, model.HumanMailboxID, "First canonical message")
	first.CreatedAt = started.Add(10 * time.Minute)
	activity := domain.HarnessActivity{
		EventID: strings.Repeat("b", 64), MailboxID: testAgentID, Harness: "codex", SessionID: "session", OperationID: "operation",
		Kind: domain.HarnessActivityPlan, Body: "Canonical middle plan", OccurredAt: started.Add(-10 * time.Minute),
	}
	final := message("canonical-final", testAgentID, model.HumanMailboxID, "Final canonical message")
	final.CreatedAt = started
	group := messageGroup{
		key: "canonical-order", entriesLoaded: true,
		entries: []domain.ConversationEntry{
			{Kind: domain.ConversationEntryMessage, EventID: strings.Repeat("a", 64), DisplayOrder: 10, Message: &first},
			{Kind: domain.ConversationEntryActivity, EventID: activity.EventID, DisplayOrder: 11, Activity: &activity},
			{Kind: domain.ConversationEntryMessage, EventID: strings.Repeat("c", 64), DisplayOrder: 12, Message: &final},
		},
		messages: []model.Message{first, final}, activities: []domain.HarnessActivity{activity},
	}
	m := app{groups: []messageGroup{group}, messages: []model.Message{final}, editor: textarea.New(), width: 100, height: 80, paneFocus: focusMessage, markdown: newMessageMarkdownRenderer(nil)}
	rendered := ansi.Strip(m.renderGroupPanel(group, 100))
	if !(strings.Index(rendered, first.Body) < strings.Index(rendered, activity.Body) && strings.Index(rendered, activity.Body) < strings.Index(rendered, final.Body)) {
		t.Fatalf("typed timeline ignored canonical order: %q", rendered)
	}
	messageOnly := messageGroup{messages: append([]model.Message(nil), group.messages...)}
	if replyTarget(group).ID != replyTarget(messageOnly).ID || archiveTarget(group).ID != archiveTarget(messageOnly).ID || group.latest().ID != messageOnly.latest().ID {
		t.Fatalf("activity changed message actions: reply=%#v archive=%#v latest=%#v", replyTarget(group), archiveTarget(group), group.latest())
	}

	// Reordering only the authoritative entries must invalidate the render cache,
	// even though the compatibility slices are unchanged.
	group.entries[0], group.entries[1] = group.entries[1], group.entries[0]
	reordered := ansi.Strip(m.renderGroupPanel(group, 100))
	if strings.Index(reordered, activity.Body) >= strings.Index(reordered, first.Body) {
		t.Fatalf("entry-only reorder reused stale render: %q", reordered)
	}
}

func TestActivityExpansionPreservesDraftAndMessageActionTargets(t *testing.T) {
	item := message("question", testAgentID, model.HumanMailboxID, "Question")
	setMessageSemantics(&item, "Kind: question")
	group := messageGroup{
		key: "conversation", messages: []model.Message{item},
		activities: []domain.HarnessActivity{{MailboxID: testAgentID, Harness: "home-built", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityProgress, ItemID: "progress", Body: "working", OccurredAt: item.CreatedAt.Add(-time.Second)}},
	}
	editor := textarea.New()
	editor.SetValue("draft survives")
	m := app{
		groups: []messageGroup{group}, messages: []model.Message{item}, answering: true, answerID: item.ID, answerGroupKey: group.key, answerQ: item,
		activeDraftKey: group.key, editor: editor, width: 80, height: 30, paneFocus: focusMessage, markdown: newMessageMarkdownRenderer(nil),
	}
	beforeReply, beforeArchive := replyTarget(group), archiveTarget(group)
	updated, _ := m.Update(tea.KeyPressMsg{Code: 'e'})
	m = updated.(app)
	if m.editor.Value() != "draft survives" || m.answerID != item.ID || m.answerGroupKey != group.key || !m.answering {
		t.Fatalf("activity expansion changed draft/reply state: %#v", m)
	}
	if got := replyTarget(m.groups[0]); got.ID != beforeReply.ID || archiveTarget(m.groups[0]).ID != beforeArchive.ID || actionUnitKey(got) != actionUnitKey(beforeReply) {
		t.Fatalf("activity changed action targets: reply=%#v archive=%#v", got, archiveTarget(m.groups[0]))
	}
}

func TestCoalescedActivityRefreshPreservesLogicalMessageScrollAnchor(t *testing.T) {
	started := time.Date(2026, 8, 24, 12, 0, 0, 0, time.UTC)
	first := message("anchor-first", testAgentID, model.HumanMailboxID, strings.Repeat("first message line\n", 10))
	first.CreatedAt = started
	second := message("anchor-second", testAgentID, model.HumanMailboxID, strings.Repeat("second message line\n", 15))
	second.CreatedAt = started.Add(2 * time.Second)
	activity := domain.HarnessActivity{MailboxID: testAgentID, Harness: "home-built", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityPlan, Body: "short plan", OccurredAt: started.Add(time.Second)}
	group := messageGroup{key: "anchor-conversation", entriesLoaded: true, messages: []model.Message{first, second}, activities: []domain.HarnessActivity{activity}}
	group.entries = []domain.ConversationEntry{
		{Kind: domain.ConversationEntryMessage, EventID: strings.Repeat("1", 64), DisplayOrder: 1, Message: &group.messages[0]},
		{Kind: domain.ConversationEntryActivity, EventID: strings.Repeat("2", 64), DisplayOrder: 2, Activity: &group.activities[0]},
		{Kind: domain.ConversationEntryMessage, EventID: strings.Repeat("3", 64), DisplayOrder: 3, Message: &group.messages[1]},
	}
	m := app{
		groups: []messageGroup{group}, messages: []model.Message{second}, expandedActivities: map[string]bool{activityExpansionKey(activity): true},
		editor: textarea.New(), markdown: newMessageMarkdownRenderer(nil), width: 80, height: 24, paneFocus: focusMessage,
	}
	m.reconcileMessageViewport(false)
	m.scrollMessagePane(10_000)
	anchorID, before := m.messageAnchorID, m.messageScroll
	if anchorID != second.ID {
		t.Fatalf("fixture anchor = %q, want %q", anchorID, second.ID)
	}
	m.groups[0].activities[0].Body = strings.Repeat("coalesced plan grew across the viewport\n", 20)
	m.groups[0].activities[0].OccurredAt = activity.OccurredAt.Add(time.Second)
	m.reconcileMessageViewport(true)
	if !m.messageScrollManual || m.messageAnchorID != anchorID || m.messageScroll <= before {
		t.Fatalf("coalesced activity lost logical anchor: scroll=%d before=%d anchor=%q", m.messageScroll, before, m.messageAnchorID)
	}
}

func TestActivityRefreshDoesNotCreateInboxRows(t *testing.T) {
	item := message("only-message", testAgentID, model.HumanMailboxID, "Only message")
	key := conversationKeyForMessage(item)
	stableKey := conversationKeyString(key)
	summary := model.ConversationSummary{Key: key, Latest: item, OldestOpen: &item}
	m := app{conversationMode: true, conversations: []model.ConversationSummary{summary}, histories: map[string][]model.Message{stableKey: {item}}, editor: textarea.New()}
	m.setMessages()
	updated, _ := m.Update(historyLoadedMsg{
		key: stableKey, messages: []model.Message{item},
		activities: []domain.HarnessActivity{{MailboxID: testAgentID, Harness: "home-built", SessionID: "session", OperationID: "operation", Kind: domain.HarnessActivityProgress, ItemID: "progress", Body: "working", OccurredAt: item.CreatedAt}},
	})
	m = updated.(app)
	if len(m.messages) != 1 || len(m.groups) != 1 || len(m.groups[0].messages) != 1 || len(m.groups[0].activities) != 1 || m.groups[0].latest().ID != item.ID {
		t.Fatalf("activity changed inbox rows: messages=%#v groups=%#v", m.messages, m.groups)
	}
}

func TestTUISubscribesToLocalActivityChanges(t *testing.T) {
	if !slices.Contains(tuiChangeTopics(), domain.TopicActivities) {
		t.Fatalf("TUI change topics = %#v", tuiChangeTopics())
	}
}

func fakeTimelineActivities(started time.Time) []domain.HarnessActivity {
	base := domain.HarnessActivity{MailboxID: testAgentID, Harness: "home-built", SessionID: "session", OperationID: "operation"}
	activity := func(offset time.Duration, kind domain.HarnessActivityKind, item, title, body string, status domain.HarnessActivityStatus) domain.HarnessActivity {
		result := base
		result.Kind, result.ItemID, result.Title, result.Body, result.Status, result.OccurredAt = kind, item, title, body, status, started.Add(offset)
		return result
	}
	result := []domain.HarnessActivity{
		activity(0, domain.HarnessActivityOperation, "", "", "", domain.HarnessActivityRunning),
		activity(time.Second, domain.HarnessActivityPlan, "", "", "first plan line\nsecond plan line", ""),
		activity(2*time.Second, domain.HarnessActivityDiff, "", "", "diff --git a/a b/a", ""),
		activity(3*time.Second, domain.HarnessActivityCommand, "command", "go test ./...", "Exit code: 1\nFAIL", domain.HarnessActivityFailed),
		activity(4*time.Second, domain.HarnessActivityFile, "file", "main.go", "updated", domain.HarnessActivityCompleted),
		activity(5*time.Second, domain.HarnessActivityTool, "tool", "search", "found matches", domain.HarnessActivityCompleted),
		activity(6*time.Second, domain.HarnessActivityProgress, "progress", "", "working", ""),
	}
	result[3].Truncated = true
	return result
}
