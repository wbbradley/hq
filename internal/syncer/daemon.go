package syncer

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"sync/atomic"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/buildinfo"
	"github.com/wbbradley/hq/internal/localwire"
)

var ErrControlUnavailable = errors.New("sync daemon control is unavailable on this platform")

type Daemon struct {
	Engine         SyncEngine
	Domain         *localwire.ModeConfig
	RuntimeFactory RuntimeFactory
	Coordinator    SyncCoordinator
	DatabasePath   string
	PollInterval   time.Duration
	Logger         *slog.Logger
}

type Runtime struct {
	Engine SyncEngine
	Domain *localwire.ModeConfig
	Closer io.Closer
}

type RuntimeFactory func(context.Context) (Runtime, error)

func (d Daemon) Run(ctx context.Context) error {
	if (d.Engine == nil && d.RuntimeFactory == nil) || d.Coordinator == nil || d.DatabasePath == "" {
		return errors.New("daemon needs an engine or runtime factory, coordinator, and database path")
	}
	if d.PollInterval <= 0 {
		d.PollInterval = 15 * time.Second
	}
	paths, err := ResolveRuntimePaths(d.DatabasePath)
	if err != nil {
		return err
	}
	logger := d.logger().With("component", "daemon", "database", paths.Database)
	logger.Info("daemon starting", "pid", os.Getpid(), "build", buildinfo.Version, "log_path", paths.Log)
	logger.Debug("acquiring daemon ownership lock", "lock_path", paths.OwnershipLock)
	lock, err := d.Coordinator.TryAcquire()
	if err != nil {
		logger.Error("daemon ownership lock failed", "error", err)
		return err
	}
	logger.Info("daemon ownership acquired", "lock_path", paths.OwnershipLock)
	defer func() {
		if err := lock.Release(); err != nil {
			logger.Error("daemon ownership release failed", "error", err)
		} else {
			logger.Debug("daemon ownership released")
		}
	}()
	defer logger.Info("daemon stopped")
	for {
		var closer io.Closer
		if d.RuntimeFactory != nil {
			logger.Debug("constructing daemon runtime")
			runtime, err := d.RuntimeFactory(ctx)
			if err != nil {
				logger.Error("construct daemon runtime", "error", err)
				return err
			}
			if runtime.Engine == nil {
				if runtime.Closer != nil {
					_ = runtime.Closer.Close()
				}
				logger.Error("daemon runtime factory returned no engine")
				return errors.New("daemon runtime factory returned no engine")
			}
			d.Engine, d.Domain, closer = runtime.Engine, runtime.Domain, runtime.Closer
			logger.Info("daemon runtime constructed", "domain_rpc", d.Domain != nil)
		}
		restart, err := d.runRuntime(ctx, paths)
		if closer != nil {
			closeErr := closer.Close()
			if closeErr != nil {
				logger.Error("close daemon runtime", "error", closeErr)
			}
			err = errors.Join(err, closeErr)
		}
		if err != nil || !restart {
			if err != nil {
				logger.Error("daemon runtime stopped with error", "error", err)
			} else {
				logger.Info("daemon runtime stopped", "reason", "shutdown")
			}
			return err
		}
		logger.Info("restarting daemon runtime")
	}
}

func (d Daemon) runRuntime(ctx context.Context, paths RuntimePaths) (bool, error) {
	logger := d.logger().With("component", "daemon", "database", paths.Database)
	runCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	var restarting atomic.Bool
	restart := func() {
		logger.Info("daemon restart requested")
		restarting.Store(true)
		cancel()
	}
	wake := make(chan struct{}, 1)
	var state atomic.Value
	state.Store("starting")
	metadata := localwire.PeerMetadata{Build: buildinfo.Version, InstanceID: uuid.NewString(), StartedAt: time.Now().UTC()}
	control, err := startControl(runCtx, paths, wake, cancel, restart, func() string {
		return state.Load().(string)
	}, metadata, d.Domain)
	if err != nil && !errors.Is(err, ErrControlUnavailable) {
		logger.Error("start daemon control plane", "socket", paths.Socket, "error", err)
		return false, err
	}
	if errors.Is(err, ErrControlUnavailable) {
		logger.Warn("daemon control plane unavailable")
	} else {
		logger.Info("daemon control plane ready", "socket", paths.Socket, "instance_id", metadata.InstanceID)
	}
	if control != nil {
		defer func() {
			if err := control.Close(); err != nil {
				logger.Error("close daemon control plane", "error", err)
			}
		}()
	}
	if engine, ok := d.Engine.(WakeSyncEngine); ok {
		state.Store("running; live relay subscriptions starting")
		logger.Info("sync engine starting", "mode", "live")
		err := engine.RunWithWake(runCtx, wake)
		if restarting.Load() {
			logger.Info("sync engine stopped for restart")
			return true, nil
		}
		if runCtx.Err() != nil {
			logger.Info("sync engine stopped", "reason", "context canceled")
			return false, nil
		}
		if err != nil {
			logger.Error("sync engine stopped unexpectedly", "error", err)
		}
		return false, err
	}
	logger.Info("sync engine starting", "mode", "poll", "interval", d.PollInterval)
	for {
		err := d.Engine.RunOnce(runCtx)
		if runCtx.Err() != nil {
			logger.Info("sync engine stopped", "restart", restarting.Load())
			return restarting.Load(), nil
		}
		if err != nil {
			state.Store("running; last sync error: " + err.Error())
			logger.Warn("sync iteration failed", "error", err)
		} else {
			state.Store("running; last sync succeeded at " + time.Now().UTC().Format(time.RFC3339))
			logger.Debug("sync iteration succeeded")
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

func (d Daemon) logger() *slog.Logger {
	if d.Logger == nil {
		return slog.New(slog.DiscardHandler)
	}
	return d.Logger
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
	handshake, err := controlCommand(databasePath, stopMethod, &response)
	if err != nil {
		return err
	}
	if response.State != "stopping" {
		return fmt.Errorf("unexpected daemon response %q", response.State)
	}
	paths, err := ResolveRuntimePaths(databasePath)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	coordinator := FileCoordinator{DatabasePath: databasePath}
	for {
		metadata, readErr := ReadInstanceMetadata(paths)
		if readErr == nil && metadata.InstanceID != handshake.Server.InstanceID {
			return nil
		}
		if readErr != nil && !errors.Is(readErr, os.ErrNotExist) {
			return fmt.Errorf("read stopping daemon metadata: %w", readErr)
		}
		lock, lockErr := coordinator.TryAcquire()
		if lockErr == nil {
			return lock.Release()
		}
		if !errors.Is(lockErr, ErrNodeOwned) {
			return fmt.Errorf("wait for stopping daemon ownership release: %w", lockErr)
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("daemon did not stop within 30s: %w", ctx.Err())
		case <-ticker.C:
		}
	}
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
