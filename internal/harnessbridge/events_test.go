package harnessbridge

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/harness/fake"
	"github.com/wbbradley/hq/internal/model"
)

type blockedOutputStore struct{ release chan struct{} }

func (s *blockedOutputStore) HumanMailbox(context.Context) (model.Mailbox, error) {
	return model.Mailbox{ID: model.HumanMailboxID}, nil
}
func (s *blockedOutputStore) Create(context.Context, model.Message) error { return nil }
func (s *blockedOutputStore) Get(context.Context, string) (model.Message, error) {
	<-s.release
	return model.Message{}, domain.ErrNotFound
}
func (s *blockedOutputStore) List(context.Context, model.Filter) ([]model.Message, error) {
	return nil, nil
}
func (s *blockedOutputStore) Archive(context.Context, string) error { return nil }

type blockedActivityStore struct {
	*blockedOutputStore
	activityRelease chan struct{}
}

func (s *blockedActivityStore) UpsertHarnessActivity(context.Context, domain.HarnessActivity) error {
	<-s.activityRelease
	return nil
}

func TestEventRelayFailsFastWhenPersistenceQueueIsSaturated(t *testing.T) {
	factory := fake.NewFactory("bounded")
	instance, err := factory.Launch(context.Background(), harness.LaunchConfig{
		InstanceID: "bounded-instance", AgentName: "agent", Directory: "/work", SessionMode: harness.SessionNew,
	})
	if err != nil {
		t.Fatal(err)
	}
	store := &blockedOutputStore{release: make(chan struct{})}
	relay := startEventRelay(context.Background(), instance, store, nil, newMemoryLedger(), nil, model.Mailbox{ID: "agent-mailbox"}, model.RepositoryContext{}, nil, Terminology{ProviderName: "Bounded", SessionName: "session", OperationName: "operation", ItemName: "item"}, newOperationTracker())
	for index := 0; index < eventQueueCapacity+2; index++ {
		if err := factory.Emit(instance, "operation", harnessItemID(index), harness.OutputEvent{Text: "output", Final: true}); err != nil {
			t.Fatal(err)
		}
	}
	select {
	case <-relay.Failed():
		if err := relay.Err(); err == nil || !strings.Contains(err.Error(), "64-event bound") {
			t.Fatalf("relay error = %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("saturated relay did not fail fast")
	}
	close(store.release)
	_ = instance.Shutdown(context.Background())
	relay.StopAndWait()
}

func TestEventRelayFailsFastWhenActivityPersistenceQueueIsSaturated(t *testing.T) {
	factory := fake.NewFactory("bounded-activity")
	instance, err := factory.Launch(context.Background(), harness.LaunchConfig{
		InstanceID: "bounded-activity-instance", AgentName: "agent", Directory: "/work", SessionMode: harness.SessionNew,
	})
	if err != nil {
		t.Fatal(err)
	}
	store := &blockedActivityStore{blockedOutputStore: &blockedOutputStore{release: make(chan struct{})}, activityRelease: make(chan struct{})}
	relay := startEventRelay(context.Background(), instance, store, nil, newMemoryLedger(), nil, model.Mailbox{ID: "agent-mailbox"}, model.RepositoryContext{}, nil, Terminology{ProviderName: "Bounded", SessionName: "session", OperationName: "operation", ItemName: "item"}, newOperationTracker())
	for index := 0; index < eventQueueCapacity+2; index++ {
		if err := factory.Emit(instance, "operation", harnessItemID(index), harness.ProgressEvent{Message: "working"}); err != nil {
			t.Fatal(err)
		}
	}
	select {
	case <-relay.Failed():
		if err := relay.Err(); err == nil || !strings.Contains(err.Error(), "64-event bound") {
			t.Fatalf("relay error = %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("saturated activity relay did not fail fast")
	}
	close(store.activityRelease)
	_ = instance.Shutdown(context.Background())
	relay.StopAndWait()
}

func harnessItemID(index int) string {
	return fmt.Sprintf("item-%d", index)
}
