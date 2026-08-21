//go:build !windows

package syncer

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"sync"
	"syscall"
	"time"

	"github.com/wbbradley/hq/internal/buildinfo"
	"github.com/wbbradley/hq/internal/localwire"
)

type unixControl struct {
	listener      net.Listener
	path          string
	once          sync.Once
	connectionsMu sync.Mutex
	connections   map[net.Conn]struct{}
	closed        bool
	paths         RuntimePaths
	instanceID    string
}

func startControl(ctx context.Context, paths RuntimePaths, wake chan<- struct{}, stop, restart context.CancelFunc, status func() string, metadata localwire.PeerMetadata) (io.Closer, error) {
	if err := paths.EnsureDirectories(); err != nil {
		return nil, err
	}
	listener, err := listenLocalSocket(paths.Socket)
	if err != nil {
		return nil, err
	}
	if err := os.Chmod(paths.Socket, 0o600); err != nil {
		listener.Close()
		os.Remove(paths.Socket)
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
		os.Remove(paths.Socket)
		return nil, err
	}
	handle := &unixControl{listener: listener, path: paths.Socket, connections: make(map[net.Conn]struct{}), paths: paths, instanceID: metadata.InstanceID}
	if err := writeRuntimeMetadata(paths, metadata); err != nil {
		_ = handle.Close()
		return nil, err
	}
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

func listenLocalSocket(path string) (net.Listener, error) {
	info, err := os.Lstat(path)
	switch {
	case err == nil && info.Mode()&os.ModeSocket == 0:
		return nil, fmt.Errorf("local HQ socket path %s is not a socket", path)
	case err == nil:
		connection, dialErr := net.DialTimeout("unix", path, 100*time.Millisecond)
		if dialErr == nil {
			_ = connection.Close()
			return nil, ErrNodeOwned
		}
		if !errors.Is(dialErr, syscall.ECONNREFUSED) && !errors.Is(dialErr, syscall.ENOENT) {
			return nil, fmt.Errorf("probe existing local HQ socket: %w", dialErr)
		}
		if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
			return nil, fmt.Errorf("remove stale local HQ socket: %w", err)
		}
	case errors.Is(err, os.ErrNotExist):
	default:
		return nil, fmt.Errorf("inspect local HQ socket: %w", err)
	}
	return net.Listen("unix", path)
}

func controlCommand(databasePath, method string, destination any) (localwire.HandshakeResponse, error) {
	paths, err := ResolveRuntimePaths(databasePath)
	if err != nil {
		return localwire.HandshakeResponse{}, err
	}
	connection, err := net.DialTimeout("unix", paths.Socket, 500*time.Millisecond)
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
		err = errors.Join(err, removeRuntimeMetadata(c.paths, c.instanceID))
	})
	return err
}
