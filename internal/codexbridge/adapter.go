package codexbridge

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"strings"
	"sync"
	"time"

	"github.com/wbbradley/hq/internal/harness"
)

const CodexProviderID harness.ProviderID = "codex"

const adapterStreamCapacity = 128

type CodexOptions struct {
	Yolo                  bool   `json:"yolo,omitempty"`
	DeveloperInstructions string `json:"developer_instructions,omitempty"`
}

func (CodexOptions) Provider() harness.ProviderID { return CodexProviderID }

// HarnessFactory is the Codex implementation of the harness runtime factory.
// Process and protocol details remain private to this package.
type HarnessFactory struct {
	Executable string
	Starter    ProcessStarter
	Stderr     io.Writer
	Logger     *slog.Logger

	legacyRequests      RequestHandler
	legacyNotifications NotificationHandler
}

func NewHarnessFactory() *HarnessFactory { return &HarnessFactory{} }

func newHarnessFactoryWithHandlers(starter ProcessStarter, stderr io.Writer, logger *slog.Logger, requests RequestHandler, notifications NotificationHandler) *HarnessFactory {
	return &HarnessFactory{Starter: starter, Stderr: stderr, Logger: logger, legacyRequests: requests, legacyNotifications: notifications}
}

func (f *HarnessFactory) Provider() harness.Provider {
	return harness.Provider{
		ID: CodexProviderID, DisplayName: "Codex",
		Capabilities: harness.Capabilities{
			Resume: true, SteerActiveOperation: true, Interrupt: true, Approvals: true,
			SubmissionLookup: true, Plans: true, Diffs: true, ToolLifecycle: true, Streaming: true,
		},
	}
}

func (f *HarnessFactory) Launch(ctx context.Context, config harness.LaunchConfig) (harness.Instance, error) {
	provider := f.Provider()
	if err := config.Validate(provider); err != nil {
		return nil, err
	}
	options := CodexOptions{}
	if config.Options != nil {
		switch value := config.Options.(type) {
		case CodexOptions:
			options = value
		case *CodexOptions:
			if value == nil {
				return nil, fmt.Errorf("Codex launch options are nil")
			}
			options = *value
		default:
			return nil, fmt.Errorf("Codex launch options have unsupported type %T", config.Options)
		}
	}
	logger := f.Logger
	if logger == nil {
		logger = slog.New(slog.DiscardHandler)
	}
	starter := f.Starter
	if starter == nil {
		environment := append([]string(nil), config.Environment...)
		starter = &ExecStarter{Path: f.Executable, Environment: environment, UseEnvironment: true, Logger: logger}
	}
	process, err := starter.Start(config.Directory)
	if err != nil {
		return nil, &harness.ProviderError{Provider: CodexProviderID, Operation: "start app-server", Cause: errors.Join(harness.ErrProviderUnavailable, err)}
	}
	instanceContext, cancel := context.WithCancel(context.Background())
	instance := &codexInstance{
		id: config.InstanceID, provider: CodexProviderID, process: process, logger: logger,
		ctx: instanceContext, cancel: cancel, processDone: make(chan struct{}), events: make(chan harness.Event, adapterStreamCapacity),
		requests: make(chan harness.Request, adapterStreamCapacity), done: make(chan struct{}), shutdownDone: make(chan struct{}),
		state: harness.RuntimeState{Phase: harness.RuntimeStarting, Since: time.Now().UTC()}, threadState: NewThreadState(""),
		pendingRequests: make(map[harness.RequestID]*adapterPendingRequest),
	}
	go func() {
		instance.processErr = process.Wait()
		close(instance.processDone)
	}()
	go func() { _ = forwardStderr(f.Stderr, process.Errors()) }()
	requestHandler := &adapterRequestHandler{instance: instance, legacy: f.legacyRequests}
	notificationHandler := &adapterNotificationHandler{instance: instance, legacy: f.legacyNotifications}
	instance.client = NewClient(instanceContext, process.Output(), process.Input(), requestHandler, notificationHandler)
	instance.session = &codexSession{instance: instance}

	cleanupLaunch := func() {
		instance.beginShutdown(nil)
		<-instance.shutdownDone
	}
	initialize := InitializeParams{
		ClientInfo:   ClientInfo{Name: "hq", Title: "HQ Codex bridge", Version: TestedCodexVersion},
		Capabilities: InitializeCapabilities{ExperimentalAPI: true},
	}
	if err := instance.client.Call(ctx, "initialize", initialize, nil); err != nil {
		cleanupLaunch()
		return nil, &harness.ProviderError{Provider: CodexProviderID, Operation: "initialize app-server", Cause: err}
	}
	if err := instance.client.Notify("initialized", struct{}{}); err != nil {
		cleanupLaunch()
		return nil, &harness.ProviderError{Provider: CodexProviderID, Operation: "acknowledge app-server initialization", Cause: err}
	}

	var response ThreadResponse
	switch config.SessionMode {
	case harness.SessionNew:
		params := ThreadStartParams{CWD: config.Directory, DeveloperInstructions: options.DeveloperInstructions}
		applyYoloThreadSettings(options.Yolo, &params.ApprovalPolicy, &params.Sandbox)
		if err := instance.client.Call(ctx, "thread/start", params, &response); err != nil {
			cleanupLaunch()
			return nil, &harness.RuntimeError{Provider: CodexProviderID, Action: "start session", Cause: err}
		}
	case harness.SessionResume:
		params := ThreadResumeParams{ThreadID: string(config.RequestedSession), CWD: config.Directory}
		applyYoloThreadSettings(options.Yolo, &params.ApprovalPolicy, &params.Sandbox)
		if err := instance.client.Call(ctx, "thread/resume", params, &response); err != nil {
			cleanupLaunch()
			return nil, &harness.RuntimeError{Provider: CodexProviderID, Session: config.RequestedSession, Action: "resume session", Cause: err}
		}
	}
	threadID := strings.TrimSpace(response.Thread.ID)
	if threadID == "" {
		cleanupLaunch()
		return nil, &harness.RuntimeError{Provider: CodexProviderID, Action: "validate session", Cause: errors.New("app-server returned an empty session ID")}
	}
	if config.SessionMode == harness.SessionResume && harness.SessionID(threadID) != config.RequestedSession {
		cleanupLaunch()
		return nil, &harness.RuntimeError{
			Provider: CodexProviderID, Session: config.RequestedSession, Action: "validate resumed session",
			Cause: fmt.Errorf("app-server resumed session %s instead of requested session %s", threadID, config.RequestedSession),
		}
	}
	instance.session.identity = harness.SessionIdentity{Provider: CodexProviderID, ID: harness.SessionID(threadID)}
	instance.initialThread = response.Thread
	instance.threadState.BindThread(threadID)
	instance.threadState.UpdateThread(response.Thread)
	instance.mu.Lock()
	instance.state = harness.RuntimeState{Phase: harness.RuntimeRunning, Since: time.Now().UTC()}
	instance.mu.Unlock()
	go instance.monitor()
	return instance, nil
}

type codexInstance struct {
	id       harness.InstanceID
	provider harness.ProviderID
	process  Process
	client   *Client
	logger   *slog.Logger
	ctx      context.Context
	cancel   context.CancelFunc
	session  *codexSession

	threadState   *ThreadState
	initialThread Thread
	processDone   chan struct{}
	processErr    error
	events        chan harness.Event
	requests      chan harness.Request
	done          chan struct{}
	shutdownDone  chan struct{}

	mu          sync.Mutex
	state       harness.RuntimeState
	terminalErr error
	closed      bool

	streamMu        sync.Mutex
	sequence        uint64
	streamsClosed   bool
	pendingMu       sync.Mutex
	pendingRequests map[harness.RequestID]*adapterPendingRequest

	shutdownOnce sync.Once
	finishOnce   sync.Once
	requestsOnce sync.Once
}

func (i *codexInstance) ID() harness.InstanceID           { return i.id }
func (i *codexInstance) Provider() harness.ProviderID     { return i.provider }
func (i *codexInstance) Session() harness.Session         { return i.session }
func (i *codexInstance) Events() <-chan harness.Event     { return i.events }
func (i *codexInstance) Requests() <-chan harness.Request { return i.requests }

func (i *codexInstance) State() harness.RuntimeState {
	i.mu.Lock()
	defer i.mu.Unlock()
	return i.state
}

func (i *codexInstance) Shutdown(ctx context.Context) error {
	i.beginShutdown(nil)
	select {
	case <-i.shutdownDone:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (i *codexInstance) Wait(ctx context.Context) error {
	select {
	case <-i.done:
		i.mu.Lock()
		err := i.terminalErr
		i.mu.Unlock()
		return err
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (i *codexInstance) monitor() {
	select {
	case <-i.client.Done():
		transportErr := i.client.Err()
		select {
		case <-i.processDone:
			transportErr = processExitError(i.processErr)
		case <-time.After(100 * time.Millisecond):
		}
		i.beginShutdown(transportErr)
	case <-i.processDone:
		select {
		case <-i.client.Done():
		case <-time.After(time.Second):
		}
		i.beginShutdown(processExitError(i.processErr))
	case <-i.done:
	}
}

func (i *codexInstance) beginShutdown(cause error) {
	i.mu.Lock()
	if i.state.Phase != harness.RuntimeStopping && i.state.Phase != harness.RuntimeStopped && i.state.Phase != harness.RuntimeFailed {
		i.state = harness.RuntimeState{Phase: harness.RuntimeStopping, Since: time.Now().UTC()}
		if cause != nil {
			i.terminalErr = cause
		}
	}
	i.mu.Unlock()
	i.shutdownOnce.Do(func() { go i.shutdownProcess() })
}

func (i *codexInstance) shutdownProcess() {
	i.stopRequests()
	_ = i.process.Input().Close()
	select {
	case <-i.processDone:
	case <-time.After(gracefulProcessStop):
		_ = i.process.Kill()
		<-i.processDone
	}
	i.finish()
	close(i.shutdownDone)
}

func (i *codexInstance) stopRequests() {
	i.requestsOnce.Do(func() {
		i.cancel()
		i.client.StopRequestsAndWait()
	})
}

func (i *codexInstance) finish() {
	i.finishOnce.Do(func() {
		i.mu.Lock()
		i.closed = true
		terminalErr := i.terminalErr
		if terminalErr != nil {
			i.state = harness.RuntimeState{Phase: harness.RuntimeFailed, Since: time.Now().UTC(), Err: terminalErr}
		} else {
			i.state = harness.RuntimeState{Phase: harness.RuntimeStopped, Since: time.Now().UTC()}
		}
		i.mu.Unlock()
		i.pendingMu.Lock()
		for _, pending := range i.pendingRequests {
			close(pending.responses)
		}
		i.pendingMu.Unlock()
		i.streamMu.Lock()
		i.streamsClosed = true
		close(i.requests)
		close(i.events)
		i.streamMu.Unlock()
		close(i.done)
	})
}

func (i *codexInstance) running() error {
	i.mu.Lock()
	defer i.mu.Unlock()
	if i.closed || i.state.Phase != harness.RuntimeRunning {
		return harness.ErrInstanceStopped
	}
	return nil
}

type codexSession struct {
	instance *codexInstance
	identity harness.SessionIdentity
}

func (s *codexSession) Identity() harness.SessionIdentity { return s.identity }

func (s *codexSession) Submit(ctx context.Context, submission harness.Submission) (harness.DeliveryResult, error) {
	if err := ctx.Err(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	provider := s.instanceFactoryProvider()
	if err := submission.Validate(provider); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	if err := s.instance.running(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	input, err := codexInput(submission)
	if err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	params := TurnStartParams{ThreadID: string(s.identity.ID), Input: input, ClientUserMessageID: string(submission.ID)}
	var response TurnResponse
	if err := s.instance.client.Call(ctx, "turn/start", params, &response); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryUncertain}, err
	}
	operationID := strings.TrimSpace(response.Turn.ID)
	if operationID == "" {
		return harness.DeliveryResult{State: harness.DeliveryUncertain}, errors.New("turn/start returned an empty operation ID")
	}
	s.instance.threadState.SetActive(operationID)
	return harness.DeliveryResult{State: harness.DeliveryAccepted, OperationID: harness.OperationID(operationID)}, nil
}

func (s *codexSession) SubmitToActive(ctx context.Context, expected harness.OperationID, submission harness.Submission) (harness.DeliveryResult, error) {
	if err := ctx.Err(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	if err := submission.Validate(s.instanceFactoryProvider()); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	if err := s.instance.running(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	input, err := codexInput(submission)
	if err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	params := TurnSteerParams{
		ThreadID: string(s.identity.ID), ExpectedTurnID: string(expected), Input: input, ClientUserMessageID: string(submission.ID),
	}
	var response TurnSteerResponse
	if err := s.instance.client.Call(ctx, "turn/steer", params, &response); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryUncertain}, err
	}
	if response.TurnID != string(expected) {
		return harness.DeliveryResult{State: harness.DeliveryUncertain, OperationID: harness.OperationID(response.TurnID)}, fmt.Errorf("turn/steer accepted operation %q instead of expected operation %q", response.TurnID, expected)
	}
	return harness.DeliveryResult{State: harness.DeliveryAccepted, OperationID: expected}, nil
}

func (s *codexSession) Reconcile(ctx context.Context, submissionID harness.SubmissionID) (harness.RecoveryResult, error) {
	if err := s.instance.running(); err != nil {
		return harness.RecoveryResult{}, err
	}
	var response ThreadResponse
	if err := s.instance.client.Call(ctx, "thread/read", ThreadReadParams{ThreadID: string(s.identity.ID), IncludeTurns: true}, &response); err != nil {
		return harness.RecoveryResult{}, err
	}
	if response.Thread.ID != string(s.identity.ID) {
		return harness.RecoveryResult{}, &harness.RuntimeError{Provider: CodexProviderID, Session: s.identity.ID, Action: "reconcile submission", Cause: fmt.Errorf("thread/read returned session %q", response.Thread.ID)}
	}
	s.instance.threadState.UpdateThread(response.Thread)
	for _, turn := range response.Thread.Turns {
		for _, item := range turn.Items {
			if item.Type == "userMessage" && item.ClientID == string(submissionID) {
				return harness.RecoveryResult{State: harness.RecoveryAccepted, OperationID: harness.OperationID(turn.ID)}, nil
			}
		}
	}
	return harness.RecoveryResult{State: harness.RecoveryNotFound}, nil
}

func (s *codexSession) Interrupt(ctx context.Context, operationID harness.OperationID) error {
	if err := s.instance.running(); err != nil {
		return err
	}
	params := TurnInterruptParams{ThreadID: string(s.identity.ID), TurnID: string(operationID)}
	if err := s.instance.client.Call(ctx, "turn/interrupt", params, nil); err != nil {
		return &harness.RuntimeError{Provider: CodexProviderID, Session: s.identity.ID, Operation: operationID, Action: "interrupt", Cause: err}
	}
	return nil
}

func (s *codexSession) instanceFactoryProvider() harness.Provider {
	return (&HarnessFactory{}).Provider()
}

func codexInput(submission harness.Submission) ([]TextInput, error) {
	input := make([]TextInput, 0, len(submission.Input))
	for _, part := range submission.Input {
		text, ok := part.(harness.TextInput)
		if !ok {
			return nil, harness.NewCapabilityError(CodexProviderID, harness.CapabilityStructuredInput)
		}
		input = append(input, TextInput{Type: "text", Text: text.Text})
	}
	return input, nil
}

var (
	_ harness.Factory                  = (*HarnessFactory)(nil)
	_ harness.Instance                 = (*codexInstance)(nil)
	_ harness.Session                  = (*codexSession)(nil)
	_ harness.SubmissionReconciler     = (*codexSession)(nil)
	_ harness.ActiveOperationSubmitter = (*codexSession)(nil)
	_ harness.Interrupter              = (*codexSession)(nil)
)
