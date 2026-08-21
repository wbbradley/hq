package hqclient

import (
	"bytes"
	"context"
	"encoding/json"
	"net"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

type reconnectFixture struct {
	t           *testing.T
	lifetime    context.Context
	mu          sync.Mutex
	handlers    []localwire.Handler
	connections []net.Conn
	connects    int
}

func (f *reconnectFixture) connect(context.Context) (*localwire.Client, error) {
	f.mu.Lock()
	index := f.connects
	f.connects++
	handler := f.handlers[min(index, len(f.handlers)-1)]
	f.mu.Unlock()
	server, err := localwire.NewServer(localwire.ServerOptions{
		Metadata: localwire.PeerMetadata{Build: "test-node"},
		Modes: map[localwire.HandshakeMode]localwire.ModeConfig{
			localwire.DomainMode: {Supported: localwire.DomainVersions, Handler: handler},
		},
	})
	if err != nil {
		return nil, err
	}
	clientConnection, serverConnection := net.Pipe()
	f.mu.Lock()
	f.connections = append(f.connections, serverConnection)
	f.mu.Unlock()
	go server.ServeConn(context.Background(), serverConnection)
	return localwire.NewClient(f.lifetime, clientConnection, localwire.ClientOptions{
		Mode: localwire.DomainMode, Supported: localwire.DomainVersions,
		Metadata: localwire.PeerMetadata{Build: "test-client"},
	})
}

func (f *reconnectFixture) disconnect(index int) {
	f.mu.Lock()
	connection := f.connections[index]
	f.mu.Unlock()
	connection.Close()
}

func newReconnectClient(t *testing.T, handlers ...localwire.Handler) (*Client, *reconnectFixture) {
	t.Helper()
	lifetime, cancel := context.WithCancel(context.Background())
	fixture := &reconnectFixture{t: t, lifetime: lifetime, handlers: handlers}
	client := newClient(lifetime, cancel)
	client.connect = fixture.connect
	wireClient, err := fixture.connect(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	client.attach(wireClient)
	t.Cleanup(func() { client.Close() })
	return client, fixture
}

func TestMutationReconnectPreservesExactRequest(t *testing.T) {
	firstRequest := make(chan json.RawMessage, 1)
	secondRequest := make(chan json.RawMessage, 1)
	firstEntered := make(chan struct{})
	client, fixture := newReconnectClient(t,
		func(ctx context.Context, _ *localwire.Session, method string, raw json.RawMessage) (any, *localwire.RPCError) {
			if method == domainrpc.CreateMethod {
				firstRequest <- append(json.RawMessage(nil), raw...)
				close(firstEntered)
				<-ctx.Done()
			}
			return nil, nil
		},
		func(_ context.Context, _ *localwire.Session, method string, raw json.RawMessage) (any, *localwire.RPCError) {
			if method == domainrpc.CreateMethod {
				secondRequest <- append(json.RawMessage(nil), raw...)
			}
			return nil, nil
		},
	)
	done := make(chan error, 1)
	go func() { done <- client.Create(context.Background(), model.Message{ID: "message", Body: "retry"}) }()
	<-firstEntered
	fixture.disconnect(0)
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("mutation did not reconcile after reconnect")
	}
	if first, second := <-firstRequest, <-secondRequest; !bytes.Equal(first, second) {
		t.Fatalf("retried request changed:\n%s\n%s", first, second)
	}
	fixture.mu.Lock()
	connects := fixture.connects
	fixture.mu.Unlock()
	if connects != 2 {
		t.Fatalf("connection attempts = %d", connects)
	}
}

func TestConcurrentCallsShareOneReconnect(t *testing.T) {
	const callers = 4
	entered := make(chan struct{}, callers)
	client, fixture := newReconnectClient(t,
		func(ctx context.Context, _ *localwire.Session, method string, _ json.RawMessage) (any, *localwire.RPCError) {
			if method == domainrpc.ListMethod {
				entered <- struct{}{}
				<-ctx.Done()
			}
			return nil, nil
		},
		func(context.Context, *localwire.Session, string, json.RawMessage) (any, *localwire.RPCError) {
			return []model.Message{}, nil
		},
	)
	errors := make(chan error, callers)
	for range callers {
		go func() {
			_, err := client.List(context.Background(), model.Filter{})
			errors <- err
		}()
	}
	for range callers {
		select {
		case <-entered:
		case <-time.After(2 * time.Second):
			t.Fatal("concurrent calls did not reach the failed connection")
		}
	}
	fixture.disconnect(0)
	for range callers {
		select {
		case err := <-errors:
			if err != nil {
				t.Fatal(err)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("concurrent call did not reconcile after reconnect")
		}
	}
	fixture.mu.Lock()
	connects := fixture.connects
	fixture.mu.Unlock()
	if connects != 2 {
		t.Fatalf("connection attempts = %d", connects)
	}
}

func TestSubscriptionResubscribesBeforeFullSnapshotSignal(t *testing.T) {
	handler := func(revision uint64) localwire.Handler {
		return func(_ context.Context, _ *localwire.Session, method string, _ json.RawMessage) (any, *localwire.RPCError) {
			if method == domainrpc.SubscribeChangesMethod {
				return domainrpc.SubscribeChangesResponse{Revision: revision}, nil
			}
			return nil, nil
		}
	}
	client, fixture := newReconnectClient(t, handler(3), handler(9))
	subscription, err := client.Subscribe(context.Background(), domain.TopicMessages)
	if err != nil {
		t.Fatal(err)
	}
	fixture.disconnect(0)
	select {
	case change := <-subscription.Changes():
		if !change.FullSnapshot || change.Revision != 9 {
			t.Fatalf("reconnect change = %#v", change)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("subscription did not resubscribe")
	}
	if got := (ConnectionState{Phase: ConnectionDrift}).Diagnostic(); got == "" {
		t.Fatal("build drift has no diagnostic")
	}
}

func TestConnectionStateDiagnosticsPreserveIncompatibilityAction(t *testing.T) {
	incompatible := localwire.NewIncompatibility(localwire.VersionRange{Min: 1, Max: 1}, localwire.VersionRange{Min: 2, Max: 2})
	state := ConnectionState{Phase: ConnectionIncompatible, Err: incompatible}
	if diagnostic := state.Diagnostic(); !strings.Contains(diagnostic, "upgrade this HQ client") {
		t.Fatalf("incompatibility diagnostic = %q", diagnostic)
	}
	if diagnostic := (ConnectionState{Phase: ConnectionDrift}).Diagnostic(); !strings.Contains(diagnostic, "restart") {
		t.Fatalf("drift diagnostic = %q", diagnostic)
	}
}
