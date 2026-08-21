package hqclient

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

func TestClientCallsEveryDomainMethod(t *testing.T) {
	var lock sync.Mutex
	var methods []string
	handler := func(_ context.Context, _ *localwire.Session, method string, _ json.RawMessage) (any, *localwire.RPCError) {
		lock.Lock()
		methods = append(methods, method)
		lock.Unlock()
		switch method {
		case domainrpc.HumanMailboxMethod, domainrpc.ResolveMailboxMethod:
			return model.Mailbox{}, nil
		case domainrpc.FindMailboxesMethod:
			return []model.Mailbox{}, nil
		case domainrpc.GetMethod, domainrpc.ClaimMethod:
			return model.Message{}, nil
		case domainrpc.ListMethod:
			return []model.Message{}, nil
		case domainrpc.ListPeersMethod:
			return []domain.Peer{}, nil
		case domainrpc.HumanAccountMethod:
			return domain.HumanAccount{}, nil
		case domainrpc.HumanDevicesMethod:
			return []domain.HumanDevice{}, nil
		case domainrpc.CreateHumanInviteMethod:
			return domain.PairingBundle{}, nil
		case domainrpc.ListRelaysMethod:
			return []domain.RelayConfig{}, nil
		case domainrpc.NetworkStatusMethod:
			return domain.NetworkStatus{}, nil
		default:
			return nil, nil
		}
	}
	client, stop := testClient(t, handler)
	defer stop()
	ctx := context.Background()
	_, _ = client.HumanMailbox(ctx)
	_, _ = client.ResolveMailbox(ctx, model.SessionIdentity{}, model.RepositoryContext{})
	_, _ = client.FindMailboxes(ctx, model.RepositoryContext{})
	_ = client.Create(ctx, model.Message{})
	_ = client.Reply(ctx, "original", model.Message{})
	_, _ = client.Get(ctx, "message")
	_, _ = client.List(ctx, model.Filter{})
	_ = client.Archive(ctx, "message")
	_, _ = client.Claim(ctx, domain.Claim{}, "token")
	_ = client.Complete(ctx, "message", "token")
	_ = client.Release(ctx, "message", "token")
	_ = client.TrustPeer(ctx, domain.Peer{})
	_ = client.DistrustPeer(ctx, "installation")
	_, _ = client.ListPeers(ctx)
	_, _ = client.HumanAccount(ctx)
	_, _ = client.HumanDevices(ctx)
	_, _ = client.CreateHumanInvite(ctx, domain.HumanInviteRequest{})
	_ = client.JoinHumanInvite(ctx, []byte("bundle"))
	_ = client.RevokeHumanDevice(ctx, "installation")
	_ = client.SetMailboxShare(ctx, "mailbox", "installation", true)
	_ = client.AddRelay(ctx, domain.RelayConfig{})
	_ = client.RemoveRelay(ctx, "wss://relay.example")
	_, _ = client.ListRelays(ctx)
	_, _ = client.NetworkStatus(ctx)
	_ = client.Synchronize(ctx)
	want := []string{
		domainrpc.HumanMailboxMethod, domainrpc.ResolveMailboxMethod, domainrpc.FindMailboxesMethod,
		domainrpc.CreateMethod, domainrpc.ReplyMethod, domainrpc.GetMethod, domainrpc.ListMethod,
		domainrpc.ArchiveMethod, domainrpc.ClaimMethod, domainrpc.CompleteMethod, domainrpc.ReleaseMethod,
		domainrpc.TrustPeerMethod, domainrpc.DistrustPeerMethod, domainrpc.ListPeersMethod,
		domainrpc.HumanAccountMethod, domainrpc.HumanDevicesMethod, domainrpc.CreateHumanInviteMethod,
		domainrpc.JoinHumanInviteMethod, domainrpc.RevokeHumanDeviceMethod, domainrpc.SetMailboxShareMethod,
		domainrpc.AddRelayMethod, domainrpc.RemoveRelayMethod, domainrpc.ListRelaysMethod,
		domainrpc.NetworkStatusMethod, domainrpc.SynchronizeMethod,
	}
	lock.Lock()
	defer lock.Unlock()
	if len(methods) != len(want) {
		t.Fatalf("methods = %#v", methods)
	}
	for index := range want {
		if methods[index] != want[index] {
			t.Fatalf("method %d = %q, want %q", index, methods[index], want[index])
		}
	}
}

func TestClientRestoresDomainSentinelErrors(t *testing.T) {
	client, stop := testClient(t, func(context.Context, *localwire.Session, string, json.RawMessage) (any, *localwire.RPCError) {
		return nil, domainrpc.EncodeError(domain.ErrNotFound)
	})
	defer stop()
	if _, err := client.Get(context.Background(), "missing"); !errors.Is(err, domain.ErrNotFound) {
		t.Fatalf("error = %v", err)
	}
}

func testClient(t *testing.T, handler localwire.Handler) (*Client, func()) {
	t.Helper()
	server, err := localwire.NewServer(localwire.ServerOptions{
		Modes: map[localwire.HandshakeMode]localwire.ModeConfig{
			localwire.DomainMode: {Supported: localwire.DomainVersions, Handler: handler},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	clientConnection, serverConnection := net.Pipe()
	done := make(chan error, 1)
	go func() { done <- server.ServeConn(context.Background(), serverConnection) }()
	wireClient, err := localwire.NewClient(context.Background(), clientConnection, localwire.ClientOptions{Mode: localwire.DomainMode, Supported: localwire.DomainVersions})
	if err != nil {
		t.Fatal(err)
	}
	client := New(wireClient)
	return client, func() {
		_ = client.Close()
		select {
		case <-done:
		case <-time.After(time.Second):
			t.Fatal("domain test server did not stop")
		}
	}
}
