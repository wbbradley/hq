package codexbridge

import (
	"context"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

const gracefulProcessStop = 2 * time.Second

const (
	defaultAgentLeaseDuration = 30 * time.Second
	defaultAgentRenewInterval = 10 * time.Second
)

type MailboxStore interface {
	DeliveryStore
	QuestionStore
	ResolveMailbox(context.Context, model.SessionIdentity, model.RepositoryContext) (model.Mailbox, error)
}

type NamedAgentStore interface {
	CreateNamedAgent(context.Context, string, string) (domain.NamedAgent, error)
	SelectNamedAgentSession(context.Context, string, model.SessionIdentity, model.RepositoryContext) (domain.NamedAgent, error)
	AcquireNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error)
	RenewNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error)
	ReleaseNamedAgent(context.Context, string, string) error
}

type Options struct {
	Directory          string
	ResumeThreadID     string
	AgentName          string
	NewThread          bool
	InitialPrompt      string
	Yolo               bool
	Repository         model.RepositoryContext
	Store              MailboxStore
	Starter            ProcessStarter
	Stderr             io.Writer
	Sync               func(context.Context) error
	Ledger             DeliveryLedger
	LedgerPath         string
	Replies            *ReplyRegistry
	RepairInterval     time.Duration
	Updates            domain.ClientUpdates
	AgentLeaseDuration time.Duration
	AgentRenewInterval time.Duration
}

func Run(ctx context.Context, options Options) error {
	if strings.TrimSpace(options.Directory) == "" {
		return errors.New("Codex bridge working directory is required")
	}
	if options.Store == nil {
		return errors.New("Codex bridge mailbox store is required")
	}
	if options.AgentName != "" && options.ResumeThreadID != "" {
		return errors.New("named Codex agents cannot use an explicit resume thread")
	}
	if options.NewThread && options.AgentName == "" {
		return errors.New("starting a replacement thread requires a named Codex agent")
	}
	resumeThreadID := options.ResumeThreadID
	var namedAgent domain.NamedAgent
	var namedStore NamedAgentStore
	var stopOwnership func()
	var ownershipErrors <-chan error
	if options.AgentName != "" {
		var ok bool
		namedStore, ok = options.Store.(NamedAgentStore)
		if !ok {
			return errors.New("Codex bridge store does not support named agents")
		}
		var err error
		namedAgent, err = namedStore.CreateNamedAgent(ctx, options.AgentName, "")
		if err != nil {
			return fmt.Errorf("resolve named agent %s: %w", options.AgentName, err)
		}
		if !options.NewThread && namedAgent.CurrentSessionID != "" {
			if namedAgent.Harness != "codex" {
				return fmt.Errorf("named agent %s currently selects %s session %s; use --new-thread to attach Codex", options.AgentName, namedAgent.Harness, namedAgent.CurrentSessionID)
			}
			resumeThreadID = namedAgent.CurrentSessionID
		}
		var leaseErr error
		stopOwnership, ownershipErrors, leaseErr = holdNamedAgent(ctx, namedStore, options, options.AgentName)
		if leaseErr != nil {
			return fmt.Errorf("acquire named agent %s: %w", options.AgentName, leaseErr)
		}
		defer stopOwnership()
	}
	var subscription domain.ChangeSubscription
	if options.Updates.Subscribe != nil {
		var subscribeErr error
		subscription, subscribeErr = options.Updates.Subscribe(ctx, domain.TopicMessages, domain.TopicMailboxes)
		if subscribeErr != nil {
			return fmt.Errorf("subscribe to Codex HQ mailbox: %w", subscribeErr)
		}
		defer subscription.Close()
	}
	reporterContext, stopReporter := context.WithCancel(ctx)
	reporterDone := make(chan struct{})
	go func() {
		reportConnectionUpdates(reporterContext, options.Stderr, options.Updates)
		close(reporterDone)
	}()
	defer func() {
		stopReporter()
		<-reporterDone
	}()
	ledger := options.Ledger
	if ledger == nil {
		openedLedger, openErr := OpenFileLedger(options.LedgerPath)
		if openErr != nil {
			return openErr
		}
		ledger = openedLedger
	}
	replies := options.Replies
	if replies == nil {
		replies = NewReplyRegistry()
	}
	starter := options.Starter
	if starter == nil {
		starter = ExecStarter{Yolo: options.Yolo}
	}
	process, err := starter.Start(options.Directory)
	if err != nil {
		return err
	}
	processDone := make(chan struct{})
	var processErr error
	go func() {
		processErr = process.Wait()
		close(processDone)
	}()
	go func() { _ = forwardStderr(options.Stderr, process.Errors()) }()

	transportContext, cancelTransport := context.WithCancel(context.Background())
	threadState := NewThreadState("")
	requestRouter := NewRequestRouter(options.Store, replies)
	outputRelay := NewOutputRelay(options.Store, ledger, options.Sync)
	notifications := NewNotificationHub(threadState, outputRelay)
	client := NewClient(transportContext, process.Output(), process.Input(), requestRouter, notifications)
	var mailbox model.Mailbox
	shutdown := func() {
		_ = process.Input().Close()
		select {
		case <-processDone:
		case <-time.After(gracefulProcessStop):
			_ = process.Kill()
			<-processDone
		}
	}
	defer shutdown()
	stopRequests := func() {
		cancelTransport()
		client.StopRequestsAndWait()
	}
	defer stopRequests()
	defer outputRelay.StopAndWait()

	initialize := InitializeParams{
		ClientInfo:   ClientInfo{Name: "hq", Title: "HQ Codex bridge", Version: TestedCodexVersion},
		Capabilities: InitializeCapabilities{ExperimentalAPI: true},
	}
	if err := client.Call(ctx, "initialize", initialize, nil); err != nil {
		if ctx.Err() != nil {
			return nil
		}
		return fmt.Errorf("initialize Codex app-server: %w", err)
	}
	if err := client.Notify("initialized", struct{}{}); err != nil {
		return fmt.Errorf("acknowledge Codex app-server initialization: %w", err)
	}

	var threadResponse ThreadResponse
	if resumeThreadID == "" {
		params := ThreadStartParams{CWD: options.Directory, DeveloperInstructions: RequireStructuredHumanInput}
		if err := client.Call(ctx, "thread/start", params, &threadResponse); err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return fmt.Errorf("start Codex thread: %w", err)
		}
	} else {
		params := ThreadResumeParams{ThreadID: resumeThreadID, CWD: options.Directory}
		if err := client.Call(ctx, "thread/resume", params, &threadResponse); err != nil {
			if ctx.Err() != nil {
				return nil
			}
			if options.AgentName != "" {
				return fmt.Errorf("resume Codex thread %s for named agent %s: %w; use --new-thread to rotate explicitly", resumeThreadID, options.AgentName, err)
			}
			return fmt.Errorf("resume Codex thread %s: %w", resumeThreadID, err)
		}
	}
	threadID := strings.TrimSpace(threadResponse.Thread.ID)
	if threadID == "" {
		return errors.New("Codex app-server returned an empty thread ID")
	}
	if resumeThreadID != "" && threadID != resumeThreadID {
		return fmt.Errorf("Codex app-server resumed thread %s instead of requested thread %s", threadID, resumeThreadID)
	}
	threadState.BindThread(threadID)
	threadState.UpdateThread(threadResponse.Thread)
	if options.AgentName != "" {
		selected, selectErr := namedStore.SelectNamedAgentSession(ctx, options.AgentName, model.SessionIdentity{Harness: "codex", ExternalSessionID: threadID}, options.Repository)
		if selectErr != nil {
			return fmt.Errorf("select Codex thread for named agent %s: %w", options.AgentName, selectErr)
		}
		namedAgent = selected
		mailbox = model.Mailbox{ID: namedAgent.MailboxID, Kind: model.MailboxAgent, Harness: "codex", Label: namedAgent.Name, Context: options.Repository}
	} else {
		mailbox, err = options.Store.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: threadID}, options.Repository)
		if err != nil {
			return fmt.Errorf("bind Codex thread to HQ mailbox: %w", err)
		}
	}
	requestRouter.Bind(threadID, mailbox, options.Repository, options.Sync, options.Updates.Subscribe, options.RepairInterval)
	outputRelay.Bind(threadID, mailbox, options.Repository)
	if err := sendStatus(ctx, options, mailbox, threadID, "Codex bridge ready", "The Codex app-server thread is connected and waiting for HQ input."); err != nil {
		return err
	}
	finishBeforeDispatcher := func(bridgeErr error, status string) error {
		stopRequests()
		outputRelay.StopAndWait()
		_ = sendStatusAt(context.Background(), options, mailbox, threadID, "Codex bridge stopped", status, outputRelay.nextCreatedAt())
		return bridgeErr
	}

	if prompt := strings.TrimSpace(options.InitialPrompt); prompt != "" {
		messageID, err := uuid.NewV7()
		if err != nil {
			wrapped := fmt.Errorf("create initial Codex message ID: %w", err)
			return finishBeforeDispatcher(wrapped, wrapped.Error())
		}
		params := TurnStartParams{
			ThreadID: threadID, Input: []TextInput{{Type: "text", Text: prompt}}, ClientUserMessageID: messageID.String(),
		}
		var turnResponse TurnResponse
		if err := client.Call(ctx, "turn/start", params, &turnResponse); err != nil {
			if ctx.Err() != nil {
				return finishBeforeDispatcher(nil, "Bridge cancelled; the app-server process is being terminated.")
			}
			wrapped := fmt.Errorf("start initial Codex turn: %w", err)
			return finishBeforeDispatcher(wrapped, wrapped.Error())
		}
		threadState.SetActive(turnResponse.Turn.ID)
	}

	dispatcherContext, cancelDispatcher := context.WithCancel(ctx)
	dispatcherDone := make(chan struct{})
	var dispatcherErr error
	dispatcher := &Dispatcher{
		Client: client, Store: options.Store, Ledger: ledger, Replies: replies, State: threadState,
		ThreadID: threadID, MailboxID: mailbox.ID, RepairInterval: options.RepairInterval, Sync: options.Sync,
	}
	if subscription != nil {
		dispatcher.Invalidations = subscription.Changes()
	}
	go func() {
		dispatcherErr = dispatcher.Run(dispatcherContext)
		close(dispatcherDone)
	}()
	stopDispatcher := func() {
		cancelDispatcher()
		<-dispatcherDone
	}
	defer stopDispatcher()
	stopRuntime := func() {
		stopDispatcher()
		stopRequests()
		outputRelay.StopAndWait()
	}
	finish := func(bridgeErr error, status string) error {
		stopRuntime()
		_ = sendStatusAt(context.Background(), options, mailbox, threadID, "Codex bridge stopped", status, outputRelay.nextCreatedAt())
		return bridgeErr
	}

	select {
	case ownershipErr := <-ownershipErrors:
		return finish(fmt.Errorf("named agent ownership lost: %w", ownershipErr), ownershipErr.Error())
	case <-ctx.Done():
		return finish(nil, "Bridge cancelled; the app-server process is being terminated.")
	case <-client.Done():
		transportErr := client.Err()
		select {
		case <-processDone:
			transportErr = processExitError(processErr)
		case <-time.After(100 * time.Millisecond):
		}
		return finish(transportErr, transportErr.Error())
	case <-processDone:
		select {
		case <-client.Done():
		case <-time.After(time.Second):
		}
		exitErr := processExitError(processErr)
		return finish(exitErr, exitErr.Error())
	case <-dispatcherDone:
		if dispatcherErr == nil {
			dispatcherErr = errors.New("Codex HQ input dispatcher stopped unexpectedly")
		}
		return finish(dispatcherErr, dispatcherErr.Error())
	case <-outputRelay.Done():
		outputErr := outputRelay.Err()
		if outputErr == nil {
			outputErr = errors.New("Codex output relay stopped unexpectedly")
		}
		return finish(outputErr, outputErr.Error())
	}
}

func holdNamedAgent(ctx context.Context, store NamedAgentStore, options Options, name string) (func(), <-chan error, error) {
	duration := options.AgentLeaseDuration
	if duration <= 0 {
		duration = defaultAgentLeaseDuration
	}
	interval := options.AgentRenewInterval
	if interval <= 0 {
		interval = defaultAgentRenewInterval
	}
	token := uuid.NewString()
	if _, err := store.AcquireNamedAgent(ctx, name, token, duration); err != nil {
		return nil, nil, err
	}
	leaseContext, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	errorsChannel := make(chan error, 1)
	go func() {
		defer close(done)
		ticker := time.NewTicker(interval)
		defer ticker.Stop()
		for {
			select {
			case <-leaseContext.Done():
				return
			case <-ticker.C:
				if _, err := store.RenewNamedAgent(leaseContext, name, token, duration); err != nil {
					select {
					case errorsChannel <- err:
					default:
					}
					return
				}
			}
		}
	}()
	stop := func() {
		cancel()
		<-done
		releaseContext, releaseCancel := context.WithTimeout(context.Background(), gracefulProcessStop)
		defer releaseCancel()
		_ = store.ReleaseNamedAgent(releaseContext, name, token)
	}
	return stop, errorsChannel, nil
}

func reportConnectionUpdates(ctx context.Context, destination io.Writer, updates domain.ClientUpdates) {
	last := ""
	write := func(update domain.ConnectionUpdate) {
		if destination == nil || update.Diagnostic == "" || update.Diagnostic == last {
			return
		}
		last = update.Diagnostic
		_, _ = fmt.Fprintf(destination, "hq codex: %s\n", update.Diagnostic)
	}
	write(updates.Initial)
	if updates.States == nil {
		return
	}
	for {
		select {
		case <-ctx.Done():
			return
		case update := <-updates.States:
			if update.Diagnostic == "" {
				last = ""
				continue
			}
			write(update)
		}
	}
}

func processExitError(err error) error {
	if err == nil {
		return errors.New("Codex app-server exited")
	}
	return fmt.Errorf("Codex app-server failed: %w", err)
}

func sendStatus(ctx context.Context, options Options, mailbox model.Mailbox, threadID, body, status string) error {
	return sendStatusAt(ctx, options, mailbox, threadID, body, status, time.Now().UTC())
}

func sendStatusAt(ctx context.Context, options Options, mailbox model.Mailbox, threadID, body, status string, createdAt time.Time) error {
	human, err := options.Store.HumanMailbox(ctx)
	if err != nil {
		return fmt.Errorf("resolve human mailbox: %w", err)
	}
	messageID, err := uuid.NewV7()
	if err != nil {
		return fmt.Errorf("create bridge status ID: %w", err)
	}
	details := fmt.Sprintf("Kind: status\nCodex thread: %s\nHQ mailbox: %s\nStatus: %s", threadID, mailbox.ID, status)
	message := model.Message{
		ID: messageID.String(), Context: options.Repository, SenderMailboxID: mailbox.ID,
		RecipientMailboxID: human.ID, Body: body, Details: details, CreatedAt: createdAt,
	}
	if err := options.Store.Create(ctx, message); err != nil {
		return fmt.Errorf("send Codex bridge status to HQ: %w", err)
	}
	if options.Sync != nil {
		if err := options.Sync(ctx); err != nil {
			return fmt.Errorf("sync Codex bridge status: %w", err)
		}
	}
	return nil
}
