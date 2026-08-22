package store

import (
	"context"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

type Signer interface {
	Sign(context.Context, event.Content, time.Time) (event.SignedEvent, error)
}
type EventLog interface {
	AppendCanonical(context.Context, []event.SignedEvent) error
	Rebuild(context.Context) error
}
type Reducer interface {
	Reduce([][]byte, event.Policy) event.State
}
type ReducerFunc func([][]byte, event.Policy) event.State

func (f ReducerFunc) Reduce(raw [][]byte, policy event.Policy) event.State { return f(raw, policy) }

type OutboundJob struct {
	EventID                 string
	RecipientInstallationID string
	ExactCanonicalBytes     []byte
	State                   string
}
type Outbox interface {
	PendingOutbox(context.Context, int) ([]OutboundJob, error)
}

var (
	ErrNotFound       = domain.ErrNotFound
	ErrAlreadyHandled = domain.ErrAlreadyHandled
	ErrNotReady       = domain.ErrNotReady
	ErrClaimed        = domain.ErrClaimed
)

type Claim = domain.Claim
type Peer = domain.Peer
type HumanAccount = domain.HumanAccount
type HumanDevice = domain.HumanDevice
type HumanInviteRequest = domain.HumanInviteRequest
type PairingBundle = domain.PairingBundle
type RelayConfig = domain.RelayConfig
type RelayHealth = domain.RelayHealth
type NetworkStatus = domain.NetworkStatus

type Store interface {
	HumanMailbox(context.Context) (model.Mailbox, error)
	ResolveMailbox(context.Context, model.SessionIdentity, model.RepositoryContext) (model.Mailbox, error)
	FindMailboxes(context.Context, model.RepositoryContext) ([]model.Mailbox, error)
	Create(context.Context, model.Message) error
	Reply(context.Context, string, model.Message) error
	Get(context.Context, string) (model.Message, error)
	List(context.Context, model.Filter) ([]model.Message, error)
	Archive(context.Context, string) error
	Restore(context.Context, string) error
	Claim(context.Context, Claim, string) (model.Message, error)
	Complete(context.Context, string, string) error
	Release(context.Context, string, string) error
	TrustPeer(context.Context, Peer) error
	DistrustPeer(context.Context, string) error
	ListPeers(context.Context) ([]Peer, error)
	HumanAccount(context.Context) (HumanAccount, error)
	HumanDevices(context.Context) ([]HumanDevice, error)
	CreateHumanInvite(context.Context, HumanInviteRequest) (PairingBundle, error)
	JoinHumanInvite(context.Context, []byte) error
	RevokeHumanDevice(context.Context, string) error
	SetMailboxShare(context.Context, string, string, bool) error
	AddRelay(context.Context, RelayConfig) error
	RemoveRelay(context.Context, string) error
	ListRelays(context.Context) ([]RelayConfig, error)
	NetworkStatus(context.Context) (NetworkStatus, error)
	Rebuild(context.Context) error
	Close() error
}
