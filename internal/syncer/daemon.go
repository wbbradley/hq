package syncer

import (
	"context"
	"errors"
	"fmt"
	"sync/atomic"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/buildinfo"
	"github.com/wbbradley/hq/internal/localwire"
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
	for {
		restart, err := d.runRuntime(ctx)
		if err != nil || !restart {
			return err
		}
	}
}

func (d Daemon) runRuntime(ctx context.Context) (bool, error) {
	runCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	var restarting atomic.Bool
	restart := func() {
		restarting.Store(true)
		cancel()
	}
	wake := make(chan struct{}, 1)
	var state atomic.Value
	state.Store("starting")
	metadata := localwire.PeerMetadata{Build: buildinfo.Version, InstanceID: uuid.NewString(), StartedAt: time.Now().UTC()}
	control, err := startControl(runCtx, d.DatabasePath, wake, cancel, restart, func() string {
		return state.Load().(string)
	}, metadata)
	if err != nil && !errors.Is(err, ErrControlUnavailable) {
		return false, err
	}
	if control != nil {
		defer control.Close()
	}
	if engine, ok := d.Engine.(WakeSyncEngine); ok {
		state.Store("running; live relay subscriptions starting")
		err := engine.RunWithWake(runCtx, wake)
		if restarting.Load() {
			return true, nil
		}
		if runCtx.Err() != nil {
			return false, nil
		}
		return false, err
	}
	for {
		err := d.Engine.RunOnce(runCtx)
		if runCtx.Err() != nil {
			return restarting.Load(), nil
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
			return restarting.Load(), nil
		case <-wake:
			timer.Stop()
		case <-timer.C:
		}
	}
}

func Wake(databasePath string) error {
	var response lifecycleAcknowledgement
	_, err := controlCommand(databasePath, wakeMethod, &response)
	if err == nil && response.State != "awake" {
		return fmt.Errorf("unexpected daemon response %q", response.State)
	}
	return err
}

func DaemonStatus(databasePath string) (string, error) {
	var response lifecycleStatus
	_, err := controlCommand(databasePath, statusMethod, &response)
	return response.State, err
}

func StopDaemon(databasePath string) error {
	var response lifecycleAcknowledgement
	_, err := controlCommand(databasePath, stopMethod, &response)
	if err != nil {
		return err
	}
	if response.State != "stopping" {
		return fmt.Errorf("unexpected daemon response %q", response.State)
	}
	return nil
}

func RestartDaemon(databasePath string) error {
	var response lifecycleAcknowledgement
	_, err := controlCommand(databasePath, restartMethod, &response)
	if err != nil {
		return err
	}
	if response.State != "restarting" {
		return fmt.Errorf("unexpected daemon response %q", response.State)
	}
	return nil
}
