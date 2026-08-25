package codexbridge

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
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
}

type NamedAgentStore interface {
	CreateNamedAgent(context.Context, string, string) (domain.NamedAgent, error)
	SelectNamedAgentSession(context.Context, string, model.SessionIdentity, model.RepositoryContext) (domain.NamedAgent, error)
	AcquireNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error)
	RenewNamedAgent(context.Context, string, string, time.Duration) (domain.NamedAgent, error)
	ReleaseNamedAgent(context.Context, string, string) error
}

type Store interface {
	MailboxStore
	NamedAgentStore
}

type ProjectStore interface {
	domain.ProjectDeliveryOperations
	domain.ProjectOutputOperations
}

type Options struct {
	Directory          string
	ResumeThreadID     string
	AgentName          string
	ProjectID          string
	ProjectReady       func(BridgeReady) (ProjectBinding, error)
	NewThread          bool
	InitialPrompt      string
	Yolo               bool
	Repository         model.RepositoryContext
	Store              Store
	ProjectStore       ProjectStore
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
	OnReady            func(BridgeReady)
	SuppressStatus     bool
	Logger             *slog.Logger
}

type ProjectBinding struct {
	ProjectID       string
	AssignmentID    string
	ProjectThreadID string
	MailboxID       string
	ProjectName     string
}

type BridgeReady struct {
	AgentName string
	ThreadID  string
	Directory string
}

func runLegacy(ctx context.Context, options Options) error {
	logger := options.Logger
	if logger == nil {
		logger = slog.New(slog.DiscardHandler)
	}
	logger = logger.With("agent", options.AgentName, "directory", options.Directory)
	logger.Info("Codex bridge starting", "resume_thread_id", options.ResumeThreadID, "new_thread", options.NewThread, "yolo", options.Yolo, "has_initial_prompt", strings.TrimSpace(options.InitialPrompt) != "")
	if strings.TrimSpace(options.Directory) == "" {
		return errors.New("Codex bridge working directory is required")
	}
	if strings.TrimSpace(options.AgentName) == "" {
		return errors.New("Codex bridge requires a durable agent name")
	}
	if options.Store == nil {
		return errors.New("Codex bridge mailbox store is required")
	}
	if options.NewThread && options.ResumeThreadID != "" {
		return errors.New("starting a new thread cannot also resume a thread")
	}
	if options.ProjectID != "" && options.ProjectReady == nil {
		return errors.New("project Codex bridge requires a ready binding callback")
	}
	resumeThreadID := options.ResumeThreadID
	if options.ProjectID != "" && options.ProjectStore == nil {
		return errors.New("project Codex bridge store is required")
	}
	namedStore := options.Store
	namedAgent, err := namedStore.CreateNamedAgent(ctx, options.AgentName, "")
	if err != nil {
		logger.Error("resolve named agent", "error", err)
		return fmt.Errorf("resolve named agent %s: %w", options.AgentName, err)
	}
	if options.ProjectID == "" && resumeThreadID == "" && !options.NewThread && namedAgent.CurrentSessionID != "" {
		if namedAgent.Harness != "codex" {
			return fmt.Errorf("named agent %s currently selects %s session %s; use --new-thread to attach Codex", options.AgentName, namedAgent.Harness, namedAgent.CurrentSessionID)
		}
		resumeThreadID = namedAgent.CurrentSessionID
	}
	stopOwnership, ownershipErrors, leaseErr := holdNamedAgent(ctx, namedStore, options, options.AgentName)
	if leaseErr != nil {
		logger.Error("acquire named agent ownership", "error", leaseErr)
		return fmt.Errorf("acquire named agent %s: %w", options.AgentName, leaseErr)
	}
	logger.Info("named agent ownership acquired")
	defer stopOwnership()
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
		starter = &ExecStarter{}
	}
	requestRouter := NewRequestRouter(options.Store, replies)
	outputRelay := NewOutputRelay(options.Store, options.ProjectStore, ledger, options.Sync)
	defer outputRelay.StopAndWait()
	factory := newHarnessFactoryWithHandlers(starter, options.Stderr, logger.With("component", "codex_adapter"), requestRouter, outputRelay)
	mode := harness.SessionNew
	requestedSession := harness.SessionID("")
	if resumeThreadID == "" {
		logger.Info("starting Codex thread")
	} else {
		logger.Info("resuming Codex thread", "thread_id", resumeThreadID)
		mode = harness.SessionResume
		requestedSession = harness.SessionID(resumeThreadID)
	}
	launched, launchErr := factory.Launch(ctx, harness.LaunchConfig{
		InstanceID: harness.InstanceID(options.AgentName), AgentName: options.AgentName, Directory: options.Directory,
		SessionMode: mode, RequestedSession: requestedSession,
		Options: CodexOptions{Yolo: options.Yolo, DeveloperInstructions: NamedAgentDeveloperInstructions(options.AgentName)},
	})
	if launchErr != nil {
		outputRelay.StopAndWait()
		if ctx.Err() != nil {
			return nil
		}
		return bridgeAdapterLaunchError(launchErr, resumeThreadID, options.AgentName)
	}
	instance := launched.(*codexInstance)
	client := instance.client
	threadState := instance.threadState
	threadID := string(instance.session.identity.ID)
	eventDrainDone := make(chan struct{})
	go func() {
		for range instance.Events() {
		}
		close(eventDrainDone)
	}()
	defer func() {
		shutdownContext, cancel := context.WithTimeout(context.Background(), gracefulProcessStop+time.Second)
		defer cancel()
		_ = instance.Shutdown(shutdownContext)
		_ = instance.Wait(shutdownContext)
		<-eventDrainDone
	}()
	stopRequests := instance.stopRequests
	defer stopRequests()
	var mailbox model.Mailbox
	logger = logger.With("thread_id", threadID)
	logger.Info("Codex thread connected", "resumed", resumeThreadID != "")
	var projectBinding ProjectBinding
	if options.ProjectID != "" {
		projectBinding, err = options.ProjectReady(BridgeReady{AgentName: options.AgentName, ThreadID: threadID, Directory: options.Directory})
		if err != nil {
			return fmt.Errorf("activate project Codex thread: %w", err)
		}
		if projectBinding.ProjectID != options.ProjectID || projectBinding.AssignmentID == "" || projectBinding.ProjectThreadID == "" || projectBinding.MailboxID == "" {
			return errors.New("project ready callback returned an invalid binding")
		}
		mailbox = model.Mailbox{ID: projectBinding.MailboxID, Kind: model.MailboxProject, Harness: "codex", Label: options.AgentName + " · " + projectBinding.ProjectName, Context: options.Repository}
	} else {
		selected, selectErr := namedStore.SelectNamedAgentSession(ctx, options.AgentName, model.SessionIdentity{Harness: "codex", ExternalSessionID: threadID}, options.Repository)
		if selectErr != nil {
			logger.Error("select Codex thread for named agent", "error", selectErr)
			return fmt.Errorf("select Codex thread for named agent %s: %w", options.AgentName, selectErr)
		}
		namedAgent = selected
		mailbox = model.Mailbox{ID: namedAgent.MailboxID, Kind: model.MailboxAgent, Harness: "codex", Label: namedAgent.Name, Context: options.Repository}
	}
	requestRouter.Bind(threadID, mailbox, options.Repository, options.Sync, options.Updates.Subscribe, options.RepairInterval)
	outputRelay.Bind(threadID, mailbox, options.Repository)
	if projectBinding.ProjectID != "" {
		outputRelay.BindProject(domain.ProjectOutputBinding{
			ProjectID: projectBinding.ProjectID, AssignmentID: projectBinding.AssignmentID,
			AgentName: options.AgentName, ProjectThreadID: projectBinding.ProjectThreadID,
			ExternalThreadID: threadID, RuntimeState: "connected",
		})
	}
	if !options.SuppressStatus {
		if err := sendStatus(ctx, options, mailbox, threadID, bridgeReadyBody(options), "The Codex app-server thread is connected and waiting for HQ input."); err != nil {
			return err
		}
	}
	finishBeforeDispatcher := func(bridgeErr error, status string) error {
		stopRequests()
		outputRelay.StopAndWait()
		if !options.SuppressStatus {
			_ = sendStatusAt(context.Background(), options, mailbox, threadID, "Codex bridge stopped", status, outputRelay.nextCreatedAt())
		}
		return bridgeErr
	}

	if prompt := strings.TrimSpace(options.InitialPrompt); prompt != "" {
		messageID, err := uuid.NewV7()
		if err != nil {
			wrapped := fmt.Errorf("create initial Codex message ID: %w", err)
			return finishBeforeDispatcher(wrapped, wrapped.Error())
		}
		_, err = instance.session.Submit(ctx, harness.Submission{
			ID: harness.SubmissionID(messageID.String()), Input: []harness.InputPart{harness.TextInput{Text: prompt}},
		})
		if err != nil {
			if ctx.Err() != nil {
				return finishBeforeDispatcher(nil, "Bridge cancelled; the app-server process is being terminated.")
			}
			wrapped := fmt.Errorf("start initial Codex turn: %w", err)
			return finishBeforeDispatcher(wrapped, wrapped.Error())
		}
	}
	if options.OnReady != nil {
		options.OnReady(BridgeReady{AgentName: options.AgentName, ThreadID: threadID, Directory: options.Directory})
	}
	logger.Info("Codex bridge ready")

	dispatcherContext, cancelDispatcher := context.WithCancel(ctx)
	dispatcherDone := make(chan struct{})
	var dispatcherErr error
	dispatcher := &Dispatcher{
		Client: client, Store: options.Store, Ledger: ledger, Replies: replies, State: threadState,
		ThreadID: threadID, MailboxID: mailbox.ID, RepairInterval: options.RepairInterval, Sync: options.Sync,
		ProjectStore: options.ProjectStore, ProjectID: projectBinding.ProjectID, AssignmentID: projectBinding.AssignmentID, ProjectThreadID: projectBinding.ProjectThreadID,
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
		if !options.SuppressStatus {
			_ = sendStatusAt(context.Background(), options, mailbox, threadID, "Codex bridge stopped", status, outputRelay.nextCreatedAt())
		}
		return bridgeErr
	}

	select {
	case ownershipErr := <-ownershipErrors:
		logger.Error("Codex bridge stopping", "reason", "ownership lost", "error", ownershipErr)
		return finish(fmt.Errorf("named agent ownership lost: %w", ownershipErr), ownershipErr.Error())
	case <-ctx.Done():
		logger.Info("Codex bridge stopping", "reason", "context canceled")
		return finish(nil, "Bridge cancelled; the app-server process is being terminated.")
	case <-instance.done:
		runtimeErr := instance.Wait(context.Background())
		if runtimeErr == nil {
			runtimeErr = errors.New("Codex harness instance stopped unexpectedly")
		}
		logger.Error("Codex bridge stopping", "reason", "harness instance stopped", "error", runtimeErr)
		return finish(runtimeErr, runtimeErr.Error())
	case <-dispatcherDone:
		if dispatcherErr == nil {
			dispatcherErr = errors.New("Codex HQ input dispatcher stopped unexpectedly")
		}
		logger.Error("Codex bridge stopping", "reason", "input dispatcher stopped", "error", dispatcherErr)
		return finish(dispatcherErr, dispatcherErr.Error())
	case <-outputRelay.Done():
		outputErr := outputRelay.Err()
		if outputErr == nil {
			outputErr = errors.New("Codex output relay stopped unexpectedly")
		}
		logger.Error("Codex bridge stopping", "reason", "output relay stopped", "error", outputErr)
		return finish(outputErr, outputErr.Error())
	}
}

func applyYoloThreadSettings(yolo bool, approvalPolicy, sandbox *string) {
	if !yolo {
		return
	}
	*approvalPolicy = approvalPolicyNever
	*sandbox = sandboxModeDangerFullAccess
}

func holdNamedAgent(ctx context.Context, store NamedAgentStore, options Options, name string) (func(), <-chan error, error) {
	logger := options.Logger
	if logger == nil {
		logger = slog.New(slog.DiscardHandler)
	}
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
				agent, err := store.RenewNamedAgent(leaseContext, name, token, duration)
				if err != nil {
					logger.Error("renew named agent ownership", "agent", name, "error", err)
					select {
					case errorsChannel <- err:
					default:
					}
					return
				}
				logger.Debug("named agent ownership renewed", "agent", name, "lease_expires_at", agent.LeaseExpiresAt)
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

func bridgeAdapterLaunchError(err error, resumeThreadID, agentName string) error {
	var providerErr *harness.ProviderError
	if errors.As(err, &providerErr) {
		switch providerErr.Operation {
		case "start app-server":
			return fmt.Errorf("start Codex app-server: %w", err)
		case "initialize app-server":
			return fmt.Errorf("initialize Codex app-server: %w", err)
		case "acknowledge app-server initialization":
			return fmt.Errorf("acknowledge Codex app-server initialization: %w", err)
		}
	}
	if resumeThreadID != "" {
		return fmt.Errorf("resume Codex thread %s for named agent %s: %w; use --new-thread to rotate explicitly", resumeThreadID, agentName, err)
	}
	return fmt.Errorf("start Codex thread: %w", err)
}

func bridgeReadyBody(options Options) string {
	name := strings.TrimSpace(options.AgentName)
	if name == "" {
		name = "Codex"
	}
	return fmt.Sprintf("%s ready in %s", name, options.Directory)
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
	message := model.Message{
		ID: messageID.String(), Context: options.Repository, SenderMailboxID: mailbox.ID,
		RecipientMailboxID: human.ID, Purpose: model.MessagePurposeSystemNotice, Body: body,
		Presentation: model.PresentationStatus, Correlation: model.MessageCorrelation{Provider: "codex", SessionID: threadID},
		TechnicalSections: []model.TechnicalSection{{Namespace: "hq.harness.status", Fields: []model.TechnicalField{
			{Key: "mailbox_id", Label: "HQ mailbox", Value: mailbox.ID}, {Key: "status", Label: "Status", Value: status},
		}}}, CreatedAt: createdAt,
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
