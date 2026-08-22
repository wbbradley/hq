package domainrpc

import (
	"context"
	"encoding/json"
	"errors"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

type recordingOperations struct {
	called string
	calls  int
	err    error
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
func (s *recordingOperations) RetireNamedAgent(context.Context, string) error {
	return s.record(RetireNamedAgentMethod)
}
func (s *recordingOperations) SelectNamedAgentSession(context.Context, string, model.SessionIdentity, model.RepositoryContext) (domain.NamedAgent, error) {
	return domain.NamedAgent{}, s.record(SelectAgentSessionMethod)
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
func (s *recordingOperations) List(context.Context, model.Filter) ([]model.Message, error) {
	return nil, s.record(ListMethod)
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
		{RetireNamedAgentMethod, NamedAgentRequest{MutationID: mutationID}},
		{SelectAgentSessionMethod, AgentSessionRequest{MutationID: mutationID}},
		{AcquireAgentMethod, AgentOwnershipRequest{MutationID: mutationID}},
		{RenewAgentMethod, AgentOwnershipRequest{MutationID: mutationID}},
		{ReleaseAgentMethod, AgentOwnershipRequest{MutationID: mutationID}},
		{CreateMethod, MessageRequest{MutationID: mutationID}},
		{ReplyMethod, ReplyRequest{MutationID: mutationID}},
		{GetMethod, IDRequest{}},
		{ListMethod, FilterRequest{}},
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
	for _, sentinel := range []error{domain.ErrNotFound, domain.ErrAlreadyHandled, domain.ErrNotReady, domain.ErrClaimed} {
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
