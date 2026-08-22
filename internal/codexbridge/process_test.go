package codexbridge

import (
	"bytes"
	"slices"
	"strings"
	"testing"
)

func TestExecStarterArguments(t *testing.T) {
	if got, want := (ExecStarter{}).arguments(), []string{"app-server", "--stdio"}; !slices.Equal(got, want) {
		t.Fatalf("default arguments = %#v; want %#v", got, want)
	}
	if got, want := (ExecStarter{Yolo: true}).arguments(), []string{"--yolo", "app-server", "--stdio"}; !slices.Equal(got, want) {
		t.Fatalf("yolo arguments = %#v; want %#v", got, want)
	}
}

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
