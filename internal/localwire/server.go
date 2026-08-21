package localwire

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sync"
	"time"
)

const DefaultMaximumConcurrentRequests = 32

type Handler func(context.Context, *Session, string, json.RawMessage) (any, *RPCError)

// DeferredResponse runs After only after Value has been written to the peer.
type DeferredResponse struct {
	Value any
	After func()
}

type ModeConfig struct {
	Supported VersionRange
	Handler   Handler
}

type ServerOptions struct {
	Metadata                  PeerMetadata
	Modes                     map[HandshakeMode]ModeConfig
	MaximumFrameBytes         int
	MaximumConcurrentRequests int
	RequestTimeout            time.Duration
	HandshakeTimeout          time.Duration
	WriteTimeout              time.Duration
}

type Server struct{ options ServerOptions }

func NewServer(options ServerOptions) (*Server, error) {
	if len(options.Modes) == 0 {
		return nil, errors.New("local-wire server needs at least one handshake mode")
	}
	for mode, config := range options.Modes {
		if mode != DomainMode && mode != LifecycleMode {
			return nil, fmt.Errorf("unsupported local-wire handshake mode %q", mode)
		}
		if err := config.Supported.Validate(); err != nil {
			return nil, fmt.Errorf("%s mode: %w", mode, err)
		}
		if config.Handler == nil {
			return nil, fmt.Errorf("%s mode needs a handler", mode)
		}
	}
	if options.MaximumConcurrentRequests <= 0 {
		options.MaximumConcurrentRequests = DefaultMaximumConcurrentRequests
	}
	if options.RequestTimeout <= 0 {
		options.RequestTimeout = 5 * time.Second
	}
	if options.HandshakeTimeout <= 0 {
		options.HandshakeTimeout = 5 * time.Second
	}
	if options.WriteTimeout <= 0 {
		options.WriteTimeout = 5 * time.Second
	}
	return &Server{options: options}, nil
}

type Session struct {
	codec   *Codec
	mode    HandshakeMode
	version int
	client  PeerMetadata
	done    <-chan struct{}
}

func (s *Session) Mode() HandshakeMode  { return s.mode }
func (s *Session) Version() int         { return s.version }
func (s *Session) Client() PeerMetadata { return s.client }

func (s *Session) Notify(method string, params any) error {
	return s.NotifySubscription("", method, params)
}

func (s *Session) NotifySubscription(subscriptionID, method string, params any) error {
	raw, err := marshalRaw(params)
	if err != nil {
		return err
	}
	select {
	case <-s.done:
		return errors.New("local-wire session is closed")
	default:
		return s.codec.Write(Envelope{Kind: NotificationKind, Version: s.version, SubscriptionID: subscriptionID, Method: method, Params: raw})
	}
}

func (s *Server) ServeConn(ctx context.Context, connection io.ReadWriteCloser) error {
	if connection == nil {
		return errors.New("local-wire server needs a connection")
	}
	defer connection.Close()
	if deadlineConnection, ok := connection.(interface{ SetDeadline(time.Time) error }); ok {
		if err := deadlineConnection.SetDeadline(time.Now().Add(s.options.HandshakeTimeout)); err != nil {
			return err
		}
	}
	codec := NewCodecWithTimeout(connection, connection, s.options.MaximumFrameBytes, s.options.WriteTimeout)
	first, err := codec.Read()
	if err != nil {
		return fmt.Errorf("read local-wire handshake: %w", err)
	}
	if first.Kind != HandshakeKind || len(first.Params) == 0 {
		_ = codec.Write(Envelope{Kind: ErrorKind, ID: "handshake", Error: &RPCError{Code: CodeInvalidRequest, Message: "the first local-wire message must be a handshake"}})
		return errors.New("local-wire command received before handshake")
	}
	var request HandshakeRequest
	if err := decodeStrict(first.Params, &request); err != nil {
		_ = codec.Write(Envelope{Kind: ErrorKind, ID: "handshake", Error: &RPCError{Code: CodeInvalidRequest, Message: "invalid local-wire handshake"}})
		return fmt.Errorf("decode local-wire handshake: %w", err)
	}
	config, exists := s.options.Modes[request.Mode]
	if !exists {
		_ = codec.Write(Envelope{Kind: ErrorKind, ID: "handshake", Error: &RPCError{Code: CodeInvalidRequest, Message: fmt.Sprintf("local-wire mode %q is unavailable", request.Mode)}})
		return fmt.Errorf("local-wire mode %q is unavailable", request.Mode)
	}
	version, err := Negotiate(request.Supported, config.Supported)
	if err != nil {
		var incompatible *IncompatibilityError
		if errors.As(err, &incompatible) {
			_ = codec.Write(Envelope{Kind: ErrorKind, ID: "handshake", Error: incompatible.RPCError()})
		}
		return err
	}
	result, err := marshalRaw(HandshakeResponse{Mode: request.Mode, Version: version, Supported: config.Supported, Server: s.options.Metadata})
	if err != nil {
		return err
	}
	if err := codec.Write(Envelope{Kind: HandshakeKind, Result: result}); err != nil {
		return fmt.Errorf("write local-wire handshake: %w", err)
	}
	if deadlineConnection, ok := connection.(interface{ SetDeadline(time.Time) error }); ok {
		if err := deadlineConnection.SetDeadline(time.Time{}); err != nil {
			return err
		}
	}
	sessionCtx, cancel := context.WithCancel(ctx)
	session := &Session{codec: codec, mode: request.Mode, version: version, client: request.Client, done: sessionCtx.Done()}
	concurrency := make(chan struct{}, s.options.MaximumConcurrentRequests)
	var handlers sync.WaitGroup
	defer func() {
		cancel()
		handlers.Wait()
	}()
	for {
		envelope, err := codec.Read()
		if err != nil {
			if errors.Is(err, io.EOF) || sessionCtx.Err() != nil {
				return nil
			}
			return fmt.Errorf("read local-wire request: %w", err)
		}
		if envelope.Kind != RequestKind {
			return fmt.Errorf("unexpected local-wire %s after handshake", envelope.Kind)
		}
		if envelope.Version != version {
			_ = codec.Write(Envelope{Kind: ErrorKind, Version: version, ID: envelope.ID, Error: &RPCError{Code: CodeInvalidRequest, Message: "request wire version does not match the negotiated version"}})
			continue
		}
		select {
		case concurrency <- struct{}{}:
		case <-sessionCtx.Done():
			return nil
		default:
			_ = codec.Write(Envelope{Kind: ErrorKind, Version: version, ID: envelope.ID, Error: &RPCError{Code: CodeInvalidRequest, Message: "too many concurrent local-wire requests"}})
			continue
		}
		handlers.Add(1)
		go func(request Envelope) {
			defer handlers.Done()
			defer func() { <-concurrency }()
			requestCtx, requestCancel := context.WithTimeout(sessionCtx, s.options.RequestTimeout)
			defer requestCancel()
			result, rpcErr := config.Handler(requestCtx, session, request.Method, request.Params)
			if rpcErr != nil {
				_ = codec.Write(Envelope{Kind: ErrorKind, Version: version, ID: request.ID, Error: rpcErr})
				return
			}
			var after func()
			if deferred, ok := result.(DeferredResponse); ok {
				result = deferred.Value
				after = deferred.After
			}
			raw, err := marshalRaw(result)
			if err != nil {
				_ = codec.Write(Envelope{Kind: ErrorKind, Version: version, ID: request.ID, Error: &RPCError{Code: CodeInternal, Message: "encode local-wire response"}})
				return
			}
			writeErr := codec.Write(Envelope{Kind: ResponseKind, Version: version, ID: request.ID, Result: raw})
			if writeErr == nil && after != nil {
				after()
			}
		}(envelope)
	}
}
