package codexbridge

import (
	"context"
	"errors"
	"sync"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

type ClaimStore interface {
	Claim(context.Context, domain.Claim, string) (model.Message, error)
	Complete(context.Context, string, string) error
	Release(context.Context, string, string) error
}

type ClaimedReply struct {
	Message model.Message
	store   ClaimStore
	token   string
	mu      sync.Mutex
	done    bool
}

func (r *ClaimedReply) Complete(ctx context.Context) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.done {
		return nil
	}
	if err := r.store.Complete(ctx, r.Message.ID, r.token); err != nil {
		return err
	}
	r.done = true
	return nil
}

func (r *ClaimedReply) Release(ctx context.Context) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.done {
		return nil
	}
	if err := r.store.Release(ctx, r.Message.ID, r.token); err != nil {
		return err
	}
	r.done = true
	return nil
}

type ReplyWaiter struct {
	Replies <-chan *ClaimedReply
	cancel  func()
}

func (w *ReplyWaiter) Cancel() {
	if w != nil && w.cancel != nil {
		w.cancel()
	}
}

type replyRegistration struct {
	questionID string
	replies    chan *ClaimedReply
}

type ReplyRegistry struct {
	mu      sync.Mutex
	ordered []string
	waiters map[string]*replyRegistration
}

func NewReplyRegistry() *ReplyRegistry {
	return &ReplyRegistry{waiters: make(map[string]*replyRegistration)}
}

func (r *ReplyRegistry) Register(questionID string) (*ReplyWaiter, error) {
	if questionID == "" {
		return nil, errors.New("question message ID is required")
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.waiters[questionID]; exists {
		return nil, errors.New("question message ID is already registered")
	}
	registration := &replyRegistration{questionID: questionID, replies: make(chan *ClaimedReply, 1)}
	r.waiters[questionID] = registration
	r.ordered = append(r.ordered, questionID)
	return &ReplyWaiter{Replies: registration.replies, cancel: func() { r.cancel(questionID) }}, nil
}

func (r *ReplyRegistry) ClaimOne(ctx context.Context, claimStore ClaimStore, mailboxID string) (bool, error) {
	for _, registration := range r.snapshot() {
		token, err := uuid.NewV7()
		if err != nil {
			return false, err
		}
		message, err := claimStore.Claim(ctx, domain.Claim{ReplyTo: registration.questionID, RecipientMailboxID: mailboxID, Purpose: model.MessagePurposeProtocolAnswer}, token.String())
		if errors.Is(err, domain.ErrNotReady) {
			continue
		}
		if err != nil {
			return false, err
		}
		claimed := &ClaimedReply{Message: message, store: claimStore, token: token.String()}
		if !r.deliver(registration.questionID, claimed) {
			_ = claimed.Release(ctx)
			continue
		}
		return true, nil
	}
	return false, nil
}

func (r *ReplyRegistry) OutstandingIDs() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	result := make([]string, 0, len(r.ordered))
	for _, questionID := range r.ordered {
		if r.waiters[questionID] != nil {
			result = append(result, questionID)
		}
	}
	return result
}

func (r *ReplyRegistry) snapshot() []*replyRegistration {
	r.mu.Lock()
	defer r.mu.Unlock()
	registrations := make([]*replyRegistration, 0, len(r.ordered))
	for _, questionID := range r.ordered {
		if registration := r.waiters[questionID]; registration != nil {
			registrations = append(registrations, registration)
		}
	}
	return registrations
}

func (r *ReplyRegistry) deliver(questionID string, claimed *ClaimedReply) bool {
	r.mu.Lock()
	registration := r.waiters[questionID]
	if registration != nil {
		delete(r.waiters, questionID)
		r.removeOrdered(questionID)
	}
	r.mu.Unlock()
	if registration == nil {
		return false
	}
	registration.replies <- claimed
	close(registration.replies)
	return true
}

func (r *ReplyRegistry) cancel(questionID string) {
	r.mu.Lock()
	registration := r.waiters[questionID]
	if registration != nil {
		delete(r.waiters, questionID)
		r.removeOrdered(questionID)
	}
	r.mu.Unlock()
	if registration != nil {
		close(registration.replies)
	}
}

func (r *ReplyRegistry) removeOrdered(questionID string) {
	for index, candidate := range r.ordered {
		if candidate == questionID {
			r.ordered = append(r.ordered[:index], r.ordered[index+1:]...)
			return
		}
	}
}
