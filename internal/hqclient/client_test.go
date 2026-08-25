package hqclient

import (
	"context"
	"encoding/json"
	"errors"
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

func TestClientCallsEveryDomainMethod(t *testing.T) {
	t.Setenv("HQ_CLIENT_WAKE_ENV", "expected")
	var lock sync.Mutex
	var methods []string
	messageEnvironmentCalls := 0
	var listFilter model.Filter
	var conversationFilter model.ConversationFilter
	var historyFilter model.ConversationHistoryFilter
	var activityFilter domain.HarnessActivityFilter
	handler := func(_ context.Context, _ *localwire.Session, method string, raw json.RawMessage) (any, *localwire.RPCError) {
		lock.Lock()
		methods = append(methods, method)
		lock.Unlock()
		if method == domainrpc.CreateMethod || method == domainrpc.ReplyMethod {
			var request struct {
				Environment []string `json:"environment"`
			}
			if json.Unmarshal(raw, &request) == nil {
				for _, entry := range request.Environment {
					if entry == "HQ_CLIENT_WAKE_ENV=expected" {
						messageEnvironmentCalls++
					}
				}
			}
		}
		switch method {
		case domainrpc.HumanMailboxMethod, domainrpc.ResolveMailboxMethod:
			return model.Mailbox{}, nil
		case domainrpc.FindMailboxesMethod:
			return []model.Mailbox{}, nil
		case domainrpc.CreateNamedAgentMethod, domainrpc.GetNamedAgentMethod, domainrpc.SelectAgentSessionMethod, domainrpc.AcquireAgentMethod, domainrpc.RenewAgentMethod:
			return domain.NamedAgent{}, nil
		case domainrpc.ListNamedAgentsMethod:
			return []domain.NamedAgent{}, nil
		case domainrpc.ListAgentSessionsMethod:
			return []domain.AgentSession{}, nil
		case domainrpc.RenameAgentSessionMethod:
			return domain.AgentSession{}, nil
		case domainrpc.LaunchHarnessAgentMethod, domainrpc.StopHarnessAgentMethod, domainrpc.HarnessRuntimeMethod:
			return domain.HarnessRuntime{}, nil
		case domainrpc.GetMethod, domainrpc.ClaimMethod:
			return model.Message{}, nil
		case domainrpc.ListMethod:
			var request domainrpc.FilterRequest
			if err := json.Unmarshal(raw, &request); err != nil {
				return nil, &localwire.RPCError{Code: localwire.CodeInvalidRequest, Message: err.Error()}
			}
			listFilter = request.Filter
			return []model.Message{}, nil
		case domainrpc.ListConversationsMethod:
			var request domainrpc.ConversationFilterRequest
			if err := json.Unmarshal(raw, &request); err != nil {
				return nil, &localwire.RPCError{Code: localwire.CodeInvalidRequest, Message: err.Error()}
			}
			conversationFilter = request.Filter
			return model.ConversationPage{Conversations: []model.ConversationSummary{{Key: model.ConversationKey{CounterpartyMailboxID: "agent", CodexThreadID: "thread"}}}, NextCursor: "summary-next"}, nil
		case domainrpc.ConversationHistoryMethod:
			var request domainrpc.ConversationHistoryRequest
			if err := json.Unmarshal(raw, &request); err != nil {
				return nil, &localwire.RPCError{Code: localwire.CodeInvalidRequest, Message: err.Error()}
			}
			historyFilter = request.Filter
			return model.MessagePage{Messages: []model.Message{{ID: "history-message"}}, NextCursor: "history-next"}, nil
		case domainrpc.ListHarnessActivitiesMethod:
			var request domainrpc.HarnessActivityFilterRequest
			if err := json.Unmarshal(raw, &request); err != nil {
				return nil, &localwire.RPCError{Code: localwire.CodeInvalidRequest, Message: err.Error()}
			}
			activityFilter = request.Filter
			return []domain.HarnessActivity{{ItemID: "activity-result"}}, nil
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
		case domainrpc.SubscribeChangesMethod:
			return domainrpc.SubscribeChangesResponse{}, nil
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
	_, _ = client.CreateNamedAgent(ctx, "fred", "")
	_, _ = client.GetNamedAgent(ctx, "fred")
	_, _ = client.ListNamedAgents(ctx)
	_, _ = client.ListNamedAgentSessions(ctx, "fred")
	_, _ = client.RenameNamedAgentSession(ctx, "fred", model.SessionIdentity{Harness: "codex", ExternalSessionID: "thread"}, "Build auth")
	_ = client.RetireNamedAgent(ctx, "fred")
	_, _ = client.SelectNamedAgentSession(ctx, "fred", model.SessionIdentity{}, model.RepositoryContext{})
	_, _ = client.AcquireNamedAgent(ctx, "fred", "owner", time.Minute)
	_, _ = client.RenewNamedAgent(ctx, "fred", "owner", time.Minute)
	_ = client.ReleaseNamedAgent(ctx, "fred", "owner")
	_, _ = client.LaunchHarnessAgent(ctx, domain.HarnessLaunchRequest{RequestID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d60", AgentName: "fred"})
	_, _ = client.StopHarnessAgent(ctx, "fred")
	_, _ = client.HarnessAgentRuntime(ctx, "fred")
	_ = client.Create(ctx, model.Message{})
	_ = client.Reply(ctx, "original", model.Message{})
	_, _ = client.Get(ctx, "message")
	wantListFilter := model.Filter{CounterpartyMailboxID: "counterparty", ThreadID: "hq-thread", CodexThreadID: "codex-thread", CodexTurnID: "codex-turn"}
	_, _ = client.List(ctx, wantListFilter)
	wantConversationFilter := model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Cursor: "summary-cursor", Limit: 17}
	conversationResult, _ := client.ListConversations(ctx, wantConversationFilter)
	wantHistoryFilter := model.ConversationHistoryFilter{Key: model.ConversationKey{CounterpartyMailboxID: "agent", CodexThreadID: "thread"}, Cursor: "history-cursor", Limit: 23}
	historyResult, _ := client.ListConversationHistory(ctx, wantHistoryFilter)
	_ = client.UpsertHarnessActivity(ctx, domain.HarnessActivity{ItemID: "activity-input"})
	wantActivityFilter := domain.HarnessActivityFilter{MailboxID: "agent", Harness: "fake", SessionID: "session", Limit: 31}
	activityResult, _ := client.ListHarnessActivities(ctx, wantActivityFilter)
	_ = client.Archive(ctx, "message")
	_ = client.Restore(ctx, "message")
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
	_, _ = client.Subscribe(ctx, domain.TopicMessages)
	want := []string{
		domainrpc.HumanMailboxMethod, domainrpc.ResolveMailboxMethod, domainrpc.FindMailboxesMethod,
		domainrpc.CreateNamedAgentMethod, domainrpc.GetNamedAgentMethod, domainrpc.ListNamedAgentsMethod, domainrpc.ListAgentSessionsMethod, domainrpc.RenameAgentSessionMethod,
		domainrpc.RetireNamedAgentMethod, domainrpc.SelectAgentSessionMethod, domainrpc.AcquireAgentMethod, domainrpc.RenewAgentMethod, domainrpc.ReleaseAgentMethod,
		domainrpc.LaunchHarnessAgentMethod, domainrpc.StopHarnessAgentMethod, domainrpc.HarnessRuntimeMethod,
		domainrpc.CreateMethod, domainrpc.ReplyMethod, domainrpc.GetMethod, domainrpc.ListMethod, domainrpc.ListConversationsMethod, domainrpc.ConversationHistoryMethod,
		domainrpc.UpsertHarnessActivityMethod, domainrpc.ListHarnessActivitiesMethod,
		domainrpc.ArchiveMethod, domainrpc.RestoreMethod, domainrpc.ClaimMethod, domainrpc.CompleteMethod, domainrpc.ReleaseMethod,
		domainrpc.TrustPeerMethod, domainrpc.DistrustPeerMethod, domainrpc.ListPeersMethod,
		domainrpc.HumanAccountMethod, domainrpc.HumanDevicesMethod, domainrpc.CreateHumanInviteMethod,
		domainrpc.JoinHumanInviteMethod, domainrpc.RevokeHumanDeviceMethod, domainrpc.SetMailboxShareMethod,
		domainrpc.AddRelayMethod, domainrpc.RemoveRelayMethod, domainrpc.ListRelaysMethod,
		domainrpc.NetworkStatusMethod, domainrpc.SynchronizeMethod, domainrpc.SubscribeChangesMethod,
	}
	lock.Lock()
	defer lock.Unlock()
	if len(methods) != len(want) {
		t.Fatalf("methods = %#v", methods)
	}
	if messageEnvironmentCalls != 2 {
		t.Fatalf("message environment calls = %d; methods=%s", messageEnvironmentCalls, strings.Join(methods, ","))
	}
	for index := range want {
		if methods[index] != want[index] {
			t.Fatalf("method %d = %q, want %q", index, methods[index], want[index])
		}
	}
	if listFilter != wantListFilter {
		t.Fatalf("list filter = %#v; want %#v", listFilter, wantListFilter)
	}
	if conversationFilter != wantConversationFilter || historyFilter != wantHistoryFilter {
		t.Fatalf("conversation filters = %#v / %#v; want %#v / %#v", conversationFilter, historyFilter, wantConversationFilter, wantHistoryFilter)
	}
	if activityFilter != wantActivityFilter || len(activityResult) != 1 || activityResult[0].ItemID != "activity-result" {
		t.Fatalf("activity result/filter = %#v / %#v", activityResult, activityFilter)
	}
	if len(conversationResult.Conversations) != 1 || conversationResult.NextCursor != "summary-next" || len(historyResult.Messages) != 1 || historyResult.NextCursor != "history-next" {
		t.Fatalf("conversation results = %#v / %#v", conversationResult, historyResult)
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
