package model

import "strings"

// MessageCorrelation identifies the harness session and operation recorded in
// line-oriented message details. These identifiers are distinct from ThreadID,
// which is HQ's canonical causal thread root.
type MessageCorrelation struct {
	HarnessProvider    string
	HarnessSessionID   string
	HarnessOperationID string
}

// ParseMessageCorrelation extracts the first non-empty exact correlation value
// for each supported details line.
func ParseMessageCorrelation(details string) MessageCorrelation {
	var correlation MessageCorrelation
	for _, line := range strings.Split(details, "\n") {
		line = strings.TrimSpace(line)
		if correlation.HarnessProvider == "" {
			correlation.HarnessProvider = correlationValue(line, "Harness provider:")
		}
		if correlation.HarnessSessionID == "" {
			correlation.HarnessSessionID = correlationValue(line, "Harness session:")
			if correlation.HarnessSessionID == "" {
				correlation.HarnessSessionID = correlationValue(line, "Codex thread:")
				if correlation.HarnessSessionID != "" && correlation.HarnessProvider == "" {
					correlation.HarnessProvider = "codex"
				}
			}
		}
		if correlation.HarnessOperationID == "" {
			correlation.HarnessOperationID = correlationValue(line, "Harness operation:")
			if correlation.HarnessOperationID == "" {
				correlation.HarnessOperationID = correlationValue(line, "Codex turn:")
			}
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
