package localwire

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strconv"
	"sync"
	"time"
)

const DefaultNotificationBuffer = 64

type ClientOptions struct {
	Mode               HandshakeMode
	Supported          VersionRange
	Metadata           PeerMetadata
	MaximumFrameBytes  int
	NotificationBuffer int
	HandshakeTimeout   time.Duration
	WriteTimeout       time.Duration
}

type Notification struct {
	SubscriptionID string
	Method         string
	Params         json.RawMessage
}

type callResult struct {
	result json.RawMessage
	err    error
}

type Client struct {
	connection io.ReadWriteCloser
	codec      *Codec
	handshake  HandshakeResponse
	client     PeerMetadata

	stateMu sync.Mutex
	nextID  uint64
	pending map[string]chan callResult
	doneErr error
	done    chan struct{}
	stop    sync.Once
	notices chan Notification
}

func NewClient(ctx context.Context, connection io.ReadWriteCloser, options ClientOptions) (*Client, error) {
	if connection == nil {
		return nil, errors.New("local-wire client needs a connection")
	}
	if options.Mode == "" {
		options.Mode = DomainMode
	}
	if err := options.Supported.Validate(); err != nil {
		return nil, err
	}
	if options.NotificationBuffer <= 0 {
		options.NotificationBuffer = DefaultNotificationBuffer
	}
	if options.HandshakeTimeout <= 0 {
		options.HandshakeTimeout = 5 * time.Second
	}
	if options.WriteTimeout <= 0 {
		options.WriteTimeout = 5 * time.Second
	}
	if deadlineConnection, ok := connection.(interface{ SetDeadline(time.Time) error }); ok {
		if err := deadlineConnection.SetDeadline(time.Now().Add(options.HandshakeTimeout)); err != nil {
			return nil, err
		}
		defer deadlineConnection.SetDeadline(time.Time{})
	}
	codec := NewCodecWithTimeout(connection, connection, options.MaximumFrameBytes, options.WriteTimeout)
	params, err := marshalRaw(HandshakeRequest{Mode: options.Mode, Supported: options.Supported, Client: options.Metadata})
	if err != nil {
		return nil, err
	}
	if err := codec.Write(Envelope{Kind: HandshakeKind, Params: params}); err != nil {
		return nil, fmt.Errorf("write local-wire handshake: %w", err)
	}
	response, err := codec.Read()
	if err != nil {
		return nil, fmt.Errorf("read local-wire handshake: %w", err)
	}
	if response.Kind == ErrorKind {
		return nil, incompatibilityFromRPC(response.Error)
	}
	if response.Kind != HandshakeKind || len(response.Result) == 0 {
		return nil, errors.New("local-wire server did not return a handshake")
	}
	var handshake HandshakeResponse
	if err := decodeStrict(response.Result, &handshake); err != nil {
		return nil, fmt.Errorf("decode local-wire handshake: %w", err)
	}
	if handshake.Mode != options.Mode {
		return nil, fmt.Errorf("local-wire server negotiated mode %q, want %q", handshake.Mode, options.Mode)
	}
	if handshake.Version < options.Supported.Min || handshake.Version > options.Supported.Max {
		return nil, fmt.Errorf("local-wire server selected unsupported version %d", handshake.Version)
	}
	client := &Client{
		connection: connection, codec: codec, handshake: handshake, client: options.Metadata,
		pending: make(map[string]chan callResult), done: make(chan struct{}),
		notices: make(chan Notification, options.NotificationBuffer),
	}
	go client.readLoop(ctx)
	return client, nil
}

func (c *Client) Handshake() HandshakeResponse { return c.handshake }

func (c *Client) BinaryDrift() bool {
	return c.client.Build != "" && c.handshake.Server.Build != "" && c.handshake.Server.Build != c.client.Build
}

func (c *Client) Call(ctx context.Context, method string, params, destination any) error {
	c.stateMu.Lock()
	if c.doneErr != nil {
		err := c.doneErr
		c.stateMu.Unlock()
		return err
	}
	c.nextID++
	id := strconv.FormatUint(c.nextID, 10)
	response := make(chan callResult, 1)
	c.pending[id] = response
	c.stateMu.Unlock()
	raw, err := marshalRaw(params)
	if err != nil {
		c.removePending(id)
		return err
	}
	request := Envelope{Kind: RequestKind, Version: c.handshake.Version, ID: id, Method: method, Params: raw}
	if err := c.codec.Write(request); err != nil {
		c.removePending(id)
		c.finish(fmt.Errorf("write %s request: %w", method, err))
		return err
	}
	select {
	case reply := <-response:
		if reply.err != nil {
			return fmt.Errorf("%s: %w", method, reply.err)
		}
		if destination == nil || len(reply.result) == 0 || bytes.Equal(reply.result, []byte("null")) {
			return nil
		}
		if err := decodeStrict(reply.result, destination); err != nil {
			return fmt.Errorf("decode %s response: %w", method, err)
		}
		return nil
	case <-ctx.Done():
		c.removePending(id)
		return ctx.Err()
	case <-c.done:
		c.removePending(id)
		return c.Err()
	}
}

func (c *Client) Notifications() <-chan Notification { return c.notices }
func (c *Client) Done() <-chan struct{}              { return c.done }

func (c *Client) Err() error {
	c.stateMu.Lock()
	defer c.stateMu.Unlock()
	return c.doneErr
}

func (c *Client) Close() error {
	err := c.connection.Close()
	c.finish(errors.New("local-wire client closed"))
	return err
}

func (c *Client) readLoop(ctx context.Context) {
	go func() {
		select {
		case <-ctx.Done():
			_ = c.connection.Close()
		case <-c.done:
		}
	}()
	for {
		envelope, err := c.codec.Read()
		if err != nil {
			c.finish(fmt.Errorf("read local-wire message: %w", err))
			return
		}
		if envelope.Version != c.handshake.Version {
			c.finish(fmt.Errorf("local-wire message version %d does not match negotiated version %d", envelope.Version, c.handshake.Version))
			return
		}
		switch envelope.Kind {
		case ResponseKind, ErrorKind:
			c.stateMu.Lock()
			response := c.pending[envelope.ID]
			delete(c.pending, envelope.ID)
			c.stateMu.Unlock()
			if response == nil {
				continue
			}
			if envelope.Kind == ErrorKind {
				response <- callResult{err: envelope.Error}
			} else {
				response <- callResult{result: envelope.Result}
			}
		case NotificationKind:
			select {
			case c.notices <- Notification{SubscriptionID: envelope.SubscriptionID, Method: envelope.Method, Params: envelope.Params}:
			case <-c.done:
				return
			default:
				c.finish(&RPCError{Code: CodeNotificationOverflow, Message: "local-wire notification buffer is full; reconnect and resubscribe"})
				_ = c.connection.Close()
				return
			}
		default:
			c.finish(fmt.Errorf("unexpected local-wire %s after handshake", envelope.Kind))
			_ = c.connection.Close()
			return
		}
	}
}

func (c *Client) removePending(id string) {
	c.stateMu.Lock()
	delete(c.pending, id)
	c.stateMu.Unlock()
}

func (c *Client) finish(err error) {
	c.stop.Do(func() {
		if err == nil {
			err = errors.New("local-wire connection stopped")
		}
		c.stateMu.Lock()
		c.doneErr = err
		pending := c.pending
		c.pending = make(map[string]chan callResult)
		c.stateMu.Unlock()
		for _, response := range pending {
			response <- callResult{err: err}
		}
		close(c.done)
	})
}

func decodeStrict(raw []byte, destination any) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(destination); err != nil {
		return err
	}
	if decoder.Decode(&struct{}{}) != io.EOF {
		return errors.New("trailing JSON value")
	}
	return nil
}
