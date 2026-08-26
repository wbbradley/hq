//go:build !windows

package syncer

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"
)

func TestConcurrentEnsureNodeCallsConvergeOnOneOwner(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	database := filepath.Join(root, "installation", "hq.db")
	const callers = 12
	var starts atomic.Int32
	daemonResults := make(chan error, callers)
	starter := func(paths RuntimePaths) error {
		starts.Add(1)
		daemon := Daemon{
			Engine: &countingEngine{}, Coordinator: FileCoordinator{DatabasePath: paths.Database},
			DatabasePath: paths.Database, PollInterval: time.Hour,
		}
		go func() { daemonResults <- daemon.Run(context.Background()) }()
		return nil
	}
	launcher := NodeLauncher{Start: starter, ReadyTimeout: 2 * time.Second, PollInterval: 5 * time.Millisecond}
	var wait sync.WaitGroup
	errorsSeen := make(chan error, callers)
	for range callers {
		wait.Add(1)
		go func() {
			defer wait.Done()
			if err := launcher.Ensure(context.Background(), database); err != nil {
				errorsSeen <- err
			}
		}()
	}
	wait.Wait()
	close(errorsSeen)
	for err := range errorsSeen {
		t.Fatal(err)
	}
	started := int(starts.Load())
	if started < 1 || started > callers {
		t.Fatalf("detached contenders = %d", started)
	}
	paths, err := ResolveRuntimePaths(database)
	if err != nil {
		t.Fatal(err)
	}
	metadata, err := ReadInstanceMetadata(paths)
	if err != nil || metadata.InstanceID == "" {
		t.Fatalf("owner metadata = %#v, %v", metadata, err)
	}
	if err := StopDaemon(database); err != nil {
		t.Fatal(err)
	}
	winners := 0
	for range started {
		select {
		case err := <-daemonResults:
			if err == nil {
				winners++
			} else if !errors.Is(err, ErrNodeOwned) {
				t.Fatalf("contender error = %v", err)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("daemon contender did not exit")
		}
	}
	if winners != 1 {
		t.Fatalf("ownership winners = %d, starts = %d", winners, started)
	}
}

func TestEnsureNodeReadinessTimeoutIsBounded(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	database := filepath.Join(root, "hq.db")
	launcher := NodeLauncher{
		Start:        func(RuntimePaths) error { return nil },
		ReadyTimeout: 50 * time.Millisecond, PollInterval: 5 * time.Millisecond,
	}
	started := time.Now()
	err := launcher.Ensure(context.Background(), database)
	if err == nil || !strings.Contains(err.Error(), "did not become ready") || !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("readiness error = %v", err)
	}
	if elapsed := time.Since(started); elapsed > time.Second {
		t.Fatalf("readiness timeout took %s", elapsed)
	}
}

func TestEnsureNodeSurfacesDetachedStartupError(t *testing.T) {
	root := t.TempDir()
	t.Setenv("HOME", filepath.Join(root, "home"))
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	database := filepath.Join(root, "hq.db")
	launcher := NodeLauncher{
		Start: func(paths RuntimePaths) error {
			return os.WriteFile(paths.StartupLog, []byte("hq: unsupported HQ database schema 32; archive or remove the database\n"), 0o600)
		},
		ReadyTimeout: time.Second, PollInterval: 5 * time.Millisecond,
	}
	started := time.Now()
	err := launcher.Ensure(context.Background(), database)
	if err == nil || err.Error() != "local HQ node failed during startup: unsupported HQ database schema 32; archive or remove the database" {
		t.Fatalf("startup error = %v", err)
	}
	if elapsed := time.Since(started); elapsed > 500*time.Millisecond {
		t.Fatalf("startup error took %s to surface", elapsed)
	}
}

func TestEnsureNodeIgnoresStaleStartupErrors(t *testing.T) {
	root := t.TempDir()
	t.Setenv("HOME", filepath.Join(root, "home"))
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	database := filepath.Join(root, "hq.db")
	paths, err := ResolveRuntimePaths(database)
	if err != nil {
		t.Fatal(err)
	}
	if err := paths.EnsureDirectories(); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(paths.StartupLog, []byte("hq: stale startup failure\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	launcher := NodeLauncher{
		Start:        func(RuntimePaths) error { return nil },
		ReadyTimeout: 30 * time.Millisecond, PollInterval: 5 * time.Millisecond,
	}
	err = launcher.Ensure(context.Background(), database)
	if err == nil || strings.Contains(err.Error(), "stale startup failure") || !strings.Contains(err.Error(), "did not become ready") {
		t.Fatalf("readiness error = %v", err)
	}
}

func TestLiveSocketIsNeverRemovedAndStaleSocketIsReplaced(t *testing.T) {
	root := t.TempDir()
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	paths, err := ResolveRuntimePaths(filepath.Join(root, "hq.db"))
	if err != nil {
		t.Fatal(err)
	}
	if err := paths.EnsureDirectories(); err != nil {
		t.Fatal(err)
	}
	owner, err := listenLocalSocket(paths.Socket)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := listenLocalSocket(paths.Socket); !errors.Is(err, ErrNodeOwned) {
		t.Fatalf("live socket probe = %v", err)
	}
	if _, err := os.Stat(paths.Socket); err != nil {
		t.Fatalf("live owner socket was removed: %v", err)
	}
	if unixOwner, ok := owner.(interface{ SetUnlinkOnClose(bool) }); ok {
		unixOwner.SetUnlinkOnClose(false)
	}
	if err := owner.Close(); err != nil {
		t.Fatal(err)
	}
	replacement, err := listenLocalSocket(paths.Socket)
	if err != nil {
		t.Fatalf("replace stale socket: %v", err)
	}
	_ = replacement.Close()
}

func TestDetachedCommandContinuesAfterLauncherReturns(t *testing.T) {
	ready := filepath.Join(t.TempDir(), "detached-ready")
	logPath := filepath.Join(t.TempDir(), "detached.log")
	t.Setenv("HQ_DETACHED_CHILD_READY", ready)
	if err := startDetachedCommand(detachedCommand{
		Executable: os.Args[0], Arguments: []string{"-test.run=^TestDetachedChildProcess$"},
		Directory: t.TempDir(), LogPath: logPath,
	}); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(2 * time.Second)
	var pid int
	for time.Now().Before(deadline) {
		raw, err := os.ReadFile(ready)
		if err == nil {
			pid, err = strconv.Atoi(strings.TrimSpace(string(raw)))
			if err != nil {
				t.Fatal(err)
			}
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if pid <= 0 {
		t.Fatal("detached child did not remain alive after launch returned")
	}
	if info, err := os.Stat(logPath); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("detached log mode = %v, %v", info.Mode().Perm(), err)
	}
	process, err := os.FindProcess(pid)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = process.Kill() })
	if err := process.Signal(syscall.Signal(0)); err != nil {
		t.Fatalf("detached child is not alive: %v", err)
	}
}

func TestEnsureNodeWaitsForDetachedNodeProcess(t *testing.T) {
	root := t.TempDir()
	t.Setenv("HOME", filepath.Join(root, "home"))
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(root, "config"))
	t.Setenv("XDG_RUNTIME_DIR", filepath.Join(root, "runtime"))
	database := filepath.Join(root, "detached-node", "hq.db")
	t.Setenv("HQ_AUTOSTART_NODE_DATABASE", database)
	launcher := NodeLauncher{
		Start: func(paths RuntimePaths) error {
			return startDetachedCommand(detachedCommand{
				Executable: os.Args[0], Arguments: []string{"-test.run=^TestAutoStartedNodeProcess$"},
				Directory: filepath.Dir(paths.Database), LogPath: paths.Log,
			})
		},
		ReadyTimeout: 2 * time.Second, PollInterval: 10 * time.Millisecond,
	}
	if err := launcher.Ensure(context.Background(), database); err != nil {
		t.Fatal(err)
	}
	paths, err := ResolveRuntimePaths(database)
	if err != nil {
		t.Fatal(err)
	}
	metadata, err := ReadInstanceMetadata(paths)
	if err != nil || metadata.PID == os.Getpid() {
		t.Fatalf("detached metadata = %#v, %v", metadata, err)
	}
	if err := StopDaemon(database); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(paths.InstanceMetadata); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("detached node runtime metadata remains after stop: %v", err)
	}
}

func TestAutoStartedNodeProcess(t *testing.T) {
	database := os.Getenv("HQ_AUTOSTART_NODE_DATABASE")
	if database == "" {
		return
	}
	daemon := Daemon{
		Engine: &countingEngine{}, Coordinator: FileCoordinator{DatabasePath: database},
		DatabasePath: database, PollInterval: time.Hour,
	}
	if err := daemon.Run(context.Background()); err != nil {
		t.Fatal(err)
	}
}

func TestDetachedChildProcess(t *testing.T) {
	ready := os.Getenv("HQ_DETACHED_CHILD_READY")
	if ready == "" {
		return
	}
	if err := os.WriteFile(ready, []byte(strconv.Itoa(os.Getpid())+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	time.Sleep(5 * time.Second)
}
