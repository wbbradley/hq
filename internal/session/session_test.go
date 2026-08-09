package session

import (
	"errors"
	"strings"
	"testing"
)

func TestResolve(t *testing.T) {
	tests := []struct {
		name, explicit string
		env            map[string]string
		wantHarness    string
		wantID         string
		wantError      string
	}{
		{name: "explicit wins", explicit: " direct ", env: map[string]string{"HQ_SESSION": "env", "CODEX_THREAD_ID": "codex"}, wantHarness: "custom", wantID: "direct"},
		{name: "hq override", env: map[string]string{"HQ_SESSION": " env ", "CODEX_THREAD_ID": "codex"}, wantHarness: "custom", wantID: "env"},
		{name: "codex", env: map[string]string{"CODEX_THREAD_ID": " c "}, wantHarness: "codex", wantID: "c"},
		{name: "claude", env: map[string]string{"CLAUDE_CODE_SESSION_ID": "cl"}, wantHarness: "claude-code", wantID: "cl"},
		{name: "pi", env: map[string]string{"PI_SESSION_ID": "pi-id"}, wantHarness: "pi", wantID: "pi-id"},
		{name: "blank", env: map[string]string{"CODEX_THREAD_ID": "  "}, wantError: ErrNotFound.Error()},
		{name: "conflict", env: map[string]string{"CODEX_THREAD_ID": "same", "PI_SESSION_ID": "same"}, wantError: "ambiguous"},
		{name: "reserved", explicit: "human", wantError: "reserved"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			r := Resolver{Getenv: func(name string) string { return tt.env[name] }}
			got, err := r.Resolve(tt.explicit)
			if tt.wantError != "" {
				if err == nil || !strings.Contains(err.Error(), tt.wantError) {
					t.Fatalf("error = %v", err)
				}
				return
			}
			if err != nil || got.Harness != tt.wantHarness || got.ExternalSessionID != tt.wantID {
				t.Fatalf("Resolve = %#v, %v", got, err)
			}
		})
	}
}

func TestNotFoundSentinel(t *testing.T) {
	_, err := (Resolver{}).Resolve("")
	if !errors.Is(err, ErrNotFound) {
		t.Fatalf("error = %v", err)
	}
}
