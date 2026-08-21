package syncer

import (
	"context"
	"errors"
	"fmt"
	"time"
)

type ProcessStarter func(RuntimePaths) error

type NodeLauncher struct {
	Start        ProcessStarter
	ReadyTimeout time.Duration
	PollInterval time.Duration
}

func EnsureNode(ctx context.Context, databasePath string) error {
	return (NodeLauncher{}).Ensure(ctx, databasePath)
}

func (l NodeLauncher) Ensure(ctx context.Context, databasePath string) error {
	paths, err := ResolveRuntimePaths(databasePath)
	if err != nil {
		return err
	}
	if err := paths.EnsureDirectories(); err != nil {
		return err
	}
	if _, err := DaemonStatus(paths.Database); err == nil {
		return nil
	} else if !isNodeAbsent(err) {
		return fmt.Errorf("connect to local HQ node: %w", err)
	}
	coordinator := FileCoordinator{DatabasePath: paths.Database}
	lock, err := coordinator.TryAcquire()
	switch {
	case err == nil:
		if err := lock.Release(); err != nil {
			return err
		}
		starter := l.Start
		if starter == nil {
			starter = startDetachedNode
		}
		if err := starter(paths); err != nil {
			return fmt.Errorf("start local HQ node: %w", err)
		}
	case errors.Is(err, ErrNodeOwned):
		// The owner may still be creating its socket; wait for readiness below.
	case err != nil:
		return err
	}
	if l.ReadyTimeout <= 0 {
		l.ReadyTimeout = 5 * time.Second
	}
	if l.PollInterval <= 0 {
		l.PollInterval = 20 * time.Millisecond
	}
	readyContext, cancel := context.WithTimeout(ctx, l.ReadyTimeout)
	defer cancel()
	ticker := time.NewTicker(l.PollInterval)
	defer ticker.Stop()
	var lastErr error
	for {
		if _, err := DaemonStatus(paths.Database); err == nil {
			return nil
		} else if !isNodeAbsent(err) {
			return fmt.Errorf("connect to starting local HQ node: %w", err)
		} else {
			lastErr = err
		}
		select {
		case <-readyContext.Done():
			return fmt.Errorf("local HQ node did not become ready within %s: %w", l.ReadyTimeout, errors.Join(readyContext.Err(), lastErr))
		case <-ticker.C:
		}
	}
}
