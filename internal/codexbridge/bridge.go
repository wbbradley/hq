package codexbridge

import (
	"context"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/model"
)

const gracefulProcessStop = 2 * time.Second

type MailboxStore interface {
	DeliveryStore
	QuestionStore
	ResolveMailbox(context.Context, model.SessionIdentity, model.RepositoryContext) (model.Mailbox, error)
}

type Options struct {
	Directory      string
	ResumeThreadID string
	InitialPrompt  string
	Repository     model.RepositoryContext
	Store          MailboxStore
	Starter        ProcessStarter
	Stderr         io.Writer
	Sync           func(context.Context) error
	Ledger         DeliveryLedger
	LedgerPath     string
	Replies        *ReplyRegistry
	PollInterval   time.Duration
}

func Run(ctx context.Context, options Options) error {
	if strings.TrimSpace(options.Directory) == "" {
		return errors.New("Codex bridge working directory is required")
	}
	if options.Store == nil {
		return errors.New("Codex bridge mailbox store is required")
	}
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
		starter = ExecStarter{}
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
	if options.ResumeThreadID == "" {
		params := ThreadStartParams{CWD: options.Directory, DeveloperInstructions: RequireStructuredHumanInput}
		if err := client.Call(ctx, "thread/start", params, &threadResponse); err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return fmt.Errorf("start Codex thread: %w", err)
		}
	} else {
		params := ThreadResumeParams{ThreadID: options.ResumeThreadID, CWD: options.Directory}
		if err := client.Call(ctx, "thread/resume", params, &threadResponse); err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return fmt.Errorf("resume Codex thread %s: %w", options.ResumeThreadID, err)
		}
	}
	threadID := strings.TrimSpace(threadResponse.Thread.ID)
	if threadID == "" {
		return errors.New("Codex app-server returned an empty thread ID")
	}
	if options.ResumeThreadID != "" && threadID != options.ResumeThreadID {
		return fmt.Errorf("Codex app-server resumed thread %s instead of requested thread %s", threadID, options.ResumeThreadID)
	}
	threadState.BindThread(threadID)
	threadState.UpdateThread(threadResponse.Thread)
	mailbox, err = options.Store.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: threadID}, options.Repository)
	if err != nil {
		return fmt.Errorf("bind Codex thread to HQ mailbox: %w", err)
	}
	requestRouter.Bind(threadID, mailbox, options.Repository, options.Sync, options.PollInterval)
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
		ThreadID: threadID, MailboxID: mailbox.ID, PollInterval: options.PollInterval, Sync: options.Sync,
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
	details := fmt.Sprintf("Codex thread: %s\nHQ mailbox: %s\nStatus: %s", threadID, mailbox.ID, status)
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
