package codexbridge

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"io"
	"path/filepath"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

type dispatcherProtocol struct {
	client       *Client
	requests     chan recordedRequest
	serverInput  *io.PipeReader
	serverOutput *io.PipeWriter
}

func newDispatcherProtocol(t *testing.T, notifications NotificationHandler) *dispatcherProtocol {
	t.Helper()
	serverInput, clientOutput := io.Pipe()
	clientInput, serverOutput := io.Pipe()
	protocol := &dispatcherProtocol{
		client:   NewClient(context.Background(), clientInput, clientOutput, nil, notifications),
		requests: make(chan recordedRequest, 16), serverInput: serverInput, serverOutput: serverOutput,
	}
	go func() {
		scanner := bufio.NewScanner(serverInput)
		for scanner.Scan() {
			var request recordedRequest
			if json.Unmarshal(scanner.Bytes(), &request) == nil {
				protocol.requests <- request
			}
		}
	}()
	t.Cleanup(func() {
		_ = clientOutput.Close()
		_ = serverOutput.Close()
		_ = serverInput.Close()
		_ = clientInput.Close()
	})
	return protocol
}

func (p *dispatcherProtocol) next(t *testing.T, method string) recordedRequest {
	t.Helper()
	select {
	case request := <-p.requests:
		if request.Method != method {
			t.Fatalf("method = %q, want %q; params=%s", request.Method, method, request.Params)
		}
		return request
	case <-time.After(2 * time.Second):
		t.Fatalf("timed out waiting for %s", method)
		return recordedRequest{}
	}
}

func (p *dispatcherProtocol) result(t *testing.T, request recordedRequest, result string) {
	t.Helper()
	if _, err := io.WriteString(p.serverOutput, `{"id":`+jsonNumber(request.ID)+`,"result":`+result+`}`+"\n"); err != nil {
		t.Fatal(err)
	}
}

func (p *dispatcherProtocol) rpcError(t *testing.T, request recordedRequest, message string) {
	t.Helper()
	raw, _ := json.Marshal(message)
	if _, err := io.WriteString(p.serverOutput, `{"id":`+jsonNumber(request.ID)+`,"error":{"code":-32602,"message":`+string(raw)+`}}`+"\n"); err != nil {
		t.Fatal(err)
	}
}

func (p *dispatcherProtocol) notification(t *testing.T, method, params string) {
	t.Helper()
	if _, err := io.WriteString(p.serverOutput, `{"method":"`+method+`","params":`+params+`}`+"\n"); err != nil {
		t.Fatal(err)
	}
}

type dispatcherFixture struct {
	store   *store.SQLite
	agent   model.Mailbox
	thread  string
	state   *ThreadState
	ledger  DeliveryLedger
	replies *ReplyRegistry
}

func newDispatcherFixture(t *testing.T) dispatcherFixture {
	t.Helper()
	database := filepath.Join(t.TempDir(), "hq.db")
	keyPath, err := identity.KeyPath(database)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		t.Fatal(err)
	}
	databaseStore, err := store.Open(database)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { databaseStore.Close() })
	threadID := "019c0000-0000-7000-8000-000000000100"
	repository := model.RepositoryContext{Directory: "/work/repo"}
	agent, err := databaseStore.ResolveMailbox(context.Background(), model.SessionIdentity{Harness: "codex", ExternalSessionID: threadID}, repository)
	if err != nil {
		t.Fatal(err)
	}
	return dispatcherFixture{
		store: databaseStore, agent: agent, thread: threadID, state: NewThreadState(threadID),
		ledger: NewMemoryLedger(), replies: NewReplyRegistry(),
	}
}

func (f dispatcherFixture) addHumanMessage(t *testing.T, id, body string, created time.Time) {
	t.Helper()
	message := model.Message{
		ID: id, Context: model.RepositoryContext{Directory: "/work/repo"}, SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: f.agent.ID, Body: body, CreatedAt: created,
	}
	if err := f.store.Create(context.Background(), message); err != nil {
		t.Fatal(err)
	}
}

func (f dispatcherFixture) dispatcher(protocol *dispatcherProtocol) *Dispatcher {
	return &Dispatcher{
		Client: protocol.client, Store: f.store, Ledger: f.ledger, Replies: f.replies, State: f.state,
		ThreadID: f.thread, MailboxID: f.agent.ID, RepairInterval: 2 * time.Millisecond,
	}
}

func runDispatcher(t *testing.T, dispatcher *Dispatcher) (context.CancelFunc, <-chan error) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- dispatcher.Run(ctx) }()
	return cancel, done
}

func stopDispatcherTest(t *testing.T, cancel context.CancelFunc, done <-chan error) {
	t.Helper()
	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("dispatcher did not stop")
	}
}

func waitForCompleted(t *testing.T, databaseStore *store.SQLite, messageID string) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		message, err := databaseStore.Get(context.Background(), messageID)
		if err == nil && message.CompletedAt != nil {
			return
		}
		time.Sleep(2 * time.Millisecond)
	}
	t.Fatalf("message %s was not completed", messageID)
}

func TestDispatcherStartsIdleTurnAndCompletesClaim(t *testing.T) {
	fixture := newDispatcherFixture(t)
	messageID := "019c0000-0000-7000-8000-000000000101"
	fixture.addHumanMessage(t, messageID, "start work", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	request := protocol.next(t, "turn/start")
	var params TurnStartParams
	if json.Unmarshal(request.Params, &params) != nil || params.ThreadID != fixture.thread || params.ClientUserMessageID != messageID || params.Input[0].Text != "start work" {
		t.Fatalf("params = %s", request.Params)
	}
	protocol.result(t, request, `{"turn":{"id":"turn-1","status":"inProgress"}}`)
	waitForCompleted(t, fixture.store, messageID)
	record, found, _ := fixture.ledger.Delivery(fixture.thread, messageID)
	if !found || record.State != DeliveryAccepted || fixture.state.ActiveTurnID() != "turn-1" {
		t.Fatalf("record=%#v found=%t active=%q", record, found, fixture.state.ActiveTurnID())
	}
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherSteersActiveTurn(t *testing.T) {
	fixture := newDispatcherFixture(t)
	fixture.state.SetActive("turn-active")
	messageID := "019c0000-0000-7000-8000-000000000102"
	fixture.addHumanMessage(t, messageID, "one more thing", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	request := protocol.next(t, "turn/steer")
	var params TurnSteerParams
	if json.Unmarshal(request.Params, &params) != nil || params.ExpectedTurnID != "turn-active" || params.ClientUserMessageID != messageID {
		t.Fatalf("params = %s", request.Params)
	}
	protocol.result(t, request, `{"turnId":"turn-active"}`)
	waitForCompleted(t, fixture.store, messageID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherRetriesCompletedSteerRaceAsTurnStart(t *testing.T) {
	fixture := newDispatcherFixture(t)
	fixture.state.SetActive("turn-stale")
	messageID := "019c0000-0000-7000-8000-000000000103"
	fixture.addHumanMessage(t, messageID, "continue", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	steer := protocol.next(t, "turn/steer")
	protocol.rpcError(t, steer, "expectedTurnId does not match the active turn")
	read := protocol.next(t, "thread/read")
	protocol.result(t, read, `{"thread":{"id":"`+fixture.thread+`","turns":[{"id":"turn-stale","status":"completed","items":[]}]}}`)
	start := protocol.next(t, "turn/start")
	protocol.result(t, start, `{"turn":{"id":"turn-new","status":"inProgress"}}`)
	waitForCompleted(t, fixture.store, messageID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherRetargetsSteerWhenActiveTurnChanges(t *testing.T) {
	fixture := newDispatcherFixture(t)
	fixture.state.SetActive("turn-old")
	messageID := "019c0000-0000-7000-8000-000000000114"
	fixture.addHumanMessage(t, messageID, "steer the current turn", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	oldSteer := protocol.next(t, "turn/steer")
	protocol.rpcError(t, oldSteer, "expectedTurnId does not match the active turn")
	read := protocol.next(t, "thread/read")
	protocol.result(t, read, `{"thread":{"id":"`+fixture.thread+`","turns":[{"id":"turn-old","status":"completed","items":[]},{"id":"turn-new","status":"inProgress","items":[]}]}}`)
	newSteer := protocol.next(t, "turn/steer")
	var params TurnSteerParams
	_ = json.Unmarshal(newSteer.Params, &params)
	if params.ExpectedTurnID != "turn-new" {
		t.Fatalf("expected turn = %q", params.ExpectedTurnID)
	}
	protocol.result(t, newSteer, `{"turnId":"turn-new"}`)
	waitForCompleted(t, fixture.store, messageID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherDefersNonSteerableMessageUntilTurnCompletes(t *testing.T) {
	fixture := newDispatcherFixture(t)
	fixture.state.SetActive("turn-busy")
	messageID := "019c0000-0000-7000-8000-000000000104"
	fixture.addHumanMessage(t, messageID, "after the operation", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	steer := protocol.next(t, "turn/steer")
	protocol.rpcError(t, steer, "active operation cannot accept steering")
	read := protocol.next(t, "thread/read")
	protocol.result(t, read, `{"thread":{"id":"`+fixture.thread+`","turns":[{"id":"turn-busy","status":"inProgress","items":[]}]}}`)
	select {
	case request := <-protocol.requests:
		t.Fatalf("message was dispatched early: %#v", request)
	case <-time.After(40 * time.Millisecond):
	}
	protocol.notification(t, "turn/completed", `{"threadId":"`+fixture.thread+`","turn":{"id":"turn-busy","status":"completed"}}`)
	start := protocol.next(t, "turn/start")
	protocol.result(t, start, `{"turn":{"id":"turn-after","status":"inProgress"}}`)
	waitForCompleted(t, fixture.store, messageID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherRetainsNonSteerableClaimWhenHistoryReadFails(t *testing.T) {
	fixture := newDispatcherFixture(t)
	fixture.state.SetActive("turn-busy")
	messageID := "019c0000-0000-7000-8000-000000000118"
	fixture.addHumanMessage(t, messageID, "keep waiting", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	steer := protocol.next(t, "turn/steer")
	protocol.rpcError(t, steer, "active operation cannot accept steering")
	read := protocol.next(t, "thread/read")
	protocol.rpcError(t, read, "history temporarily unavailable")
	if _, err := fixture.store.Claim(context.Background(), store.Claim{MessageID: messageID}, "thief"); !errors.Is(err, store.ErrNotReady) {
		t.Fatalf("retained claim was stealable: %v", err)
	}
	protocol.notification(t, "turn/completed", `{"threadId":"`+fixture.thread+`","turn":{"id":"turn-busy","status":"completed"}}`)
	start := protocol.next(t, "turn/start")
	protocol.result(t, start, `{"turn":{"id":"turn-later","status":"inProgress"}}`)
	waitForCompleted(t, fixture.store, messageID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherPreservesConcurrentMessageOrder(t *testing.T) {
	fixture := newDispatcherFixture(t)
	firstID := "019c0000-0000-7000-8000-000000000105"
	secondID := "019c0000-0000-7000-8000-000000000106"
	now := time.Now().UTC()
	fixture.addHumanMessage(t, firstID, "first", now)
	fixture.addHumanMessage(t, secondID, "second", now.Add(time.Millisecond))
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	start := protocol.next(t, "turn/start")
	var startParams TurnStartParams
	_ = json.Unmarshal(start.Params, &startParams)
	protocol.result(t, start, `{"turn":{"id":"turn-order","status":"inProgress"}}`)
	steer := protocol.next(t, "turn/steer")
	var steerParams TurnSteerParams
	_ = json.Unmarshal(steer.Params, &steerParams)
	if startParams.ClientUserMessageID != firstID || steerParams.ClientUserMessageID != secondID {
		t.Fatalf("delivery order = %q, %q", startParams.ClientUserMessageID, steerParams.ClientUserMessageID)
	}
	protocol.result(t, steer, `{"turnId":"turn-order"}`)
	waitForCompleted(t, fixture.store, secondID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherReconcilesUncertainDeliveryAfterRestart(t *testing.T) {
	fixture := newDispatcherFixture(t)
	messageID := "019c0000-0000-7000-8000-000000000107"
	fixture.addHumanMessage(t, messageID, "already accepted", time.Now().UTC())
	ledgerPath := filepath.Join(t.TempDir(), "bridge-deliveries.json")
	fileLedger, err := OpenFileLedger(ledgerPath)
	if err != nil {
		t.Fatal(err)
	}
	if err := fileLedger.SetDelivery(fixture.thread, messageID, DeliveryUncertain); err != nil {
		t.Fatal(err)
	}
	fixture.ledger, err = OpenFileLedger(ledgerPath)
	if err != nil {
		t.Fatal(err)
	}
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	read := protocol.next(t, "thread/read")
	protocol.result(t, read, `{"thread":{"id":"`+fixture.thread+`","turns":[{"id":"turn-old","status":"completed","items":[{"type":"userMessage","id":"item-1","clientId":"`+messageID+`"}]}]}}`)
	waitForCompleted(t, fixture.store, messageID)
	select {
	case request := <-protocol.requests:
		t.Fatalf("duplicate Codex message was sent: %#v", request)
	case <-time.After(30 * time.Millisecond):
	}
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherCompletesDuplicateAcceptedLeaseWithoutCodexCall(t *testing.T) {
	fixture := newDispatcherFixture(t)
	messageID := "019c0000-0000-7000-8000-000000000112"
	fixture.addHumanMessage(t, messageID, "duplicate lease", time.Now().UTC())
	if err := fixture.ledger.SetDelivery(fixture.thread, messageID, DeliveryAccepted); err != nil {
		t.Fatal(err)
	}
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	waitForCompleted(t, fixture.store, messageID)
	select {
	case request := <-protocol.requests:
		t.Fatalf("accepted duplicate reached Codex: %#v", request)
	case <-time.After(30 * time.Millisecond):
	}
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherWakesImmediatelyForMailboxInvalidation(t *testing.T) {
	fixture := newDispatcherFixture(t)
	protocol := newDispatcherProtocol(t, fixture.state)
	dispatcher := fixture.dispatcher(protocol)
	dispatcher.RepairInterval = time.Hour
	invalidations := make(chan domain.Invalidation, 1)
	dispatcher.Invalidations = invalidations
	cancel, done := runDispatcher(t, dispatcher)
	time.Sleep(20 * time.Millisecond)

	messageID := "019c0000-0000-7000-8000-000000000118"
	fixture.addHumanMessage(t, messageID, "live update", time.Now().UTC())
	started := time.Now()
	invalidations <- domain.Invalidation{Revision: 2, Topics: []domain.ChangeTopic{domain.TopicMessages}}
	request := protocol.next(t, "turn/start")
	if elapsed := time.Since(started); elapsed > 500*time.Millisecond {
		t.Fatalf("mailbox invalidation wake took %s", elapsed)
	}
	protocol.result(t, request, `{"turn":{"id":"turn-live"}}`)
	waitForCompleted(t, fixture.store, messageID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherReleasesClaimAfterTransientCodexFailure(t *testing.T) {
	fixture := newDispatcherFixture(t)
	messageID := "019c0000-0000-7000-8000-000000000113"
	fixture.addHumanMessage(t, messageID, "try later", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	dispatcher := fixture.dispatcher(protocol)
	dispatcher.RepairInterval = time.Second
	cancel, done := runDispatcher(t, dispatcher)
	start := protocol.next(t, "turn/start")
	protocol.rpcError(t, start, "temporarily unavailable")
	time.Sleep(20 * time.Millisecond)
	stopDispatcherTest(t, cancel, done)
	message, err := fixture.store.Claim(context.Background(), store.Claim{MessageID: messageID, RecipientMailboxID: fixture.agent.ID}, "retry-owner")
	if err != nil || message.ID != messageID {
		t.Fatalf("released claim = %#v, %v", message, err)
	}
	_ = fixture.store.Release(context.Background(), messageID, "retry-owner")
}

func TestDispatcherPrioritizesRegisteredReplyOverGeneralInput(t *testing.T) {
	fixture := newDispatcherFixture(t)
	human, err := fixture.store.HumanMailbox(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	questionID := "019c0000-0000-7000-8000-000000000108"
	question := model.Message{ID: questionID, Context: model.RepositoryContext{Directory: "/work/repo"}, SenderMailboxID: fixture.agent.ID, RecipientMailboxID: human.ID, Body: "Approve?", CreatedAt: time.Now().UTC()}
	if err := fixture.store.Create(context.Background(), question); err != nil {
		t.Fatal(err)
	}
	waiter, err := fixture.replies.Register(questionID)
	if err != nil {
		t.Fatal(err)
	}
	reply := model.Message{ID: "019c0000-0000-7000-8000-000000000109", Context: question.Context, SenderMailboxID: human.ID, RecipientMailboxID: fixture.agent.ID, Body: "yes", CreatedAt: time.Now().UTC()}
	if err := fixture.store.Reply(context.Background(), questionID, reply); err != nil {
		t.Fatal(err)
	}
	unsolicitedID := "019c0000-0000-7000-8000-000000000110"
	fixture.addHumanMessage(t, unsolicitedID, "ordinary input", time.Now().UTC().Add(time.Millisecond))
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	var claimedReply *ClaimedReply
	select {
	case claimedReply = <-waiter.Replies:
	case <-time.After(2 * time.Second):
		t.Fatal("registered reply was not claimed")
	}
	if claimedReply == nil || claimedReply.Message.ID != reply.ID {
		t.Fatalf("claimed reply = %#v", claimedReply)
	}
	request := protocol.next(t, "turn/start")
	var params TurnStartParams
	_ = json.Unmarshal(request.Params, &params)
	if params.ClientUserMessageID != unsolicitedID {
		t.Fatalf("general dispatcher consumed %q", params.ClientUserMessageID)
	}
	if err := claimedReply.Complete(context.Background()); err != nil {
		t.Fatal(err)
	}
	protocol.result(t, request, `{"turn":{"id":"turn-general","status":"inProgress"}}`)
	waitForCompleted(t, fixture.store, unsolicitedID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherTreatsCurrentThreadReplyAsNormalInput(t *testing.T) {
	fixture := newDispatcherFixture(t)
	human, err := fixture.store.HumanMailbox(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	questionID := "019c0000-0000-7000-8000-000000000115"
	question := model.Message{ID: questionID, Context: model.RepositoryContext{Directory: "/work/repo"}, SenderMailboxID: fixture.agent.ID, RecipientMailboxID: human.ID, Body: "Earlier output", CreatedAt: time.Now().UTC()}
	if err := fixture.store.Create(context.Background(), question); err != nil {
		t.Fatal(err)
	}
	reply := model.Message{ID: "019c0000-0000-7000-8000-000000000116", Context: question.Context, SenderMailboxID: human.ID, RecipientMailboxID: fixture.agent.ID, Body: "follow up", Details: "Codex thread: " + fixture.thread, CreatedAt: time.Now().UTC()}
	if err := fixture.store.Reply(context.Background(), questionID, reply); err != nil {
		t.Fatal(err)
	}
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	request := protocol.next(t, "turn/start")
	var params TurnStartParams
	if json.Unmarshal(request.Params, &params) != nil || params.ClientUserMessageID != reply.ID || params.Input[0].Text != reply.Body {
		t.Fatalf("params = %s", request.Params)
	}
	protocol.result(t, request, `{"turn":{"id":"turn-reply","status":"inProgress"}}`)
	waitForCompleted(t, fixture.store, reply.ID)
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherDoesNotDeliverReplyFromAnotherThread(t *testing.T) {
	fixture := newDispatcherFixture(t)
	human, err := fixture.store.HumanMailbox(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	questionID := "019c0000-0000-7000-8000-000000000119"
	question := model.Message{ID: questionID, Context: model.RepositoryContext{Directory: "/work/repo"}, SenderMailboxID: fixture.agent.ID, RecipientMailboxID: human.ID, Body: "Old thread output", CreatedAt: time.Now().UTC()}
	if err := fixture.store.Create(context.Background(), question); err != nil {
		t.Fatal(err)
	}
	reply := model.Message{ID: "019c0000-0000-7000-8000-000000000120", Context: question.Context, SenderMailboxID: human.ID, RecipientMailboxID: fixture.agent.ID, Body: "stale follow up", Details: "Codex thread: replaced-thread", CreatedAt: time.Now().UTC()}
	if err := fixture.store.Reply(context.Background(), questionID, reply); err != nil {
		t.Fatal(err)
	}
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	select {
	case request := <-protocol.requests:
		t.Fatalf("old-thread reply became %s", request.Method)
	case <-time.After(150 * time.Millisecond):
	}
	stopDispatcherTest(t, cancel, done)
}

func TestDispatcherReleasesClaimOnCancellation(t *testing.T) {
	fixture := newDispatcherFixture(t)
	fixture.state.SetActive("turn-busy")
	messageID := "019c0000-0000-7000-8000-000000000111"
	fixture.addHumanMessage(t, messageID, "retain me", time.Now().UTC())
	protocol := newDispatcherProtocol(t, fixture.state)
	cancel, done := runDispatcher(t, fixture.dispatcher(protocol))
	steer := protocol.next(t, "turn/steer")
	protocol.rpcError(t, steer, "active operation cannot accept steering")
	read := protocol.next(t, "thread/read")
	protocol.result(t, read, `{"thread":{"id":"`+fixture.thread+`","turns":[{"id":"turn-busy","status":"inProgress","items":[]}]}}`)
	stopDispatcherTest(t, cancel, done)
	message, err := fixture.store.Claim(context.Background(), store.Claim{MessageID: messageID, RecipientMailboxID: fixture.agent.ID}, "new-owner")
	if err != nil || message.ID != messageID {
		t.Fatalf("released claim = %#v, %v", message, err)
	}
	_ = fixture.store.Release(context.Background(), messageID, "new-owner")
}

func TestThreadHistoryMatchesClientIDNotItemID(t *testing.T) {
	thread := Thread{Turns: []Turn{{Items: []ThreadItem{{Type: "userMessage", ID: "same", ClientID: "different"}}}}}
	if threadHasClientID(thread, "same") {
		t.Fatal("Codex item ID was mistaken for clientUserMessageId")
	}
	if !threadHasClientID(thread, "different") {
		t.Fatal("clientId was not found")
	}
}

func TestNonSteerableErrorClassificationFailsClosed(t *testing.T) {
	if !nonSteerableError(errors.New("active operation cannot accept steering")) {
		t.Fatal("known non-steerable error was not recognized")
	}
	if nonSteerableError(errors.New("permission denied")) {
		t.Fatal("unrelated error was treated as non-steerable")
	}
}
