package syncer

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

func TestFileCoordinatorContentionReleaseAndStalePath(t *testing.T) {
	database := filepath.Join(t.TempDir(), "one", "hq.db")
	coordinator := FileCoordinator{DatabasePath: database}
	first, err := coordinator.TryAcquire()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := coordinator.TryAcquire(); !errors.Is(err, ErrSyncLocked) {
		t.Fatalf("second lock = %v", err)
	}
	if err := first.Release(); err != nil {
		t.Fatal(err)
	}
	second, err := coordinator.TryAcquire()
	if err != nil {
		t.Fatalf("lock after release = %v", err)
	}
	if err := second.Release(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(coordinator.LockPath()); err != nil && !errors.Is(err, os.ErrNotExist) {
		t.Fatal(err)
	}
}

type countingEngine struct{ calls atomic.Int32 }

func (e *countingEngine) RunOnce(context.Context) error {
	e.calls.Add(1)
	return nil
}

func (e *countingEngine) Run(ctx context.Context) error {
	<-ctx.Done()
	return ctx.Err()
}

func TestDaemonWakeStatusStopAndStaleSocket(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	stale := database + ".sync.sock"
	if err := os.WriteFile(stale, []byte("stale"), 0o600); err != nil {
		t.Fatal(err)
	}
	engine := &countingEngine{}
	daemon := Daemon{Engine: engine, Coordinator: FileCoordinator{DatabasePath: database}, DatabasePath: database, PollInterval: time.Hour}
	done := make(chan error, 1)
	go func() { done <- daemon.Run(context.Background()) }()
	deadline := time.Now().Add(2 * time.Second)
	for {
		status, err := DaemonStatus(database)
		if err == nil && strings.Contains(status, "running") {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("daemon did not start: %v", err)
		}
		time.Sleep(10 * time.Millisecond)
	}
	info, err := os.Stat(stale)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("socket mode = %#v, %v", info, err)
	}
	before := engine.calls.Load()
	if err := Wake(database); err != nil {
		t.Fatal(err)
	}
	for engine.calls.Load() == before && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if engine.calls.Load() == before {
		t.Fatal("wake did not start a sync pass")
	}
	if err := StopDaemon(database); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("daemon did not stop")
	}
	if _, err := os.Stat(stale); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("socket remains after stop: %v", err)
	}
	lock, err := (FileCoordinator{DatabasePath: database}).TryAcquire()
	if err != nil {
		t.Fatalf("daemon did not release lock: %v", err)
	}
	_ = lock.Release()
}

func TestFileLockReleasesWhenOwnerProcessExits(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	command := exec.Command(os.Args[0], "-test.run=TestLockHelperProcess")
	command.Env = append(os.Environ(), "HQ_LOCK_HELPER="+database)
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("lock helper: %v: %s", err, output)
	}
	lock, err := (FileCoordinator{DatabasePath: database}).TryAcquire()
	if err != nil {
		t.Fatalf("lock remained after process exit: %v", err)
	}
	_ = lock.Release()
}

func TestLockHelperProcess(t *testing.T) {
	database := os.Getenv("HQ_LOCK_HELPER")
	if database == "" {
		return
	}
	lock, err := (FileCoordinator{DatabasePath: database}).TryAcquire()
	if err != nil {
		t.Fatal(err)
	}
	_ = lock
}

func TestDaemonAndCLIStoresShareSQLiteWAL(t *testing.T) {
	database := filepath.Join(t.TempDir(), "hq.db")
	key, err := identity.KeyPath(database)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(key, nil); err != nil {
		t.Fatal(err)
	}
	daemonStore, err := store.Open(database)
	if err != nil {
		t.Fatal(err)
	}
	defer daemonStore.Close()
	engine := &Engine{State: daemonStore, Codec: daemonStore.WireCodec(nil, nil)}
	daemon := Daemon{Engine: engine, Coordinator: FileCoordinator{DatabasePath: database}, DatabasePath: database, PollInterval: 10 * time.Millisecond}
	done := make(chan error, 1)
	go func() { done <- daemon.Run(context.Background()) }()
	deadline := time.Now().Add(2 * time.Second)
	for {
		if _, err := DaemonStatus(database); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("daemon did not start")
		}
		time.Sleep(10 * time.Millisecond)
	}
	cliStore, err := store.Open(database)
	if err != nil {
		t.Fatal(err)
	}
	agent, err := cliStore.ResolveMailbox(context.Background(), model.SessionIdentity{Harness: "codex", ExternalSessionID: "wal-test"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	for index := 0; index < 20; index++ {
		id, err := uuid.NewV7()
		if err != nil {
			t.Fatal(err)
		}
		message := model.Message{ID: id.String(), SenderMailboxID: agent.ID, RecipientMailboxID: model.HumanMailboxID, Body: "concurrent", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}
		if err := cliStore.Create(context.Background(), message); err != nil {
			t.Fatal(err)
		}
	}
	if err := cliStore.Close(); err != nil {
		t.Fatal(err)
	}
	if err := StopDaemon(database); err != nil {
		t.Fatal(err)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	check, err := store.Open(database)
	if err != nil {
		t.Fatal(err)
	}
	defer check.Close()
	items, err := check.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil || len(items) != 20 {
		t.Fatalf("concurrent messages = %d, %v", len(items), err)
	}
}

func TestFileCoordinatorKeepsDatabasesIndependent(t *testing.T) {
	dir := t.TempDir()
	one, err := (FileCoordinator{DatabasePath: filepath.Join(dir, "one.db")}).TryAcquire()
	if err != nil {
		t.Fatal(err)
	}
	defer one.Release()
	two, err := (FileCoordinator{DatabasePath: filepath.Join(dir, "two.db")}).TryAcquire()
	if err != nil {
		t.Fatal(err)
	}
	defer two.Release()
}
