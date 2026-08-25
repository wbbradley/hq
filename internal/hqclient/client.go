package hqclient

import (
	"context"
	"errors"
	"io"
	"net"
	"os"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/buildinfo"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/syncer"
)

type Client struct {
	mu            sync.RWMutex
	wire          *localwire.Client
	connect       func(context.Context) (*localwire.Client, error)
	reconnectMu   sync.Mutex
	subscriptions map[string]*Subscription
	states        chan ConnectionState
	state         ConnectionState
	updates       chan domain.ConnectionUpdate
	update        domain.ConnectionUpdate
	lifetime      context.Context
	cancel        context.CancelFunc
	closed        bool
}

func Open(ctx context.Context, databasePath string) (*Client, error) {
	lifetime, cancel := context.WithCancel(context.Background())
	client := newClient(lifetime, cancel)
	client.connect = func(connectContext context.Context) (*localwire.Client, error) {
		return openWire(connectContext, lifetime, databasePath)
	}
	client.publishState(ConnectionState{Phase: ConnectionConnecting})
	wireClient, err := client.connect(ctx)
	if err != nil {
		cancel()
		client.publishConnectionError(err)
		return nil, err
	}
	client.attach(wireClient)
	client.publishReady(wireClient)
	return client, nil
}

func openWire(ctx, lifetime context.Context, databasePath string) (*localwire.Client, error) {
	if err := syncer.EnsureNode(ctx, databasePath); err != nil {
		return nil, err
	}
	paths, err := syncer.ResolveRuntimePaths(databasePath)
	if err != nil {
		return nil, err
	}
	connection, err := dial(ctx, paths.Socket)
	if err != nil {
		return nil, err
	}
	wireClient, err := localwire.NewClient(lifetime, connection, localwire.ClientOptions{
		Mode: localwire.DomainMode, Supported: localwire.DomainVersions,
		Metadata: localwire.PeerMetadata{Build: buildinfo.Version},
	})
	if err != nil {
		connection.Close()
		return nil, err
	}
	return wireClient, nil
}

func New(wireClient *localwire.Client) *Client {
	lifetime, cancel := context.WithCancel(context.Background())
	client := newClient(lifetime, cancel)
	client.attach(wireClient)
	client.publishReady(wireClient)
	return client
}

func newClient(lifetime context.Context, cancel context.CancelFunc) *Client {
	return &Client{
		subscriptions: make(map[string]*Subscription), states: make(chan ConnectionState, 1),
		updates:  make(chan domain.ConnectionUpdate, 1),
		lifetime: lifetime, cancel: cancel,
	}
}

func (c *Client) Close() error {
	if c == nil {
		return nil
	}
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil
	}
	c.closed = true
	wireClient := c.wire
	c.mu.Unlock()
	c.cancel()
	c.publishState(ConnectionState{Phase: ConnectionDisconnected, Err: errors.New("domain client closed")})
	if wireClient == nil {
		return nil
	}
	err := wireClient.Close()
	if errors.Is(err, net.ErrClosed) {
		return nil
	}
	return err
}

func (c *Client) call(ctx context.Context, method string, request, response any) error {
	for {
		wireClient := c.currentWire()
		if wireClient == nil {
			var err error
			wireClient, err = c.reconnect(ctx, nil)
			if err != nil {
				return err
			}
		}
		callErr := wireClient.Call(ctx, method, request, response)
		if callErr == nil {
			return nil
		}
		if !reconnectable(callErr) {
			return domainrpc.DecodeError(callErr)
		}
		if _, reconnectErr := c.reconnect(ctx, wireClient); reconnectErr != nil {
			return reconnectErr
		}
	}
}

func (c *Client) mutatingCall(ctx context.Context, method string, request func(string) any, response any) error {
	id, err := uuid.NewV7()
	if err != nil {
		return err
	}
	return c.call(ctx, method, request(id.String()), response)
}

func (c *Client) HumanMailbox(ctx context.Context) (model.Mailbox, error) {
	var result model.Mailbox
	err := c.call(ctx, domainrpc.HumanMailboxMethod, nil, &result)
	return result, err
}

func (c *Client) ResolveMailbox(ctx context.Context, session model.SessionIdentity, repository model.RepositoryContext) (model.Mailbox, error) {
	var result model.Mailbox
	err := c.mutatingCall(ctx, domainrpc.ResolveMailboxMethod, func(id string) any {
		return domainrpc.ResolveMailboxRequest{MutationID: id, Harness: session.Harness, ExternalSessionID: session.ExternalSessionID, Repository: repository}
	}, &result)
	return result, err
}

func (c *Client) FindMailboxes(ctx context.Context, repository model.RepositoryContext) ([]model.Mailbox, error) {
	var result []model.Mailbox
	err := c.call(ctx, domainrpc.FindMailboxesMethod, domainrpc.RepositoryRequest{Repository: repository}, &result)
	return result, err
}

func (c *Client) CreateNamedAgent(ctx context.Context, name, mailboxID string) (domain.NamedAgent, error) {
	var result domain.NamedAgent
	err := c.mutatingCall(ctx, domainrpc.CreateNamedAgentMethod, func(id string) any {
		return domainrpc.NamedAgentRequest{MutationID: id, Name: name, MailboxID: mailboxID}
	}, &result)
	return result, err
}

func (c *Client) GetNamedAgent(ctx context.Context, name string) (domain.NamedAgent, error) {
	var result domain.NamedAgent
	err := c.call(ctx, domainrpc.GetNamedAgentMethod, domainrpc.NamedAgentRequest{Name: name}, &result)
	return result, err
}

func (c *Client) ListNamedAgents(ctx context.Context) ([]domain.NamedAgent, error) {
	var result []domain.NamedAgent
	err := c.call(ctx, domainrpc.ListNamedAgentsMethod, nil, &result)
	return result, err
}

func (c *Client) ListNamedAgentSessions(ctx context.Context, name string) ([]domain.AgentSession, error) {
	var result []domain.AgentSession
	err := c.call(ctx, domainrpc.ListAgentSessionsMethod, domainrpc.NamedAgentRequest{Name: name}, &result)
	return result, err
}

func (c *Client) RenameNamedAgentSession(ctx context.Context, name string, session model.SessionIdentity, sessionName string) (domain.AgentSession, error) {
	var result domain.AgentSession
	err := c.mutatingCall(ctx, domainrpc.RenameAgentSessionMethod, func(id string) any {
		return domainrpc.AgentSessionRenameRequest{MutationID: id, Name: name, Harness: session.Harness, SessionID: session.ExternalSessionID, SessionName: sessionName}
	}, &result)
	return result, err
}

func (c *Client) LaunchHarnessAgent(ctx context.Context, request domain.HarnessLaunchRequest) (domain.HarnessRuntime, error) {
	var result domain.HarnessRuntime
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	err := c.call(ctx, domainrpc.LaunchHarnessAgentMethod, request, &result)
	return result, err
}

func (c *Client) StopHarnessAgent(ctx context.Context, name string) (domain.HarnessRuntime, error) {
	var result domain.HarnessRuntime
	err := c.call(ctx, domainrpc.StopHarnessAgentMethod, domainrpc.HarnessAgentRequest{Name: name}, &result)
	return result, err
}

func (c *Client) HarnessAgentRuntime(ctx context.Context, name string) (domain.HarnessRuntime, error) {
	var result domain.HarnessRuntime
	err := c.call(ctx, domainrpc.HarnessRuntimeMethod, domainrpc.HarnessAgentRequest{Name: name}, &result)
	return result, err
}

func (c *Client) ActivateHarnessProject(ctx context.Context, request domain.ProjectHarnessActivationRequest) (domain.ProjectHarnessActivation, error) {
	var result domain.ProjectHarnessActivation
	if request.Launch.RequestID == "" {
		request.Launch.RequestID = uuid.NewString()
	}
	err := c.call(ctx, domainrpc.ActivateHarnessProjectMethod, request, &result)
	return result, err
}

func (c *Client) CloseHarnessProject(ctx context.Context, request domain.ProjectHarnessCloseRequest) (domain.Project, error) {
	var result domain.Project
	err := c.call(ctx, domainrpc.CloseHarnessProjectMethod, request, &result)
	return result, err
}

func (c *Client) HandoffHarnessProject(ctx context.Context, request domain.ProjectHarnessHandoffRequest) (domain.ProjectHarnessActivation, error) {
	var result domain.ProjectHarnessActivation
	if request.Launch.RequestID == "" {
		request.Launch.RequestID = uuid.NewString()
	}
	err := c.call(ctx, domainrpc.HandoffHarnessProjectMethod, request, &result)
	return result, err
}

func (c *Client) ProvisionProjectWorktree(ctx context.Context, request domain.ProjectWorktreeRequest) (domain.Project, error) {
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	if request.ProjectID == "" {
		request.ProjectID = uuid.NewString()
	}
	var project domain.Project
	err := c.call(ctx, domainrpc.ProvisionProjectWorktreeMethod, request, &project)
	return project, err
}
func (c *Client) RetireHarnessAgent(ctx context.Context, request domain.HarnessRetireAgentRequest) error {
	if request.RequestID == "" {
		request.RequestID = uuid.NewString()
	}
	return c.call(ctx, domainrpc.RetireHarnessAgentMethod, request, nil)
}

func (c *Client) RetireNamedAgent(ctx context.Context, name string) error {
	return c.mutatingCall(ctx, domainrpc.RetireNamedAgentMethod, func(id string) any { return domainrpc.NamedAgentRequest{MutationID: id, Name: name} }, nil)
}

func (c *Client) SelectNamedAgentSession(ctx context.Context, name string, session model.SessionIdentity, repository model.RepositoryContext) (domain.NamedAgent, error) {
	var result domain.NamedAgent
	err := c.mutatingCall(ctx, domainrpc.SelectAgentSessionMethod, func(id string) any {
		return domainrpc.AgentSessionRequest{MutationID: id, Name: name, Harness: session.Harness, SessionID: session.ExternalSessionID, Repository: repository}
	}, &result)
	return result, err
}

func (c *Client) AcquireNamedAgent(ctx context.Context, name, token string, duration time.Duration) (domain.NamedAgent, error) {
	return c.agentOwnership(ctx, domainrpc.AcquireAgentMethod, name, token, duration)
}

func (c *Client) RenewNamedAgent(ctx context.Context, name, token string, duration time.Duration) (domain.NamedAgent, error) {
	return c.agentOwnership(ctx, domainrpc.RenewAgentMethod, name, token, duration)
}

func (c *Client) agentOwnership(ctx context.Context, method, name, token string, duration time.Duration) (domain.NamedAgent, error) {
	var result domain.NamedAgent
	err := c.mutatingCall(ctx, method, func(id string) any {
		return domainrpc.AgentOwnershipRequest{MutationID: id, Name: name, OwnerToken: token, Duration: duration}
	}, &result)
	return result, err
}

func (c *Client) ReleaseNamedAgent(ctx context.Context, name, token string) error {
	return c.mutatingCall(ctx, domainrpc.ReleaseAgentMethod, func(id string) any {
		return domainrpc.AgentOwnershipRequest{MutationID: id, Name: name, OwnerToken: token}
	}, nil)
}

func (c *Client) Create(ctx context.Context, message model.Message) error {
	environment := os.Environ()
	defer clearEnvironment(environment)
	return c.mutatingCall(ctx, domainrpc.CreateMethod, func(id string) any {
		return domainrpc.MessageRequest{MutationID: id, Message: message, Environment: environment}
	}, nil)
}

func (c *Client) Reply(ctx context.Context, originalID string, reply model.Message) error {
	environment := os.Environ()
	defer clearEnvironment(environment)
	return c.mutatingCall(ctx, domainrpc.ReplyMethod, func(id string) any {
		return domainrpc.ReplyRequest{MutationID: id, OriginalID: originalID, Reply: reply, Environment: environment}
	}, nil)
}

func clearEnvironment(environment []string) {
	for index := range environment {
		environment[index] = ""
	}
}

func (c *Client) Get(ctx context.Context, id string) (model.Message, error) {
	var result model.Message
	err := c.call(ctx, domainrpc.GetMethod, domainrpc.IDRequest{ID: id}, &result)
	return result, err
}

func (c *Client) List(ctx context.Context, filter model.Filter) ([]model.Message, error) {
	var result []model.Message
	err := c.call(ctx, domainrpc.ListMethod, domainrpc.FilterRequest{Filter: filter}, &result)
	return result, err
}

func (c *Client) ListConversations(ctx context.Context, filter model.ConversationFilter) (model.ConversationPage, error) {
	var result model.ConversationPage
	err := c.call(ctx, domainrpc.ListConversationsMethod, domainrpc.ConversationFilterRequest{Filter: filter}, &result)
	return result, err
}

func (c *Client) ListConversationHistory(ctx context.Context, filter model.ConversationHistoryFilter) (model.MessagePage, error) {
	var result model.MessagePage
	err := c.call(ctx, domainrpc.ConversationHistoryMethod, domainrpc.ConversationHistoryRequest{Filter: filter}, &result)
	return result, err
}

func (c *Client) ListHarnessActivities(ctx context.Context, filter domain.HarnessActivityFilter) ([]domain.HarnessActivity, error) {
	var result []domain.HarnessActivity
	err := c.call(ctx, domainrpc.ListHarnessActivitiesMethod, domainrpc.HarnessActivityFilterRequest{Filter: filter}, &result)
	return result, err
}

func (c *Client) Archive(ctx context.Context, id string) error {
	return c.mutatingCall(ctx, domainrpc.ArchiveMethod, func(mutationID string) any { return domainrpc.MutationIDRequest{MutationID: mutationID, ID: id} }, nil)
}

func (c *Client) Restore(ctx context.Context, id string) error {
	return c.mutatingCall(ctx, domainrpc.RestoreMethod, func(mutationID string) any { return domainrpc.MutationIDRequest{MutationID: mutationID, ID: id} }, nil)
}

func (c *Client) Claim(ctx context.Context, claim domain.Claim, token string) (model.Message, error) {
	var result model.Message
	err := c.mutatingCall(ctx, domainrpc.ClaimMethod, func(id string) any { return domainrpc.ClaimRequest{MutationID: id, Claim: claim, Token: token} }, &result)
	return result, err
}

func (c *Client) Complete(ctx context.Context, id, token string) error {
	return c.mutatingCall(ctx, domainrpc.CompleteMethod, func(mutationID string) any {
		return domainrpc.LeaseRequest{MutationID: mutationID, ID: id, Token: token}
	}, nil)
}

func (c *Client) Release(ctx context.Context, id, token string) error {
	return c.mutatingCall(ctx, domainrpc.ReleaseMethod, func(mutationID string) any {
		return domainrpc.LeaseRequest{MutationID: mutationID, ID: id, Token: token}
	}, nil)
}

func (c *Client) TrustPeer(ctx context.Context, peer domain.Peer) error {
	return c.mutatingCall(ctx, domainrpc.TrustPeerMethod, func(id string) any { return domainrpc.PeerRequest{MutationID: id, Peer: peer} }, nil)
}

func (c *Client) DistrustPeer(ctx context.Context, installationID string) error {
	return c.mutatingCall(ctx, domainrpc.DistrustPeerMethod, func(id string) any {
		return domainrpc.MutationInstallationRequest{MutationID: id, InstallationID: installationID}
	}, nil)
}

func (c *Client) ListPeers(ctx context.Context) ([]domain.Peer, error) {
	var result []domain.Peer
	err := c.call(ctx, domainrpc.ListPeersMethod, nil, &result)
	return result, err
}

func (c *Client) HumanAccount(ctx context.Context) (domain.HumanAccount, error) {
	var result domain.HumanAccount
	err := c.call(ctx, domainrpc.HumanAccountMethod, nil, &result)
	return result, err
}

func (c *Client) HumanDevices(ctx context.Context) ([]domain.HumanDevice, error) {
	var result []domain.HumanDevice
	err := c.call(ctx, domainrpc.HumanDevicesMethod, nil, &result)
	return result, err
}

func (c *Client) CreateHumanInvite(ctx context.Context, request domain.HumanInviteRequest) (domain.PairingBundle, error) {
	var result domain.PairingBundle
	err := c.mutatingCall(ctx, domainrpc.CreateHumanInviteMethod, func(id string) any { return domainrpc.HumanInviteRequest{MutationID: id, Invite: request} }, &result)
	return result, err
}

func (c *Client) JoinHumanInvite(ctx context.Context, bundle []byte) error {
	return c.mutatingCall(ctx, domainrpc.JoinHumanInviteMethod, func(id string) any { return domainrpc.PairingRequest{MutationID: id, Bundle: bundle} }, nil)
}

func (c *Client) RevokeHumanDevice(ctx context.Context, installationID string) error {
	return c.mutatingCall(ctx, domainrpc.RevokeHumanDeviceMethod, func(id string) any {
		return domainrpc.MutationInstallationRequest{MutationID: id, InstallationID: installationID}
	}, nil)
}

func (c *Client) SetMailboxShare(ctx context.Context, mailboxID, peerInstallationID string, active bool) error {
	return c.mutatingCall(ctx, domainrpc.SetMailboxShareMethod, func(id string) any {
		return domainrpc.MailboxShareRequest{MutationID: id, MailboxID: mailboxID, PeerInstallationID: peerInstallationID, Active: active}
	}, nil)
}

func (c *Client) AddRelay(ctx context.Context, relay domain.RelayConfig) error {
	return c.mutatingCall(ctx, domainrpc.AddRelayMethod, func(id string) any { return domainrpc.RelayRequest{MutationID: id, Relay: relay} }, nil)
}

func (c *Client) RemoveRelay(ctx context.Context, url string) error {
	return c.mutatingCall(ctx, domainrpc.RemoveRelayMethod, func(id string) any { return domainrpc.MutationURLRequest{MutationID: id, URL: url} }, nil)
}

func (c *Client) ListRelays(ctx context.Context) ([]domain.RelayConfig, error) {
	var result []domain.RelayConfig
	err := c.call(ctx, domainrpc.ListRelaysMethod, nil, &result)
	return result, err
}

func (c *Client) NetworkStatus(ctx context.Context) (domain.NetworkStatus, error) {
	var result domain.NetworkStatus
	err := c.call(ctx, domainrpc.NetworkStatusMethod, nil, &result)
	return result, err
}

func (c *Client) CreateProject(ctx context.Context, request domain.CreateProjectRequest) (domain.Project, error) {
	var result domain.Project
	err := c.mutatingCall(ctx, domainrpc.CreateProjectMethod, func(id string) any { return domainrpc.CreateProjectRequest{MutationID: id, Project: request} }, &result)
	return result, err
}

func (c *Client) GetProject(ctx context.Context, id string) (domain.Project, error) {
	var result domain.Project
	err := c.call(ctx, domainrpc.GetProjectMethod, domainrpc.ProjectRequest{ProjectID: id}, &result)
	return result, err
}

func (c *Client) ListProjects(ctx context.Context, includeArchived bool) ([]domain.Project, error) {
	var result []domain.Project
	err := c.call(ctx, domainrpc.ListProjectsMethod, domainrpc.ListProjectsRequest{IncludeArchived: includeArchived}, &result)
	return result, err
}

func (c *Client) ListProjectThreads(ctx context.Context, projectID string) ([]domain.ProjectThread, error) {
	var result []domain.ProjectThread
	err := c.call(ctx, domainrpc.ListProjectThreadsMethod, domainrpc.ProjectRequest{ProjectID: projectID}, &result)
	return result, err
}

func (c *Client) projectMutation(ctx context.Context, method, projectID, expected string, build func(domainrpc.ProjectRequest) any) (domain.Project, error) {
	var result domain.Project
	err := c.mutatingCall(ctx, method, func(id string) any {
		return build(domainrpc.ProjectRequest{MutationID: id, ProjectID: projectID, ExpectedHead: expected})
	}, &result)
	return result, err
}

func (c *Client) OpenProject(ctx context.Context, id, expected string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.OpenProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any { return r })
}
func (c *Client) BeginCloseProject(ctx context.Context, id, expected string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.BeginCloseProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any { return r })
}
func (c *Client) FinalizeCloseProject(ctx context.Context, id, expected string, forced bool, observation string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.FinalizeCloseProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.FinalizeCloseProjectRequest{ProjectRequest: r, Forced: forced, RuntimeObservation: observation}
	})
}
func (c *Client) SetProjectArchived(ctx context.Context, id, expected string, archived bool) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.ArchiveProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.ArchiveProjectRequest{ProjectRequest: r, Archived: archived}
	})
}
func (c *Client) UpdateProjectMetadata(ctx context.Context, id, expected, name, brief string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.UpdateProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.UpdateProjectRequest{ProjectRequest: r, Name: name, Brief: brief}
	})
}
func (c *Client) AddProjectPath(ctx context.Context, id, expected string, path domain.ProjectPathInput, primary bool) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.AddProjectPathMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.ProjectPathRequest{ProjectRequest: r, Path: path, Primary: primary}
	})
}
func (c *Client) RemoveProjectResource(ctx context.Context, id, expected, resourceID string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.RemoveProjectResourceMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.ProjectResourceRequest{ProjectRequest: r, ResourceID: resourceID}
	})
}
func (c *Client) ReplaceProjectPath(ctx context.Context, id, expected, resourceID string, path domain.ProjectPathInput) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.ReplaceProjectPathMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.ReplaceProjectPathRequest{ProjectResourceRequest: domainrpc.ProjectResourceRequest{ProjectRequest: r, ResourceID: resourceID}, Path: path}
	})
}
func (c *Client) SetProjectPrimaryResource(ctx context.Context, id, expected, resourceID string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.SetProjectPrimaryMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.ProjectResourceRequest{ProjectRequest: r, ResourceID: resourceID}
	})
}
func (c *Client) CheckProjectResource(ctx context.Context, projectID, resourceID string) (domain.ProjectResource, error) {
	var result domain.ProjectResource
	err := c.call(ctx, domainrpc.CheckProjectResourceMethod, domainrpc.CheckProjectResourceRequest{ProjectID: projectID, ResourceID: resourceID}, &result)
	return result, err
}
func (c *Client) AssignProject(ctx context.Context, id, expected, agent string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.AssignProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.AssignProjectRequest{ProjectRequest: r, AgentName: agent}
	})
}
func (c *Client) ActivateProjectAssignment(ctx context.Context, id, expected string, activation domain.ActivateProjectAssignmentRequest) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.ActivateProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.ActivateProjectRequest{ProjectRequest: r, Activation: activation}
	})
}
func (c *Client) AbortProjectAssignment(ctx context.Context, id, expected, diagnostic string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.AbortProjectAssignmentMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.EndProjectAssignmentRequest{ProjectRequest: r, RuntimeObservation: diagnostic}
	})
}
func (c *Client) BlockProjectAssignment(ctx context.Context, id, expected, diagnostic string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.BlockProjectAssignmentMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.EndProjectAssignmentRequest{ProjectRequest: r, RuntimeObservation: diagnostic}
	})
}
func (c *Client) UnassignProject(ctx context.Context, id, expected string, forced bool, observation string) (domain.Project, error) {
	return c.projectMutation(ctx, domainrpc.UnassignProjectMethod, id, expected, func(r domainrpc.ProjectRequest) any {
		return domainrpc.EndProjectAssignmentRequest{ProjectRequest: r, Forced: forced, RuntimeObservation: observation}
	})
}

func (c *Client) Synchronize(ctx context.Context) error {
	return c.call(ctx, domainrpc.SynchronizeMethod, nil, nil)
}

var _ domain.Store = (*Client)(nil)
var _ domain.HarnessRuntimeController = (*Client)(nil)
var _ domain.ProjectHarnessRuntimeController = (*Client)(nil)
var _ domain.ProjectWorktreeProvisioner = (*Client)(nil)
var _ io.Closer = (*Client)(nil)
