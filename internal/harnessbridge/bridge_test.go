package harnessbridge

import (
	"context"
	"errors"
	"path/filepath"
	"sync"
	"testing"
	"time"

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
	waitForBody(t, database, "Neutral output")

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
