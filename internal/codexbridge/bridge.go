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
	ResolveMailbox(context.Context, model.SessionIdentity, model.RepositoryContext) (model.Mailbox, error)
	HumanMailbox(context.Context) (model.Mailbox, error)
	Create(context.Context, model.Message) error
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
}

func Run(ctx context.Context, options Options) error {
	if strings.TrimSpace(options.Directory) == "" {
		return errors.New("Codex bridge working directory is required")
	}
	if options.Store == nil {
		return errors.New("Codex bridge mailbox store is required")
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
	defer cancelTransport()
	client := NewClient(transportContext, process.Output(), process.Input(), nil, nil)
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
	mailbox, err = options.Store.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: threadID}, options.Repository)
	if err != nil {
		return fmt.Errorf("bind Codex thread to HQ mailbox: %w", err)
	}
	if err := sendStatus(ctx, options, mailbox, threadID, "Codex bridge ready", "The Codex app-server thread is connected and waiting for HQ input."); err != nil {
		return err
	}

	if prompt := strings.TrimSpace(options.InitialPrompt); prompt != "" {
		messageID, err := uuid.NewV7()
		if err != nil {
			return fmt.Errorf("create initial Codex message ID: %w", err)
		}
		params := TurnStartParams{
			ThreadID: threadID, Input: []TextInput{{Type: "text", Text: prompt}}, ClientUserMessageID: messageID.String(),
		}
		var turnResponse TurnResponse
		if err := client.Call(ctx, "turn/start", params, &turnResponse); err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return fmt.Errorf("start initial Codex turn: %w", err)
		}
	}

	select {
	case <-ctx.Done():
		_ = sendStatus(context.Background(), options, mailbox, threadID, "Codex bridge stopped", "Bridge cancelled; the app-server process is being terminated.")
		return nil
	case <-client.Done():
		transportErr := client.Err()
		select {
		case <-processDone:
			transportErr = processExitError(processErr)
		case <-time.After(100 * time.Millisecond):
		}
		_ = sendStatus(context.Background(), options, mailbox, threadID, "Codex bridge stopped", transportErr.Error())
		return transportErr
	case <-processDone:
		exitErr := processExitError(processErr)
		_ = sendStatus(context.Background(), options, mailbox, threadID, "Codex bridge stopped", exitErr.Error())
		return exitErr
	}
}

func processExitError(err error) error {
	if err == nil {
		return errors.New("Codex app-server exited")
	}
	return fmt.Errorf("Codex app-server failed: %w", err)
}

func sendStatus(ctx context.Context, options Options, mailbox model.Mailbox, threadID, body, status string) error {
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
		RecipientMailboxID: human.ID, Body: body, Details: details, CreatedAt: time.Now().UTC(),
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
