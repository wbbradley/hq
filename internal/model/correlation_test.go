package model

import "testing"

func TestParseMessageCorrelation(t *testing.T) {
	tests := []struct {
		name    string
		details string
		want    MessageCorrelation
	}{
		{name: "both", details: "Kind: update\nCodex thread: thread-1\nCodex turn: turn-1", want: MessageCorrelation{CodexThreadID: "thread-1", CodexTurnID: "turn-1"}},
		{name: "trimmed", details: "  Codex thread:   thread-2  \nCodex turn: turn-2  ", want: MessageCorrelation{CodexThreadID: "thread-2", CodexTurnID: "turn-2"}},
		{name: "none", details: "Codex thread: (none)\nCodex turn: (none)"},
		{name: "blank", details: "Codex thread:\nCodex turn:   "},
		{name: "embedded text is ignored", details: "Reason: Codex thread: wrong\nCodex threadish: also-wrong"},
		{name: "first exact value wins", details: "Codex thread: first\nCodex thread: second", want: MessageCorrelation{CodexThreadID: "first"}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := ParseMessageCorrelation(test.details); got != test.want {
				t.Fatalf("ParseMessageCorrelation() = %#v; want %#v", got, test.want)
			}
		})
	}
}
