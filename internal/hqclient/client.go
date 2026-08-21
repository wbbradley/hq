package hqclient

import (
	"context"
	"io"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/buildinfo"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/syncer"
)

type Client struct {
	wire *localwire.Client
}

func Open(ctx context.Context, databasePath string) (*Client, error) {
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
	wireClient, err := localwire.NewClient(ctx, connection, localwire.ClientOptions{
		Mode: localwire.DomainMode, Supported: localwire.DomainVersions,
		Metadata: localwire.PeerMetadata{Build: buildinfo.Version},
	})
	if err != nil {
		connection.Close()
		return nil, err
	}
	return New(wireClient), nil
}

func New(wireClient *localwire.Client) *Client { return &Client{wire: wireClient} }

func (c *Client) Close() error {
	if c == nil || c.wire == nil {
		return nil
	}
	return c.wire.Close()
}

func (c *Client) call(ctx context.Context, method string, request, response any) error {
	return domainrpc.DecodeError(c.wire.Call(ctx, method, request, response))
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

func (c *Client) Create(ctx context.Context, message model.Message) error {
	return c.mutatingCall(ctx, domainrpc.CreateMethod, func(id string) any { return domainrpc.MessageRequest{MutationID: id, Message: message} }, nil)
}

func (c *Client) Reply(ctx context.Context, originalID string, reply model.Message) error {
	return c.mutatingCall(ctx, domainrpc.ReplyMethod, func(id string) any {
		return domainrpc.ReplyRequest{MutationID: id, OriginalID: originalID, Reply: reply}
	}, nil)
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

func (c *Client) Archive(ctx context.Context, id string) error {
	return c.mutatingCall(ctx, domainrpc.ArchiveMethod, func(mutationID string) any { return domainrpc.MutationIDRequest{MutationID: mutationID, ID: id} }, nil)
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

func (c *Client) Synchronize(ctx context.Context) error {
	return c.call(ctx, domainrpc.SynchronizeMethod, nil, nil)
}

var _ domain.Store = (*Client)(nil)
var _ io.Closer = (*Client)(nil)
