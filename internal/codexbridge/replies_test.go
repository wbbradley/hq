package codexbridge

import (
	"context"
	"errors"
	"testing"

	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

type replyClaimStore struct {
	messages  []model.Message
	claimed   map[string]string
	completed []string
	released  []string
	failFirst bool
}

func (s *replyClaimStore) Claim(_ context.Context, claim store.Claim, token string) (model.Message, error) {
	for _, message := range s.messages {
		if s.claimed[message.ID] != "" || message.RecipientMailboxID != claim.RecipientMailboxID {
			continue
		}
		if claim.ReplyTo != "" && (message.ReplyTo == nil || *message.ReplyTo != claim.ReplyTo) {
			continue
		}
		s.claimed[message.ID] = token
		return message, nil
	}
	return model.Message{}, store.ErrNotReady
}

func (s *replyClaimStore) Complete(_ context.Context, id, token string) error {
	if s.claimed[id] != token {
		return store.ErrNotReady
	}
	if s.failFirst {
		s.failFirst = false
		return errors.New("temporary completion failure")
	}
	s.completed = append(s.completed, id)
	return nil
}

func (s *replyClaimStore) Release(_ context.Context, id, token string) error {
	if s.claimed[id] != token {
		return store.ErrNotReady
	}
	s.released = append(s.released, id)
	delete(s.claimed, id)
	return nil
}

func TestReplyRegistryClaimsOnlyMatchingReply(t *testing.T) {
	questionID := "question-1"
	otherQuestionID := "question-other"
	claimStore := &replyClaimStore{claimed: make(map[string]string), messages: []model.Message{
		{ID: "unsolicited", RecipientMailboxID: "agent"},
		{ID: "other-reply", RecipientMailboxID: "agent", ReplyTo: &otherQuestionID},
		{ID: "matching-reply", RecipientMailboxID: "agent", ReplyTo: &questionID, Body: "approve"},
	}}
	registry := NewReplyRegistry()
	waiter, err := registry.Register(questionID)
	if err != nil {
		t.Fatal(err)
	}
	claimed, err := registry.ClaimOne(context.Background(), claimStore, "agent")
	if err != nil || !claimed {
		t.Fatalf("claimed = %t, %v", claimed, err)
	}
	reply := <-waiter.Replies
	if reply == nil || reply.Message.ID != "matching-reply" {
		t.Fatalf("reply = %#v", reply)
	}
	if err := reply.Complete(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(claimStore.completed) != 1 || claimStore.completed[0] != "matching-reply" || claimStore.claimed["unsolicited"] != "" {
		t.Fatalf("store = %#v", claimStore)
	}
}

func TestClaimedReplyCanReleaseAfterCompletionFailure(t *testing.T) {
	questionID := "question-1"
	claimStore := &replyClaimStore{claimed: make(map[string]string), failFirst: true, messages: []model.Message{{ID: "reply", RecipientMailboxID: "agent", ReplyTo: &questionID}}}
	registry := NewReplyRegistry()
	waiter, _ := registry.Register(questionID)
	if claimed, err := registry.ClaimOne(context.Background(), claimStore, "agent"); err != nil || !claimed {
		t.Fatalf("claimed = %t, %v", claimed, err)
	}
	reply := <-waiter.Replies
	if err := reply.Complete(context.Background()); err == nil {
		t.Fatal("completion unexpectedly succeeded")
	}
	if err := reply.Release(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(claimStore.released) != 1 || claimStore.released[0] != "reply" {
		t.Fatalf("released = %v", claimStore.released)
	}
}

func TestReplyRegistryRejectsDuplicateRegistration(t *testing.T) {
	registry := NewReplyRegistry()
	waiter, err := registry.Register("question")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := registry.Register("question"); err == nil {
		t.Fatal("duplicate registration succeeded")
	}
	waiter.Cancel()
	if reply, open := <-waiter.Replies; open || reply != nil {
		t.Fatalf("cancelled waiter = %#v, %t", reply, open)
	}
}
