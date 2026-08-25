package harnessbridge

import (
	"context"
	"errors"
	"fmt"
	"sync"
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
func (s *blockedOutputStore) Get(ctx context.Context, _ string) (model.Message, error) {
	select {
	case <-s.release:
		return model.Message{}, domain.ErrNotFound
	case <-ctx.Done():
		return model.Message{}, ctx.Err()
	}
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

type recordingActivityStore struct {
	*blockedOutputStore
	release    chan struct{}
	started    chan struct{}
	startOnce  sync.Once
	mu         sync.Mutex
	activities []domain.HarnessActivity
}

func (s *recordingActivityStore) UpsertHarnessActivity(ctx context.Context, activity domain.HarnessActivity) error {
	s.startOnce.Do(func() { close(s.started) })
	select {
	case <-s.release:
	case <-ctx.Done():
		return ctx.Err()
	}
	s.mu.Lock()
	s.activities = append(s.activities, activity)
	s.mu.Unlock()
	return nil
}

func (s *recordingActivityStore) recorded() []domain.HarnessActivity {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]domain.HarnessActivity(nil), s.activities...)
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
	if err := relay.publish(context.Background(), output); err != nil {
		t.Fatal(err)
	}
	if projectStore.calls != 1 || projectStore.binding != project || projectStore.message.Presentation != output.presentation || projectStore.message.Correlation != output.correlation {
		t.Fatalf("project reconciliation = calls %d, binding %#v, message %#v", projectStore.calls, projectStore.binding, projectStore.message)
	}
}

func TestEventRelayBackpressuresAndPersistsAllTerminalActivity(t *testing.T) {
	factory := fake.NewFactory("durable-activity")
	instance, err := factory.Launch(context.Background(), harness.LaunchConfig{
		InstanceID: "durable-activity-instance", AgentName: "agent", Directory: "/work", SessionMode: harness.SessionNew,
	})
	if err != nil {
		t.Fatal(err)
	}
	store := &recordingActivityStore{blockedOutputStore: &blockedOutputStore{release: make(chan struct{})}, release: make(chan struct{}), started: make(chan struct{})}
	relay := startEventRelay(context.Background(), instance, store, nil, newMemoryLedger(), nil, model.Mailbox{ID: "agent-mailbox"}, model.RepositoryContext{}, nil, Terminology{ProviderName: "Bounded", SessionName: "session", OperationName: "operation", ItemName: "item"}, newOperationTracker())
	want := eventQueueCapacity + 40
	emitted := make(chan error, 1)
	go func() {
		for index := 0; index < want; index++ {
			if err := factory.Emit(instance, "operation", harnessItemID(index), harness.CommandEvent{Command: "command", Status: harness.OperationCompleted}); err != nil {
				emitted <- err
				return
			}
		}
		emitted <- nil
	}()
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("terminal activity persistence did not start")
	}
	select {
	case err := <-emitted:
		t.Fatalf("terminal activity did not backpressure at capacity: %v", err)
	case <-time.After(100 * time.Millisecond):
	}
	close(store.release)
	select {
	case err := <-emitted:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("terminal activity producer remained blocked after persistence resumed")
	}
	_ = instance.Shutdown(context.Background())
	relay.StopAndWait()
	activities := store.recorded()
	if len(activities) != want {
		t.Fatalf("persisted terminal activities = %d, want %d", len(activities), want)
	}
	for index, activity := range activities {
		if activity.Kind != domain.HarnessActivityCommand || activity.Sequence != uint64(index+1) {
			t.Fatalf("terminal activity %d = %#v", index, activity)
		}
	}
}

func TestEventRelayCoalescesPendingSnapshotsToLatestAtTail(t *testing.T) {
	factory := fake.NewFactory("coalesced-activity")
	instance, err := factory.Launch(context.Background(), harness.LaunchConfig{InstanceID: "coalesced-instance", AgentName: "agent", Directory: "/work", SessionMode: harness.SessionNew})
	if err != nil {
		t.Fatal(err)
	}
	store := &recordingActivityStore{blockedOutputStore: &blockedOutputStore{release: make(chan struct{})}, release: make(chan struct{}), started: make(chan struct{})}
	relay := startEventRelay(context.Background(), instance, store, nil, newMemoryLedger(), nil, model.Mailbox{ID: "agent-mailbox"}, model.RepositoryContext{}, nil, Terminology{}, newOperationTracker())
	if err := factory.Emit(instance, "operation", "progress", harness.ProgressEvent{Message: "progress-1"}); err != nil {
		t.Fatal(err)
	}
	select {
	case <-store.started:
	case <-time.After(time.Second):
		t.Fatal("first coalesced activity did not enter persistence")
	}
	wantSequence := uint64(eventQueueCapacity + 80)
	emitted := make(chan error, 1)
	go func() {
		for sequence := uint64(2); sequence <= wantSequence; sequence++ {
			if err := factory.Emit(instance, "operation", "progress", harness.ProgressEvent{Message: fmt.Sprintf("progress-%d", sequence)}); err != nil {
				emitted <- err
				return
			}
		}
		emitted <- nil
	}()
	select {
	case err := <-emitted:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("same-key snapshots filled the bounded buffer")
	}
	deadline := time.Now().Add(time.Second)
	for {
		relay.queue.mu.Lock()
		pendingLatest := len(relay.queue.items) == 1 && relay.queue.items[0].work.activity != nil && relay.queue.items[0].work.activity.Sequence == wantSequence
		relay.queue.mu.Unlock()
		if pendingLatest {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("latest snapshot was not accepted into the coalescing buffer")
		}
		time.Sleep(time.Millisecond)
	}
	close(store.release)
	_ = instance.Shutdown(context.Background())
	relay.StopAndWait()
	activities := store.recorded()
	if len(activities) != 2 || activities[0].Sequence != 1 || activities[1].Sequence != wantSequence || activities[1].Body != fmt.Sprintf("progress-%d", wantSequence) {
		t.Fatalf("coalesced activities = %#v", activities)
	}
}

func TestEventBufferReplacementPreservesTailOrderAndCancellation(t *testing.T) {
	buffer := newEventBuffer(2)
	activityWork := func(item string, sequence uint64) eventWork {
		return eventWork{activity: &domain.HarnessActivity{
			MailboxID: "mailbox", Correlation: model.MessageCorrelation{Provider: "provider", SessionID: "session", OperationID: "operation", ItemID: item},
			Kind: domain.HarnessActivityProgress, ItemID: item, Sequence: sequence,
		}}
	}
	first := activityWork("same", 1)
	durable := eventWork{activity: &domain.HarnessActivity{Kind: domain.HarnessActivityCommand, Sequence: 2}}
	latest := activityWork("same", 3)
	if err := buffer.enqueue(context.Background(), first); err != nil {
		t.Fatal(err)
	}
	if err := buffer.enqueue(context.Background(), durable); err != nil {
		t.Fatal(err)
	}
	if err := buffer.enqueue(context.Background(), latest); err != nil {
		t.Fatal(err)
	}
	if got, _ := buffer.dequeue(); got.activity.Sequence != 2 {
		t.Fatalf("durable work did not retain its position: %#v", got)
	}
	if got, _ := buffer.dequeue(); got.activity.Sequence != 3 {
		t.Fatalf("replacement was not moved to the tail: %#v", got)
	}
	if err := buffer.enqueue(context.Background(), activityWork("one", 4)); err != nil {
		t.Fatal(err)
	}
	if err := buffer.enqueue(context.Background(), activityWork("two", 5)); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	blocked := make(chan error, 1)
	go func() { blocked <- buffer.enqueue(ctx, activityWork("three", 6)) }()
	select {
	case err := <-blocked:
		t.Fatalf("distinct key did not backpressure: %v", err)
	case <-time.After(50 * time.Millisecond):
	}
	cancel()
	if err := <-blocked; !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled enqueue error = %v", err)
	}
}

func TestEventRelayAssignsOneDeterministicOutputActivityTimeline(t *testing.T) {
	relay := &eventRelay{
		identity: harness.SessionIdentity{Provider: "provider", ID: "session"}, runtimeID: "runtime", mailbox: model.Mailbox{ID: "mailbox"},
		activity: &recordingActivityStore{}, now: func() time.Time { return time.Unix(99, 0).UTC() },
	}
	first := relay.normalize(harness.Event{
		Sequence: 1, Session: relay.identity, Operation: "operation", ItemID: "output", OccurredAt: time.Unix(10, 900_000_000).UTC(),
		Payload: harness.OutputEvent{Text: "output", Final: true},
	})
	second := relay.normalize(harness.Event{
		Sequence: 2, Session: relay.identity, Operation: "operation", ItemID: "progress", OccurredAt: time.Unix(10, 950_000_000).UTC(),
		Payload: harness.ProgressEvent{Message: "progress"},
	})
	third := relay.normalize(harness.Event{
		Sequence: 3, Session: relay.identity, Operation: "operation", OccurredAt: time.Unix(10, 0).UTC(),
		Payload: harness.OperationStatusEvent{Status: harness.OperationFailed, Error: "boom"},
	})
	if first.output == nil || !first.output.createdAt.Equal(time.Unix(10, 0).UTC()) {
		t.Fatalf("first output time = %#v", first.output)
	}
	if second.activity == nil || !second.activity.OccurredAt.Equal(time.Unix(11, 950_000_000).UTC()) {
		t.Fatalf("second activity time = %#v", second.activity)
	}
	if third.output == nil || third.activity == nil || !third.output.createdAt.Equal(time.Unix(12, 0).UTC()) || !third.activity.OccurredAt.Equal(time.Unix(12, 1_000_000).UTC()) {
		t.Fatalf("combined work times = %#v / %#v", third.output, third.activity)
	}
}

func harnessItemID(index int) string {
	return fmt.Sprintf("item-%d", index)
}
