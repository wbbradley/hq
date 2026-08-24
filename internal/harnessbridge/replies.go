package harnessbridge

import (
	"context"
	"errors"
	"sync"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

type claimedReply struct {
	message model.Message
	store   ClaimStore
	token   string
	mu      sync.Mutex
	done    bool
}

func (r *claimedReply) complete(ctx context.Context) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.done {
		return nil
	}
	if err := r.store.Complete(ctx, r.message.ID, r.token); err != nil {
		return err
	}
	r.done = true
	return nil
}

func (r *claimedReply) release(ctx context.Context) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.done {
		return nil
	}
	if err := r.store.Release(ctx, r.message.ID, r.token); err != nil {
		return err
	}
	r.done = true
	return nil
}

type replyRegistration struct {
	questionID string
	replies    chan *claimedReply
}

type replyRegistry struct {
	mu      sync.Mutex
	ordered []string
	waiters map[string]*replyRegistration
}

func newReplyRegistry() *replyRegistry {
	return &replyRegistry{waiters: make(map[string]*replyRegistration)}
}

func (r *replyRegistry) register(questionID string) (*replyRegistration, error) {
	if questionID == "" {
		return nil, errors.New("question message ID is required")
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.waiters[questionID]; exists {
		return nil, errors.New("question message ID is already registered")
	}
	registration := &replyRegistration{questionID: questionID, replies: make(chan *claimedReply, 1)}
	r.waiters[questionID] = registration
	r.ordered = append(r.ordered, questionID)
	return registration, nil
}

func (r *replyRegistry) claimOne(ctx context.Context, store ClaimStore, mailboxID string) (bool, error) {
	for _, registration := range r.snapshot() {
		token, err := uuid.NewV7()
		if err != nil {
			return false, err
		}
		message, err := store.Claim(ctx, domain.Claim{ReplyTo: registration.questionID, RecipientMailboxID: mailboxID, Purpose: model.MessagePurposeProtocolAnswer}, token.String())
		if errors.Is(err, domain.ErrNotReady) {
			continue
		}
		if err != nil {
			return false, err
		}
		claimed := &claimedReply{message: message, store: store, token: token.String()}
		if !r.deliver(registration.questionID, claimed) {
			_ = claimed.release(ctx)
			continue
		}
		return true, nil
	}
	return false, nil
}

func (r *replyRegistry) outstandingIDs() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	result := make([]string, 0, len(r.ordered))
	for _, id := range r.ordered {
		if r.waiters[id] != nil {
			result = append(result, id)
		}
	}
	return result
}

func (r *replyRegistry) snapshot() []*replyRegistration {
	r.mu.Lock()
	defer r.mu.Unlock()
	result := make([]*replyRegistration, 0, len(r.ordered))
	for _, id := range r.ordered {
		if registration := r.waiters[id]; registration != nil {
			result = append(result, registration)
		}
	}
	return result
}

func (r *replyRegistry) deliver(id string, reply *claimedReply) bool {
	r.mu.Lock()
	registration := r.waiters[id]
	if registration != nil {
		delete(r.waiters, id)
		r.remove(id)
	}
	r.mu.Unlock()
	if registration == nil {
		return false
	}
	registration.replies <- reply
	close(registration.replies)
	return true
}

func (r *replyRegistry) cancel(id string) {
	r.mu.Lock()
	registration := r.waiters[id]
	if registration != nil {
		delete(r.waiters, id)
		r.remove(id)
	}
	r.mu.Unlock()
	if registration != nil {
		close(registration.replies)
	}
}

func (r *replyRegistry) remove(id string) {
	for index, candidate := range r.ordered {
		if candidate == id {
			r.ordered = append(r.ordered[:index], r.ordered[index+1:]...)
			return
		}
	}
}
