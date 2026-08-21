package syncer

import (
	"context"
	"errors"
)

var ErrNodeOwned = errors.New("another local HQ node owns this database")

// ErrSyncLocked is retained while foreground sync still shares the ownership
// coordinator. Domain RPC removes that transitional path in the next migration.
var ErrSyncLocked = ErrNodeOwned

type Lock interface {
	Release() error
}

type SyncCoordinator interface {
	TryAcquire() (Lock, error)
}

type FileCoordinator struct{ DatabasePath string }

func (c FileCoordinator) LockPath() string {
	paths, err := ResolveRuntimePaths(c.DatabasePath)
	if err != nil {
		return c.DatabasePath + ".sync.lock"
	}
	return paths.OwnershipLock
}

type CoordinatedEngine struct {
	Engine      SyncEngine
	Coordinator SyncCoordinator
}

func (e CoordinatedEngine) RunOnce(ctx context.Context) error {
	if e.Engine == nil || e.Coordinator == nil {
		return errors.New("coordinated sync needs an engine and coordinator")
	}
	lock, err := e.Coordinator.TryAcquire()
	if err != nil {
		return err
	}
	defer lock.Release()
	return e.Engine.RunOnce(ctx)
}

func (e CoordinatedEngine) Run(ctx context.Context) error {
	if e.Engine == nil || e.Coordinator == nil {
		return errors.New("coordinated sync needs an engine and coordinator")
	}
	lock, err := e.Coordinator.TryAcquire()
	if err != nil {
		return err
	}
	defer lock.Release()
	return e.Engine.Run(ctx)
}
