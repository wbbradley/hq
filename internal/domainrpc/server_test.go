package domainrpc

import (
	"context"
	"encoding/json"
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

type recordingOperations struct {
	called             string
	calls              int
	err                error
	listFilter         model.Filter
	conversationFilter model.ConversationFilter
	historyFilter      model.ConversationHistoryFilter
}

type recordingRuntime struct {
	called           string
	wakeMessages     []model.Message
	wakeEnvironments [][]string
}

func (r *recordingRuntime) LaunchHarnessAgent(_ context.Context, request domain.HarnessLaunchRequest) (domain.HarnessRuntime, error) {
	r.called = LaunchHarnessAgentMethod
	return domain.HarnessRuntime{AgentName: request.AgentName, Phase: domain.HarnessRuntimeRunning}, nil
}
func (r *recordingRuntime) StopHarnessAgent(_ context.Context, name string) (domain.HarnessRuntime, error) {
	r.called = StopHarnessAgentMethod
	return domain.HarnessRuntime{AgentName: name, Phase: domain.HarnessRuntimeOffline}, nil
}
func (r *recordingRuntime) HarnessAgentRuntime(_ context.Context, name string) (domain.HarnessRuntime, error) {
	r.called = HarnessRuntimeMethod
	return domain.HarnessRuntime{AgentName: name, Phase: domain.HarnessRuntimeOffline}, nil
}
func (r *recordingRuntime) ActivateHarnessProject(_ context.Context, request domain.ProjectHarnessActivationRequest) (domain.ProjectHarnessActivation, error) {
	r.called = ActivateHarnessProjectMethod
	return domain.ProjectHarnessActivation{Runtime: domain.HarnessRuntime{AgentName: request.AgentName, Phase: domain.HarnessRuntimeRunning}}, nil
}
func (r *recordingRuntime) CloseHarnessProject(_ context.Context, request domain.ProjectHarnessCloseRequest) (domain.Project, error) {
	r.called = CloseHarnessProjectMethod
	return domain.Project{ID: request.ProjectID}, nil
}
func (r *recordingRuntime) HandoffHarnessProject(_ context.Context, request domain.ProjectHarnessHandoffRequest) (domain.ProjectHarnessActivation, error) {
	r.called = HandoffHarnessProjectMethod
	return domain.ProjectHarnessActivation{}, nil
}
func (r *recordingRuntime) RetireHarnessAgent(context.Context, domain.HarnessRetireAgentRequest) error {
	r.called = RetireHarnessAgentMethod
	return nil
}
func (r *recordingRuntime) WakeHarnessAgent(message model.Message, environment []string) {
	r.wakeMessages = append(r.wakeMessages, message)
	r.wakeEnvironments = append(r.wakeEnvironments, append([]string(nil), environment...))
}

func (s *recordingOperations) MutationResult(_ context.Context, mutation domain.Mutation) (json.RawMessage, bool, error) {
	if s.called == mutation.Method {
		return json.RawMessage(`null`), true, nil
	}
	return nil, false, nil
}
func (*recordingOperations) CurrentRevision(context.Context) (uint64, error) { return 0, nil }

func (s *recordingOperations) record(method string) error { s.called = method; s.calls++; return s.err }
func (s *recordingOperations) HumanMailbox(context.Context) (model.Mailbox, error) {
	return model.Mailbox{}, s.record(HumanMailboxMethod)
}
func (s *recordingOperations) ResolveMailbox(context.Context, model.SessionIdentity, model.RepositoryContext) (model.Mailbox, error) {
	return model.Mailbox{}, s.record(ResolveMailboxMethod)
}
func (s *recordingOperations) FindMailboxes(context.Context, model.RepositoryContext) ([]model.Mailbox, error) {
	return nil, s.record(FindMailboxesMethod)
}
func (s *recordingOperations) CreateNamedAgent(context.Context, string, string) (domain.NamedAgent, error) {
	return domain.NamedAgent{}, s.record(CreateNamedAgentMethod)
}
func (s *recordingOperations) GetNamedAgent(context.Context, string) (domain.NamedAgent, error) {
	return domain.NamedAgent{}, s.record(GetNamedAgentMethod)
}
func (s *recordingOperations) ListNamedAgents(context.Context) ([]domain.NamedAgent, error) {
	return nil, s.record(ListNamedAgentsMethod)
}
func (s *recordingOperations) ListNamedAgentSessions(context.Context, string) ([]domain.AgentSession, error) {
	return nil, s.record(ListAgentSessionsMethod)
}
func (s *recordingOperations) RetireNamedAgent(context.Context, string) error {
	return s.record(RetireNamedAgentMethod)
}
func (s *recordingOperations) SelectNamedAgentSession(context.Context, string, model.SessionIdentity, model.RepositoryContext) (domain.NamedAgent, error) {
	return domain.NamedAgent{}, s.record(SelectAgentSessionMethod)
}
func (s *recordingOperations) RenameNamedAgentSession(context.Context, string, model.SessionIdentity, string) (domain.AgentSession, error) {
	return domain.AgentSession{}, s.record(RenameAgentSessionMethod)
}
func (s *recordingOperations) AcquireNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error) {
	return domain.NamedAgent{}, s.record(AcquireAgentMethod)
}
func (s *recordingOperations) RenewNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error) {
	return domain.NamedAgent{}, s.record(RenewAgentMethod)
}
func (s *recordingOperations) ReleaseNamedAgent(context.Context, string, string) error {
	return s.record(ReleaseAgentMethod)
}
func (s *recordingOperations) Create(context.Context, model.Message) error {
	return s.record(CreateMethod)
}
func (s *recordingOperations) Reply(context.Context, string, model.Message) error {
	return s.record(ReplyMethod)
}
func (s *recordingOperations) Get(context.Context, string) (model.Message, error) {
	return model.Message{}, s.record(GetMethod)
}

func (s *recordingOperations) List(_ context.Context, filter model.Filter) ([]model.Message, error) {
	s.listFilter = filter
	return nil, s.record(ListMethod)
}
func (s *recordingOperations) ListConversations(_ context.Context, filter model.ConversationFilter) (model.ConversationPage, error) {
	s.conversationFilter = filter
	return model.ConversationPage{}, s.record(ListConversationsMethod)
}
func (s *recordingOperations) ListConversationHistory(_ context.Context, filter model.ConversationHistoryFilter) (model.MessagePage, error) {
	s.historyFilter = filter
	return model.MessagePage{}, s.record(ConversationHistoryMethod)
}
func (s *recordingOperations) Archive(context.Context, string) error { return s.record(ArchiveMethod) }
func (s *recordingOperations) Restore(context.Context, string) error { return s.record(RestoreMethod) }
func (s *recordingOperations) Claim(context.Context, domain.Claim, string) (model.Message, error) {
	return model.Message{}, s.record(ClaimMethod)
}
func (s *recordingOperations) Complete(context.Context, string, string) error {
	return s.record(CompleteMethod)
}
func (s *recordingOperations) Release(context.Context, string, string) error {
	return s.record(ReleaseMethod)
}
func (s *recordingOperations) TrustPeer(context.Context, domain.Peer) error {
	return s.record(TrustPeerMethod)
}
func (s *recordingOperations) DistrustPeer(context.Context, string) error {
	return s.record(DistrustPeerMethod)
}
func (s *recordingOperations) ListPeers(context.Context) ([]domain.Peer, error) {
	return nil, s.record(ListPeersMethod)
}
func (s *recordingOperations) HumanAccount(context.Context) (domain.HumanAccount, error) {
	return domain.HumanAccount{}, s.record(HumanAccountMethod)
}
func (s *recordingOperations) HumanDevices(context.Context) ([]domain.HumanDevice, error) {
	return nil, s.record(HumanDevicesMethod)
}
func (s *recordingOperations) CreateHumanInvite(context.Context, domain.HumanInviteRequest) (domain.PairingBundle, error) {
	return domain.PairingBundle{}, s.record(CreateHumanInviteMethod)
}
func (s *recordingOperations) JoinHumanInvite(context.Context, []byte) error {
	return s.record(JoinHumanInviteMethod)
}
func (s *recordingOperations) RevokeHumanDevice(context.Context, string) error {
	return s.record(RevokeHumanDeviceMethod)
}
func (s *recordingOperations) SetMailboxShare(context.Context, string, string, bool) error {
	return s.record(SetMailboxShareMethod)
}
func (s *recordingOperations) AddRelay(context.Context, domain.RelayConfig) error {
	return s.record(AddRelayMethod)
}
func (s *recordingOperations) RemoveRelay(context.Context, string) error {
	return s.record(RemoveRelayMethod)
}
func (s *recordingOperations) ListRelays(context.Context) ([]domain.RelayConfig, error) {
	return nil, s.record(ListRelaysMethod)
}
func (s *recordingOperations) NetworkStatus(context.Context) (domain.NetworkStatus, error) {
	return domain.NetworkStatus{}, s.record(NetworkStatusMethod)
}
func (s *recordingOperations) CreateProject(context.Context, domain.CreateProjectRequest) (domain.Project, error) {
	return domain.Project{}, s.record(CreateProjectMethod)
}
func (s *recordingOperations) GetProject(context.Context, string) (domain.Project, error) {
	return domain.Project{}, s.record(GetProjectMethod)
}
func (s *recordingOperations) ListProjects(context.Context, bool) ([]domain.Project, error) {
	return nil, s.record(ListProjectsMethod)
}
func (s *recordingOperations) ListProjectThreads(context.Context, string) ([]domain.ProjectThread, error) {
	return nil, s.record(ListProjectThreadsMethod)
}
func (s *recordingOperations) OpenProject(context.Context, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(OpenProjectMethod)
}
func (s *recordingOperations) BeginCloseProject(context.Context, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(BeginCloseProjectMethod)
}
func (s *recordingOperations) FinalizeCloseProject(context.Context, string, string, bool, string) (domain.Project, error) {
	return domain.Project{}, s.record(FinalizeCloseProjectMethod)
}
func (s *recordingOperations) SetProjectArchived(context.Context, string, string, bool) (domain.Project, error) {
	return domain.Project{}, s.record(ArchiveProjectMethod)
}
func (s *recordingOperations) UpdateProjectMetadata(context.Context, string, string, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(UpdateProjectMethod)
}
func (s *recordingOperations) AddProjectPath(context.Context, string, string, domain.ProjectPathInput, bool) (domain.Project, error) {
	return domain.Project{}, s.record(AddProjectPathMethod)
}
func (s *recordingOperations) RemoveProjectResource(context.Context, string, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(RemoveProjectResourceMethod)
}
func (s *recordingOperations) ReplaceProjectPath(context.Context, string, string, string, domain.ProjectPathInput) (domain.Project, error) {
	return domain.Project{}, s.record(ReplaceProjectPathMethod)
}
func (s *recordingOperations) SetProjectPrimaryResource(context.Context, string, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(SetProjectPrimaryMethod)
}
func (s *recordingOperations) CheckProjectResource(context.Context, string, string) (domain.ProjectResource, error) {
	return domain.ProjectResource{}, s.record(CheckProjectResourceMethod)
}
func (s *recordingOperations) AssignProject(context.Context, string, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(AssignProjectMethod)
}
func (s *recordingOperations) ActivateProjectAssignment(context.Context, string, string, domain.ActivateProjectAssignmentRequest) (domain.Project, error) {
	return domain.Project{}, s.record(ActivateProjectMethod)
}
func (s *recordingOperations) AbortProjectAssignment(context.Context, string, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(AbortProjectAssignmentMethod)
}
func (s *recordingOperations) BlockProjectAssignment(context.Context, string, string, string) (domain.Project, error) {
	return domain.Project{}, s.record(BlockProjectAssignmentMethod)
}
func (s *recordingOperations) UnassignProject(context.Context, string, string, bool, string) (domain.Project, error) {
	return domain.Project{}, s.record(UnassignProjectMethod)
}

func TestServiceDispatchesEveryDomainMethod(t *testing.T) {
	mutationID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d60"
	tests := []struct {
		method string
		value  any
	}{
		{HumanMailboxMethod, nil},
		{ResolveMailboxMethod, ResolveMailboxRequest{MutationID: mutationID}},
		{FindMailboxesMethod, RepositoryRequest{}},
		{CreateNamedAgentMethod, NamedAgentRequest{MutationID: mutationID}},
		{GetNamedAgentMethod, NamedAgentRequest{}},
		{ListNamedAgentsMethod, nil},
		{ListAgentSessionsMethod, NamedAgentRequest{}},
		{RenameAgentSessionMethod, AgentSessionRenameRequest{MutationID: mutationID}},
		{RetireNamedAgentMethod, NamedAgentRequest{MutationID: mutationID}},
		{SelectAgentSessionMethod, AgentSessionRequest{MutationID: mutationID}},
		{AcquireAgentMethod, AgentOwnershipRequest{MutationID: mutationID}},
		{RenewAgentMethod, AgentOwnershipRequest{MutationID: mutationID}},
		{ReleaseAgentMethod, AgentOwnershipRequest{MutationID: mutationID}},
		{CreateMethod, MessageRequest{MutationID: mutationID}},
		{ReplyMethod, ReplyRequest{MutationID: mutationID}},
		{GetMethod, IDRequest{}},
		{ListMethod, FilterRequest{}},
		{ListConversationsMethod, ConversationFilterRequest{}},
		{ConversationHistoryMethod, ConversationHistoryRequest{}},
		{ArchiveMethod, MutationIDRequest{MutationID: mutationID}},
		{RestoreMethod, MutationIDRequest{MutationID: mutationID}},
		{ClaimMethod, ClaimRequest{MutationID: mutationID}},
		{CompleteMethod, LeaseRequest{MutationID: mutationID}},
		{ReleaseMethod, LeaseRequest{MutationID: mutationID}},
		{TrustPeerMethod, PeerRequest{MutationID: mutationID}},
		{DistrustPeerMethod, MutationInstallationRequest{MutationID: mutationID}},
		{ListPeersMethod, nil},
		{HumanAccountMethod, nil},
		{HumanDevicesMethod, nil},
		{CreateHumanInviteMethod, HumanInviteRequest{MutationID: mutationID}},
		{JoinHumanInviteMethod, PairingRequest{MutationID: mutationID}},
		{RevokeHumanDeviceMethod, MutationInstallationRequest{MutationID: mutationID}},
		{SetMailboxShareMethod, MailboxShareRequest{MutationID: mutationID}},
		{AddRelayMethod, RelayRequest{MutationID: mutationID}},
		{RemoveRelayMethod, MutationURLRequest{MutationID: mutationID}},
		{ListRelaysMethod, nil},
		{NetworkStatusMethod, nil},
		{CreateProjectMethod, CreateProjectRequest{MutationID: mutationID}},
		{GetProjectMethod, ProjectRequest{}},
		{ListProjectsMethod, ListProjectsRequest{}},
		{ListProjectThreadsMethod, ProjectRequest{}},
		{OpenProjectMethod, ProjectRequest{MutationID: mutationID}},
		{BeginCloseProjectMethod, ProjectRequest{MutationID: mutationID}},
		{FinalizeCloseProjectMethod, FinalizeCloseProjectRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{ArchiveProjectMethod, ArchiveProjectRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{UpdateProjectMethod, UpdateProjectRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{AddProjectPathMethod, ProjectPathRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{RemoveProjectResourceMethod, ProjectResourceRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{ReplaceProjectPathMethod, ReplaceProjectPathRequest{ProjectResourceRequest: ProjectResourceRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}}},
		{SetProjectPrimaryMethod, ProjectResourceRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{CheckProjectResourceMethod, CheckProjectResourceRequest{}},
		{AssignProjectMethod, AssignProjectRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{ActivateProjectMethod, ActivateProjectRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{AbortProjectAssignmentMethod, EndProjectAssignmentRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{BlockProjectAssignmentMethod, EndProjectAssignmentRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
		{UnassignProjectMethod, EndProjectAssignmentRequest{ProjectRequest: ProjectRequest{MutationID: mutationID}}},
	}
	for _, test := range tests {
		t.Run(test.method, func(t *testing.T) {
			operations := &recordingOperations{}
			service := Service{Store: operations}
			raw, err := json.Marshal(test.value)
			if err != nil {
				t.Fatal(err)
			}
			_, rpcErr := service.Handle(context.Background(), nil, test.method, raw)
			if rpcErr != nil || operations.called != test.method {
				t.Fatalf("called=%q error=%v", operations.called, rpcErr)
			}
		})
	}
	operations := &recordingOperations{}
	service := Service{Store: operations, Synchronize: func(context.Context) error { operations.called = SynchronizeMethod; return nil }}
	if _, rpcErr := service.Handle(context.Background(), nil, SynchronizeMethod, nil); rpcErr != nil || operations.called != SynchronizeMethod {
		t.Fatalf("sync called=%q error=%v", operations.called, rpcErr)
	}
}

func TestCommittedHumanMessagesAttemptNamedAgentWakeWithTransientEnvironment(t *testing.T) {
	mutationID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d60"
	message := model.Message{ID: "message-1", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: "named-mailbox", Body: "continue"}
	runtime := &recordingRuntime{}
	operations := &recordingOperations{}
	service := Service{Store: operations, Runtime: runtime}
	request := MessageRequest{MutationID: mutationID, Message: message, Environment: []string{"PATH=/sender/bin", "TOKEN=transient"}}
	raw, err := json.Marshal(request)
	if err != nil {
		t.Fatal(err)
	}
	if _, rpcErr := service.Handle(context.Background(), nil, CreateMethod, raw); rpcErr != nil {
		t.Fatal(rpcErr)
	}
	// A replay that finds the durable mutation receipt must repair the wake
	// attempt too, covering a daemon exit between commit and wake dispatch.
	if _, rpcErr := service.Handle(context.Background(), nil, CreateMethod, raw); rpcErr != nil {
		t.Fatal(rpcErr)
	}
	if len(runtime.wakeMessages) != 2 || runtime.wakeMessages[0].ID != message.ID || strings.Join(runtime.wakeEnvironments[0], "|") != "PATH=/sender/bin|TOKEN=transient" {
		t.Fatalf("wake messages=%#v environments=%#v", runtime.wakeMessages, runtime.wakeEnvironments)
	}
}

func TestRPCProjectReplyIsAcceptedAndAssignmentDispatchable(t *testing.T) {
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, err := identity.KeyPath(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	ctx := context.Background()
	if _, err := database.CreateNamedAgent(ctx, "rpc-agent", ""); err != nil {
		t.Fatal(err)
	}
	project, err := database.CreateProject(ctx, domain.CreateProjectRequest{Name: "RPC reply", Open: true})
	if err != nil {
		t.Fatal(err)
	}
	project, err = database.AssignProject(ctx, project.ID, project.HeadEventID, "rpc-agent")
	if err == nil {
		project, err = database.ActivateProjectAssignment(ctx, project.ID, project.HeadEventID, domain.ActivateProjectAssignmentRequest{Harness: "codex", ExternalThread: "rpc-thread", LaunchDirectory: t.TempDir()})
	}
	if err != nil {
		t.Fatal(err)
	}
	outputID := "019d0000-0000-7000-8000-000000000061"
	binding := domain.ProjectOutputBinding{ProjectID: project.ID, AssignmentID: project.Assignment.ID, AgentName: "rpc-agent", ProjectThreadID: project.Assignment.SelectedThreadID, ExternalThreadID: "rpc-thread", RuntimeState: "connected"}
	if err := database.CreateProjectOutput(ctx, binding, model.Message{ID: outputID, SenderMailboxID: project.MailboxID, RecipientMailboxID: model.HumanMailboxID, Body: "RPC output", CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	request := ReplyRequest{
		MutationID: uuid.NewString(), OriginalID: outputID,
		Reply: model.Message{ID: "019d0000-0000-7000-8000-000000000062", SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: project.MailboxID, Body: "RPC follow-up", CreatedAt: time.Now().UTC()},
	}
	raw, err := json.Marshal(request)
	if err != nil {
		t.Fatal(err)
	}
	runtime := &recordingRuntime{}
	service := Service{Store: database, Runtime: runtime}
	if _, rpcErr := service.Handle(ctx, nil, ReplyMethod, raw); rpcErr != nil {
		t.Fatal(rpcErr)
	}
	after, err := database.GetProject(ctx, project.ID)
	if err != nil || after.HeadEventID == project.HeadEventID {
		t.Fatalf("RPC reply project = %#v, %v", after, err)
	}
	stored, err := database.Get(ctx, request.Reply.ID)
	if err != nil || stored.Purpose != model.MessagePurposeProjectInput || stored.ReplyTo == nil || *stored.ReplyTo != outputID {
		t.Fatalf("RPC reply = %#v, %v", stored, err)
	}
	delivery, err := database.ClaimProjectMessage(ctx, project.ID, project.Assignment.ID, project.Assignment.SelectedThreadID, "rpc-owner")
	if err != nil || delivery.Message.ID != request.Reply.ID || delivery.AgentName != "rpc-agent" || delivery.ExternalThreadID != "rpc-thread" {
		t.Fatalf("RPC reply delivery = %#v, %v", delivery, err)
	}
	if _, rpcErr := service.Handle(ctx, nil, ReplyMethod, raw); rpcErr != nil {
		t.Fatal(rpcErr)
	}
	replayed, err := database.GetProject(ctx, project.ID)
	if err != nil || replayed.HeadEventID != after.HeadEventID || len(runtime.wakeMessages) != 2 {
		t.Fatalf("RPC replay project=%#v wakes=%d: %v", replayed, len(runtime.wakeMessages), err)
	}
}

func TestServicePassesStructuredConversationFilter(t *testing.T) {
	operations := &recordingOperations{}
	service := Service{Store: operations}
	want := model.Filter{
		CounterpartyMailboxID: "counterparty",
		ThreadID:              "hq-thread",
		CodexThreadID:         "codex-thread",
		CodexTurnID:           "codex-turn",
	}
	raw, err := json.Marshal(FilterRequest{Filter: want})
	if err != nil {
		t.Fatal(err)
	}
	if _, rpcErr := service.Handle(context.Background(), nil, ListMethod, raw); rpcErr != nil {
		t.Fatal(rpcErr)
	}
	if operations.listFilter != want {
		t.Fatalf("list filter = %#v; want %#v", operations.listFilter, want)
	}
}

func TestServicePassesConversationPageRequests(t *testing.T) {
	operations := &recordingOperations{}
	service := Service{Store: operations}
	conversationFilter := model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Cursor: "summary-cursor", Limit: 17}
	raw, _ := json.Marshal(ConversationFilterRequest{Filter: conversationFilter})
	if _, rpcErr := service.Handle(context.Background(), nil, ListConversationsMethod, raw); rpcErr != nil || operations.conversationFilter != conversationFilter {
		t.Fatalf("conversation filter = %#v, error=%v", operations.conversationFilter, rpcErr)
	}
	historyFilter := model.ConversationHistoryFilter{Key: model.ConversationKey{CounterpartyMailboxID: "agent", CodexThreadID: "thread"}, Cursor: "history-cursor", Limit: 23}
	raw, _ = json.Marshal(ConversationHistoryRequest{Filter: historyFilter})
	if _, rpcErr := service.Handle(context.Background(), nil, ConversationHistoryMethod, raw); rpcErr != nil || operations.historyFilter != historyFilter {
		t.Fatalf("history filter = %#v, error=%v", operations.historyFilter, rpcErr)
	}
}

func TestServiceDispatchesLocalHarnessRuntimeWithoutMutationReceipts(t *testing.T) {
	operations := &recordingOperations{}
	runtime := &recordingRuntime{}
	service := Service{Store: operations, Runtime: runtime}
	for _, test := range []struct {
		method          string
		value           any
		allowsNilResult bool
	}{
		{LaunchHarnessAgentMethod, domain.HarnessLaunchRequest{RequestID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d60", AgentName: "fred"}, false},
		{ActivateHarnessProjectMethod, domain.ProjectHarnessActivationRequest{}, false},
		{CloseHarnessProjectMethod, domain.ProjectHarnessCloseRequest{}, false},
		{HandoffHarnessProjectMethod, domain.ProjectHarnessHandoffRequest{}, false},
		{RetireHarnessAgentMethod, domain.HarnessRetireAgentRequest{}, true},
		{StopHarnessAgentMethod, HarnessAgentRequest{Name: "fred"}, false},
		{HarnessRuntimeMethod, HarnessAgentRequest{Name: "fred"}, false},
	} {
		raw, _ := json.Marshal(test.value)
		result, rpcErr := service.Handle(context.Background(), nil, test.method, raw)
		if rpcErr != nil || (!test.allowsNilResult && result == nil) || runtime.called != test.method || operations.called != "" {
			t.Fatalf("%s result=%#v rpc=%v runtime=%q store=%q", test.method, result, rpcErr, runtime.called, operations.called)
		}
	}
}

func TestServiceRejectsMalformedAndUnknownRequests(t *testing.T) {
	service := Service{Store: &recordingOperations{}}
	if _, rpcErr := service.Handle(context.Background(), nil, CreateMethod, json.RawMessage(`{"message":{},"unknown":true}`)); rpcErr == nil || rpcErr.Code != localwire.CodeInvalidRequest {
		t.Fatalf("malformed error = %#v", rpcErr)
	}
	if _, rpcErr := service.Handle(context.Background(), nil, "database/query", nil); rpcErr == nil || rpcErr.Code != localwire.CodeMethodNotFound {
		t.Fatalf("unknown error = %#v", rpcErr)
	}
}

func TestMutationMetadataIsRequiredCanonicalAndReplayed(t *testing.T) {
	mutationID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d60"
	first, mutating, err := mutationForRequest(CreateMethod, json.RawMessage(`{"mutation_id":"`+mutationID+`","message":{"body":"hello"}}`))
	if err != nil || !mutating {
		t.Fatalf("first mutation = %#v, %t, %v", first, mutating, err)
	}
	second, _, err := mutationForRequest(CreateMethod, json.RawMessage(`{"message":{"body":"hello"},"mutation_id":"`+mutationID+`"}`))
	if err != nil || first.RequestDigest != second.RequestDigest {
		t.Fatalf("canonical digests = %q, %q, %v", first.RequestDigest, second.RequestDigest, err)
	}
	if _, _, err := mutationForRequest(CreateMethod, json.RawMessage(`{"message":{}}`)); err == nil {
		t.Fatal("missing mutation ID was accepted")
	}

	operations := &recordingOperations{}
	service := Service{Store: operations}
	raw := json.RawMessage(`{"mutation_id":"` + mutationID + `","message":{}}`)
	if _, rpcErr := service.Handle(context.Background(), nil, CreateMethod, raw); rpcErr != nil {
		t.Fatal(rpcErr)
	}
	if _, rpcErr := service.Handle(context.Background(), nil, CreateMethod, raw); rpcErr != nil {
		t.Fatal(rpcErr)
	}
	if operations.calls != 1 {
		t.Fatalf("mutation dispatch calls = %d", operations.calls)
	}
}

func TestDomainSentinelErrorsRoundTrip(t *testing.T) {
	for _, sentinel := range []error{domain.ErrNotFound, domain.ErrAlreadyHandled, domain.ErrNotReady, domain.ErrClaimed, harness.ErrUnknownProvider, harness.ErrProviderUnavailable, harness.ErrCapabilityUnavailable} {
		t.Run(sentinel.Error(), func(t *testing.T) {
			wireError := EncodeError(sentinel)
			decoded := DecodeError(wireError)
			if !errors.Is(decoded, sentinel) {
				t.Fatalf("decoded = %v; want %v", decoded, sentinel)
			}
		})
	}
	custom := errors.New("validation failed")
	if wireError := EncodeError(custom); wireError.Code != CodeDomain || wireError.Message != custom.Error() {
		t.Fatalf("custom wire error = %#v", wireError)
	}
}
