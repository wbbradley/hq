package domainrpc

import (
	"errors"
	"sync"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/localwire"
)

type SubscriptionHub struct {
	mu          sync.Mutex
	subscribers map[*changeSubscriber]bool
}

type changeSubscriber struct {
	hub            *SubscriptionHub
	session        *localwire.Session
	id             string
	topics         map[domain.ChangeTopic]bool
	queue          chan domain.Invalidation
	active         bool
	pending        domain.Invalidation
	activationOnce sync.Once
}

func NewSubscriptionHub() *SubscriptionHub {
	return &SubscriptionHub{subscribers: make(map[*changeSubscriber]bool)}
}

func (h *SubscriptionHub) Register(session *localwire.Session, id string, topics []domain.ChangeTopic) (*changeSubscriber, error) {
	if session == nil {
		return nil, errors.New("change subscription needs a local-wire session")
	}
	if id == "" {
		return nil, errors.New("subscription_id is required")
	}
	subscriber := &changeSubscriber{
		hub: h, session: session, id: id, topics: make(map[domain.ChangeTopic]bool),
		queue: make(chan domain.Invalidation, 1),
	}
	for _, topic := range topics {
		subscriber.topics[topic] = true
	}
	h.mu.Lock()
	h.subscribers[subscriber] = true
	h.mu.Unlock()
	go func() {
		<-session.Done()
		subscriber.Close()
	}()
	return subscriber, nil
}

func (h *SubscriptionHub) Publish(change domain.Invalidation) {
	if change.Revision == 0 {
		return
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	for subscriber := range h.subscribers {
		filtered := subscriber.filter(change)
		if len(filtered.Topics) == 0 {
			continue
		}
		if !subscriber.active {
			subscriber.pending = mergeInvalidations(subscriber.pending, filtered)
			continue
		}
		subscriber.enqueue(filtered)
	}
}

func (s *changeSubscriber) Activate(acknowledged uint64) {
	s.activationOnce.Do(func() {
		s.hub.mu.Lock()
		s.active = true
		if s.pending.Revision > acknowledged {
			s.enqueue(s.pending)
		}
		s.pending = domain.Invalidation{}
		s.hub.mu.Unlock()
		go s.writeLoop()
	})
}

func (s *changeSubscriber) Close() {
	s.hub.mu.Lock()
	delete(s.hub.subscribers, s)
	s.hub.mu.Unlock()
}

func (s *changeSubscriber) writeLoop() {
	defer s.Close()
	for {
		select {
		case change := <-s.queue:
			if err := s.session.NotifySubscription(s.id, InvalidatedMethod, change); err != nil {
				return
			}
		case <-s.session.Done():
			return
		}
	}
}

func (s *changeSubscriber) filter(change domain.Invalidation) domain.Invalidation {
	if len(s.topics) == 0 {
		return change
	}
	filtered := domain.Invalidation{Revision: change.Revision}
	for _, topic := range change.Topics {
		if s.topics[topic] {
			filtered.Topics = append(filtered.Topics, topic)
		}
	}
	return filtered
}

func (s *changeSubscriber) enqueue(change domain.Invalidation) {
	select {
	case queued := <-s.queue:
		change = mergeInvalidations(queued, change)
	default:
	}
	select {
	case s.queue <- change:
	default:
	}
}

func mergeInvalidations(left, right domain.Invalidation) domain.Invalidation {
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
