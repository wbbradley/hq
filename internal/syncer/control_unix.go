//go:build !windows

package syncer

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

type unixControl struct {
	listener net.Listener
	path     string
	once     sync.Once
}

const maxUnixSocketPath = 103

func socketPath(databasePath string) string {
	direct := databasePath + ".sync.sock"
	if len(direct) <= maxUnixSocketPath {
		return direct
	}
	sum := sha256.Sum256([]byte(filepath.Clean(databasePath)))
	name := hex.EncodeToString(sum[:16]) + ".sock"
	return filepath.Join("/tmp", fmt.Sprintf("hq-%d", os.Getuid()), name)
}

func startControl(ctx context.Context, databasePath string, wake chan<- struct{}, stop context.CancelFunc, status func() string) (io.Closer, error) {
	path := socketPath(databasePath)
	if path != databasePath+".sync.sock" {
		directory := filepath.Dir(path)
		if err := os.MkdirAll(directory, 0o700); err != nil {
			return nil, err
		}
		if err := os.Chmod(directory, 0o700); err != nil {
			return nil, err
		}
	}
	if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	listener, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	if err := os.Chmod(path, 0o600); err != nil {
		listener.Close()
		os.Remove(path)
		return nil, err
	}
	handle := &unixControl{listener: listener, path: path}
	go func() {
		<-ctx.Done()
		_ = handle.Close()
	}()
	go func() {
		for {
			connection, err := listener.Accept()
			if err != nil {
				return
			}
			go handleControl(connection, wake, stop, status)
		}
	}()
	return handle, nil
}

func handleControl(connection net.Conn, wake chan<- struct{}, stop context.CancelFunc, status func() string) {
	defer connection.Close()
	_ = connection.SetDeadline(time.Now().Add(2 * time.Second))
	command, err := bufio.NewReader(io.LimitReader(connection, 64)).ReadString('\n')
	if err != nil {
		return
	}
	response := "unknown command"
	switch strings.TrimSpace(command) {
	case "wake":
		select {
		case wake <- struct{}{}:
		default:
		}
		response = "awake"
	case "status":
		response = status()
	case "stop":
		response = "stopping"
		defer stop()
	}
	_, _ = io.WriteString(connection, response+"\n")
}

func controlCommand(databasePath, command string) (string, error) {
	connection, err := net.DialTimeout("unix", socketPath(databasePath), 500*time.Millisecond)
	if err != nil {
		return "", err
	}
	defer connection.Close()
	_ = connection.SetDeadline(time.Now().Add(time.Second))
	if _, err := io.WriteString(connection, command+"\n"); err != nil {
		return "", err
	}
	response, err := bufio.NewReader(io.LimitReader(connection, 4096)).ReadString('\n')
	return strings.TrimSpace(response), err
}

func (c *unixControl) Close() error {
	var err error
	c.once.Do(func() {
		err = c.listener.Close()
		removeErr := os.Remove(c.path)
		if removeErr != nil && !errors.Is(removeErr, os.ErrNotExist) {
			err = errors.Join(err, removeErr)
		}
	})
	return err
}
