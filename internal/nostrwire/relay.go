package nostrwire

import (
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"
)

type OK struct {
	Accepted bool
	Message  string
}

type Filter struct {
	Kinds []int               `json:"kinds,omitempty"`
	Tags  map[string][]string `json:"-"`
	Until int64               `json:"until,omitempty"`
	Limit int                 `json:"limit,omitempty"`
}

func (f Filter) MarshalJSON() ([]byte, error) {
	value := make(map[string]any)
	if len(f.Kinds) > 0 {
		value["kinds"] = f.Kinds
	}
	if f.Until != 0 {
		value["until"] = f.Until
	}
	if f.Limit != 0 {
		value["limit"] = f.Limit
	}
	for name, values := range f.Tags {
		value["#"+name] = values
	}
	return json.Marshal(value)
}

type Subscription struct {
	ID     string
	Events chan []byte
	EOSE   chan struct{}
	Closed chan string
	Frames chan SubscriptionFrame
	once   sync.Once
}

type SubscriptionFrame struct {
	Event []byte
	EOSE  bool
}

type RelayClient interface {
	Publish(context.Context, []byte, string) (OK, error)
	Subscribe(context.Context, string, Filter) (*Subscription, error)
	WaitAuth(context.Context) error
	Authenticated() bool
	Close() error
}

type Dialer interface {
	Dial(context.Context, string, *Codec) (RelayClient, error)
}

type WebSocketDialer struct{}

func (WebSocketDialer) Dial(ctx context.Context, relayURL string, codec *Codec) (RelayClient, error) {
	connection, _, err := websocket.Dial(ctx, relayURL, nil)
	if err != nil {
		return nil, err
	}
	connection.SetReadLimit(MaxGiftWrapBytes + 64<<10)
	clientCtx, cancel := context.WithCancel(context.Background())
	client := &WebSocketClient{
		url: relayURL, codec: codec, connection: connection, ctx: clientCtx, cancel: cancel,
		pending: make(map[string]chan OK), subscriptions: make(map[string]*Subscription), authDone: make(chan struct{}),
	}
	go client.readLoop()
	return client, nil
}

type WebSocketClient struct {
	url           string
	codec         *Codec
	connection    *websocket.Conn
	ctx           context.Context
	cancel        context.CancelFunc
	writeMu       sync.Mutex
	mu            sync.Mutex
	pending       map[string]chan OK
	subscriptions map[string]*Subscription
	authIDs       map[string]bool
	authDone      chan struct{}
	authStart     sync.Once
	authOnce      sync.Once
	authError     error
	authed        atomic.Bool
	closeOnce     sync.Once
}

func (c *WebSocketClient) Publish(ctx context.Context, raw []byte, eventID string) (OK, error) {
	result := make(chan OK, 1)
	c.mu.Lock()
	c.pending[eventID] = result
	c.mu.Unlock()
	defer func() {
		c.mu.Lock()
		delete(c.pending, eventID)
		c.mu.Unlock()
	}()
	frame, err := json.Marshal([]any{"EVENT", json.RawMessage(raw)})
	if err != nil {
		return OK{}, err
	}
	if err := c.write(ctx, frame); err != nil {
		return OK{}, err
	}
	select {
	case ok := <-result:
		return ok, nil
	case <-ctx.Done():
		return OK{}, ctx.Err()
	case <-c.ctx.Done():
		return OK{}, errors.New("relay disconnected")
	}
}

func (c *WebSocketClient) Subscribe(ctx context.Context, id string, filter Filter) (*Subscription, error) {
	if id == "" {
		return nil, errors.New("subscription ID is required")
	}
	sub := &Subscription{ID: id, Events: make(chan []byte, 512), EOSE: make(chan struct{}), Closed: make(chan string, 1), Frames: make(chan SubscriptionFrame, 512)}
	c.mu.Lock()
	if _, exists := c.subscriptions[id]; exists {
		c.mu.Unlock()
		return nil, errors.New("subscription ID is already active")
	}
	c.subscriptions[id] = sub
	c.mu.Unlock()
	filterJSON, err := json.Marshal(filter)
	if err != nil {
		return nil, err
	}
	frame, err := json.Marshal([]any{"REQ", id, json.RawMessage(filterJSON)})
	if err != nil {
		return nil, err
	}
	if err := c.write(ctx, frame); err != nil {
		c.removeSubscription(id, err.Error())
		return nil, err
	}
	go func() {
		select {
		case <-ctx.Done():
			_ = c.closeSubscription(context.Background(), id)
		case <-c.ctx.Done():
		}
	}()
	return sub, nil
}

func (c *WebSocketClient) closeSubscription(ctx context.Context, id string) error {
	frame, _ := json.Marshal([]any{"CLOSE", id})
	err := c.write(ctx, frame)
	c.removeSubscription(id, "closed")
	return err
}

func (c *WebSocketClient) WaitAuth(ctx context.Context) error {
	select {
	case <-c.authDone:
		return c.authError
	case <-ctx.Done():
		return ctx.Err()
	case <-c.ctx.Done():
		return errors.New("relay disconnected before authentication")
	}
}

func (c *WebSocketClient) Authenticated() bool { return c.authed.Load() }

func (c *WebSocketClient) Close() error {
	c.closeOnce.Do(func() {
		c.cancel()
		_ = c.connection.Close(websocket.StatusNormalClosure, "hq sync complete")
	})
	return nil
}

func (c *WebSocketClient) write(ctx context.Context, raw []byte) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	return c.connection.Write(ctx, websocket.MessageText, raw)
}

func (c *WebSocketClient) readLoop() {
	defer c.Close()
	for {
		_, raw, err := c.connection.Read(c.ctx)
		if err != nil {
			c.failAll(err.Error())
			return
		}
		c.handle(raw)
	}
}

func (c *WebSocketClient) handle(raw []byte) {
	var parts []json.RawMessage
	if json.Unmarshal(raw, &parts) != nil || len(parts) == 0 {
		return
	}
	var label string
	if json.Unmarshal(parts[0], &label) != nil {
		return
	}
	switch label {
	case "OK":
		if len(parts) != 4 {
			return
		}
		var id, message string
		var accepted bool
		if json.Unmarshal(parts[1], &id) != nil || json.Unmarshal(parts[2], &accepted) != nil || json.Unmarshal(parts[3], &message) != nil {
			return
		}
		c.mu.Lock()
		pending := c.pending[id]
		isAuth := c.authIDs != nil && c.authIDs[id]
		c.mu.Unlock()
		if pending != nil {
			select {
			case pending <- OK{Accepted: accepted, Message: message}:
			default:
			}
		}
		if isAuth {
			if accepted {
				c.authed.Store(true)
			} else {
				c.authError = fmt.Errorf("relay rejected NIP-42 auth: %s", message)
			}
			c.authOnce.Do(func() { close(c.authDone) })
		}
	case "AUTH":
		if len(parts) != 2 {
			return
		}
		var challenge string
		if json.Unmarshal(parts[1], &challenge) == nil {
			c.authStart.Do(func() { go c.authenticate(challenge) })
		}
	case "EVENT":
		if len(parts) != 3 {
			return
		}
		var id string
		if json.Unmarshal(parts[1], &id) != nil {
			return
		}
		c.mu.Lock()
		sub := c.subscriptions[id]
		c.mu.Unlock()
		if sub != nil {
			copyRaw := append([]byte(nil), parts[2]...)
			select {
			case sub.Frames <- SubscriptionFrame{Event: copyRaw}:
			case <-c.ctx.Done():
			}
		}
	case "EOSE":
		if len(parts) != 2 {
			return
		}
		var id string
		_ = json.Unmarshal(parts[1], &id)
		c.mu.Lock()
		sub := c.subscriptions[id]
		c.mu.Unlock()
		if sub != nil {
			sub.once.Do(func() {
				select {
				case sub.Frames <- SubscriptionFrame{EOSE: true}:
				case <-c.ctx.Done():
				}
				close(sub.EOSE)
			})
		}
	case "CLOSED":
		if len(parts) != 3 {
			return
		}
		var id, reason string
		_ = json.Unmarshal(parts[1], &id)
		_ = json.Unmarshal(parts[2], &reason)
		c.removeSubscription(id, reason)
	case "NOTICE":
		return
	}
}

func (c *WebSocketClient) authenticate(challenge string) {
	auth, err := c.codec.AuthEvent(c.url, challenge)
	if err != nil {
		c.authError = err
		c.authOnce.Do(func() { close(c.authDone) })
		return
	}
	raw, err := json.Marshal(auth)
	if err != nil {
		c.authError = err
		c.authOnce.Do(func() { close(c.authDone) })
		return
	}
	c.mu.Lock()
	if c.authIDs == nil {
		c.authIDs = make(map[string]bool)
	}
	c.authIDs[auth.ID] = true
	c.mu.Unlock()
	frame, _ := json.Marshal([]any{"AUTH", json.RawMessage(raw)})
	if err := c.write(c.ctx, frame); err != nil {
		c.authError = err
		c.authOnce.Do(func() { close(c.authDone) })
	}
}

func (c *WebSocketClient) removeSubscription(id, reason string) {
	c.mu.Lock()
	sub := c.subscriptions[id]
	delete(c.subscriptions, id)
	c.mu.Unlock()
	if sub != nil {
		select {
		case sub.Closed <- reason:
		default:
		}
	}
}

func (c *WebSocketClient) failAll(reason string) {
	c.mu.Lock()
	subs := make([]*Subscription, 0, len(c.subscriptions))
	for _, sub := range c.subscriptions {
		subs = append(subs, sub)
	}
	c.subscriptions = make(map[string]*Subscription)
	c.mu.Unlock()
	for _, sub := range subs {
		select {
		case sub.Closed <- reason:
		default:
		}
	}
}

func Backoff(attempt int) time.Duration {
	if attempt < 0 {
		attempt = 0
	}
	if attempt > 8 {
		attempt = 8
	}
	return time.Second * time.Duration(1<<attempt)
}

func BackoffWithJitter(attempt int, random io.Reader) time.Duration {
	base := Backoff(attempt)
	if random == nil {
		random = rand.Reader
	}
	span := base / 2
	value, err := rand.Int(random, big.NewInt(int64(span)+1))
	if err != nil {
		return base
	}
	return base - base/4 + time.Duration(value.Int64())
}
