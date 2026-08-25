package codexbridge

import (
	"context"
	"encoding/json"
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

func outputNotification(method, params string) Notification {
	return Notification{Method: method, Params: json.RawMessage(params)}
}

func boundOutputRelay(fixture dispatcherFixture, ledger DeliveryLedger, syncMailbox func(context.Context) error) *OutputRelay {
	relay := NewOutputRelay(fixture.store, fixture.store, ledger, syncMailbox)
	relay.Bind(fixture.thread, fixture.agent, model.RepositoryContext{Directory: "/work/repo"})
	return relay
}

func TestOutputRelayPublishesOnlyCanonicalCompletedAgentMessages(t *testing.T) {
	fixture := newDispatcherFixture(t)
	ledger := NewMemoryLedger()
	relay := boundOutputRelay(fixture, ledger, nil)
	noise := []Notification{
		outputNotification("item/started", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"agentMessage","id":"started","text":"not final"}}`),
		outputNotification("item/agentMessage/delta", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","itemId":"agent-1","delta":"partial"}`),
		outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"reasoning","id":"reason-1","summary":["private"]}}`),
		outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"plan","id":"plan-1","text":"plan"}}`),
		outputNotification("rawResponseItem/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"message"}}`),
		outputNotification("item/commandExecution/outputDelta", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","delta":"logs"}`),
		outputNotification("item/mcpToolCall/progress", `{"threadId":"`+fixture.thread+`","turnId":"turn-1"}`),
		outputNotification("item/completed", `{malformed`),
		outputNotification("item/completed", `{"threadId":"other","turnId":"turn-1","item":{"type":"agentMessage","id":"wrong-thread","text":"wrong"}}`),
		outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"agentMessage","id":"empty","text":"  "}}`),
	}
	for _, notification := range noise {
		relay.HandleNotification(context.Background(), notification)
	}
	canonical := outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"agentMessage","id":"agent-1","text":"Canonical final text","phase":"final_answer"}}`)
	relay.HandleNotification(context.Background(), canonical)
	relay.HandleNotification(context.Background(), canonical)
	relay.HandleNotification(context.Background(), outputNotification("turn/completed", `{"threadId":"`+fixture.thread+`","turn":{"id":"turn-1","status":"completed"}}`))
	relay.StopAndWait()
	if err := relay.Err(); err != nil {
		t.Fatal(err)
	}

	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil || len(messages) != 1 {
		t.Fatalf("messages = %#v, %v", messages, err)
	}
	message := messages[0]
	if message.Body != "Canonical final text" || !strings.Contains(message.Details, "Kind: final-answer") || !strings.Contains(message.Details, "Harness operation: turn-1") || !strings.Contains(message.Details, "Harness item: agent-1") || !strings.Contains(message.Details, "Phase: final_answer") {
		t.Fatalf("message = %#v", message)
	}
	if message.ID != stableOutputMessageID(fixture.thread, "agent-1") {
		t.Fatalf("message ID = %s", message.ID)
	}
	sent, err := ledger.OutputSent(fixture.thread, "agent-1")
	if err != nil || !sent {
		t.Fatalf("ledger sent = %t, %v", sent, err)
	}
}

func TestOutputRelayPublishesFailedAndInterruptedTurnStatusesInOrder(t *testing.T) {
	fixture := newDispatcherFixture(t)
	relay := boundOutputRelay(fixture, NewMemoryLedger(), nil)
	relay.HandleNotification(context.Background(), outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-failed","item":{"type":"agentMessage","id":"agent-before-failure","text":"I got partway there","phase":"commentary"}}`))
	failed := outputNotification("turn/completed", `{"threadId":"`+fixture.thread+`","turn":{"id":"turn-failed","status":"failed","error":{"message":"upstream unavailable","additionalDetails":"retry later"}}}`)
	relay.HandleNotification(context.Background(), failed)
	relay.HandleNotification(context.Background(), failed)
	relay.HandleNotification(context.Background(), outputNotification("turn/completed", `{"threadId":"`+fixture.thread+`","turn":{"id":"turn-interrupted","status":"interrupted"}}`))
	relay.StopAndWait()

	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil || len(messages) != 3 {
		t.Fatalf("messages = %#v, %v", messages, err)
	}
	if messages[0].Body != "I got partway there" || messages[1].Body != "Codex turn failed" || messages[2].Body != "Codex turn interrupted" {
		t.Fatalf("message order = %#v", messages)
	}
	if !strings.Contains(messages[0].Details, "Kind: update") || !strings.Contains(messages[1].Details, "Kind: status") || !strings.Contains(messages[2].Details, "Kind: status") {
		t.Fatalf("message kinds = %#v", messages)
	}
	if !strings.Contains(messages[1].Details, "upstream unavailable") || !strings.Contains(messages[1].Details, "retry later") {
		t.Fatalf("failure = %#v", messages[1])
	}
}

func TestOutputRelayDeduplicatesAcrossPersistentLedgerRestart(t *testing.T) {
	fixture := newDispatcherFixture(t)
	path := filepath.Join(t.TempDir(), "outputs.json")
	ledger, err := OpenFileLedger(path)
	if err != nil {
		t.Fatal(err)
	}
	notification := outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"agentMessage","id":"agent-replayed","text":"Once"}}`)
	first := boundOutputRelay(fixture, ledger, nil)
	first.HandleNotification(context.Background(), notification)
	first.StopAndWait()

	reopened, err := OpenFileLedger(path)
	if err != nil {
		t.Fatal(err)
	}
	second := boundOutputRelay(fixture, reopened, nil)
	second.HandleNotification(context.Background(), notification)
	second.StopAndWait()
	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil || len(messages) != 1 {
		t.Fatalf("messages = %#v, %v", messages, err)
	}
}

func TestOutputRelayRecoversStoreBeforeLedgerCrashWindow(t *testing.T) {
	fixture := newDispatcherFixture(t)
	ledger := NewMemoryLedger()
	notification := outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"agentMessage","id":"agent-crash-window","text":"Recover me"}}`)
	first := boundOutputRelay(fixture, ledger, func(context.Context) error { return errors.New("sync unavailable") })
	first.HandleNotification(context.Background(), notification)
	<-first.Done()
	if err := first.Err(); err == nil || !strings.Contains(err.Error(), "sync unavailable") {
		t.Fatalf("error = %v", err)
	}
	first.StopAndWait()
	sent, _ := ledger.OutputSent(fixture.thread, "agent-crash-window")
	if sent {
		t.Fatal("output was checkpointed before sync")
	}

	second := boundOutputRelay(fixture, ledger, nil)
	second.HandleNotification(context.Background(), notification)
	second.StopAndWait()
	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil || len(messages) != 1 {
		t.Fatalf("messages = %#v, %v", messages, err)
	}
	sent, _ = ledger.OutputSent(fixture.thread, "agent-crash-window")
	if !sent {
		t.Fatal("recovered output was not checkpointed")
	}
}

func TestOutputRelayRejectsDeterministicMessageCollision(t *testing.T) {
	fixture := newDispatcherFixture(t)
	itemID := "agent-collision"
	collision := model.Message{
		ID: stableOutputMessageID(fixture.thread, itemID), Context: model.RepositoryContext{Directory: "/work/repo"},
		SenderMailboxID: fixture.agent.ID, RecipientMailboxID: model.HumanMailboxID, Body: "different", CreatedAt: testTime(),
	}
	if err := fixture.store.Create(context.Background(), collision); err != nil {
		t.Fatal(err)
	}
	relay := boundOutputRelay(fixture, NewMemoryLedger(), nil)
	relay.HandleNotification(context.Background(), outputNotification("item/completed", `{"threadId":"`+fixture.thread+`","turnId":"turn-1","item":{"type":"agentMessage","id":"`+itemID+`","text":"expected"}}`))
	<-relay.Done()
	if err := relay.Err(); err == nil || !strings.Contains(err.Error(), "collides") {
		t.Fatalf("error = %v", err)
	}
	relay.StopAndWait()
}

type failingOutputStore struct{ err error }

func (s failingOutputStore) Create(context.Context, model.Message) error { return s.err }
func (s failingOutputStore) Get(context.Context, string) (model.Message, error) {
	return model.Message{}, store.ErrNotFound
}

func TestOutputRelaySurfacesStoreFailure(t *testing.T) {
	relay := NewOutputRelay(failingOutputStore{err: errors.New("disk full")}, nil, NewMemoryLedger(), nil)
	relay.Bind("thread-1", model.Mailbox{ID: "agent-1"}, model.RepositoryContext{Directory: "/work"})
	relay.HandleNotification(context.Background(), outputNotification("item/completed", `{"threadId":"thread-1","turnId":"turn-1","item":{"type":"agentMessage","id":"agent-1","text":"result"}}`))
	<-relay.Done()
	if err := relay.Err(); err == nil || !strings.Contains(err.Error(), "disk full") {
		t.Fatalf("error = %v", err)
	}
	relay.StopAndWait()
}

func testTime() time.Time { return time.Unix(1, 0).UTC() }
