package domain

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

var (
	ErrNotFound       = errors.New("message not found")
	ErrAlreadyHandled = errors.New("message has already been handled")
	ErrNotReady       = errors.New("no message is ready")
	ErrClaimed        = errors.New("message is being delivered by another process")
	ErrAgentNotFound  = errors.New("named agent not found")
	ErrAgentRetired   = errors.New("named agent is retired")
	ErrAgentNameTaken = errors.New("agent name is permanently reserved")
	ErrMailboxNamed   = errors.New("mailbox already belongs to a named agent")
	ErrAgentOwned     = errors.New("named agent is owned by another process")
)

type Claim struct {
	MessageID            string               `json:"message_id,omitempty"`
	ReplyTo              string               `json:"reply_to,omitempty"`
	ExcludeReplyTo       []string             `json:"exclude_reply_to,omitempty"`
	RecipientMailboxID   string               `json:"recipient_mailbox_id,omitempty"`
	Purpose              model.MessagePurpose `json:"purpose,omitempty"`
	CorrelationProvider  string               `json:"correlation_provider,omitempty"`
	CorrelationSessionID string               `json:"correlation_session_id,omitempty"`
	UnthreadedOnly       bool                 `json:"unthreaded_only,omitempty"`
}

type NamedAgent struct {
	Name              string                  `json:"name"`
	MailboxID         string                  `json:"mailbox_id"`
	Retired           bool                    `json:"retired"`
	Harness           string                  `json:"harness,omitempty"`
	CurrentSessionID  string                  `json:"current_session_id,omitempty"`
	CurrentThreadName string                  `json:"current_thread_name,omitempty"`
	Context           model.RepositoryContext `json:"context,omitempty"`
	Active            bool                    `json:"active"`
	LeaseExpiresAt    *time.Time              `json:"lease_expires_at,omitempty"`
	LastActiveAt      *time.Time              `json:"last_active_at,omitempty"`
	AssignedProjectID string                  `json:"assigned_project_id,omitempty"`
	Idle              bool                    `json:"idle"`
}

// AgentSession is the durable, installation-private projection of one harness
// session bound to a named agent. Runtime state is deliberately not part of
// this record; AgentActive describes the owning agent at query time.
type AgentSession struct {
	AgentName      string                  `json:"agent_name"`
	MailboxID      string                  `json:"mailbox_id"`
	Harness        string                  `json:"harness"`
	SessionID      string                  `json:"session_id"`
	ThreadName     string                  `json:"thread_name,omitempty"`
	Context        model.RepositoryContext `json:"context"`
	CreatedAt      time.Time               `json:"created_at"`
	LastSelectedAt time.Time               `json:"last_selected_at"`
	Current        bool                    `json:"current"`
	AgentActive    bool                    `json:"agent_active"`
}

type AgentOwnershipConflict struct {
	Name      string
	ExpiresAt time.Time
}

func (e *AgentOwnershipConflict) Error() string {
	return fmt.Sprintf("%s: %s until %s", ErrAgentOwned, e.Name, e.ExpiresAt.Format(time.RFC3339))
}
func (e *AgentOwnershipConflict) Unwrap() error { return ErrAgentOwned }

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
	CreateNamedAgent(context.Context, string, string) (NamedAgent, error)
	GetNamedAgent(context.Context, string) (NamedAgent, error)
	ListNamedAgents(context.Context) ([]NamedAgent, error)
	ListNamedAgentSessions(context.Context, string) ([]AgentSession, error)
	RenameNamedAgentSession(context.Context, string, model.SessionIdentity, string) (AgentSession, error)
	RetireNamedAgent(context.Context, string) error
	SelectNamedAgentSession(context.Context, string, model.SessionIdentity, model.RepositoryContext) (NamedAgent, error)
	AcquireNamedAgent(context.Context, string, string, time.Duration) (NamedAgent, error)
	RenewNamedAgent(context.Context, string, string, time.Duration) (NamedAgent, error)
	ReleaseNamedAgent(context.Context, string, string) error
	Create(context.Context, model.Message) error
	Reply(context.Context, string, model.Message) error
	Get(context.Context, string) (model.Message, error)
	List(context.Context, model.Filter) ([]model.Message, error)
	ListConversations(context.Context, model.ConversationFilter) (model.ConversationPage, error)
	ListConversationHistory(context.Context, model.ConversationHistoryFilter) (model.MessagePage, error)
	ListConversationEntries(context.Context, model.ConversationHistoryFilter) (ConversationEntryPage, error)
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
	ProjectOperations
	Synchronize(context.Context) error
	Close() error
}
