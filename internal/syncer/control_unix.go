//go:build !windows

package syncer

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/wbbradley/hq/internal/buildinfo"
	"github.com/wbbradley/hq/internal/localwire"
)

const maxUnixSocketPath = 103

type unixControl struct {
	listener      net.Listener
	path          string
	once          sync.Once
	connectionsMu sync.Mutex
	connections   map[net.Conn]struct{}
	closed        bool
}

func socketPath(databasePath string) string {
	direct := databasePath + ".sync.sock"
	if len(direct) <= maxUnixSocketPath {
		return direct
	}
	sum := sha256.Sum256([]byte(filepath.Clean(databasePath)))
	name := hex.EncodeToString(sum[:16]) + ".sock"
	return filepath.Join("/tmp", fmt.Sprintf("hq-%d", os.Getuid()), name)
}

func startControl(ctx context.Context, databasePath string, wake chan<- struct{}, stop, restart context.CancelFunc, status func() string, metadata localwire.PeerMetadata) (io.Closer, error) {
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
	handler := func(_ context.Context, _ *localwire.Session, method string, _ json.RawMessage) (any, *localwire.RPCError) {
		switch method {
		case wakeMethod:
			select {
			case wake <- struct{}{}:
			default:
			}
			return lifecycleAcknowledgement{State: "awake"}, nil
		case statusMethod:
			return lifecycleStatus{State: status()}, nil
		case stopMethod:
			return localwire.DeferredResponse{Value: lifecycleAcknowledgement{State: "stopping"}, After: stop}, nil
		case restartMethod:
			return localwire.DeferredResponse{Value: lifecycleAcknowledgement{State: "restarting"}, After: restart}, nil
		default:
			return nil, &localwire.RPCError{Code: localwire.CodeMethodNotFound, Message: fmt.Sprintf("unknown lifecycle method %q", method)}
		}
	}
	server, err := localwire.NewServer(localwire.ServerOptions{
		Metadata: metadata,
		Modes: map[localwire.HandshakeMode]localwire.ModeConfig{
			localwire.LifecycleMode: {Supported: localwire.LifecycleVersions, Handler: handler},
		},
		RequestTimeout: 2 * time.Second,
	})
	if err != nil {
		listener.Close()
		os.Remove(path)
		return nil, err
	}
	handle := &unixControl{listener: listener, path: path, connections: make(map[net.Conn]struct{})}
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
			handle.connectionsMu.Lock()
			if handle.closed {
				handle.connectionsMu.Unlock()
				_ = connection.Close()
				return
			}
			handle.connections[connection] = struct{}{}
			handle.connectionsMu.Unlock()
			go func() {
				_ = server.ServeConn(ctx, connection)
				handle.connectionsMu.Lock()
				delete(handle.connections, connection)
				handle.connectionsMu.Unlock()
			}()
		}
	}()
	return handle, nil
}

func controlCommand(databasePath, method string, destination any) (localwire.HandshakeResponse, error) {
	connection, err := net.DialTimeout("unix", socketPath(databasePath), 500*time.Millisecond)
	if err != nil {
		return localwire.HandshakeResponse{}, err
	}
	_ = connection.SetDeadline(time.Now().Add(2 * time.Second))
	client, err := localwire.NewClient(context.Background(), connection, localwire.ClientOptions{
		Mode: localwire.LifecycleMode, Supported: localwire.LifecycleVersions,
		Metadata: localwire.PeerMetadata{Build: buildinfo.Version},
	})
	if err != nil {
		connection.Close()
		return localwire.HandshakeResponse{}, err
	}
	defer client.Close()
	callContext, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	err = client.Call(callContext, method, nil, destination)
	return client.Handshake(), err
}

func (c *unixControl) Close() error {
	var err error
	c.once.Do(func() {
		err = c.listener.Close()
		c.connectionsMu.Lock()
		c.closed = true
		for connection := range c.connections {
			err = errors.Join(err, connection.Close())
		}
		c.connections = make(map[net.Conn]struct{})
		c.connectionsMu.Unlock()
		removeErr := os.Remove(c.path)
		if removeErr != nil && !errors.Is(removeErr, os.ErrNotExist) {
			err = errors.Join(err, removeErr)
		}
	})
	return err
}
