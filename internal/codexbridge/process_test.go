package codexbridge

import (
	"bytes"
	"io"
	"log/slog"
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

func TestExecProcessLogsExitDetailsWithoutEnvironmentValues(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell fixture requires Unix")
	}
	directory := t.TempDir()
	script := filepath.Join(directory, "fake-codex")
	if err := os.WriteFile(script, []byte("#!/bin/sh\nprintf 'diagnostic\\n' >&2\nexit 7\n"), 0o700); err != nil {
		t.Fatal(err)
	}
	const secret = "environment-secret"
	var diagnostics bytes.Buffer
	starter := &ExecStarter{
		Path: script, Environment: []string{"TOKEN=" + secret}, UseEnvironment: true,
		Logger: slog.New(slog.NewTextHandler(&diagnostics, &slog.HandlerOptions{Level: slog.LevelDebug})),
	}
	process, err := starter.Start(directory)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := io.Copy(io.Discard, process.Errors()); err != nil {
		t.Fatal(err)
	}
	if err := process.Wait(); err == nil {
		t.Fatal("process unexpectedly succeeded")
	}
	log := diagnostics.String()
	for _, expected := range []string{`msg="starting Codex app-server process"`, `msg="Codex app-server process started"`, `msg="Codex app-server process exited"`, "exit_code=7"} {
		if !strings.Contains(log, expected) {
			t.Fatalf("process log omitted %q: %s", expected, log)
		}
	}
	if strings.Contains(log, secret) {
		t.Fatalf("process log exposed environment value: %s", log)
	}
}
