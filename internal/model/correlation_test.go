package model

import "testing"

func TestParseMessageCorrelation(t *testing.T) {
	tests := []struct {
		name    string
		details string
		want    MessageCorrelation
	}{
		{name: "both", details: "Kind: update\nHarness provider: home-built\nHarness session: thread-1\nHarness operation: turn-1", want: MessageCorrelation{HarnessProvider: "home-built", HarnessSessionID: "thread-1", HarnessOperationID: "turn-1"}},
		{name: "legacy Codex", details: "Codex thread: thread-old\nCodex turn: turn-old", want: MessageCorrelation{HarnessProvider: "codex", HarnessSessionID: "thread-old", HarnessOperationID: "turn-old"}},
		{name: "trimmed", details: "  Harness session:   thread-2  \nHarness operation: turn-2  ", want: MessageCorrelation{HarnessSessionID: "thread-2", HarnessOperationID: "turn-2"}},
		{name: "none", details: "Harness session: (none)\nHarness operation: (none)"},
		{name: "blank", details: "Harness session:\nHarness operation:   "},
		{name: "embedded text is ignored", details: "Reason: Harness session: wrong\nCodex threadish: also-wrong"},
		{name: "first exact value wins", details: "Harness session: first\nHarness session: second", want: MessageCorrelation{HarnessSessionID: "first"}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := ParseMessageCorrelation(test.details); got != test.want {
				t.Fatalf("ParseMessageCorrelation() = %#v; want %#v", got, test.want)
			}
		})
	}
}
