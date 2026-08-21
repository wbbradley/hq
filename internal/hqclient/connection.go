package hqclient

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/domainrpc"
	"github.com/wbbradley/hq/internal/localwire"
	"github.com/wbbradley/hq/internal/nostrwire"
)

type ConnectionPhase string

const (
	ConnectionConnecting   ConnectionPhase = "connecting"
	ConnectionConnected    ConnectionPhase = "connected"
	ConnectionDrift        ConnectionPhase = "drift"
	ConnectionIncompatible ConnectionPhase = "incompatible"
	ConnectionDisconnected ConnectionPhase = "disconnected"
)

type ConnectionState struct {
	Phase     ConnectionPhase
	Handshake localwire.HandshakeResponse
	Err       error
}

func (s ConnectionState) Diagnostic() string {
	switch s.Phase {
	case ConnectionDrift:
		return "HQ client and local node builds differ; restart the local HQ node"
	case ConnectionIncompatible:
		return fmt.Sprintf("HQ local protocol is incompatible: %v", s.Err)
	case ConnectionDisconnected:
		return fmt.Sprintf("HQ local node disconnected: %v", s.Err)
	default:
		return ""
	}
}

type Subscription struct {
	client  *Client
	id      string
	topics  []domain.ChangeTopic
	changes chan domain.Invalidation
	mu      sync.Mutex
	last    uint64
	close   sync.Once
}

func (c *Client) States() <-chan ConnectionState { return c.states }

func (c *Client) Subscribe(ctx context.Context, topics ...domain.ChangeTopic) (*Subscription, error) {
	id, err := uuid.NewV7()
	if err != nil {
		return nil, err
	}
	subscription := &Subscription{
		client: c, id: id.String(), topics: append([]domain.ChangeTopic(nil), topics...),
		changes: make(chan domain.Invalidation, 1),
	}
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil, errors.New("domain client is closed")
	}
	c.subscriptions[subscription.id] = subscription
	c.mu.Unlock()
	var acknowledged domainrpc.SubscribeChangesResponse
	err = c.call(ctx, domainrpc.SubscribeChangesMethod, domainrpc.SubscribeChangesRequest{
		SubscriptionID: subscription.id, Topics: subscription.topics,
	}, &acknowledged)
	if err != nil {
		subscription.Close()
		return nil, err
	}
	subscription.mu.Lock()
	subscription.last = acknowledged.Revision
	subscription.mu.Unlock()
	return subscription, nil
}

func (s *Subscription) Changes() <-chan domain.Invalidation { return s.changes }

func (s *Subscription) Close() {
	s.close.Do(func() {
		s.client.mu.Lock()
		delete(s.client.subscriptions, s.id)
		s.client.mu.Unlock()
	})
}

func (s *Subscription) deliver(change domain.Invalidation) {
	s.mu.Lock()
	if !change.FullSnapshot && change.Revision <= s.last {
		s.mu.Unlock()
		return
	}
	if change.Revision > s.last {
		s.last = change.Revision
	}
	s.mu.Unlock()
	select {
	case queued := <-s.changes:
		change = mergeClientInvalidations(queued, change)
	default:
	}
	select {
	case s.changes <- change:
	default:
	}
}

func (c *Client) currentWire() *localwire.Client {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.wire
}

func (c *Client) attach(wireClient *localwire.Client) {
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		wireClient.Close()
		return
	}
	c.wire = wireClient
	c.mu.Unlock()
	handshake := wireClient.Handshake()
	phase := ConnectionConnected
	if wireClient.BinaryDrift() {
		phase = ConnectionDrift
	}
	c.publishState(ConnectionState{Phase: phase, Handshake: handshake})
	go c.monitor(wireClient)
}

func (c *Client) monitor(wireClient *localwire.Client) {
	for {
		select {
		case notice := <-wireClient.Notifications():
			if notice.Method != domainrpc.InvalidatedMethod {
				continue
			}
			var change domain.Invalidation
			if json.Unmarshal(notice.Params, &change) != nil {
				continue
			}
			c.mu.RLock()
			subscription := c.subscriptions[notice.SubscriptionID]
			c.mu.RUnlock()
			if subscription != nil {
				subscription.deliver(change)
			}
		case <-wireClient.Done():
			c.mu.RLock()
			current, closed := c.wire, c.closed
			c.mu.RUnlock()
			if closed || current != wireClient {
				return
			}
			c.publishState(ConnectionState{Phase: ConnectionDisconnected, Err: wireClient.Err()})
			go c.reconnectUntilReady(wireClient)
			return
		}
	}
}

func (c *Client) reconnectUntilReady(failed *localwire.Client) {
	attempt := 0
	for {
		if _, err := c.reconnect(c.lifetime, failed); err == nil {
			return
		}
		select {
		case <-c.lifetime.Done():
			return
		case <-time.After(nostrwire.Backoff(attempt)):
			attempt++
		}
	}
}

func (c *Client) reconnect(ctx context.Context, failed *localwire.Client) (*localwire.Client, error) {
	c.reconnectMu.Lock()
	defer c.reconnectMu.Unlock()
	c.mu.RLock()
	current, connector, closed := c.wire, c.connect, c.closed
	c.mu.RUnlock()
	if closed {
		return nil, errors.New("domain client is closed")
	}
	if current != nil && current != failed {
		return current, nil
	}
	if connector == nil {
		return nil, errors.New("domain client cannot reconnect")
	}
	c.publishState(ConnectionState{Phase: ConnectionConnecting})
	wireClient, err := connector(ctx)
	if err != nil {
		c.publishConnectionError(err)
		return nil, err
	}
	c.attach(wireClient)
	if err := c.resubscribe(ctx, wireClient); err != nil {
		wireClient.Close()
		return nil, err
	}
	return wireClient, nil
}

func (c *Client) resubscribe(ctx context.Context, wireClient *localwire.Client) error {
	c.mu.RLock()
	subscriptions := make([]*Subscription, 0, len(c.subscriptions))
	for _, subscription := range c.subscriptions {
		subscriptions = append(subscriptions, subscription)
	}
	c.mu.RUnlock()
	for _, subscription := range subscriptions {
		var acknowledged domainrpc.SubscribeChangesResponse
		err := wireClient.Call(ctx, domainrpc.SubscribeChangesMethod, domainrpc.SubscribeChangesRequest{
			SubscriptionID: subscription.id, Topics: subscription.topics,
		}, &acknowledged)
		if err != nil {
			return err
		}
		subscription.deliver(domain.Invalidation{Revision: acknowledged.Revision, FullSnapshot: true})
	}
	return nil
}

func (c *Client) publishState(state ConnectionState) {
	select {
	case <-c.states:
	default:
	}
	select {
	case c.states <- state:
	default:
	}
}

func (c *Client) publishConnectionError(err error) {
	phase := ConnectionDisconnected
	var incompatible *localwire.IncompatibilityError
	if errors.As(err, &incompatible) {
		phase = ConnectionIncompatible
	}
	c.publishState(ConnectionState{Phase: phase, Err: err})
}

func reconnectable(err error) bool {
	if err == nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		return false
	}
	var rpcError *localwire.RPCError
	return !errors.As(err, &rpcError)
}

func mergeClientInvalidations(left, right domain.Invalidation) domain.Invalidation {
	result := domain.Invalidation{Revision: max(left.Revision, right.Revision), FullSnapshot: left.FullSnapshot || right.FullSnapshot}
	seen := make(map[domain.ChangeTopic]bool)
	for _, topics := range [][]domain.ChangeTopic{left.Topics, right.Topics} {
		for _, topic := range topics {
			if !seen[topic] {
				seen[topic] = true
				result.Topics = append(result.Topics, topic)
			}
		}
	}
	return result
}
