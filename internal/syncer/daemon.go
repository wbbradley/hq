package syncer

import (
	"context"
	"errors"
	"fmt"
	"sync/atomic"
	"time"
)

var ErrControlUnavailable = errors.New("sync daemon control is unavailable on this platform")

type Daemon struct {
	Engine       SyncEngine
	Coordinator  SyncCoordinator
	DatabasePath string
	PollInterval time.Duration
}

func (d Daemon) Run(ctx context.Context) error {
	if d.Engine == nil || d.Coordinator == nil || d.DatabasePath == "" {
		return errors.New("daemon needs an engine, coordinator, and database path")
	}
	if d.PollInterval <= 0 {
		d.PollInterval = 15 * time.Second
	}
	lock, err := d.Coordinator.TryAcquire()
	if err != nil {
		return err
	}
	defer lock.Release()
	runCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	wake := make(chan struct{}, 1)
	var state atomic.Value
	state.Store("starting")
	control, err := startControl(runCtx, d.DatabasePath, wake, cancel, func() string {
		return state.Load().(string)
	})
	if err != nil && !errors.Is(err, ErrControlUnavailable) {
		return err
	}
	if control != nil {
		defer control.Close()
	}
	if engine, ok := d.Engine.(WakeSyncEngine); ok {
		state.Store("running; live relay subscriptions starting")
		err := engine.RunWithWake(runCtx, wake)
		if runCtx.Err() != nil {
			return nil
		}
		return err
	}
	for {
		err := d.Engine.RunOnce(runCtx)
		if runCtx.Err() != nil {
			return nil
		}
		if err != nil {
			state.Store("running; last sync error: " + err.Error())
		} else {
			state.Store("running; last sync succeeded at " + time.Now().UTC().Format(time.RFC3339))
		}
		timer := time.NewTimer(d.PollInterval)
		select {
		case <-runCtx.Done():
			timer.Stop()
			return nil
		case <-wake:
			timer.Stop()
		case <-timer.C:
		}
	}
}

func Wake(databasePath string) error {
	_, err := controlCommand(databasePath, "wake")
	return err
}

func DaemonStatus(databasePath string) (string, error) {
	return controlCommand(databasePath, "status")
}

func StopDaemon(databasePath string) error {
	response, err := controlCommand(databasePath, "stop")
	if err != nil {
		return err
	}
	if response != "stopping" {
		return fmt.Errorf("unexpected daemon response %q", response)
	}
	return nil
}
