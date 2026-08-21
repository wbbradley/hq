package localwire

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"strings"
	"sync"
	"testing"
	"time"
)

func testServer(t *testing.T, options ServerOptions) (*Client, func()) {
	t.Helper()
	server, err := NewServer(options)
	if err != nil {
		t.Fatal(err)
	}
	clientConnection, serverConnection := net.Pipe()
	serverDone := make(chan error, 1)
	go func() { serverDone <- server.ServeConn(context.Background(), serverConnection) }()
	client, err := NewClient(context.Background(), clientConnection, ClientOptions{
		Mode: DomainMode, Supported: VersionRange{Min: 1, Max: 3},
		Metadata: PeerMetadata{Build: "client-build"}, NotificationBuffer: 8,
	})
	if err != nil {
		t.Fatal(err)
	}
	return client, func() {
		_ = client.Close()
		select {
		case <-serverDone:
		case <-time.After(time.Second):
			t.Fatal("server did not stop")
		}
	}
}

func TestHandshakeAllowsBinaryDriftAndNegotiatesRange(t *testing.T) {
	client, stop := testServer(t, ServerOptions{
		Metadata: PeerMetadata{Build: "server-build", InstanceID: "node-one", StartedAt: time.Unix(10, 0).UTC()},
		Modes:    map[HandshakeMode]ModeConfig{DomainMode: {Supported: VersionRange{Min: 2, Max: 4}, Handler: echoHandler}},
	})
	defer stop()
	if got := client.Handshake(); got.Version != 3 || got.Server.InstanceID != "node-one" {
		t.Fatalf("handshake = %#v", got)
	}
	if !client.BinaryDrift() {
		t.Fatal("different builds were not reported as drift")
	}
}

func TestHandshakeReturnsTypedIncompatibility(t *testing.T) {
	server, err := NewServer(ServerOptions{
		Metadata: PeerMetadata{Build: "new-server"},
		Modes:    map[HandshakeMode]ModeConfig{DomainMode: {Supported: VersionRange{Min: 4, Max: 5}, Handler: echoHandler}},
	})
	if err != nil {
		t.Fatal(err)
	}
	clientConnection, serverConnection := net.Pipe()
	go server.ServeConn(context.Background(), serverConnection)
	_, err = NewClient(context.Background(), clientConnection, ClientOptions{Mode: DomainMode, Supported: VersionRange{Min: 1, Max: 2}, Metadata: PeerMetadata{Build: "old-client"}})
	var incompatible *IncompatibilityError
	if !errors.As(err, &incompatible) || incompatible.Data.StaleSide != "client" {
		t.Fatalf("error = %#v", err)
	}
}

func TestStableLifecycleLaneWorksWhenDomainVersionsAreIncompatible(t *testing.T) {
	server, err := NewServer(ServerOptions{
		Metadata: PeerMetadata{Build: "old-node"},
		Modes: map[HandshakeMode]ModeConfig{
			DomainMode:    {Supported: VersionRange{Min: 4, Max: 5}, Handler: echoHandler},
			LifecycleMode: {Supported: LifecycleVersions, Handler: echoHandler},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	dial := func(mode HandshakeMode, supported VersionRange) (*Client, error) {
		clientConnection, serverConnection := net.Pipe()
		go server.ServeConn(context.Background(), serverConnection)
		return NewClient(context.Background(), clientConnection, ClientOptions{Mode: mode, Supported: supported})
	}
	if _, err := dial(DomainMode, VersionRange{Min: 1, Max: 2}); err == nil {
		t.Fatal("incompatible domain handshake succeeded")
	}
	client, err := dial(LifecycleMode, LifecycleVersions)
	if err != nil {
		t.Fatalf("lifecycle handshake failed: %v", err)
	}
	defer client.Close()
	var response string
	if err := client.Call(context.Background(), "lifecycle/status", "running", &response); err != nil || response != "running" {
		t.Fatalf("lifecycle call = %q, %v", response, err)
	}
	if client.Handshake().Version != CurrentLifecycleVersion {
		t.Fatalf("lifecycle version = %d", client.Handshake().Version)
	}
}

func TestConcurrentCallsCorrelateOutOfOrderResponsesAndNotifications(t *testing.T) {
	handler := func(ctx context.Context, session *Session, _ string, raw json.RawMessage) (any, *RPCError) {
		var request struct {
			Value string `json:"value"`
			Delay int    `json:"delay_ms"`
		}
		if err := json.Unmarshal(raw, &request); err != nil {
			return nil, &RPCError{Code: CodeInvalidRequest, Message: err.Error()}
		}
		_ = session.NotifySubscription(request.Value, "changed", map[string]string{"value": request.Value})
		select {
		case <-time.After(time.Duration(request.Delay) * time.Millisecond):
		case <-ctx.Done():
			return nil, &RPCError{Code: CodeInternal, Message: ctx.Err().Error()}
		}
		return map[string]string{"value": request.Value}, nil
	}
	client, stop := testServer(t, ServerOptions{
		Metadata: PeerMetadata{Build: "client-build"},
		Modes:    map[HandshakeMode]ModeConfig{DomainMode: {Supported: VersionRange{Min: 1, Max: 3}, Handler: handler}},
	})
	defer stop()
	var wait sync.WaitGroup
	errorsSeen := make(chan error, 2)
	for _, request := range []struct {
		value string
		delay int
	}{{value: "slow", delay: 30}, {value: "fast", delay: 1}} {
		wait.Add(1)
		go func() {
			defer wait.Done()
			var response struct {
				Value string `json:"value"`
			}
			if err := client.Call(context.Background(), "echo", map[string]any{"value": request.value, "delay_ms": request.delay}, &response); err != nil {
				errorsSeen <- err
			} else if response.Value != request.value {
				errorsSeen <- errors.New("response correlated to the wrong request")
			}
		}()
	}
	wait.Wait()
	close(errorsSeen)
	for err := range errorsSeen {
		t.Fatal(err)
	}
	for range 2 {
		select {
		case notice := <-client.Notifications():
			if notice.Method != "changed" || notice.SubscriptionID == "" {
				t.Fatalf("notification = %#v", notice)
			}
		case <-time.After(time.Second):
			t.Fatal("notification not delivered")
		}
	}
}

func TestServerRejectsRequestBeforeHandshake(t *testing.T) {
	server, err := NewServer(ServerOptions{Modes: map[HandshakeMode]ModeConfig{DomainMode: {Supported: DomainVersions, Handler: echoHandler}}})
	if err != nil {
		t.Fatal(err)
	}
	clientConnection, serverConnection := net.Pipe()
	done := make(chan error, 1)
	go func() { done <- server.ServeConn(context.Background(), serverConnection) }()
	codec := NewCodec(clientConnection, clientConnection, 1024)
	if err := codec.Write(Envelope{Kind: RequestKind, Version: 1, ID: "early", Method: "echo", Params: json.RawMessage("null")}); err != nil {
		t.Fatal(err)
	}
	reply, err := codec.Read()
	if err != nil || reply.Kind != ErrorKind || reply.Error.Code != CodeInvalidRequest {
		t.Fatalf("reply = %#v, %v", reply, err)
	}
	if err := <-done; err == nil || !strings.Contains(err.Error(), "before handshake") {
		t.Fatalf("server error = %v", err)
	}
}

func TestNotificationBackpressureDisconnectsSlowClient(t *testing.T) {
	handler := func(_ context.Context, session *Session, _ string, _ json.RawMessage) (any, *RPCError) {
		for index := range 3 {
			if err := session.Notify("changed", map[string]int{"revision": index}); err != nil {
				break
			}
		}
		return nil, nil
	}
	server, err := NewServer(ServerOptions{Modes: map[HandshakeMode]ModeConfig{DomainMode: {Supported: DomainVersions, Handler: handler}}})
	if err != nil {
		t.Fatal(err)
	}
	clientConnection, serverConnection := net.Pipe()
	go server.ServeConn(context.Background(), serverConnection)
	client, err := NewClient(context.Background(), clientConnection, ClientOptions{Mode: DomainMode, Supported: DomainVersions, NotificationBuffer: 1})
	if err != nil {
		t.Fatal(err)
	}
	_ = client.Call(context.Background(), "flood", nil, nil)
	select {
	case <-client.Done():
	case <-time.After(time.Second):
		t.Fatal("slow client was not disconnected")
	}
	var rpcErr *RPCError
	if !errors.As(client.Err(), &rpcErr) || rpcErr.Code != CodeNotificationOverflow {
		t.Fatalf("client error = %v", client.Err())
	}
}

func TestDisconnectFailsPendingCall(t *testing.T) {
	clientConnection, serverConnection := net.Pipe()
	go func() {
		codec := NewCodec(serverConnection, serverConnection, 1024)
		request, _ := codec.Read()
		var handshake HandshakeRequest
		_ = json.Unmarshal(request.Params, &handshake)
		result, _ := json.Marshal(HandshakeResponse{Mode: handshake.Mode, Version: 1, Supported: DomainVersions})
		_ = codec.Write(Envelope{Kind: HandshakeKind, Result: result})
		_, _ = codec.Read()
		_ = serverConnection.Close()
	}()
	client, err := NewClient(context.Background(), clientConnection, ClientOptions{Mode: DomainMode, Supported: DomainVersions})
	if err != nil {
		t.Fatal(err)
	}
	if err := client.Call(context.Background(), "wait", nil, nil); err == nil || !strings.Contains(err.Error(), "read local-wire") {
		t.Fatalf("pending call error = %v", err)
	}
}

func TestServerAppliesRequestDeadline(t *testing.T) {
	handler := func(ctx context.Context, _ *Session, _ string, _ json.RawMessage) (any, *RPCError) {
		<-ctx.Done()
		return nil, &RPCError{Code: CodeInternal, Message: ctx.Err().Error()}
	}
	client, stop := testServer(t, ServerOptions{
		Modes:          map[HandshakeMode]ModeConfig{DomainMode: {Supported: VersionRange{Min: 1, Max: 3}, Handler: handler}},
		RequestTimeout: 20 * time.Millisecond,
	})
	defer stop()
	started := time.Now()
	err := client.Call(context.Background(), "bounded", nil, nil)
	if err == nil || !strings.Contains(err.Error(), context.DeadlineExceeded.Error()) {
		t.Fatalf("deadline error = %v", err)
	}
	if elapsed := time.Since(started); elapsed > time.Second {
		t.Fatalf("request deadline took %s", elapsed)
	}
}

func echoHandler(_ context.Context, _ *Session, _ string, raw json.RawMessage) (any, *RPCError) {
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {
		return nil, &RPCError{Code: CodeInvalidRequest, Message: err.Error()}
	}
	return value, nil
}
