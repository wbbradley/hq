package codexbridge

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/store"
)

func TestConnectionReporterPrintsConciseDiagnostic(t *testing.T) {
	var output bytes.Buffer
	reportConnectionUpdates(context.Background(), &output, domain.ClientUpdates{
		Initial: domain.ConnectionUpdate{Diagnostic: "HQ client and local node builds differ; restart the local HQ node"},
	})
	if got := output.String(); got != "hq codex: HQ client and local node builds differ; restart the local HQ node\n" {
		t.Fatalf("connection diagnostic = %q", got)
	}
}

type fakeMailboxStore struct {
	mu       sync.Mutex
	identity model.SessionIdentity
	repo     model.RepositoryContext
	messages []model.Message
	created  chan struct{}
}

func newFakeMailboxStore() *fakeMailboxStore {
	return &fakeMailboxStore{created: make(chan struct{}, 10)}
}

func (s *fakeMailboxStore) ResolveMailbox(_ context.Context, identity model.SessionIdentity, repo model.RepositoryContext) (model.Mailbox, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.identity, s.repo = identity, repo
	return model.Mailbox{ID: "agent-mailbox", Kind: model.MailboxAgent, Harness: identity.Harness}, nil
}

func (s *fakeMailboxStore) HumanMailbox(context.Context) (model.Mailbox, error) {
	return model.Mailbox{ID: model.HumanMailboxID, Kind: model.MailboxHuman}, nil
}

func (s *fakeMailboxStore) Create(_ context.Context, message model.Message) error {
	s.mu.Lock()
	s.messages = append(s.messages, message)
	s.mu.Unlock()
	s.created <- struct{}{}
	return nil
}

func (s *fakeMailboxStore) Get(_ context.Context, id string) (model.Message, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, message := range s.messages {
		if message.ID == id {
			return message, nil
		}
	}
	return model.Message{}, store.ErrNotFound
}

func (s *fakeMailboxStore) List(_ context.Context, filter model.Filter) ([]model.Message, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var messages []model.Message
	for _, message := range s.messages {
		if filter.ReplyTo != "" && (message.ReplyTo == nil || *message.ReplyTo != filter.ReplyTo) {
			continue
		}
		if filter.RecipientMailboxID != "" && message.RecipientMailboxID != filter.RecipientMailboxID {
			continue
		}
		messages = append(messages, message)
	}
	return messages, nil
}

func (s *fakeMailboxStore) Archive(_ context.Context, id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	for index := range s.messages {
		if s.messages[index].ID == id {
			now := time.Now().UTC()
			s.messages[index].ArchivedAt = &now
			return nil
		}
	}
	return store.ErrNotFound
}

func (s *fakeMailboxStore) Claim(context.Context, store.Claim, string) (model.Message, error) {
	return model.Message{}, store.ErrNotReady
}

func (s *fakeMailboxStore) Complete(context.Context, string, string) error { return nil }
func (s *fakeMailboxStore) Release(context.Context, string, string) error  { return nil }

type fakeProcess struct {
	clientInput  *io.PipeWriter
	clientOutput *io.PipeReader
	clientErrors *io.PipeReader
	serverInput  *io.PipeReader
	serverOutput *io.PipeWriter
	serverErrors *io.PipeWriter
	wait         chan error
	finishOnce   sync.Once
}

func newFakeProcess() *fakeProcess {
	serverInput, clientInput := io.Pipe()
	clientOutput, serverOutput := io.Pipe()
	clientErrors, serverErrors := io.Pipe()
	return &fakeProcess{
		clientInput: clientInput, clientOutput: clientOutput, clientErrors: clientErrors,
		serverInput: serverInput, serverOutput: serverOutput, serverErrors: serverErrors, wait: make(chan error, 1),
	}
}

func (p *fakeProcess) Input() io.WriteCloser { return p.clientInput }
func (p *fakeProcess) Output() io.ReadCloser { return p.clientOutput }
func (p *fakeProcess) Errors() io.ReadCloser { return p.clientErrors }
func (p *fakeProcess) Wait() error           { return <-p.wait }
func (p *fakeProcess) Kill() error {
	p.finish(errors.New("killed"))
	return nil
}
func (p *fakeProcess) finish(err error) {
	p.finishOnce.Do(func() {
		_ = p.serverOutput.Close()
		_ = p.serverErrors.Close()
		_ = p.serverInput.Close()
		p.wait <- err
	})
}

type fakeStarter struct{ process *fakeProcess }

func (s fakeStarter) Start(string) (Process, error) { return s.process, nil }

type recordedRequest struct {
	ID     int64           `json:"id"`
	Method string          `json:"method"`
	Params json.RawMessage `json:"params"`
}

func runHandshakeServer(t *testing.T, process *fakeProcess, threadID string, requests chan<- recordedRequest) {
	t.Helper()
	go func() {
		defer process.finish(nil)
		scanner := bufio.NewScanner(process.serverInput)
		for scanner.Scan() {
			var request recordedRequest
			if err := json.Unmarshal(scanner.Bytes(), &request); err != nil {
				t.Errorf("decode bridge request: %v", err)
				return
			}
			requests <- request
			var result string
			switch request.Method {
			case "initialize":
				result = `{}`
			case "initialized":
				continue
			case "thread/start", "thread/resume":
				result = `{"thread":{"id":"` + threadID + `"}}`
			case "turn/start":
				result = `{"turn":{"id":"turn-1","status":"inProgress"}}`
			default:
				t.Errorf("unexpected method %q", request.Method)
				return
			}
			_, _ = io.WriteString(process.serverOutput, `{"jsonrpc":"2.0","id":`+jsonNumber(request.ID)+`,"result":`+result+`}`+"\n")
		}
	}()
}

func jsonNumber(value int64) string {
	raw, _ := json.Marshal(value)
	return string(raw)
}

func TestRunStartsThreadBindsMailboxAndStartsInitialTurn(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 5)
	runHandshakeServer(t, process, "thread-new", requests)
	store := newFakeMailboxStore()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", InitialPrompt: "inspect the queue", Starter: fakeStarter{process}, Store: store,
			Repository: model.RepositoryContext{Directory: "/work/repo", Branch: "main"}, Stderr: io.Discard, Ledger: NewMemoryLedger(),
		})
	}()
	waitForMessages(t, store, 1)

	initialize := <-requests
	initialized := <-requests
	start := <-requests
	turn := <-requests
	if initialize.Method != "initialize" {
		t.Fatalf("first request = %s", initialize.Method)
	}
	if initialized.Method != "initialized" || initialized.ID != 0 || string(initialized.Params) != `{}` {
		t.Fatalf("initialization acknowledgement = %#v", initialized)
	}
	var initializeParams InitializeParams
	if json.Unmarshal(initialize.Params, &initializeParams) != nil || !initializeParams.Capabilities.ExperimentalAPI {
		t.Fatalf("initialize params = %s", initialize.Params)
	}
	var startParams ThreadStartParams
	if start.Method != "thread/start" || json.Unmarshal(start.Params, &startParams) != nil || startParams.CWD != "/work/repo" || startParams.DeveloperInstructions != RequireStructuredHumanInput {
		t.Fatalf("thread start = %s %s", start.Method, start.Params)
	}
	var turnParams TurnStartParams
	if turn.Method != "turn/start" || json.Unmarshal(turn.Params, &turnParams) != nil || len(turnParams.Input) != 1 || turnParams.Input[0].Text != "inspect the queue" || turnParams.ClientUserMessageID == "" {
		t.Fatalf("turn start = %s %s", turn.Method, turn.Params)
	}
	if store.identity != (model.SessionIdentity{Harness: "codex", ExternalSessionID: "thread-new"}) || store.repo.Directory != "/work/repo" {
		t.Fatalf("binding = %#v, %#v", store.identity, store.repo)
	}
	if store.messages[0].Body != "Codex bridge ready" || !strings.Contains(store.messages[0].Details, "Kind: status") || !strings.Contains(store.messages[0].Details, "thread-new") {
		t.Fatalf("ready message = %#v", store.messages[0])
	}
	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not stop after cancellation")
	}
	waitForMessages(t, store, 2)
	if store.messages[1].Body != "Codex bridge stopped" || !strings.Contains(store.messages[1].Details, "Kind: status") || !strings.Contains(store.messages[1].Details, "cancelled") {
		t.Fatalf("terminal message = %#v", store.messages[1])
	}
}

func TestRunReportsInitialTurnFailureWithSingleTerminalStatus(t *testing.T) {
	process := newFakeProcess()
	go func() {
		defer process.finish(nil)
		scanner := bufio.NewScanner(process.serverInput)
		for scanner.Scan() {
			var request recordedRequest
			if json.Unmarshal(scanner.Bytes(), &request) != nil {
				return
			}
			switch request.Method {
			case "initialize":
				_, _ = io.WriteString(process.serverOutput, `{"id":1,"result":{}}`+"\n")
			case "initialized":
			case "thread/start":
				_, _ = io.WriteString(process.serverOutput, `{"id":2,"result":{"thread":{"id":"thread-initial-failure"}}}`+"\n")
			case "turn/start":
				_, _ = io.WriteString(process.serverOutput, `{"id":3,"error":{"code":-32000,"message":"model unavailable"}}`+"\n")
			}
		}
	}()
	store := newFakeMailboxStore()
	err := Run(context.Background(), Options{
		Directory: "/work/repo", InitialPrompt: "begin", Starter: fakeStarter{process}, Store: store,
		Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard, Ledger: NewMemoryLedger(),
	})
	if err == nil || !strings.Contains(err.Error(), "model unavailable") {
		t.Fatalf("error = %v", err)
	}
	store.mu.Lock()
	messages := append([]model.Message(nil), store.messages...)
	store.mu.Unlock()
	if len(messages) != 2 || messages[0].Body != "Codex bridge ready" || messages[1].Body != "Codex bridge stopped" || !strings.Contains(messages[1].Details, "model unavailable") {
		t.Fatalf("messages = %#v", messages)
	}
}

func TestRunResumesExplicitThreadWithoutDeveloperInstruction(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 4)
	runHandshakeServer(t, process, "thread-existing", requests)
	store := newFakeMailboxStore()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{Directory: "/work/other", ResumeThreadID: "thread-existing", Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work/other"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	}()
	waitForMessages(t, store, 1)
	<-requests
	initialized := <-requests
	resume := <-requests
	if initialized.Method != "initialized" {
		t.Fatalf("second request = %s", initialized.Method)
	}
	var params map[string]any
	if resume.Method != "thread/resume" || json.Unmarshal(resume.Params, &params) != nil || params["threadId"] != "thread-existing" || params["cwd"] != "/work/other" {
		t.Fatalf("resume = %s %s", resume.Method, resume.Params)
	}
	if _, exists := params["developerInstructions"]; exists {
		t.Fatalf("resume replaced developer instructions: %s", resume.Params)
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestRunReportsUnexpectedEOF(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 4)
	runHandshakeServer(t, process, "thread-eof", requests)
	store := newFakeMailboxStore()
	done := make(chan error, 1)
	go func() {
		done <- Run(context.Background(), Options{Directory: "/work/repo", Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	}()
	waitForMessages(t, store, 1)
	_ = process.clientInput.Close()
	select {
	case err := <-done:
		if err == nil || (!strings.Contains(err.Error(), "closed") && !strings.Contains(err.Error(), "exited")) {
			t.Fatalf("error = %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not notice EOF")
	}
	waitForMessages(t, store, 2)
}

func TestRunReportsChildProcessFailure(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 4)
	runHandshakeServer(t, process, "thread-failure", requests)
	store := newFakeMailboxStore()
	done := make(chan error, 1)
	go func() {
		done <- Run(context.Background(), Options{Directory: "/work/repo", Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	}()
	waitForMessages(t, store, 1)
	process.finish(errors.New("exit status 7"))
	select {
	case err := <-done:
		if err == nil || !strings.Contains(err.Error(), "Codex app-server failed: exit status 7") {
			t.Fatalf("error = %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not report process failure")
	}
	waitForMessages(t, store, 2)
	if !strings.Contains(store.messages[1].Details, "exit status 7") {
		t.Fatalf("terminal message = %#v", store.messages[1])
	}
}

func TestBridgeDispatchesHQMessageEndToEnd(t *testing.T) {
	fixture := newDispatcherFixture(t)
	process := newFakeProcess()
	requests := make(chan recordedRequest, 8)
	runHandshakeServer(t, process, fixture.thread, requests)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", Starter: fakeStarter{process}, Store: fixture.store,
			Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard,
			Ledger: NewMemoryLedger(), RepairInterval: 2 * time.Millisecond,
		})
	}()
	initialize := <-requests
	initialized := <-requests
	startThread := <-requests
	if initialize.Method != "initialize" || initialized.Method != "initialized" || startThread.Method != "thread/start" {
		t.Fatalf("handshake = %s, %s, %s", initialize.Method, initialized.Method, startThread.Method)
	}
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "Codex bridge ready")
	messageID := "019c0000-0000-7000-8000-000000000117"
	fixture.addHumanMessage(t, messageID, "from HQ", time.Now().UTC())
	turn := <-requests
	if turn.Method != "turn/start" {
		t.Fatalf("inbound method = %s", turn.Method)
	}
	var params TurnStartParams
	if json.Unmarshal(turn.Params, &params) != nil || params.ClientUserMessageID != messageID || params.Input[0].Text != "from HQ" {
		t.Fatalf("turn params = %s", turn.Params)
	}
	waitForCompleted(t, fixture.store, messageID)
	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not stop")
	}
}

func TestBridgeRelaysCanonicalOutputBeforeSingleTerminalStatus(t *testing.T) {
	fixture := newDispatcherFixture(t)
	process := newFakeProcess()
	requests := make(chan recordedRequest, 4)
	runHandshakeServer(t, process, fixture.thread, requests)
	done := make(chan error, 1)
	go func() {
		done <- Run(context.Background(), Options{
			Directory: "/work/repo", Starter: fakeStarter{process}, Store: fixture.store,
			Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard,
			Ledger: NewMemoryLedger(), RepairInterval: 2 * time.Millisecond,
		})
	}()
	<-requests
	<-requests
	<-requests
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "Codex bridge ready")

	notifications := []string{
		`{"method":"item/agentMessage/delta","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","itemId":"agent-output","delta":"partial"}}`,
		`{"method":"item/completed","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","item":{"type":"reasoning","id":"reason-output","summary":["hidden"]}}}`,
		`{"method":"item/completed","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","item":{"type":"agentMessage","id":"agent-output","text":"Authoritative answer","phase":"final_answer"}}}`,
		`{"method":"item/completed","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","item":{"type":"agentMessage","id":"agent-output","text":"Authoritative answer","phase":"final_answer"}}}`,
		`{"method":"turn/completed","params":{"threadId":"` + fixture.thread + `","turn":{"id":"turn-output","status":"failed","error":{"message":"connection lost"}}}}`,
	}
	for _, notification := range notifications {
		if _, err := io.WriteString(process.serverOutput, notification+"\n"); err != nil {
			t.Fatal(err)
		}
	}
	process.finish(errors.New("exit status 9"))
	select {
	case err := <-done:
		if err == nil || !strings.Contains(err.Error(), "exit status 9") {
			t.Fatalf("error = %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not stop after child failure")
	}

	messages, err := fixture.store.List(context.Background(), model.Filter{RecipientMailboxID: model.HumanMailboxID, Limit: 100})
	if err != nil {
		t.Fatal(err)
	}
	var bodies []string
	counts := make(map[string]int)
	for _, message := range messages {
		bodies = append(bodies, message.Body)
		counts[message.Body]++
	}
	if counts["Authoritative answer"] != 1 || counts["Codex turn failed"] != 1 || counts["Codex bridge stopped"] != 1 || counts["partial"] != 0 {
		t.Fatalf("body counts = %#v, all=%#v", counts, bodies)
	}
	if len(bodies) != 4 || bodies[0] != "Codex bridge ready" || bodies[1] != "Authoritative answer" || bodies[2] != "Codex turn failed" || bodies[3] != "Codex bridge stopped" {
		t.Fatalf("message order = %#v", bodies)
	}
}

func TestBridgeRoutesServerApprovalThroughTemporaryHQStore(t *testing.T) {
	fixture := newDispatcherFixture(t)
	process := newFakeProcess()
	sendApproval := make(chan struct{})
	approvalResponse := make(chan rpcEnvelope, 1)
	serverDone := make(chan error, 1)
	go func() {
		defer process.finish(nil)
		scanner := bufio.NewScanner(process.serverInput)
		for scanner.Scan() {
			var envelope rpcEnvelope
			if err := json.Unmarshal(scanner.Bytes(), &envelope); err != nil {
				serverDone <- err
				return
			}
			switch envelope.Method {
			case "initialize":
				_, _ = io.WriteString(process.serverOutput, `{"id":1,"result":{}}`+"\n")
			case "initialized":
			case "thread/start":
				_, _ = io.WriteString(process.serverOutput, `{"id":2,"result":{"thread":{"id":"`+fixture.thread+`"}}}`+"\n")
				<-sendApproval
				request := `{"id":"file-approval-1","method":"item/fileChange/requestApproval","params":{"threadId":"` + fixture.thread + `","turnId":"turn-approval","itemId":"item-file","reason":"update generated code","grantRoot":"/work/repo"}}` + "\n"
				_, _ = io.WriteString(process.serverOutput, request)
			case "":
				if string(envelope.ID) == `"file-approval-1"` {
					approvalResponse <- envelope
				}
			default:
				serverDone <- fmt.Errorf("unexpected bridge method %q", envelope.Method)
				return
			}
		}
		serverDone <- scanner.Err()
	}()

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", Starter: fakeStarter{process}, Store: fixture.store,
			Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard,
			Ledger: NewMemoryLedger(), Replies: fixture.replies, RepairInterval: 2 * time.Millisecond,
		})
	}()
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "Codex bridge ready")
	close(sendApproval)

	question := waitForStoreMessage(t, fixture.store, model.HumanMailboxID, "Codex requests approval for file changes")
	if !strings.Contains(question.Details, "update generated code") || !strings.Contains(question.Details, "file-approval-1") {
		t.Fatalf("question = %#v", question)
	}
	reply := model.Message{
		ID: "019c0000-0000-7000-8000-000000000120", Context: question.Context,
		SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: fixture.agent.ID,
		Body: "accept", CreatedAt: time.Now().UTC(),
	}
	if err := fixture.store.Reply(context.Background(), question.ID, reply); err != nil {
		t.Fatal(err)
	}
	select {
	case response := <-approvalResponse:
		var result map[string]string
		if response.Error != nil || json.Unmarshal(response.Result, &result) != nil || result["decision"] != "accept" {
			t.Fatalf("approval response = %#v", response)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("app-server did not receive approval response")
	}

	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not stop")
	}
	if err := <-serverDone; err != nil {
		t.Fatal(err)
	}
}

func waitForStoreBody(t *testing.T, databaseStore *store.SQLite, recipientID, body string) {
	_ = waitForStoreMessage(t, databaseStore, recipientID, body)
}

func waitForStoreMessage(t *testing.T, databaseStore *store.SQLite, recipientID, body string) model.Message {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		messages, err := databaseStore.List(context.Background(), model.Filter{RecipientMailboxID: recipientID, Limit: 100})
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
	t.Fatalf("message %q was not stored", body)
	return model.Message{}
}

func waitForMessages(t *testing.T, store *fakeMailboxStore, count int) {
	t.Helper()
	deadline := time.After(3 * time.Second)
	for {
		store.mu.Lock()
		current := len(store.messages)
		store.mu.Unlock()
		if current >= count {
			return
		}
		select {
		case <-store.created:
		case <-deadline:
			t.Fatalf("got %d messages, want %d", current, count)
		}
	}
}
