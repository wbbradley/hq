package codexbridge

import (
	"bytes"
	"strings"
	"testing"
)

func TestForwardStderrAnnotatesEveryLine(t *testing.T) {
	var output bytes.Buffer
	if err := forwardStderr(&output, strings.NewReader("warning one\nwarning two\n")); err != nil {
		t.Fatal(err)
	}
	want := "hq codex: app-server: warning one\nhq codex: app-server: warning two\n"
	if output.String() != want {
		t.Fatalf("stderr = %q", output.String())
	}
}
