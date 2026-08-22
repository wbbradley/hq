package domain

import (
	"context"
	"errors"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

var (
	ErrNotFound       = errors.New("message not found")
	ErrAlreadyHandled = errors.New("message has already been handled")
	ErrNotReady       = errors.New("no message is ready")
	ErrClaimed        = errors.New("message is being delivered by another process")
)

type Claim struct {
	MessageID          string   `json:"message_id,omitempty"`
	ReplyTo            string   `json:"reply_to,omitempty"`
	ExcludeReplyTo     []string `json:"exclude_reply_to,omitempty"`
	RecipientMailboxID string   `json:"recipient_mailbox_id,omitempty"`
}

type Peer struct {
	InstallationID string   `json:"installation_id"`
	SignerKeyID    string   `json:"signer_key_id"`
	Name           string   `json:"name,omitempty"`
	Relays         []string `json:"relays,omitempty"`
	Trusted        bool     `json:"trusted"`
}

type HumanAccount struct {
	ID                    string `json:"id"`
	Label                 string `json:"label"`
	CreatorInstallationID string `json:"creator_installation_id"`
	CreatorSignerKeyID    string `json:"creator_signer_key_id"`
	LocalInstallationID   string `json:"local_installation_id"`
	Creator               bool   `json:"creator"`
}

type HumanDevice struct {
	AccountID      string   `json:"account_id"`
	InstallationID string   `json:"installation_id"`
	SignerKeyID    string   `json:"signer_key_id"`
	Label          string   `json:"label"`
	Relays         []string `json:"relays,omitempty"`
	State          string   `json:"state"`
}

type HumanInviteRequest struct {
	InstallationID string   `json:"installation_id"`
	SignerKeyID    string   `json:"signer_key_id"`
	Name           string   `json:"name"`
	Relays         []string `json:"relays,omitempty"`
}

type PairingBundle struct {
	Version                int      `json:"version"`
	AccountID              string   `json:"account_id"`
	AccountLabel           string   `json:"account_label"`
	CreatorInstallationID  string   `json:"creator_installation_id"`
	CreatorSignerKeyID     string   `json:"creator_signer_key_id"`
	CreatorRelays          []string `json:"creator_relays"`
	TargetInstallationID   string   `json:"target_installation_id"`
	TargetSignerKeyID      string   `json:"target_signer_key_id"`
	TargetLabel            string   `json:"target_label"`
	TargetRelays           []string `json:"target_relays"`
	AccountCreationEvent   []byte   `json:"account_creation_event"`
	DeviceGrantEvent       []byte   `json:"device_grant_event"`
	AccountAuthorityEvents [][]byte `json:"account_authority_events"`
}

type RelayConfig struct {
	URL          string `json:"url"`
	Read         bool   `json:"read"`
	Write        bool   `json:"write"`
	RequireAuth  bool   `json:"require_auth"`
	UnsafeNoAuth bool   `json:"unsafe_no_auth"`
}

type RelayHealth struct {
	URL           string     `json:"url"`
	Connected     bool       `json:"connected"`
	Authenticated bool       `json:"authenticated"`
	LastEOSE      *time.Time `json:"last_eose,omitempty"`
	LastEvent     *time.Time `json:"last_event,omitempty"`
	LastError     string     `json:"last_error,omitempty"`
}

type NetworkStatus struct {
	Queued                int           `json:"queued"`
	RelayAccepted         int           `json:"relay_accepted"`
	Rejected              int           `json:"rejected"`
	Unresolved            int           `json:"unresolved"`
	Unsupported           int           `json:"unsupported"`
	Staged                int           `json:"staged"`
	Quarantined           int           `json:"quarantined"`
	AccountMembers        int           `json:"account_members"`
	PendingAccountFanout  int           `json:"pending_account_fanout"`
	InvalidAccountTraffic int           `json:"invalid_account_traffic"`
	RevokedDeviceTraffic  int           `json:"revoked_device_traffic"`
	Relays                []RelayHealth `json:"relays"`
}

type Operations interface {
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
}

type Store interface {
	Operations
	Synchronize(context.Context) error
	Close() error
}
