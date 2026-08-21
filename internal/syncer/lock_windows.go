//go:build windows

package syncer

import (
	"errors"
	"os"

	"golang.org/x/sys/windows"
)

type fileLock struct {
	file       *os.File
	overlapped windows.Overlapped
}

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
	lock := &fileLock{file: file}
	err = windows.LockFileEx(windows.Handle(file.Fd()), windows.LOCKFILE_EXCLUSIVE_LOCK|windows.LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &lock.overlapped)
	if err != nil {
		file.Close()
		if errors.Is(err, windows.ERROR_LOCK_VIOLATION) {
			return nil, ErrNodeOwned
		}
		return nil, err
	}
	return lock, nil
}

func (l *fileLock) Release() error {
	if l.file == nil {
		return nil
	}
	err := windows.UnlockFileEx(windows.Handle(l.file.Fd()), 0, 1, 0, &l.overlapped)
	closeErr := l.file.Close()
	l.file = nil
	return errors.Join(err, closeErr)
}
