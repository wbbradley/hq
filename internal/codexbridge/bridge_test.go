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
	mu         sync.Mutex
	identity   model.SessionIdentity
	repo       model.RepositoryContext
	messages   []model.Message
	created    chan struct{}
	namedAgent domain.NamedAgent
	ownerToken string
	acquireErr error
	renewErr   error
	renewals   int
	releases   int
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

func (s *fakeMailboxStore) CreateNamedAgent(_ context.Context, name, _ string) (domain.NamedAgent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.namedAgent.Name == "" {
		s.namedAgent = domain.NamedAgent{Name: name, MailboxID: "named-mailbox"}
	}
	return s.namedAgent, nil
}

func (s *fakeMailboxStore) SelectNamedAgentSession(_ context.Context, name string, identity model.SessionIdentity, repo model.RepositoryContext) (domain.NamedAgent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.identity, s.repo = identity, repo
	s.namedAgent.Name, s.namedAgent.Harness, s.namedAgent.CurrentSessionID = name, identity.Harness, identity.ExternalSessionID
	if s.namedAgent.MailboxID == "" {
		s.namedAgent.MailboxID = "named-mailbox"
	}
	return s.namedAgent, nil
}

func (s *fakeMailboxStore) AcquireNamedAgent(_ context.Context, _ string, token string, _ time.Duration) (domain.NamedAgent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.acquireErr != nil {
		return domain.NamedAgent{}, s.acquireErr
	}
	s.ownerToken = token
	return s.namedAgent, nil
}

func (s *fakeMailboxStore) RenewNamedAgent(_ context.Context, _ string, token string, _ time.Duration) (domain.NamedAgent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.renewErr != nil {
		return domain.NamedAgent{}, s.renewErr
	}
	if token != s.ownerToken {
		return domain.NamedAgent{}, domain.ErrAgentOwned
	}
	s.renewals++
	return s.namedAgent, nil
}

func (s *fakeMailboxStore) ReleaseNamedAgent(_ context.Context, _ string, token string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if token == s.ownerToken {
		s.releases++
	}
	return nil
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
	killOnce     sync.Once
	killed       chan struct{}
}

func newFakeProcess() *fakeProcess {
	serverInput, clientInput := io.Pipe()
	clientOutput, serverOutput := io.Pipe()
	clientErrors, serverErrors := io.Pipe()
	return &fakeProcess{
		clientInput: clientInput, clientOutput: clientOutput, clientErrors: clientErrors,
		serverInput: serverInput, serverOutput: serverOutput, serverErrors: serverErrors,
		wait: make(chan error, 1), killed: make(chan struct{}),
	}
}

func (p *fakeProcess) Input() io.WriteCloser { return p.clientInput }
func (p *fakeProcess) Output() io.ReadCloser { return p.clientOutput }
func (p *fakeProcess) Errors() io.ReadCloser { return p.clientErrors }
func (p *fakeProcess) Wait() error           { return <-p.wait }
func (p *fakeProcess) Kill() error {
	p.killOnce.Do(func() { close(p.killed) })
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

func TestRunRequiresNamedAgent(t *testing.T) {
	err := Run(context.Background(), Options{Directory: "/work", Store: newFakeMailboxStore()})
	if err == nil || !strings.Contains(err.Error(), "durable agent name") {
		t.Fatalf("error = %v", err)
	}
}

func TestRunStartsYoloThreadBindsMailboxAndStartsInitialTurn(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 5)
	runHandshakeServer(t, process, "thread-new", requests)
	store := newFakeMailboxStore()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", AgentName: "test-agent", InitialPrompt: "inspect the queue", Yolo: true, Starter: fakeStarter{process}, Store: store,
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
	if start.Method != "thread/start" || json.Unmarshal(start.Params, &startParams) != nil || startParams.CWD != "/work/repo" || startParams.DeveloperInstructions != NamedAgentDeveloperInstructions("test-agent") || startParams.ApprovalPolicy != approvalPolicyNever || startParams.Sandbox != sandboxModeDangerFullAccess {
		t.Fatalf("thread start = %s %s", start.Method, start.Params)
	}
	var turnParams TurnStartParams
	if turn.Method != "turn/start" || json.Unmarshal(turn.Params, &turnParams) != nil || len(turnParams.Input) != 1 || turnParams.Input[0].Text != "inspect the queue" || turnParams.ClientUserMessageID == "" {
		t.Fatalf("turn start = %s %s", turn.Method, turn.Params)
	}
	if store.identity != (model.SessionIdentity{Harness: "codex", ExternalSessionID: "thread-new"}) || store.repo.Directory != "/work/repo" {
		t.Fatalf("binding = %#v, %#v", store.identity, store.repo)
	}
	if store.messages[0].Body != "test-agent ready in /work/repo" || !strings.Contains(store.messages[0].Details, "Kind: status") || !strings.Contains(store.messages[0].Details, "thread-new") {
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
	select {
	case <-process.killed:
		t.Fatal("graceful stdin-close shutdown killed the process")
	default:
	}
	waitForMessages(t, store, 2)
	if store.messages[1].Body != "Codex bridge stopped" || !strings.Contains(store.messages[1].Details, "Kind: status") || !strings.Contains(store.messages[1].Details, "cancelled") {
		t.Fatalf("terminal message = %#v", store.messages[1])
	}
}

func TestRunForcesProcessShutdownAfterGracePeriod(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 4)
	go func() {
		scanner := bufio.NewScanner(process.serverInput)
		for scanner.Scan() {
			var request recordedRequest
			if json.Unmarshal(scanner.Bytes(), &request) != nil {
				return
			}
			requests <- request
			switch request.Method {
			case "initialize":
				_, _ = io.WriteString(process.serverOutput, `{"id":1,"result":{}}`+"\n")
			case "initialized":
			case "thread/start":
				_, _ = io.WriteString(process.serverOutput, `{"id":2,"result":{"thread":{"id":"thread-stubborn"}}}`+"\n")
			}
		}
		// Deliberately leave stdout and the process wait channel open after
		// stdin closes. The bridge must escalate to Kill.
	}()
	store := newFakeMailboxStore()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	ready := make(chan struct{})
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", AgentName: "test-agent", Starter: fakeStarter{process}, Store: store,
			Stderr: io.Discard, Ledger: NewMemoryLedger(), SuppressStatus: true,
			OnReady: func(BridgeReady) { close(ready) },
		})
	}()
	select {
	case <-ready:
	case <-time.After(time.Second):
		t.Fatal("bridge did not become ready")
	}
	cancel()
	select {
	case <-process.killed:
	case <-time.After(gracefulProcessStop + time.Second):
		t.Fatal("bridge did not kill process after graceful shutdown timeout")
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("bridge did not finish after forced process shutdown")
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
		Directory: "/work/repo", AgentName: "test-agent", InitialPrompt: "begin", Starter: fakeStarter{process}, Store: store,
		Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard, Ledger: NewMemoryLedger(),
	})
	if err == nil || !strings.Contains(err.Error(), "model unavailable") {
		t.Fatalf("error = %v", err)
	}
	store.mu.Lock()
	messages := append([]model.Message(nil), store.messages...)
	store.mu.Unlock()
	if len(messages) != 2 || messages[0].Body != "test-agent ready in /work/repo" || messages[1].Body != "Codex bridge stopped" || !strings.Contains(messages[1].Details, "model unavailable") {
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
		done <- Run(ctx, Options{Directory: "/work/other", AgentName: "test-agent", ResumeThreadID: "thread-existing", Yolo: true, Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work/other"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	}()
	waitForMessages(t, store, 1)
	<-requests
	initialized := <-requests
	resume := <-requests
	if initialized.Method != "initialized" {
		t.Fatalf("second request = %s", initialized.Method)
	}
	var params map[string]any
	if resume.Method != "thread/resume" || json.Unmarshal(resume.Params, &params) != nil || params["threadId"] != "thread-existing" || params["cwd"] != "/work/other" || params["approvalPolicy"] != approvalPolicyNever || params["sandbox"] != sandboxModeDangerFullAccess {
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

func TestRunNamedAgentStartsSelectsRenewsAndReleases(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 8)
	runHandshakeServer(t, process, "thread-named", requests)
	store := newFakeMailboxStore()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/named", AgentName: "fred", Starter: fakeStarter{process}, Store: store,
			Repository: model.RepositoryContext{Directory: "/work/named"}, Stderr: io.Discard, Ledger: NewMemoryLedger(),
			AgentLeaseDuration: 30 * time.Millisecond, AgentRenewInterval: 5 * time.Millisecond,
		})
	}()
	waitForMessages(t, store, 1)
	<-requests
	<-requests
	start := <-requests
	if start.Method != "thread/start" {
		t.Fatalf("thread request = %s", start.Method)
	}
	var startParams ThreadStartParams
	if err := json.Unmarshal(start.Params, &startParams); err != nil {
		t.Fatal(err)
	}
	wantInstructions := NamedAgentDeveloperInstructions("fred")
	if startParams.DeveloperInstructions != wantInstructions || !strings.Contains(startParams.DeveloperInstructions, `durable agent named "fred"`) || !strings.Contains(startParams.DeveloperInstructions, RequireStructuredHumanInput) {
		t.Fatalf("developer instructions = %q", startParams.DeveloperInstructions)
	}
	store.mu.Lock()
	readyBody := store.messages[0].Body
	store.mu.Unlock()
	if readyBody != "fred ready in /work/named" {
		t.Fatalf("ready body = %q", readyBody)
	}
	time.Sleep(15 * time.Millisecond)
	store.mu.Lock()
	selected, renewals := store.namedAgent, store.renewals
	store.mu.Unlock()
	if selected.Name != "fred" || selected.MailboxID != "named-mailbox" || selected.CurrentSessionID != "thread-named" || renewals == 0 {
		t.Fatalf("selected=%#v renewals=%d", selected, renewals)
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	store.mu.Lock()
	releases := store.releases
	store.mu.Unlock()
	if releases != 1 {
		t.Fatalf("releases = %d", releases)
	}
}

func TestRunNamedAgentAutomaticallyResumesOrExplicitlyRotates(t *testing.T) {
	for _, test := range []struct {
		name       string
		newThread  bool
		returnedID string
		method     string
	}{
		{name: "resume", returnedID: "thread-existing", method: "thread/resume"},
		{name: "rotate", newThread: true, returnedID: "thread-replacement", method: "thread/start"},
	} {
		t.Run(test.name, func(t *testing.T) {
			process := newFakeProcess()
			requests := make(chan recordedRequest, 8)
			runHandshakeServer(t, process, test.returnedID, requests)
			store := newFakeMailboxStore()
			store.namedAgent = domain.NamedAgent{Name: "fred", MailboxID: "named-mailbox", Harness: "codex", CurrentSessionID: "thread-existing"}
			ctx, cancel := context.WithCancel(context.Background())
			done := make(chan error, 1)
			go func() {
				done <- Run(ctx, Options{Directory: "/work", AgentName: "fred", NewThread: test.newThread, Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
			}()
			waitForMessages(t, store, 1)
			<-requests
			<-requests
			threadRequest := <-requests
			if threadRequest.Method != test.method {
				t.Fatalf("thread request = %s", threadRequest.Method)
			}
			if test.method == "thread/resume" && !strings.Contains(string(threadRequest.Params), "thread-existing") {
				t.Fatalf("resume params = %s", threadRequest.Params)
			}
			if test.method == "thread/resume" && strings.Contains(string(threadRequest.Params), "developerInstructions") {
				t.Fatalf("resume replaced developer instructions: %s", threadRequest.Params)
			}
			cancel()
			if err := <-done; err != nil {
				t.Fatal(err)
			}
			store.mu.Lock()
			selected := store.namedAgent.CurrentSessionID
			store.mu.Unlock()
			if selected != test.returnedID {
				t.Fatalf("selected = %q", selected)
			}
		})
	}
}

func TestRunNamedAgentMissingRolloutDoesNotReplaceOrSelect(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 8)
	go func() {
		defer process.finish(nil)
		scanner := bufio.NewScanner(process.serverInput)
		for scanner.Scan() {
			var request recordedRequest
			if json.Unmarshal(scanner.Bytes(), &request) != nil {
				return
			}
			requests <- request
			switch request.Method {
			case "initialize":
				_, _ = io.WriteString(process.serverOutput, `{"id":`+jsonNumber(request.ID)+`,"result":{}}`+"\n")
			case "initialized":
			case "thread/resume":
				_, _ = io.WriteString(process.serverOutput, `{"id":`+jsonNumber(request.ID)+`,"error":{"code":-32602,"message":"no rollout found for thread id thread-empty"}}`+"\n")
			case "thread/start":
				_, _ = io.WriteString(process.serverOutput, `{"id":`+jsonNumber(request.ID)+`,"result":{"thread":{"id":"thread-replacement"}}}`+"\n")
			}
		}
	}()
	store := newFakeMailboxStore()
	store.namedAgent = domain.NamedAgent{Name: "fred", MailboxID: "named-mailbox", Harness: "codex", CurrentSessionID: "thread-empty"}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{Directory: "/work", AgentName: "fred", Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	}()
	for _, method := range []string{"initialize", "initialized", "thread/resume"} {
		if request := <-requests; request.Method != method {
			t.Fatalf("request = %q, want %q", request.Method, method)
		}
	}
	if err := <-done; err == nil || !strings.Contains(err.Error(), "no rollout found") {
		t.Fatalf("error = %v", err)
	}
	cancel()
	select {
	case request := <-requests:
		if request.Method == "thread/start" {
			t.Fatalf("missing rollout silently created a replacement: %#v", request)
		}
	default:
	}
	store.mu.Lock()
	selected := store.namedAgent.CurrentSessionID
	store.mu.Unlock()
	if selected != "thread-empty" {
		t.Fatalf("selected = %q", selected)
	}
}

func TestRunNamedAgentDrainsOfflineRootMessages(t *testing.T) {
	fixture := newDispatcherFixture(t)
	agent, err := fixture.store.CreateNamedAgent(context.Background(), "fred", "")
	if err != nil {
		t.Fatal(err)
	}
	queuedID := "019c0000-0000-7000-8000-000000000301"
	if err := fixture.store.Create(context.Background(), model.Message{
		ID: queuedID, Context: model.RepositoryContext{Directory: "/work/repo"}, SenderMailboxID: model.HumanMailboxID,
		RecipientMailboxID: agent.MailboxID, Body: "queued while offline", CreatedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatal(err)
	}
	process := newFakeProcess()
	requests := make(chan recordedRequest, 8)
	runHandshakeServer(t, process, "thread-offline", requests)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- Run(ctx, Options{
			Directory: "/work/repo", AgentName: "fred", Store: fixture.store, Starter: fakeStarter{process},
			Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard, Ledger: NewMemoryLedger(), RepairInterval: 2 * time.Millisecond,
		})
	}()
	<-requests
	<-requests
	if request := <-requests; request.Method != "thread/start" {
		t.Fatalf("thread request = %s", request.Method)
	}
	delivery := <-requests
	var params TurnStartParams
	if delivery.Method != "turn/start" || json.Unmarshal(delivery.Params, &params) != nil || params.ClientUserMessageID != queuedID || params.Input[0].Text != "queued while offline" {
		t.Fatalf("delivery = %s %s", delivery.Method, delivery.Params)
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestRunNamedAgentRejectsCompetingOwnerBeforeStartingProcess(t *testing.T) {
	store := newFakeMailboxStore()
	store.namedAgent = domain.NamedAgent{Name: "fred", MailboxID: "named-mailbox"}
	store.acquireErr = &domain.AgentOwnershipConflict{Name: "fred", ExpiresAt: time.Now().Add(time.Minute)}
	err := Run(context.Background(), Options{Directory: "/work", AgentName: "fred", Store: store, Starter: fakeStarter{newFakeProcess()}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	if !errors.Is(err, domain.ErrAgentOwned) || !strings.Contains(err.Error(), "fred") {
		t.Fatalf("error = %v", err)
	}
}

func TestRunNamedAgentRejectsCrossHarnessSelectionWithoutRotation(t *testing.T) {
	store := newFakeMailboxStore()
	store.namedAgent = domain.NamedAgent{Name: "fred", MailboxID: "named-mailbox", Harness: "claude-code", CurrentSessionID: "claude-session"}
	err := Run(context.Background(), Options{Directory: "/work", AgentName: "fred", Store: store, Starter: fakeStarter{newFakeProcess()}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	if err == nil || !strings.Contains(err.Error(), "--new-session") || !strings.Contains(err.Error(), "claude-code") {
		t.Fatalf("error = %v", err)
	}
}

func TestRunNamedAgentStopsWhenLeaseRenewalFails(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 8)
	runHandshakeServer(t, process, "thread-owned", requests)
	store := newFakeMailboxStore()
	store.renewErr = domain.ErrAgentOwned
	done := make(chan error, 1)
	go func() {
		done <- Run(context.Background(), Options{
			Directory: "/work", AgentName: "fred", Store: store, Starter: fakeStarter{process}, Stderr: io.Discard, Ledger: NewMemoryLedger(),
			AgentLeaseDuration: 30 * time.Millisecond, AgentRenewInterval: 5 * time.Millisecond,
		})
	}()
	waitForMessages(t, store, 1)
	select {
	case err := <-done:
		if !errors.Is(err, domain.ErrAgentOwned) || !strings.Contains(err.Error(), "ownership lost") {
			t.Fatalf("error = %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("bridge did not stop after lease loss")
	}
}

func TestRunNamedAgentResumeFailureRequiresExplicitRotation(t *testing.T) {
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
			case "thread/resume":
				_, _ = io.WriteString(process.serverOutput, `{"id":2,"error":{"code":-32000,"message":"missing thread"}}`+"\n")
			}
		}
	}()
	store := newFakeMailboxStore()
	store.namedAgent = domain.NamedAgent{Name: "fred", MailboxID: "named-mailbox", Harness: "codex", CurrentSessionID: "thread-missing"}
	err := Run(context.Background(), Options{Directory: "/work", AgentName: "fred", Store: store, Starter: fakeStarter{process}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
	if err == nil || !strings.Contains(err.Error(), "--new-session") || !strings.Contains(err.Error(), "thread-missing") {
		t.Fatalf("error = %v", err)
	}
	store.mu.Lock()
	selected, releases := store.namedAgent.CurrentSessionID, store.releases
	store.mu.Unlock()
	if selected != "thread-missing" || releases != 1 {
		t.Fatalf("selected=%q releases=%d", selected, releases)
	}
}

func TestRunReportsUnexpectedEOF(t *testing.T) {
	process := newFakeProcess()
	requests := make(chan recordedRequest, 4)
	runHandshakeServer(t, process, "thread-eof", requests)
	store := newFakeMailboxStore()
	done := make(chan error, 1)
	go func() {
		done <- Run(context.Background(), Options{Directory: "/work/repo", AgentName: "test-agent", Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
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
		done <- Run(context.Background(), Options{Directory: "/work/repo", AgentName: "test-agent", Starter: fakeStarter{process}, Store: store, Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard, Ledger: NewMemoryLedger()})
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
			Directory: "/work/repo", AgentName: "test-agent", Starter: fakeStarter{process}, Store: fixture.store,
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
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "test-agent ready in /work/repo")
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
			Directory: "/work/repo", AgentName: "test-agent", Starter: fakeStarter{process}, Store: fixture.store,
			Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard,
			Ledger: NewMemoryLedger(), RepairInterval: 2 * time.Millisecond,
		})
	}()
	<-requests
	<-requests
	<-requests
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "test-agent ready in /work/repo")

	notifications := []string{
		`{"method":"turn/started","params":{"threadId":"` + fixture.thread + `","turn":{"id":"turn-output","status":"inProgress"}}}`,
		`{"method":"turn/plan/updated","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","plan":[{"step":"first","status":"inProgress"}]}}`,
		`{"method":"turn/plan/updated","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","plan":[{"step":"final","status":"completed"}]}}`,
		`{"method":"turn/diff/updated","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","diff":"diff --git a/main.go b/main.go"}}`,
		`{"method":"item/started","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","item":{"type":"commandExecution","id":"command-output","command":"go test ./...","status":"inProgress"}}}`,
		`{"method":"item/commandExecution/outputDelta","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","itemId":"command-output","delta":"partial command output"}}`,
		`{"method":"item/completed","params":{"threadId":"` + fixture.thread + `","turnId":"turn-output","item":{"type":"commandExecution","id":"command-output","command":"go test ./...","status":"completed","aggregatedOutput":"ok","exitCode":0}}}`,
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
	if len(bodies) != 4 || bodies[0] != "test-agent ready in /work/repo" || bodies[1] != "Authoritative answer" || bodies[2] != "Codex turn failed" || bodies[3] != "Codex bridge stopped" {
		t.Fatalf("message order = %#v", bodies)
	}
	activities, err := fixture.store.ListHarnessActivities(context.Background(), domain.HarnessActivityFilter{MailboxID: fixture.agent.ID})
	if err != nil || len(activities) != 5 {
		t.Fatalf("Codex activities = %#v, %v", activities, err)
	}
	byKind := make(map[domain.HarnessActivityKind]domain.HarnessActivity, len(activities))
	for index, activity := range activities {
		if index > 0 && !activity.OccurredAt.After(activities[index-1].OccurredAt) {
			t.Fatalf("Codex activity order = %#v", activities)
		}
		byKind[activity.Kind] = activity
	}
	if byKind[domain.HarnessActivityPlan].Body != "- [x] final" || byKind[domain.HarnessActivityProgress].Body != "partial command output" || byKind[domain.HarnessActivityCommand].Title != "go test ./..." || byKind[domain.HarnessActivityOperation].Status != domain.HarnessActivityFailed {
		t.Fatalf("projected Codex activities = %#v", byKind)
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
			Directory: "/work/repo", AgentName: "test-agent", Starter: fakeStarter{process}, Store: fixture.store,
			Repository: model.RepositoryContext{Directory: "/work/repo"}, Stderr: io.Discard,
			Ledger: NewMemoryLedger(), Replies: fixture.replies, RepairInterval: 2 * time.Millisecond,
		})
	}()
	waitForStoreBody(t, fixture.store, model.HumanMailboxID, "test-agent ready in /work/repo")
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
