package domainrpc

import (
	"errors"
	"fmt"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

const (
	HumanMailboxMethod       = "mailbox/human"
	ResolveMailboxMethod     = "mailbox/resolve"
	FindMailboxesMethod      = "mailbox/list"
	CreateNamedAgentMethod   = "agent/create"
	GetNamedAgentMethod      = "agent/get"
	ListNamedAgentsMethod    = "agent/list"
	ListAgentSessionsMethod  = "agent/session/list"
	RenameAgentSessionMethod = "agent/session/rename"
	RetireNamedAgentMethod   = "agent/retire"
	SelectAgentSessionMethod = "agent/session/select"
	AcquireAgentMethod       = "agent/ownership/acquire"
	RenewAgentMethod         = "agent/ownership/renew"
	ReleaseAgentMethod       = "agent/ownership/release"
	LaunchCodexAgentMethod   = "codex/launch"
	StopCodexAgentMethod     = "codex/stop"
	CodexRuntimeMethod       = "codex/status"
	CreateMethod             = "message/create"
	ReplyMethod              = "message/reply"
	GetMethod                = "message/get"
	ListMethod               = "message/list"
	ArchiveMethod            = "message/archive"
	RestoreMethod            = "message/restore"
	ClaimMethod              = "delivery/claim"
	CompleteMethod           = "delivery/complete"
	ReleaseMethod            = "delivery/release"
	TrustPeerMethod          = "peer/trust"
	DistrustPeerMethod       = "peer/distrust"
	ListPeersMethod          = "peer/list"
	HumanAccountMethod       = "human/account"
	HumanDevicesMethod       = "human/devices"
	CreateHumanInviteMethod  = "human/invite/create"
	JoinHumanInviteMethod    = "human/invite/join"
	RevokeHumanDeviceMethod  = "human/device/revoke"
	SetMailboxShareMethod    = "mailbox/share/set"
	AddRelayMethod           = "relay/add"
	RemoveRelayMethod        = "relay/remove"
	ListRelaysMethod         = "relay/list"
	NetworkStatusMethod      = "status/network"
	SynchronizeMethod        = "sync/request"
	SubscribeChangesMethod   = "changes/subscribe"
	InvalidatedMethod        = "changes/invalidated"
)

const (
	CodeNotFound       = "not_found"
	CodeAlreadyHandled = "already_handled"
	CodeNotReady       = "not_ready"
	CodeClaimed        = "claimed"
	CodeAgentNotFound  = "agent_not_found"
	CodeAgentRetired   = "agent_retired"
	CodeAgentNameTaken = "agent_name_taken"
	CodeMailboxNamed   = "mailbox_named"
	CodeAgentOwned     = "agent_owned"
	CodeDomain         = "domain_error"
)

type ResolveMailboxRequest struct {
	MutationID        string                  `json:"mutation_id"`
	Harness           string                  `json:"harness"`
	ExternalSessionID string                  `json:"external_session_id"`
	Repository        model.RepositoryContext `json:"repository"`
}

type RepositoryRequest struct {
	Repository model.RepositoryContext `json:"repository"`
}

type NamedAgentRequest struct {
	MutationID string `json:"mutation_id,omitempty"`
	Name       string `json:"name"`
	MailboxID  string `json:"mailbox_id,omitempty"`
}

type AgentSessionRequest struct {
	MutationID string                  `json:"mutation_id"`
	Name       string                  `json:"name"`
	Harness    string                  `json:"harness"`
	SessionID  string                  `json:"session_id"`
	Repository model.RepositoryContext `json:"repository"`
}

type AgentSessionRenameRequest struct {
	MutationID string `json:"mutation_id"`
	Name       string `json:"name"`
	Harness    string `json:"harness"`
	SessionID  string `json:"session_id"`
	ThreadName string `json:"thread_name"`
}

type CodexAgentRequest struct {
	Name string `json:"name"`
}

type AgentOwnershipRequest struct {
	MutationID string        `json:"mutation_id"`
	Name       string        `json:"name"`
	OwnerToken string        `json:"owner_token"`
	Duration   time.Duration `json:"duration"`
}

type MessageRequest struct {
	MutationID string        `json:"mutation_id"`
	Message    model.Message `json:"message"`
}

type ReplyRequest struct {
	MutationID string        `json:"mutation_id"`
	OriginalID string        `json:"original_id"`
	Reply      model.Message `json:"reply"`
}

type IDRequest struct {
	ID string `json:"id"`
}

type MutationIDRequest struct {
	MutationID string `json:"mutation_id"`
	ID         string `json:"id"`
}

type FilterRequest struct {
	Filter model.Filter `json:"filter"`
}

type ClaimRequest struct {
	MutationID string       `json:"mutation_id"`
	Claim      domain.Claim `json:"claim"`
	Token      string       `json:"token"`
}

type LeaseRequest struct {
	MutationID string `json:"mutation_id"`
	ID         string `json:"id"`
	Token      string `json:"token"`
}

type PeerRequest struct {
	MutationID string      `json:"mutation_id"`
	Peer       domain.Peer `json:"peer"`
}

type MutationInstallationRequest struct {
	MutationID     string `json:"mutation_id"`
	InstallationID string `json:"installation_id"`
}

type HumanInviteRequest struct {
	MutationID string                    `json:"mutation_id"`
	Invite     domain.HumanInviteRequest `json:"invite"`
}

type PairingRequest struct {
	MutationID string `json:"mutation_id"`
	Bundle     []byte `json:"bundle"`
}

type MailboxShareRequest struct {
	MutationID         string `json:"mutation_id"`
	MailboxID          string `json:"mailbox_id"`
	PeerInstallationID string `json:"peer_installation_id"`
	Active             bool   `json:"active"`
}

type RelayRequest struct {
	MutationID string             `json:"mutation_id"`
	Relay      domain.RelayConfig `json:"relay"`
}

type MutationURLRequest struct {
	MutationID string `json:"mutation_id"`
	URL        string `json:"url"`
}

type SubscribeChangesRequest struct {
	SubscriptionID string               `json:"subscription_id"`
	Topics         []domain.ChangeTopic `json:"topics,omitempty"`
}

type SubscribeChangesResponse struct {
	Revision uint64 `json:"revision"`
}

func EncodeError(err error) *localwire.RPCError {
	if err == nil {
		return nil
	}
	code := CodeDomain
	switch {
	case errors.Is(err, domain.ErrNotFound):
		code = CodeNotFound
	case errors.Is(err, domain.ErrAlreadyHandled):
		code = CodeAlreadyHandled
	case errors.Is(err, domain.ErrNotReady):
		code = CodeNotReady
	case errors.Is(err, domain.ErrClaimed):
		code = CodeClaimed
	case errors.Is(err, domain.ErrAgentNotFound):
		code = CodeAgentNotFound
	case errors.Is(err, domain.ErrAgentRetired):
		code = CodeAgentRetired
	case errors.Is(err, domain.ErrAgentNameTaken):
		code = CodeAgentNameTaken
	case errors.Is(err, domain.ErrMailboxNamed):
		code = CodeMailboxNamed
	case errors.Is(err, domain.ErrAgentOwned):
		code = CodeAgentOwned
	}
	return &localwire.RPCError{Code: code, Message: err.Error()}
}

func DecodeError(err error) error {
	var rpcErr *localwire.RPCError
	if !errors.As(err, &rpcErr) {
		return err
	}
	var sentinel error
	switch rpcErr.Code {
	case CodeNotFound:
		sentinel = domain.ErrNotFound
	case CodeAlreadyHandled:
		sentinel = domain.ErrAlreadyHandled
	case CodeNotReady:
		sentinel = domain.ErrNotReady
	case CodeClaimed:
		sentinel = domain.ErrClaimed
	case CodeAgentNotFound:
		sentinel = domain.ErrAgentNotFound
	case CodeAgentRetired:
		sentinel = domain.ErrAgentRetired
	case CodeAgentNameTaken:
		sentinel = domain.ErrAgentNameTaken
	case CodeMailboxNamed:
		sentinel = domain.ErrMailboxNamed
	case CodeAgentOwned:
		sentinel = domain.ErrAgentOwned
	default:
		return err
	}
	return fmt.Errorf("%w: %s", sentinel, rpcErr.Message)
}
