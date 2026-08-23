package model

import "strings"

// MessageCorrelation identifies the Codex conversation and turn recorded in
// line-oriented message details. These identifiers are distinct from ThreadID,
// which is HQ's canonical causal thread root.
type MessageCorrelation struct {
	CodexThreadID string
	CodexTurnID   string
}

// ParseMessageCorrelation extracts the first non-empty exact correlation value
// for each supported details line.
func ParseMessageCorrelation(details string) MessageCorrelation {
	var correlation MessageCorrelation
	for _, line := range strings.Split(details, "\n") {
		line = strings.TrimSpace(line)
		if correlation.CodexThreadID == "" {
			correlation.CodexThreadID = correlationValue(line, "Codex thread:")
		}
		if correlation.CodexTurnID == "" {
			correlation.CodexTurnID = correlationValue(line, "Codex turn:")
		}
	}
	return correlation
}

func correlationValue(line, prefix string) string {
	if !strings.HasPrefix(line, prefix) {
		return ""
	}
	value := strings.TrimSpace(strings.TrimPrefix(line, prefix))
	if value == "(none)" {
		return ""
	}
	return value
}
