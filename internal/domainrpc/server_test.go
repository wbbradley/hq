package domainrpc

import (
	"context"
	"encoding/json"
	"errors"
	"testing"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

type recordingOperations struct {
	called string
	err    error
}

func (s *recordingOperations) record(method string) error { s.called = method; return s.err }
func (s *recordingOperations) HumanMailbox(context.Context) (model.Mailbox, error) {
	return model.Mailbox{}, s.record(HumanMailboxMethod)
}
func (s *recordingOperations) ResolveMailbox(context.Context, model.SessionIdentity, model.RepositoryContext) (model.Mailbox, error) {
	return model.Mailbox{}, s.record(ResolveMailboxMethod)
}
func (s *recordingOperations) FindMailboxes(context.Context, model.RepositoryContext) ([]model.Mailbox, error) {
	return nil, s.record(FindMailboxesMethod)
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
	tests := []struct {
		method string
		value  any
	}{
		{HumanMailboxMethod, nil},
		{ResolveMailboxMethod, ResolveMailboxRequest{}},
		{FindMailboxesMethod, RepositoryRequest{}},
		{CreateMethod, MessageRequest{}},
		{ReplyMethod, ReplyRequest{}},
		{GetMethod, IDRequest{}},
		{ListMethod, FilterRequest{}},
		{ArchiveMethod, IDRequest{}},
		{ClaimMethod, ClaimRequest{}},
		{CompleteMethod, LeaseRequest{}},
		{ReleaseMethod, LeaseRequest{}},
		{TrustPeerMethod, PeerRequest{}},
		{DistrustPeerMethod, InstallationRequest{}},
		{ListPeersMethod, nil},
		{HumanAccountMethod, nil},
		{HumanDevicesMethod, nil},
		{CreateHumanInviteMethod, HumanInviteRequest{}},
		{JoinHumanInviteMethod, PairingRequest{}},
		{RevokeHumanDeviceMethod, InstallationRequest{}},
		{SetMailboxShareMethod, MailboxShareRequest{}},
		{AddRelayMethod, RelayRequest{}},
		{RemoveRelayMethod, URLRequest{}},
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
