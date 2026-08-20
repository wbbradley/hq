package codexbridge

import (
	"context"
	"io"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"
)

func TestInstalledCodexV01480Smoke(t *testing.T) {
	if os.Getenv("HQ_CODEX_SMOKE") != "1" {
		t.Skip("set HQ_CODEX_SMOKE=1 to test the installed Codex app-server")
	}
	versionContext, cancelVersion := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancelVersion()
	versionOutput, err := exec.CommandContext(versionContext, "codex", "--version").CombinedOutput()
	if err != nil {
		t.Fatalf("run codex --version: %v: %s", err, versionOutput)
	}
	wantVersion := "codex-cli " + TestedCodexVersion
	if version := strings.TrimSpace(string(versionOutput)); version != wantVersion {
		t.Fatalf("installed Codex version = %q, want %q", version, wantVersion)
	}

	process, err := (ExecStarter{}).Start(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	waitDone := make(chan error, 1)
	go func() { waitDone <- process.Wait() }()
	go func() { _, _ = io.Copy(io.Discard, process.Errors()) }()
	protocolContext, cancelProtocol := context.WithTimeout(context.Background(), 10*time.Second)
	client := NewClient(protocolContext, process.Output(), process.Input(), nil, nil)
	initialize := InitializeParams{
		ClientInfo:   ClientInfo{Name: "hq-smoke", Title: "HQ Codex bridge smoke test", Version: TestedCodexVersion},
		Capabilities: InitializeCapabilities{ExperimentalAPI: true},
	}
	if err := client.Call(protocolContext, "initialize", initialize, nil); err != nil {
		cancelProtocol()
		_ = process.Kill()
		<-waitDone
		t.Fatalf("initialize installed Codex app-server: %v", err)
	}
	if err := client.Notify("initialized", struct{}{}); err != nil {
		cancelProtocol()
		_ = process.Kill()
		<-waitDone
		t.Fatalf("acknowledge installed Codex app-server: %v", err)
	}
	cancelProtocol()
	_ = process.Input().Close()
	select {
	case <-waitDone:
	case <-time.After(3 * time.Second):
		_ = process.Kill()
		<-waitDone
		t.Fatal("installed Codex app-server did not stop after stdin closed")
	}
}
