package e2e_test

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

type message struct {
	ID     string `json:"id"`
	Sender string `json:"sender"`
	Body   string `json:"body"`
}

func TestIsolatedCLIRequestReply(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("local node transport is not supported on Windows")
	}

	repository := repositoryRoot(t)
	root := t.TempDir()
	binary := filepath.Join(root, "bin", "hq")
	if err := os.MkdirAll(filepath.Dir(binary), 0o700); err != nil {
		t.Fatal(err)
	}
	build := exec.Command("go", "build", "-o", binary, "./cmd/hq")
	build.Dir = repository
	if output, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build hq: %v\n%s", err, output)
	}

	environment := isolatedEnvironment(t, root)
	database := filepath.Join(root, "data", "hq.db")
	run := func(agent bool, args ...string) string {
		t.Helper()
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		command := exec.CommandContext(ctx, binary, args...)
		command.Env = environment
		if agent {
			command.Env = append(command.Env, "CODEX_THREAD_ID=isolated-e2e-thread")
		}
		output, err := command.CombinedOutput()
		if err != nil {
			t.Fatalf("hq %s: %v\n%s", strings.Join(args, " "), err, output)
		}
		return string(output)
	}
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		command := exec.CommandContext(ctx, binary, "--db", database, "daemon", "stop")
		command.Env = environment
		_ = command.Run()
	})

	run(false, "--db", database, "identity", "init")
	sentOutput := run(true, "--no-sync", "--db", database, "send", "--json", "What is six times seven?")
	var sent message
	if err := json.Unmarshal([]byte(sentOutput), &sent); err != nil {
		t.Fatalf("decode sent message: %v\n%s", err, sentOutput)
	}
	if sent.ID == "" || sent.Body != "What is six times seven?" || !strings.HasPrefix(sent.Sender, "codex:") {
		t.Fatalf("sent message = %#v", sent)
	}

	inboxOutput := run(false, "--no-sync", "--db", database, "list", "--recipient", "human", "--json")
	var inbox []message
	if err := json.Unmarshal([]byte(inboxOutput), &inbox); err != nil {
		t.Fatalf("decode human inbox: %v\n%s", err, inboxOutput)
	}
	if len(inbox) != 1 || inbox[0] != sent {
		t.Fatalf("human inbox = %#v; sent = %#v", inbox, sent)
	}

	run(false, "--no-sync", "--db", database, "answer", sent.ID, "42")
	if reply := run(true, "--no-sync", "--db", database, "wait", "--timeout", "5s", sent.ID); reply != "42\n" {
		t.Fatalf("agent reply = %q", reply)
	}

	archivedOutput := run(false, "--no-sync", "--db", database, "list", "--recipient", "human", "--archived", "--json")
	var archived []message
	if err := json.Unmarshal([]byte(archivedOutput), &archived); err != nil {
		t.Fatalf("decode archived inbox: %v\n%s", err, archivedOutput)
	}
	if len(archived) != 1 || archived[0].ID != sent.ID {
		t.Fatalf("archived inbox = %#v", archived)
	}
	if status := run(false, "--db", database, "daemon", "status"); !strings.HasPrefix(status, "running") {
		t.Fatalf("daemon status = %q", status)
	}
	run(false, "--db", database, "daemon", "stop")
	logPath := filepath.Join(root, "home", "logs", "hq.log")
	rawLog, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatal(err)
	}
	if log := string(rawLog); !strings.Contains(log, `msg="daemon starting"`) || !strings.Contains(log, `msg="daemon control plane ready"`) || !strings.Contains(log, `msg="daemon stopped"`) {
		t.Fatalf("daemon lifecycle log = %q", log)
	}
}

func repositoryRoot(t *testing.T) string {
	t.Helper()
	_, filename, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("locate e2e test source")
	}
	return filepath.Dir(filepath.Dir(filename))
}

func isolatedEnvironment(t *testing.T, root string) []string {
	t.Helper()
	runtimeRoot, err := os.MkdirTemp("/tmp", "hq-e2e-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(runtimeRoot) })
	paths := map[string]string{
		"HOME":            filepath.Join(root, "home"),
		"TMPDIR":          runtimeRoot,
		"XDG_CACHE_HOME":  filepath.Join(root, "cache"),
		"XDG_CONFIG_HOME": filepath.Join(root, "config"),
		"XDG_RUNTIME_DIR": runtimeRoot,
		"XDG_STATE_HOME":  filepath.Join(root, "state"),
	}
	for _, path := range paths {
		if err := os.MkdirAll(path, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	blocked := map[string]bool{
		"CLAUDE_CODE_SESSION_ID": true,
		"CODEX_THREAD_ID":        true,
		"HOME":                   true,
		"HQ_DB":                  true,
		"HQ_SESSION":             true,
		"PI_SESSION_ID":          true,
		"TMPDIR":                 true,
		"XDG_CACHE_HOME":         true,
		"XDG_CONFIG_HOME":        true,
		"XDG_RUNTIME_DIR":        true,
		"XDG_STATE_HOME":         true,
	}
	environment := make([]string, 0, len(os.Environ())+len(paths)+1)
	for _, value := range os.Environ() {
		name, _, _ := strings.Cut(value, "=")
		if !blocked[name] {
			environment = append(environment, value)
		}
	}
	for name, value := range paths {
		environment = append(environment, name+"="+value)
	}
	environment = append(environment, "NO_COLOR=1")
	return environment
}
