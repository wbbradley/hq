//go:build !windows

package syncer

import (
	"errors"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
)

type detachedCommand struct {
	Executable string
	Arguments  []string
	Directory  string
	LogPath    string
}

func startDetachedNode(paths RuntimePaths) error {
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	return startDetachedCommand(detachedCommand{
		Executable: executable,
		Arguments:  []string{"--db", paths.Database, "daemon", "run"},
		Directory:  filepath.Dir(paths.Database),
		LogPath:    paths.Log,
	})
}

func startDetachedCommand(specification detachedCommand) error {
	if specification.Executable == "" || specification.Directory == "" || specification.LogPath == "" {
		return errors.New("detached command needs an executable, directory, and log path")
	}
	if err := os.MkdirAll(specification.Directory, 0o700); err != nil {
		return err
	}
	logFile, err := os.OpenFile(specification.LogPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	defer logFile.Close()
	nullInput, err := os.Open(os.DevNull)
	if err != nil {
		return err
	}
	defer nullInput.Close()
	command := exec.Command(specification.Executable, specification.Arguments...)
	command.Dir = specification.Directory
	command.Stdin = nullInput
	command.Stdout = logFile
	command.Stderr = logFile
	command.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := command.Start(); err != nil {
		return err
	}
	return command.Process.Release()
}

func isNodeAbsent(err error) bool {
	return errors.Is(err, os.ErrNotExist) || errors.Is(err, syscall.ENOENT) || errors.Is(err, syscall.ECONNREFUSED) || errors.Is(err, net.ErrClosed)
}
