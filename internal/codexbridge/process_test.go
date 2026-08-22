package codexbridge

import (
	"bytes"
	"os"
	"path/filepath"
	"runtime"
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

func TestExecStarterUsesExactEnvironmentAndDiscardsSnapshot(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell fixture requires Unix")
	}
	directory := t.TempDir()
	output := filepath.Join(directory, "environment.txt")
	script := filepath.Join(directory, "fake-codex")
	if err := os.WriteFile(script, []byte("#!/bin/sh\nprintf '%s' \"${ONLY_VAR-unset}\" > \""+output+"\"\n"), 0o700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("ONLY_VAR", "daemon-value")
	starter := &ExecStarter{Path: script, Environment: []string{}, UseEnvironment: true}
	process, err := starter.Start(directory)
	if err != nil {
		t.Fatal(err)
	}
	if err := process.Wait(); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(output)
	if err != nil || string(raw) != "unset" {
		t.Fatalf("empty child environment = %q, %v", raw, err)
	}
	if starter.Environment != nil {
		t.Fatalf("starter retained environment = %#v", starter.Environment)
	}

	starter = &ExecStarter{Path: script, Environment: []string{"ONLY_VAR=caller-value"}, UseEnvironment: true}
	process, err = starter.Start(directory)
	if err != nil {
		t.Fatal(err)
	}
	if err := process.Wait(); err != nil {
		t.Fatal(err)
	}
	raw, err = os.ReadFile(output)
	if err != nil || string(raw) != "caller-value" {
		t.Fatalf("exact child environment = %q, %v", raw, err)
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
