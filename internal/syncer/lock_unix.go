//go:build !windows

package syncer

import (
	"errors"
	"fmt"
	"os"
	"syscall"
)

type fileLock struct{ file *os.File }

func (c FileCoordinator) TryAcquire() (Lock, error) {
	paths, err := ResolveRuntimePaths(c.DatabasePath)
	if err != nil {
		return nil, err
	}
	if err := paths.EnsureDirectories(); err != nil {
		return nil, err
	}
	file, err := os.OpenFile(paths.OwnershipLock, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		file.Close()
		if errors.Is(err, syscall.EWOULDBLOCK) || errors.Is(err, syscall.EAGAIN) {
			return nil, ErrNodeOwned
		}
		return nil, fmt.Errorf("lock sync file: %w", err)
	}
	return &fileLock{file: file}, nil
}

func (l *fileLock) Release() error {
	if l.file == nil {
		return nil
	}
	err := syscall.Flock(int(l.file.Fd()), syscall.LOCK_UN)
	closeErr := l.file.Close()
	l.file = nil
	return errors.Join(err, closeErr)
}
