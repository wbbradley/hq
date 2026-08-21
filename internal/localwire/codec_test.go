package localwire

import (
	"strings"
	"testing"
)

func TestCodecRejectsInvalidFrames(t *testing.T) {
	tests := []struct {
		name, frame, want string
		limit             int
	}{
		{name: "empty", frame: "\n", want: "empty", limit: 1024},
		{name: "malformed", frame: "{nope}\n", want: "malformed", limit: 1024},
		{name: "unknown field", frame: `{"kind":"request","version":1,"id":"1","method":"x","surprise":true}` + "\n", want: "unknown field", limit: 1024},
		{name: "invalid envelope", frame: `{"kind":"request","version":1,"method":"x"}` + "\n", want: "needs version, id, and method", limit: 1024},
		{name: "oversized", frame: strings.Repeat("x", 65) + "\n", want: "exceeds 64 bytes", limit: 64},
		{name: "unterminated", frame: `{"kind":"notification"}`, want: "unterminated", limit: 1024},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, err := NewCodec(strings.NewReader(test.frame), &strings.Builder{}, test.limit).Read()
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v; want %q", err, test.want)
			}
		})
	}
}
