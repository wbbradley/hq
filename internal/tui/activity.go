package tui

import (
	"sort"
	"strings"
	"time"

	"charm.land/lipgloss/v2"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

var activityStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("241"))

type conversationTimelineEntry struct {
	message  *model.Message
	activity *domain.HarnessActivity
	at       time.Time
	key      string
}

func conversationTimeline(group messageGroup) []conversationTimelineEntry {
	if group.entriesLoaded {
		entries := make([]conversationTimelineEntry, 0, len(group.entries))
		for _, entry := range group.entries {
			switch entry.Kind {
			case domain.ConversationEntryMessage:
				if entry.Message != nil {
					entries = append(entries, conversationTimelineEntry{message: entry.Message, at: entry.Message.CreatedAt, key: "message:" + entry.EventID})
				}
			case domain.ConversationEntryActivity:
				if entry.Activity != nil {
					entries = append(entries, conversationTimelineEntry{activity: entry.Activity, at: entry.Activity.OccurredAt, key: "activity:" + entry.EventID})
				}
			}
		}
		return entries
	}
	entries := make([]conversationTimelineEntry, 0, len(group.messages)+len(group.activities))
	for index := range group.messages {
		message := &group.messages[index]
		entries = append(entries, conversationTimelineEntry{message: message, at: message.CreatedAt, key: "message:" + message.ID})
	}
	for index := range group.activities {
		activity := &group.activities[index]
		key := strings.Join([]string{activity.Harness, activity.SessionID, activity.OperationID, string(activity.Kind), activity.ItemID}, "\x00")
		entries = append(entries, conversationTimelineEntry{activity: activity, at: activity.OccurredAt, key: "activity:" + key})
	}
	sort.SliceStable(entries, func(left, right int) bool {
		if entries[left].at.Equal(entries[right].at) {
			return entries[left].key < entries[right].key
		}
		return entries[left].at.Before(entries[right].at)
	})
	return entries
}

func (m *app) toggleActivities() bool {
	group, found := m.detailGroup()
	if !found || len(group.activities) == 0 {
		return false
	}
	m.showActivities = !m.showActivities
	if m.markdown != nil {
		m.markdown.groupCache = nil
	}
	return true
}

func (m app) renderHarnessActivity(activity domain.HarnessActivity, width int) string {
	width = max(12, width)
	contentWidth := max(1, width-4)
	label := strings.ToUpper(strings.ReplaceAll(string(activity.Kind), "-", " "))
	status := strings.ToUpper(string(activity.Status))
	header := "▾ " + label
	if status != "" {
		header += " · " + status
	}
	if !activity.OccurredAt.IsZero() {
		header += " · " + activity.OccurredAt.Local().Format("3:04:05 PM")
	}
	header = truncateDisplay(header, contentWidth)
	header = activityStyle.Render(header)

	lines := []string{dimPanelEdge.Render("╭─") + " " + header}
	content := activityExpandedContent(activity)
	for _, line := range wrapActivityText(content, contentWidth) {
		lines = append(lines, dimPanelEdge.Render("│ ")+activityStyle.Render(line))
	}
	if activity.Truncated {
		lines = append(lines, dimPanelEdge.Render("│ ")+activityStyle.Render("[content truncated]"))
	}
	lines = append(lines, dimPanelEdge.Render("╰─"))
	return strings.Join(lines, "\n")
}

func activityExpandedContent(activity domain.HarnessActivity) string {
	parts := make([]string, 0, 2)
	if strings.TrimSpace(activity.Title) != "" {
		parts = append(parts, activity.Title)
	}
	if strings.TrimSpace(activity.Body) != "" {
		parts = append(parts, activity.Body)
	}
	if len(parts) == 0 {
		parts = append(parts, string(activity.Status))
	}
	return strings.Join(parts, "\n")
}

func wrapActivityText(value string, width int) []string {
	width = max(1, width)
	value = strings.ReplaceAll(value, "\t", "    ")
	var result []string
	for _, source := range strings.Split(value, "\n") {
		if source == "" {
			result = append(result, "")
			continue
		}
		var line strings.Builder
		lineWidth := 0
		for _, r := range source {
			runeWidth := lipgloss.Width(string(r))
			if lineWidth > 0 && lineWidth+runeWidth > width {
				result = append(result, line.String())
				line.Reset()
				lineWidth = 0
			}
			line.WriteRune(r)
			lineWidth += runeWidth
		}
		result = append(result, line.String())
	}
	if len(result) == 0 {
		return []string{""}
	}
	return result
}
