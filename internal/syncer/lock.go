package syncer

import (
	"context"
	"errors"
)

var ErrSyncLocked = errors.New("another sync worker owns this database")

type Lock interface {
	Release() error
}

type SyncCoordinator interface {
	TryAcquire() (Lock, error)
}

type FileCoordinator struct{ DatabasePath string }

func (c FileCoordinator) LockPath() string { return c.DatabasePath + ".sync.lock" }

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
