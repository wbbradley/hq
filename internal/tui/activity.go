package tui

import (
	"sort"
	"strings"
	"time"

	"charm.land/lipgloss/v2"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

var activityFailureStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("196"))

type conversationTimelineEntry struct {
	message  *model.Message
	activity *domain.HarnessActivity
	at       time.Time
	key      string
}

func conversationTimeline(group messageGroup) []conversationTimelineEntry {
	entries := make([]conversationTimelineEntry, 0, len(group.messages)+len(group.activities))
	for index := range group.messages {
		message := &group.messages[index]
		entries = append(entries, conversationTimelineEntry{message: message, at: message.CreatedAt, key: "message:" + message.ID})
	}
	for index := range group.activities {
		activity := &group.activities[index]
		entries = append(entries, conversationTimelineEntry{activity: activity, at: activity.OccurredAt, key: "activity:" + activityExpansionKey(*activity)})
	}
	sort.SliceStable(entries, func(left, right int) bool {
		if entries[left].at.Equal(entries[right].at) {
			return entries[left].key < entries[right].key
		}
		return entries[left].at.Before(entries[right].at)
	})
	return entries
}

func activityExpansionKey(activity domain.HarnessActivity) string {
	return strings.Join([]string{activity.Harness, activity.SessionID, activity.OperationID, string(activity.Kind), activity.ItemID}, "\x00")
}

func (m app) activityExpansionState(activities []domain.HarnessActivity) string {
	var expanded []string
	for _, activity := range activities {
		key := activityExpansionKey(activity)
		if m.expandedActivities[key] {
			expanded = append(expanded, key)
		}
	}
	sort.Strings(expanded)
	return strings.Join(expanded, "\x01")
}

func (m *app) toggleSelectedActivities() bool {
	group, found := m.detailGroup()
	if !found || len(group.activities) == 0 {
		return false
	}
	layout := responsivePaneLayout(m.width, m.height, m.answering)
	rendered := m.renderGroupPanelLayout(group, layout.messageWidth)
	if len(rendered.activitySpans) == 0 {
		return false
	}
	start := m.resolvedMessageStart(group, rendered, layout.messageHeight)
	chosen := rendered.activitySpans[0]
	for _, span := range rendered.activitySpans {
		if span.start > start {
			break
		}
		chosen = span
	}
	if m.expandedActivities == nil {
		m.expandedActivities = make(map[string]bool)
	}
	if m.expandedActivities[chosen.key] {
		delete(m.expandedActivities, chosen.key)
	} else {
		m.expandedActivities[chosen.key] = true
	}
	if m.markdown != nil {
		m.markdown.groupCache = nil
	}
	return true
}

func (m app) renderHarnessActivity(activity domain.HarnessActivity, width int, expanded bool) string {
	width = max(12, width)
	contentWidth := max(1, width-4)
	marker := "▸"
	if expanded {
		marker = "▾"
	}
	label := strings.ToUpper(strings.ReplaceAll(string(activity.Kind), "-", " "))
	status := strings.ToUpper(string(activity.Status))
	header := marker + " " + label
	if status != "" {
		header += " · " + status
	}
	if !activity.OccurredAt.IsZero() {
		header += " · " + activity.OccurredAt.Local().Format("3:04:05 PM")
	}
	header = truncateDisplay(header, contentWidth)
	if activity.Status == domain.HarnessActivityFailed {
		header = activityFailureStyle.Render(header)
	} else {
		header = titleStyle.Render(header)
	}

	lines := []string{dimPanelEdge.Render("╭─") + " " + header}
	if !expanded {
		summary := activitySummary(activity)
		if activity.Truncated {
			disclosure := " · [truncated]"
			if lipgloss.Width(disclosure) >= contentWidth {
				summary = truncateDisplay("[truncated]", contentWidth)
			} else {
				summary = truncateDisplay(summary, contentWidth-lipgloss.Width(disclosure)) + disclosure
			}
		}
		lines = append(lines, dimPanelEdge.Render("╰─")+" "+truncateDisplay(summary, contentWidth))
		return strings.Join(lines, "\n")
	}

	content := activityExpandedContent(activity)
	for _, line := range wrapActivityText(content, contentWidth) {
		lines = append(lines, dimPanelEdge.Render("│ ")+line)
	}
	if activity.Truncated {
		lines = append(lines, dimPanelEdge.Render("│ ")+activityFailureStyle.Render("[content truncated]"))
	}
	lines = append(lines, dimPanelEdge.Render("╰─"))
	return strings.Join(lines, "\n")
}

func activitySummary(activity domain.HarnessActivity) string {
	var summary string
	switch activity.Kind {
	case domain.HarnessActivityOperation:
		summary = activity.Body
		if strings.TrimSpace(summary) == "" {
			summary = string(activity.Status)
		}
	case domain.HarnessActivityCommand, domain.HarnessActivityFile, domain.HarnessActivityTool:
		summary = activity.Title
		if activity.Status == domain.HarnessActivityFailed && strings.TrimSpace(activity.Body) != "" {
			summary += " · " + firstActivityLine(activity.Body)
		}
	default:
		summary = firstActivityLine(activity.Body)
	}
	if strings.TrimSpace(summary) == "" {
		return "(no details)"
	}
	return singleLine(summary)
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

func firstActivityLine(value string) string {
	line, _, _ := strings.Cut(value, "\n")
	return strings.TrimSpace(line)
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
