package domainrpc

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
)

type Service struct {
	Store interface {
		domain.Operations
		domain.ProjectOperations
		domain.MutationLog
		domain.ChangeLog
	}
	Synchronize   func(context.Context) error
	Subscriptions *SubscriptionHub
	Runtime       domain.HarnessRuntimeController
}

func (s Service) Handle(ctx context.Context, session *localwire.Session, method string, raw json.RawMessage) (any, *localwire.RPCError) {
	if s.Store == nil {
		return nil, &localwire.RPCError{Code: localwire.CodeInternal, Message: "domain store is unavailable"}
	}
	mutation, mutating, err := mutationForRequest(method, raw)
	if err != nil {
		return nil, &localwire.RPCError{Code: localwire.CodeInvalidRequest, Message: err.Error()}
	}
	if mutating {
		if result, found, lookupErr := s.Store.MutationResult(ctx, mutation); lookupErr != nil {
			return nil, encodeMutationError(lookupErr)
		} else if found {
			s.wakeForReplayedMessageMutation(method, raw)
			return result, nil
		}
		ctx = domain.WithMutation(ctx, mutation)
	}
	result, err := s.dispatch(ctx, session, method, raw)
	if mutating {
		if persisted, found, lookupErr := s.Store.MutationResult(ctx, mutation); lookupErr != nil {
			return nil, encodeMutationError(lookupErr)
		} else if found {
			return persisted, nil
		} else if err == nil {
			err = errors.New("mutation completed without a durable receipt")
		}
	}
	var missing *methodNotFoundError
	if errors.As(err, &missing) {
		return nil, &localwire.RPCError{Code: localwire.CodeMethodNotFound, Message: err.Error()}
	}
	var invalid *invalidRequestError
	if errors.As(err, &invalid) {
		return nil, &localwire.RPCError{Code: localwire.CodeInvalidRequest, Message: err.Error()}
	}
	return result, EncodeError(err)
}

func encodeMutationError(err error) *localwire.RPCError {
	if errors.Is(err, domain.ErrMutationConflict) {
		return &localwire.RPCError{Code: localwire.CodeInvalidRequest, Message: err.Error()}
	}
	return EncodeError(err)
}

var mutationMethods = map[string]bool{
	ResolveMailboxMethod: true, CreateMethod: true, ReplyMethod: true, ArchiveMethod: true, RestoreMethod: true,
	CreateNamedAgentMethod: true, RetireNamedAgentMethod: true, SelectAgentSessionMethod: true, RenameAgentSessionMethod: true,
	AcquireAgentMethod: true, RenewAgentMethod: true, ReleaseAgentMethod: true,
	ClaimMethod: true, CompleteMethod: true, ReleaseMethod: true, TrustPeerMethod: true,
	DistrustPeerMethod: true, CreateHumanInviteMethod: true, JoinHumanInviteMethod: true,
	RevokeHumanDeviceMethod: true, SetMailboxShareMethod: true, AddRelayMethod: true,
	RemoveRelayMethod:   true,
	CreateProjectMethod: true, OpenProjectMethod: true, BeginCloseProjectMethod: true, FinalizeCloseProjectMethod: true,
	ArchiveProjectMethod: true, UpdateProjectMethod: true, AddProjectPathMethod: true, RemoveProjectResourceMethod: true,
	ReplaceProjectPathMethod: true, SetProjectPrimaryMethod: true, AssignProjectMethod: true, ActivateProjectMethod: true,
	AbortProjectAssignmentMethod: true, BlockProjectAssignmentMethod: true, UnassignProjectMethod: true,
}

func mutationForRequest(method string, raw json.RawMessage) (domain.Mutation, bool, error) {
	if !mutationMethods[method] {
		return domain.Mutation{}, false, nil
	}
	var header struct {
		MutationID string `json:"mutation_id"`
	}
	if err := json.Unmarshal(raw, &header); err != nil {
		return domain.Mutation{}, true, fmt.Errorf("decode mutation metadata: %w", err)
	}
	if _, err := uuid.Parse(header.MutationID); err != nil {
		return domain.Mutation{}, true, errors.New("mutation_id must be a UUID")
	}
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {
		return domain.Mutation{}, true, fmt.Errorf("canonicalize mutation request: %w", err)
	}
	canonical, err := json.Marshal(value)
	if err != nil {
		return domain.Mutation{}, true, fmt.Errorf("canonicalize mutation request: %w", err)
	}
	digest := sha256.Sum256(append([]byte(method+"\x00"), canonical...))
	return domain.Mutation{ID: header.MutationID, Method: method, RequestDigest: fmt.Sprintf("%x", digest)}, true, nil
}

func (s Service) dispatch(ctx context.Context, session *localwire.Session, method string, raw json.RawMessage) (any, error) {
	switch method {
	case HumanMailboxMethod:
		return s.Store.HumanMailbox(ctx)
	case ResolveMailboxMethod:
		var request ResolveMailboxRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ResolveMailbox(ctx, model.SessionIdentity{Harness: request.Harness, ExternalSessionID: request.ExternalSessionID}, request.Repository)
	case FindMailboxesMethod:
		var request RepositoryRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.FindMailboxes(ctx, request.Repository)
	case CreateNamedAgentMethod:
		var request NamedAgentRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.CreateNamedAgent(ctx, request.Name, request.MailboxID)
	case GetNamedAgentMethod:
		var request NamedAgentRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.GetNamedAgent(ctx, request.Name)
	case ListNamedAgentsMethod:
		return s.Store.ListNamedAgents(ctx)
	case ListAgentSessionsMethod:
		var request NamedAgentRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ListNamedAgentSessions(ctx, request.Name)
	case RetireNamedAgentMethod:
		var request NamedAgentRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.RetireNamedAgent(ctx, request.Name)
	case SelectAgentSessionMethod:
		var request AgentSessionRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.SelectNamedAgentSession(ctx, request.Name, model.SessionIdentity{Harness: request.Harness, ExternalSessionID: request.SessionID}, request.Repository)
	case RenameAgentSessionMethod:
		var request AgentSessionRenameRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.RenameNamedAgentSession(ctx, request.Name, model.SessionIdentity{Harness: request.Harness, ExternalSessionID: request.SessionID}, request.SessionName)
	case ListHarnessActivitiesMethod:
		var request HarnessActivityFilterRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		reader, ok := s.Store.(domain.HarnessActivityReader)
		if !ok {
			return nil, errors.New("harness activity storage is unavailable")
		}
		return reader.ListHarnessActivities(ctx, request.Filter)
	case AcquireAgentMethod, RenewAgentMethod:
		var request AgentOwnershipRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		if method == AcquireAgentMethod {
			return s.Store.AcquireNamedAgent(ctx, request.Name, request.OwnerToken, request.Duration)
		}
		return s.Store.RenewNamedAgent(ctx, request.Name, request.OwnerToken, request.Duration)
	case ReleaseAgentMethod:
		var request AgentOwnershipRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.ReleaseNamedAgent(ctx, request.Name, request.OwnerToken)
	case LaunchHarnessAgentMethod:
		if s.Runtime == nil {
			return nil, errors.New("harness runtime control is unavailable")
		}
		var request domain.HarnessLaunchRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Runtime.LaunchHarnessAgent(ctx, request)
	case ActivateHarnessProjectMethod:
		controller, ok := s.Runtime.(domain.ProjectHarnessRuntimeController)
		if !ok {
			return nil, errors.New("project harness runtime control is unavailable")
		}
		var request domain.ProjectHarnessActivationRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return controller.ActivateHarnessProject(ctx, request)
	case CloseHarnessProjectMethod:
		controller, ok := s.Runtime.(domain.ProjectHarnessRuntimeController)
		if !ok {
			return nil, errors.New("project harness runtime control is unavailable")
		}
		var request domain.ProjectHarnessCloseRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return controller.CloseHarnessProject(ctx, request)
	case PreviewHarnessProjectCloseMethod:
		controller, ok := s.Runtime.(domain.ProjectHarnessRuntimeController)
		if !ok {
			return nil, errors.New("project harness runtime control is unavailable")
		}
		var request ProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return controller.PreviewHarnessProjectClose(ctx, request.ProjectID)
	case ReplaceHarnessProjectMethod:
		controller, ok := s.Runtime.(domain.ProjectHarnessRuntimeController)
		if !ok {
			return nil, errors.New("project harness runtime control is unavailable")
		}
		var request domain.ProjectHarnessReplaceRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return controller.ReplaceHarnessProject(ctx, request)
	case HandoffHarnessProjectMethod:
		controller, ok := s.Runtime.(domain.ProjectHarnessRuntimeController)
		if !ok {
			return nil, errors.New("project harness runtime control is unavailable")
		}
		var request domain.ProjectHarnessHandoffRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return controller.HandoffHarnessProject(ctx, request)
	case RetireHarnessAgentMethod:
		controller, ok := s.Runtime.(domain.ProjectHarnessRuntimeController)
		if !ok {
			return nil, errors.New("harness retirement control is unavailable")
		}
		var request domain.HarnessRetireAgentRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, controller.RetireHarnessAgent(ctx, request)
	case ProvisionProjectWorktreeMethod:
		controller, ok := s.Runtime.(domain.ProjectWorktreeProvisioner)
		if !ok {
			return nil, errors.New("project worktree provisioning is unavailable")
		}
		var request domain.ProjectWorktreeRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return controller.ProvisionProjectWorktree(ctx, request)
	case StopHarnessAgentMethod, HarnessRuntimeMethod:
		if s.Runtime == nil {
			return nil, errors.New("harness runtime control is unavailable")
		}
		var request HarnessAgentRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		if method == StopHarnessAgentMethod {
			return s.Runtime.StopHarnessAgent(ctx, request.Name)
		}
		return s.Runtime.HarnessAgentRuntime(ctx, request.Name)
	case CreateMethod:
		var request MessageRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		defer clearEnvironment(request.Environment)
		if err := s.Store.Create(ctx, request.Message); err != nil {
			return nil, err
		}
		s.wakeHarnessAgent(request.Message, request.Environment)
		return nil, nil
	case ReplyMethod:
		var request ReplyRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		defer clearEnvironment(request.Environment)
		if err := s.Store.Reply(ctx, request.OriginalID, request.Reply); err != nil {
			return nil, err
		}
		s.wakeHarnessAgent(request.Reply, request.Environment)
		return nil, nil
	case GetMethod:
		var request IDRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.Get(ctx, request.ID)
	case ListMethod:
		var request FilterRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.List(ctx, request.Filter)
	case ListConversationsMethod:
		var request ConversationFilterRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ListConversations(ctx, request.Filter)
	case ConversationHistoryMethod:
		var request ConversationHistoryRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ListConversationHistory(ctx, request.Filter)
	case ConversationEntriesMethod:
		var request ConversationEntriesRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ListConversationEntries(ctx, request.Filter)
	case ArchiveMethod:
		var request MutationIDRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.Archive(ctx, request.ID)
	case RestoreMethod:
		var request MutationIDRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.Restore(ctx, request.ID)
	case ClaimMethod:
		var request ClaimRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.Claim(ctx, request.Claim, request.Token)
	case CompleteMethod, ReleaseMethod:
		var request LeaseRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		if method == CompleteMethod {
			return nil, s.Store.Complete(ctx, request.ID, request.Token)
		}
		return nil, s.Store.Release(ctx, request.ID, request.Token)
	case TrustPeerMethod:
		var request PeerRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.TrustPeer(ctx, request.Peer)
	case DistrustPeerMethod, RevokeHumanDeviceMethod:
		var request MutationInstallationRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		if method == DistrustPeerMethod {
			return nil, s.Store.DistrustPeer(ctx, request.InstallationID)
		}
		return nil, s.Store.RevokeHumanDevice(ctx, request.InstallationID)
	case ListPeersMethod:
		return s.Store.ListPeers(ctx)
	case HumanAccountMethod:
		return s.Store.HumanAccount(ctx)
	case HumanDevicesMethod:
		return s.Store.HumanDevices(ctx)
	case CreateHumanInviteMethod:
		var request HumanInviteRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.CreateHumanInvite(ctx, request.Invite)
	case JoinHumanInviteMethod:
		var request PairingRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.JoinHumanInvite(ctx, request.Bundle)
	case SetMailboxShareMethod:
		var request MailboxShareRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.SetMailboxShare(ctx, request.MailboxID, request.PeerInstallationID, request.Active)
	case AddRelayMethod:
		var request RelayRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.AddRelay(ctx, request.Relay)
	case RemoveRelayMethod:
		var request MutationURLRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.RemoveRelay(ctx, request.URL)
	case ListRelaysMethod:
		return s.Store.ListRelays(ctx)
	case NetworkStatusMethod:
		return s.Store.NetworkStatus(ctx)
	case CreateProjectMethod:
		var request CreateProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.CreateProject(ctx, request.Project)
	case GetProjectMethod:
		var request ProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.GetProject(ctx, request.ProjectID)
	case ListProjectsMethod:
		var request ListProjectsRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ListProjects(ctx, request.IncludeArchived)
	case ListProjectThreadsMethod:
		var request ProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ListProjectThreads(ctx, request.ProjectID)
	case OpenProjectMethod, BeginCloseProjectMethod:
		var request ProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		if method == OpenProjectMethod {
			return s.Store.OpenProject(ctx, request.ProjectID, request.ExpectedHead)
		}
		return s.Store.BeginCloseProject(ctx, request.ProjectID, request.ExpectedHead)
	case FinalizeCloseProjectMethod:
		var request FinalizeCloseProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.FinalizeCloseProject(ctx, request.ProjectID, request.ExpectedHead, request.Forced, request.RuntimeObservation)
	case ArchiveProjectMethod:
		var request ArchiveProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.SetProjectArchived(ctx, request.ProjectID, request.ExpectedHead, request.Archived)
	case UpdateProjectMethod:
		var request UpdateProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.UpdateProjectMetadata(ctx, request.ProjectID, request.ExpectedHead, request.Name, request.Brief)
	case AddProjectPathMethod:
		var request ProjectPathRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.AddProjectPath(ctx, request.ProjectID, request.ExpectedHead, request.Path, request.Primary)
	case RemoveProjectResourceMethod, SetProjectPrimaryMethod:
		var request ProjectResourceRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		if method == RemoveProjectResourceMethod {
			return s.Store.RemoveProjectResource(ctx, request.ProjectID, request.ExpectedHead, request.ResourceID)
		}
		return s.Store.SetProjectPrimaryResource(ctx, request.ProjectID, request.ExpectedHead, request.ResourceID)
	case ReplaceProjectPathMethod:
		var request ReplaceProjectPathRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ReplaceProjectPath(ctx, request.ProjectID, request.ExpectedHead, request.ResourceID, request.Path)
	case CheckProjectResourceMethod:
		var request CheckProjectResourceRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.CheckProjectResource(ctx, request.ProjectID, request.ResourceID)
	case AssignProjectMethod:
		var request AssignProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.AssignProject(ctx, request.ProjectID, request.ExpectedHead, request.AgentName)
	case ActivateProjectMethod:
		var request ActivateProjectRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return s.Store.ActivateProjectAssignment(ctx, request.ProjectID, request.ExpectedHead, request.Activation)
	case AbortProjectAssignmentMethod, BlockProjectAssignmentMethod, UnassignProjectMethod:
		var request EndProjectAssignmentRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		if method == AbortProjectAssignmentMethod {
			return s.Store.AbortProjectAssignment(ctx, request.ProjectID, request.ExpectedHead, request.RuntimeObservation)
		}
		if method == BlockProjectAssignmentMethod {
			return s.Store.BlockProjectAssignment(ctx, request.ProjectID, request.ExpectedHead, request.RuntimeObservation)
		}
		return s.Store.UnassignProject(ctx, request.ProjectID, request.ExpectedHead, request.Forced, request.RuntimeObservation)
	case SynchronizeMethod:
		if s.Synchronize == nil {
			return nil, errors.New("node synchronization is unavailable")
		}
		return nil, s.Synchronize(ctx)
	case SubscribeChangesMethod:
		if s.Subscriptions == nil {
			return nil, errors.New("change subscriptions are unavailable")
		}
		var request SubscribeChangesRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		subscriber, err := s.Subscriptions.Register(session, request.SubscriptionID, request.Topics)
		if err != nil {
			return nil, err
		}
		revision, err := s.Store.CurrentRevision(ctx)
		if err != nil {
			subscriber.Close()
			return nil, err
		}
		return localwire.DeferredResponse{
			Value: SubscribeChangesResponse{Revision: revision},
			After: func() { subscriber.Activate(revision) },
		}, nil
	default:
		return nil, &methodNotFoundError{method: method}
	}
}

func (s Service) wakeHarnessAgent(message model.Message, environment []string) {
	if runtime, ok := s.Runtime.(domain.HarnessRuntimeAutoStarter); ok {
		runtime.WakeHarnessAgent(message, environment)
	}
}

func (s Service) wakeForReplayedMessageMutation(method string, raw json.RawMessage) {
	switch method {
	case CreateMethod:
		var request MessageRequest
		if json.Unmarshal(raw, &request) == nil {
			s.wakeHarnessAgent(request.Message, request.Environment)
			clearEnvironment(request.Environment)
		}
	case ReplyMethod:
		var request ReplyRequest
		if json.Unmarshal(raw, &request) == nil {
			s.wakeHarnessAgent(request.Reply, request.Environment)
			clearEnvironment(request.Environment)
		}
	}
}

func clearEnvironment(environment []string) {
	for index := range environment {
		environment[index] = ""
	}
}

type methodNotFoundError struct{ method string }

func (e *methodNotFoundError) Error() string {
	return fmt.Sprintf("unknown domain method %q", e.method)
}

type invalidRequestError struct{ err error }

func (e *invalidRequestError) Error() string { return "decode domain request: " + e.err.Error() }
func (e *invalidRequestError) Unwrap() error { return e.err }

func decodeRequest(raw json.RawMessage, destination any) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(destination); err != nil {
		return &invalidRequestError{err: err}
	}
	if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		return &invalidRequestError{err: errors.New("trailing data")}
	}
	return nil
}
