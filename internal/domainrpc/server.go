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
		domain.MutationLog
	}
	Synchronize func(context.Context) error
}

func (s Service) Handle(ctx context.Context, _ *localwire.Session, method string, raw json.RawMessage) (any, *localwire.RPCError) {
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
			return result, nil
		}
		ctx = domain.WithMutation(ctx, mutation)
	}
	result, err := s.dispatch(ctx, method, raw)
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
	ResolveMailboxMethod: true, CreateMethod: true, ReplyMethod: true, ArchiveMethod: true,
	ClaimMethod: true, CompleteMethod: true, ReleaseMethod: true, TrustPeerMethod: true,
	DistrustPeerMethod: true, CreateHumanInviteMethod: true, JoinHumanInviteMethod: true,
	RevokeHumanDeviceMethod: true, SetMailboxShareMethod: true, AddRelayMethod: true,
	RemoveRelayMethod: true,
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

func (s Service) dispatch(ctx context.Context, method string, raw json.RawMessage) (any, error) {
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
	case CreateMethod:
		var request MessageRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.Create(ctx, request.Message)
	case ReplyMethod:
		var request ReplyRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.Reply(ctx, request.OriginalID, request.Reply)
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
	case ArchiveMethod:
		var request MutationIDRequest
		if err := decodeRequest(raw, &request); err != nil {
			return nil, err
		}
		return nil, s.Store.Archive(ctx, request.ID)
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
	case SynchronizeMethod:
		if s.Synchronize == nil {
			return nil, errors.New("node synchronization is unavailable")
		}
		return nil, s.Synchronize(ctx)
	default:
		return nil, &methodNotFoundError{method: method}
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
