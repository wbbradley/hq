package domainrpc

import (
	"errors"
	"fmt"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

const (
	HumanMailboxMethod             = "mailbox/human"
	ResolveMailboxMethod           = "mailbox/resolve"
	FindMailboxesMethod            = "mailbox/list"
	CreateNamedAgentMethod         = "agent/create"
	GetNamedAgentMethod            = "agent/get"
	ListNamedAgentsMethod          = "agent/list"
	ListAgentSessionsMethod        = "agent/session/list"
	RenameAgentSessionMethod       = "agent/session/rename"
	RetireNamedAgentMethod         = "agent/retire"
	SelectAgentSessionMethod       = "agent/session/select"
	AcquireAgentMethod             = "agent/ownership/acquire"
	RenewAgentMethod               = "agent/ownership/renew"
	ReleaseAgentMethod             = "agent/ownership/release"
	LaunchHarnessAgentMethod       = "harness/agent/launch"
	StopHarnessAgentMethod         = "harness/agent/stop"
	HarnessRuntimeMethod           = "harness/agent/status"
	ActivateHarnessProjectMethod   = "harness/project/activate"
	CloseHarnessProjectMethod      = "harness/project/close"
	HandoffHarnessProjectMethod    = "harness/project/handoff"
	RetireHarnessAgentMethod       = "harness/agent/retire"
	ProvisionProjectWorktreeMethod = "project/worktree/provision"
	CreateMethod                   = "message/create"
	ReplyMethod                    = "message/reply"
	GetMethod                      = "message/get"
	ListMethod                     = "message/list"
	ListConversationsMethod        = "conversation/list"
	ConversationHistoryMethod      = "conversation/history"
	ArchiveMethod                  = "message/archive"
	RestoreMethod                  = "message/restore"
	ClaimMethod                    = "delivery/claim"
	CompleteMethod                 = "delivery/complete"
	ReleaseMethod                  = "delivery/release"
	TrustPeerMethod                = "peer/trust"
	DistrustPeerMethod             = "peer/distrust"
	ListPeersMethod                = "peer/list"
	HumanAccountMethod             = "human/account"
	HumanDevicesMethod             = "human/devices"
	CreateHumanInviteMethod        = "human/invite/create"
	JoinHumanInviteMethod          = "human/invite/join"
	RevokeHumanDeviceMethod        = "human/device/revoke"
	SetMailboxShareMethod          = "mailbox/share/set"
	AddRelayMethod                 = "relay/add"
	RemoveRelayMethod              = "relay/remove"
	ListRelaysMethod               = "relay/list"
	NetworkStatusMethod            = "status/network"
	SynchronizeMethod              = "sync/request"
	SubscribeChangesMethod         = "changes/subscribe"
	InvalidatedMethod              = "changes/invalidated"
	CreateProjectMethod            = "project/create"
	GetProjectMethod               = "project/get"
	ListProjectsMethod             = "project/list"
	ListProjectThreadsMethod       = "project/thread/list"
	OpenProjectMethod              = "project/open"
	BeginCloseProjectMethod        = "project/close/begin"
	FinalizeCloseProjectMethod     = "project/close/finalize"
	ArchiveProjectMethod           = "project/archive/set"
	UpdateProjectMethod            = "project/metadata/update"
	AddProjectPathMethod           = "project/resource/path/add"
	RemoveProjectResourceMethod    = "project/resource/remove"
	ReplaceProjectPathMethod       = "project/resource/path/replace"
	SetProjectPrimaryMethod        = "project/resource/primary"
	CheckProjectResourceMethod     = "project/resource/check"
	AssignProjectMethod            = "project/assignment/create"
	ActivateProjectMethod          = "project/assignment/activate"
	AbortProjectAssignmentMethod   = "project/assignment/abort"
	BlockProjectAssignmentMethod   = "project/assignment/block"
	UnassignProjectMethod          = "project/assignment/end"
)

const (
	CodeNotFound              = "not_found"
	CodeAlreadyHandled        = "already_handled"
	CodeNotReady              = "not_ready"
	CodeClaimed               = "claimed"
	CodeAgentNotFound         = "agent_not_found"
	CodeAgentRetired          = "agent_retired"
	CodeAgentNameTaken        = "agent_name_taken"
	CodeMailboxNamed          = "mailbox_named"
	CodeAgentOwned            = "agent_owned"
	CodeProjectNotFound       = "project_not_found"
	CodeProjectStale          = "project_stale"
	CodeProjectState          = "project_state"
	CodeResourceConflict      = "resource_conflict"
	CodeResourceNotFound      = "resource_not_found"
	CodeAgentAssigned         = "agent_assigned"
	CodeProjectAssigned       = "project_assigned"
	CodeProjectThreadMismatch = "project_thread_mismatch"
	CodeProjectCommandPending = "project_command_pending"
	CodeProjectRuntimeUnknown = "project_runtime_unknown"
	CodeHarnessUnknown        = "harness_unknown"
	CodeHarnessUnavailable    = "harness_unavailable"
	CodeHarnessIncapable      = "harness_incapable"
	CodeDomain                = "domain_error"
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
	MutationID  string `json:"mutation_id"`
	Name        string `json:"name"`
	Harness     string `json:"harness"`
	SessionID   string `json:"session_id"`
	SessionName string `json:"session_name"`
}

type HarnessAgentRequest struct {
	Name string `json:"name"`
}

type AgentOwnershipRequest struct {
	MutationID string        `json:"mutation_id"`
	Name       string        `json:"name"`
	OwnerToken string        `json:"owner_token"`
	Duration   time.Duration `json:"duration"`
}

type MessageRequest struct {
	MutationID  string        `json:"mutation_id"`
	Message     model.Message `json:"message"`
	Environment []string      `json:"environment,omitempty"`
}

type ReplyRequest struct {
	MutationID  string        `json:"mutation_id"`
	OriginalID  string        `json:"original_id"`
	Reply       model.Message `json:"reply"`
	Environment []string      `json:"environment,omitempty"`
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

type ConversationFilterRequest struct {
	Filter model.ConversationFilter `json:"filter"`
}

type ConversationHistoryRequest struct {
	Filter model.ConversationHistoryFilter `json:"filter"`
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

type ProjectRequest struct {
	MutationID   string `json:"mutation_id,omitempty"`
	ProjectID    string `json:"project_id"`
	ExpectedHead string `json:"expected_head_event_id,omitempty"`
}

type CreateProjectRequest struct {
	MutationID string                      `json:"mutation_id"`
	Project    domain.CreateProjectRequest `json:"project"`
}
type ListProjectsRequest struct {
	IncludeArchived bool `json:"include_archived"`
}
type FinalizeCloseProjectRequest struct {
	ProjectRequest
	Forced             bool   `json:"forced"`
	RuntimeObservation string `json:"runtime_observation,omitempty"`
}
type ArchiveProjectRequest struct {
	ProjectRequest
	Archived bool `json:"archived"`
}
type UpdateProjectRequest struct {
	ProjectRequest
	Name  string `json:"name"`
	Brief string `json:"brief,omitempty"`
}
type ProjectPathRequest struct {
	ProjectRequest
	Path    domain.ProjectPathInput `json:"path"`
	Primary bool                    `json:"primary,omitempty"`
}
type ProjectResourceRequest struct {
	ProjectRequest
	ResourceID string `json:"resource_id"`
}
type ReplaceProjectPathRequest struct {
	ProjectResourceRequest
	Path domain.ProjectPathInput `json:"path"`
}
type CheckProjectResourceRequest struct {
	ProjectID  string `json:"project_id"`
	ResourceID string `json:"resource_id"`
}
type AssignProjectRequest struct {
	ProjectRequest
	AgentName string `json:"agent_name"`
}
type ActivateProjectRequest struct {
	ProjectRequest
	Activation domain.ActivateProjectAssignmentRequest `json:"activation"`
}
type EndProjectAssignmentRequest struct {
	ProjectRequest
	Forced             bool   `json:"forced"`
	RuntimeObservation string `json:"runtime_observation,omitempty"`
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
	case errors.Is(err, domain.ErrProjectNotFound):
		code = CodeProjectNotFound
	case errors.Is(err, domain.ErrProjectStale):
		code = CodeProjectStale
	case errors.Is(err, domain.ErrProjectState):
		code = CodeProjectState
	case errors.Is(err, domain.ErrResourceConflict):
		code = CodeResourceConflict
	case errors.Is(err, domain.ErrResourceNotFound):
		code = CodeResourceNotFound
	case errors.Is(err, domain.ErrAgentAssigned):
		code = CodeAgentAssigned
	case errors.Is(err, domain.ErrProjectAssigned):
		code = CodeProjectAssigned
	case errors.Is(err, domain.ErrProjectThreadMismatch):
		code = CodeProjectThreadMismatch
	case errors.Is(err, domain.ErrProjectCommandPending):
		code = CodeProjectCommandPending
	case errors.Is(err, domain.ErrProjectRuntimeUnknown):
		code = CodeProjectRuntimeUnknown
	case errors.Is(err, harness.ErrUnknownProvider):
		code = CodeHarnessUnknown
	case errors.Is(err, harness.ErrProviderUnavailable):
		code = CodeHarnessUnavailable
	case errors.Is(err, harness.ErrCapabilityUnavailable):
		code = CodeHarnessIncapable
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
	case CodeProjectNotFound:
		sentinel = domain.ErrProjectNotFound
	case CodeProjectStale:
		sentinel = domain.ErrProjectStale
	case CodeProjectState:
		sentinel = domain.ErrProjectState
	case CodeResourceConflict:
		sentinel = domain.ErrResourceConflict
	case CodeResourceNotFound:
		sentinel = domain.ErrResourceNotFound
	case CodeAgentAssigned:
		sentinel = domain.ErrAgentAssigned
	case CodeProjectAssigned:
		sentinel = domain.ErrProjectAssigned
	case CodeProjectThreadMismatch:
		sentinel = domain.ErrProjectThreadMismatch
	case CodeProjectCommandPending:
		sentinel = domain.ErrProjectCommandPending
	case CodeProjectRuntimeUnknown:
		sentinel = domain.ErrProjectRuntimeUnknown
	case CodeHarnessUnknown:
		sentinel = harness.ErrUnknownProvider
	case CodeHarnessUnavailable:
		sentinel = harness.ErrProviderUnavailable
	case CodeHarnessIncapable:
		sentinel = harness.ErrCapabilityUnavailable
	default:
		return err
	}
	return fmt.Errorf("%w: %s", sentinel, rpcErr.Message)
}
