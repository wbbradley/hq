package harnessbridge

import (
	"context"
	"fmt"
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

type existingOutputStore struct{ message model.Message }

func (s existingOutputStore) HumanMailbox(context.Context) (model.Mailbox, error) {
	return model.Mailbox{ID: model.HumanMailboxID}, nil
}
func (existingOutputStore) Create(context.Context, model.Message) error { return nil }
func (s existingOutputStore) Get(context.Context, string) (model.Message, error) {
	return s.message, nil
}
func (existingOutputStore) List(context.Context, model.Filter) ([]model.Message, error) {
	return nil, nil
}
func (existingOutputStore) Archive(context.Context, string) error { return nil }

type recordingProjectOutputStore struct {
	calls   int
	binding domain.ProjectOutputBinding
	message model.Message
}

func (s *recordingProjectOutputStore) CreateProjectOutput(_ context.Context, binding domain.ProjectOutputBinding, message model.Message) error {
	s.calls++
	s.binding, s.message = binding, message
	return nil
}

type blockedActivityStore struct {
	*blockedOutputStore
	activityRelease chan struct{}
}

func (s *blockedActivityStore) UpsertHarnessActivity(context.Context, domain.HarnessActivity) error {
	<-s.activityRelease
	return nil
}

func TestEventRelayBackpressuresCanonicalOutputWhenPersistenceQueueIsSaturated(t *testing.T) {
	factory := fake.NewFactory("bounded")
	instance, err := factory.Launch(context.Background(), harness.LaunchConfig{
		InstanceID: "bounded-instance", AgentName: "agent", Directory: "/work", SessionMode: harness.SessionNew,
	})
	if err != nil {
		t.Fatal(err)
	}
	store := &blockedOutputStore{release: make(chan struct{})}
	relay := startEventRelay(context.Background(), instance, store, nil, newMemoryLedger(), nil, model.Mailbox{ID: "agent-mailbox"}, model.RepositoryContext{}, nil, Terminology{ProviderName: "Bounded", SessionName: "session", OperationName: "operation", ItemName: "item"}, newOperationTracker())
	emitted := make(chan struct{})
	go func() {
		defer close(emitted)
		for index := 0; index < eventQueueCapacity+40; index++ {
			_ = factory.Emit(instance, "operation", harnessItemID(index), harness.OutputEvent{Text: "output", Final: true})
		}
	}()
	select {
	case <-relay.Failed():
		t.Fatalf("canonical output saturation failed the relay: %v", relay.Err())
	case <-emitted:
		t.Fatal("canonical output did not apply backpressure")
	case <-time.After(100 * time.Millisecond):
	}
	close(store.release)
	select {
	case <-emitted:
	case <-time.After(2 * time.Second):
		t.Fatal("canonical output remained blocked after persistence resumed")
	}
	_ = instance.Shutdown(context.Background())
	relay.StopAndWait()
}

func TestEventRelayDelegatesExistingProjectOutputReconciliationToProjectStore(t *testing.T) {
	project := domain.ProjectOutputBinding{ProjectID: "project", AssignmentID: "assignment", AgentName: "agent", ProjectThreadID: "project-thread", ExternalThreadID: "session"}
	projectStore := &recordingProjectOutputStore{}
	relay := &eventRelay{
		store: existingOutputStore{message: model.Message{CreatedAt: time.Unix(1, 0).UTC()}}, projectStore: projectStore,
		ledger: newMemoryLedger(), identity: harness.SessionIdentity{Provider: "home-built", ID: "session"}, mailbox: model.Mailbox{ID: "project-mailbox"},
		project: &project, terms: Terminology{OutputNamespace: "project-output"}, now: func() time.Time { return time.Unix(2, 0).UTC() },
	}
	output := canonicalOutput{
		key: "item", operation: "operation", body: "result", presentation: model.PresentationFinalAnswer,
		correlation:       model.MessageCorrelation{Provider: "home-built", SessionID: "session", OperationID: "operation", ItemID: "item"},
		technicalSections: []model.TechnicalSection{{Namespace: "hq.harness.output", Fields: []model.TechnicalField{{Key: "phase", Value: "final_answer"}}}},
	}
	if err := relay.publish(output); err != nil {
		t.Fatal(err)
	}
	if projectStore.calls != 1 || projectStore.binding != project || projectStore.message.Presentation != output.presentation || projectStore.message.Correlation != output.correlation {
		t.Fatalf("project reconciliation = calls %d, binding %#v, message %#v", projectStore.calls, projectStore.binding, projectStore.message)
	}
}

func TestEventRelayDropsActivityWhenPersistenceQueueIsSaturated(t *testing.T) {
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
		t.Fatalf("activity saturation failed the relay: %v", relay.Err())
	case <-time.After(100 * time.Millisecond):
	}
	close(store.activityRelease)
	_ = instance.Shutdown(context.Background())
	relay.StopAndWait()
}

func harnessItemID(index int) string {
	return fmt.Sprintf("item-%d", index)
}
