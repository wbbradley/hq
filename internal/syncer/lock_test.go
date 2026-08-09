package syncer

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
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
