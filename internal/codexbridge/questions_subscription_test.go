package codexbridge

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

type questionSubscription struct {
	changes chan domain.Invalidation
	closed  bool
}

func (s *questionSubscription) Changes() <-chan domain.Invalidation { return s.changes }
func (s *questionSubscription) Close()                              { s.closed = true }

func TestQuestionArchiveWakesFromInvalidationWithoutPolling(t *testing.T) {
	fixture := newDispatcherFixture(t)
	subscription := &questionSubscription{changes: make(chan domain.Invalidation, 1)}
	questioner := &Questioner{
		Store: fixture.store, Replies: fixture.replies, Mailbox: fixture.agent,
		ThreadID: fixture.thread, Repository: model.RepositoryContext{Directory: "/work/repo"}, RepairInterval: time.Hour,
		Subscribe: func(context.Context, ...domain.ChangeTopic) (domain.ChangeSubscription, error) {
			return subscription, nil
		},
	}
	pending, err := questioner.Publish(context.Background(), QuestionSpec{Body: "Continue?"})
	if err != nil {
		t.Fatal(err)
	}
	if err := fixture.store.Archive(context.Background(), pending.MessageID); err != nil {
		t.Fatal(err)
	}
	started := time.Now()
	subscription.changes <- domain.Invalidation{Revision: 3, Topics: []domain.ChangeTopic{domain.TopicMessages}}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if _, err := questioner.await(ctx, pending); !errors.Is(err, ErrHumanCancelled) {
		t.Fatalf("archived question result = %v", err)
	}
	if elapsed := time.Since(started); elapsed > 500*time.Millisecond {
		t.Fatalf("question invalidation wake took %s", elapsed)
	}
	if !subscription.closed {
		t.Fatal("question subscription was not closed")
	}
}
