package harnessbridge

import (
	"context"
	"errors"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/harness/fake"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

type recordingFactory struct {
	harness.Factory
	launched chan harness.Instance
}

func (f *recordingFactory) Launch(ctx context.Context, config harness.LaunchConfig) (harness.Instance, error) {
	instance, err := f.Factory.Launch(ctx, config)
	if err == nil {
		f.launched <- instance
	}
	return instance, err
}

type memoryLedger struct {
	mu         sync.Mutex
	deliveries map[string]DeliveryState
	outputs    map[string]bool
}

func newMemoryLedger() *memoryLedger {
	return &memoryLedger{deliveries: make(map[string]DeliveryState), outputs: make(map[string]bool)}
}

func ledgerKey(sessionID, id string) string { return sessionID + "\x00" + id }

func (l *memoryLedger) Delivery(sessionID, messageID string) (DeliveryState, bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	state, ok := l.deliveries[ledgerKey(sessionID, messageID)]
	return state, ok, nil
}

func (l *memoryLedger) SetDelivery(sessionID, messageID string, state DeliveryState) error {
	l.mu.Lock()
	l.deliveries[ledgerKey(sessionID, messageID)] = state
	l.mu.Unlock()
	return nil
}

func (l *memoryLedger) OutputSent(sessionID, itemID string) (bool, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.outputs[ledgerKey(sessionID, itemID)], nil
}

func (l *memoryLedger) MarkOutputSent(sessionID, itemID string) error {
	l.mu.Lock()
	l.outputs[ledgerKey(sessionID, itemID)] = true
	l.mu.Unlock()
	return nil
}

func TestBridgeUsesNeutralRuntimeForRecoveryRequestsAndOutput(t *testing.T) {
	database := openBridgeTestStore(t)
	provider := fake.NewFactory("home-built")
	factory := &recordingFactory{Factory: provider, launched: make(chan harness.Instance, 1)}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", AgentName: "builder", Factory: factory, Store: database, Ledger: newMemoryLedger(),
			Repository: model.RepositoryContext{Directory: "/work/repo"}, RepairInterval: 2 * time.Millisecond, SuppressStatus: true,
		})
	}()
	instance := <-factory.launched
	agent := waitForAgent(t, database, "builder")

	provider.SetNextSubmissionOutcome(harness.DeliveryUncertain, true)
	messageID := "019c0000-0000-7000-8000-000000000501"
	if err := database.Create(context.Background(), model.Message{
		ID: messageID, Context: model.RepositoryContext{Directory: "/work/repo"}, SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: agent.MailboxID, Body: "recover me", CreatedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatal(err)
	}
	waitForMessage(t, database, messageID, func(message model.Message) bool { return message.CompletedAt != nil })

	if err := provider.Emit(instance, "operation-1", "answer-1", harness.OutputEvent{Text: "Neutral output", Final: true}); err != nil {
		t.Fatal(err)
	}
	output := waitForBody(t, database, "Neutral output")
	replyID := "019c0000-0000-7000-8000-000000000503"
	replyTo := output.ID
	if err := database.Reply(context.Background(), output.ID, model.Message{
		ID: replyID, Context: output.Context, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: agent.MailboxID,
		Body: "generic follow-up", Details: "Harness provider: home-built\nHarness session: " + string(instance.Session().Identity().ID), ReplyTo: &replyTo, CreatedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatal(err)
	}
	waitForMessage(t, database, replyID, func(message model.Message) bool { return message.CompletedAt != nil })

	_, response, err := provider.Ask(instance, "operation-1", "approval-1", harness.ApprovalRequest{Kind: "command", Summary: "Run tests", Choices: []string{"accept", "decline"}})
	if err != nil {
		t.Fatal(err)
	}
	question := waitForBody(t, database, "Deterministic fake harness requests command approval")
	reply := model.Message{
		ID: "019c0000-0000-7000-8000-000000000502", Context: question.Context, SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: agent.MailboxID, Body: "accept", CreatedAt: time.Now().UTC(),
	}
	if err := database.Reply(context.Background(), question.ID, reply); err != nil {
		t.Fatal(err)
	}
	select {
	case answered := <-response:
		decision, ok := answered.Payload.(harness.DecisionResponse)
		if !ok || decision.Decision != "accept" {
			t.Fatalf("response = %#v", answered)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("interactive request was not answered")
	}

	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("generic bridge did not stop")
	}
}

func TestBridgePersistsInitialPromptBeforeRecoveringUncertainDelivery(t *testing.T) {
	database := openBridgeTestStore(t)
	provider := fake.NewFactory("home-built")
	provider.SetNextSubmissionOutcome(harness.DeliveryUncertain, true)
	factory := &recordingFactory{Factory: provider, launched: make(chan harness.Instance, 1)}
	ready := make(chan struct{})
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	const submissionID = "019c0000-0000-7000-8000-000000000504"
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", AgentName: "initial-agent", Factory: factory, Store: database, Ledger: newMemoryLedger(),
			InitialPrompt: "start durably", InitialSubmissionID: submissionID, Repository: model.RepositoryContext{Directory: "/work/repo"},
			RepairInterval: time.Millisecond, SuppressStatus: true, OnReady: func(Ready) { close(ready) },
		})
	}()
	instance := <-factory.launched
	select {
	case <-ready:
	case err := <-done:
		t.Fatalf("bridge stopped before recovering the initial prompt: %v", err)
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not recover the initial prompt")
	}
	message, err := database.Get(context.Background(), submissionID)
	if err != nil {
		t.Fatal(err)
	}
	if message.CompletedAt == nil || message.Body != "start durably" || message.HarnessSessionID != string(instance.Session().Identity().ID) || !strings.Contains(message.Details, "Harness provider: home-built") {
		t.Fatalf("durable initial prompt = %#v", message)
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestBridgePersistsAndCoalescesFakeProviderActivityAcrossRestart(t *testing.T) {
	database := openBridgeTestStore(t)
	provider := fake.NewFactory("home-built")
	start := func() (context.CancelFunc, <-chan error, harness.Instance) {
		t.Helper()
		factory := &recordingFactory{Factory: provider, launched: make(chan harness.Instance, 1)}
		ready := make(chan struct{})
		ctx, cancel := context.WithCancel(context.Background())
		done := make(chan error, 1)
		go func() {
			done <- Run(ctx, Options{Directory: "/work/repo", AgentName: "activity-agent", Factory: factory, Store: database, Ledger: newMemoryLedger(), Repository: model.RepositoryContext{Directory: "/work/repo"}, SuppressStatus: true, OnReady: func(Ready) { close(ready) }})
		}()
		instance := <-factory.launched
		select {
		case <-ready:
			return cancel, done, instance
		case err := <-done:
			t.Fatalf("activity bridge failed before readiness: %v", err)
			return nil, nil, nil
		case <-time.After(3 * time.Second):
			t.Fatal("activity bridge did not become ready")
			return nil, nil, nil
		}
	}
	stop := func(cancel context.CancelFunc, done <-chan error) {
		t.Helper()
		cancel()
		select {
		case err := <-done:
			if err != nil {
				t.Fatal(err)
			}
		case <-time.After(3 * time.Second):
			t.Fatal("activity bridge did not stop")
		}
	}

	cancel, done, instance := start()
	agent := waitForAgent(t, database, "activity-agent")
	exitCode := 0
	activities := []struct {
		item    string
		payload harness.EventPayload
	}{
		{"", harness.OperationStatusEvent{Status: harness.OperationRunning}},
		{"", harness.PlanEvent{Text: "first plan"}},
		{"", harness.PlanEvent{Text: "final plan"}},
		{"", harness.DiffEvent{Text: "diff --git a/a b/a"}},
		{"command", harness.CommandEvent{Command: "go test ./...", Output: strings.Repeat("c", domain.HarnessActivityCommandBodyBytes+512), ExitCode: &exitCode, Status: harness.OperationCompleted}},
		{"file", harness.FileChangeEvent{Path: "main.go", Summary: "updated", Status: harness.OperationCompleted}},
		{"tool", harness.ToolEvent{Name: "search", Summary: "found it", Status: harness.OperationCompleted}},
		{"progress", harness.ProgressEvent{Message: strings.Repeat("p", domain.HarnessActivityProgressBytes+512)}},
	}
	for _, activity := range activities {
		if err := provider.Emit(instance, "operation-activity", activity.item, activity.payload); err != nil {
			t.Fatal(err)
		}
	}
	projected := waitForHarnessActivities(t, database, agent.MailboxID, 7)
	if projected[1].Kind != domain.HarnessActivityPlan || projected[1].Body != "final plan" {
		t.Fatalf("coalesced plan = %#v", projected[1])
	}
	if command := projected[3]; command.Kind != domain.HarnessActivityCommand || !command.Truncated || len(command.Body) != domain.HarnessActivityCommandBodyBytes {
		t.Fatalf("bounded command = %#v", command)
	}
	if progress := projected[6]; progress.Kind != domain.HarnessActivityProgress || !progress.Truncated || len(progress.Body) != domain.HarnessActivityProgressBytes {
		t.Fatalf("bounded progress = %#v", progress)
	}
	if messages, err := database.List(context.Background(), model.Filter{CounterpartyMailboxID: agent.MailboxID}); err != nil || len(messages) != 0 {
		t.Fatalf("activity entered canonical messages: %#v, %v", messages, err)
	}
	firstSession := instance.Session().Identity().ID
	stop(cancel, done)

	cancel, done, resumed := start()
	if resumed.Session().Identity().ID != firstSession {
		t.Fatalf("resumed session = %s, want %s", resumed.Session().Identity().ID, firstSession)
	}
	if got := waitForHarnessActivities(t, database, agent.MailboxID, 7); len(got) != 7 {
		t.Fatalf("activities after restart = %d", len(got))
	}
	if err := provider.Emit(resumed, "operation-activity", "command", activities[4].payload); err != nil {
		t.Fatal(err)
	}
	if got := waitForHarnessActivities(t, database, agent.MailboxID, 7); len(got) != 7 {
		t.Fatalf("activities after replay = %d", len(got))
	}
	stop(cancel, done)
}

func TestBridgeRejectsProviderWithoutSafeRecovery(t *testing.T) {
	database := openBridgeTestStore(t)
	factory := fake.NewFactory("unsafe")
	factory.SetCapabilities(harness.Capabilities{})
	err := Run(context.Background(), Options{Directory: "/work", AgentName: "unsafe-agent", Factory: factory, Store: database, Ledger: newMemoryLedger(), SuppressStatus: true})
	if !errors.Is(err, harness.ErrCapabilityUnavailable) {
		t.Fatalf("error = %v", err)
	}
}

func openBridgeTestStore(t *testing.T) *store.SQLite {
	t.Helper()
	databasePath := filepath.Join(t.TempDir(), "hq.db")
	keyPath, err := identity.KeyPath(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	database, err := store.Open(databasePath)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return database
}

func waitForHarnessActivities(t *testing.T, database *store.SQLite, mailboxID string, count int) []domain.HarnessActivity {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		activities, err := database.ListHarnessActivities(context.Background(), domain.HarnessActivityFilter{MailboxID: mailboxID})
		if err == nil && len(activities) == count {
			return activities
		}
		time.Sleep(5 * time.Millisecond)
	}
	activities, err := database.ListHarnessActivities(context.Background(), domain.HarnessActivityFilter{MailboxID: mailboxID})
	t.Fatalf("activities = %d, err = %v; want %d", len(activities), err, count)
	return nil
}

func waitForAgent(t *testing.T, database *store.SQLite, name string) modelAgent {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		agent, err := database.GetNamedAgent(context.Background(), name)
		if err == nil && agent.CurrentSessionID != "" {
			return modelAgent{MailboxID: agent.MailboxID}
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("agent %s was not ready", name)
	return modelAgent{}
}

type modelAgent struct{ MailboxID string }

type failingActivityStore struct{ *store.SQLite }

func (s *failingActivityStore) UpsertHarnessActivity(context.Context, domain.HarnessActivity) error {
	return errors.New("activity storage failed")
}

func TestBridgePublishesCanonicalFailureBeforeReportingActivityWriteFailure(t *testing.T) {
	database := openBridgeTestStore(t)
	provider := fake.NewFactory("home-built")
	factory := &recordingFactory{Factory: provider, launched: make(chan harness.Instance, 1)}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", AgentName: "failure-agent", Factory: factory, Store: &failingActivityStore{database}, Ledger: newMemoryLedger(),
			Repository: model.RepositoryContext{Directory: "/work/repo"}, SuppressStatus: true,
		})
	}()
	instance := <-factory.launched
	_ = waitForAgent(t, database, "failure-agent")
	if err := provider.Emit(instance, "operation-failed", "", harness.OperationStatusEvent{Status: harness.OperationFailed, Error: "boom"}); err != nil {
		t.Fatal(err)
	}
	message := waitForBody(t, database, "Deterministic fake harness operation failed")
	if !strings.Contains(message.Details, "Error: boom") {
		t.Fatalf("failure message = %#v", message)
	}
	select {
	case err := <-done:
		if err == nil || !strings.Contains(err.Error(), "persist harness activity: activity storage failed") {
			t.Fatalf("bridge error = %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("activity persistence failure did not stop the bridge")
	}
}

func waitForBody(t *testing.T, database *store.SQLite, body string) model.Message {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		messages, err := database.List(context.Background(), model.Filter{Limit: 100})
		if err != nil {
			t.Fatal(err)
		}
		for _, message := range messages {
			if message.Body == body {
				return message
			}
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("message body %q was not stored", body)
	return model.Message{}
}

func waitForMessage(t *testing.T, database *store.SQLite, id string, ready func(model.Message) bool) model.Message {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		message, err := database.Get(context.Background(), id)
		if err == nil && ready(message) {
			return message
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("message %s did not reach expected state", id)
	return model.Message{}
}
