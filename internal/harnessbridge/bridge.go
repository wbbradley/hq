package harnessbridge

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/model"
)

func Run(ctx context.Context, options Options) error {
	defer clearEnvironment(options.Environment)
	logger := options.Logger
	if logger == nil {
		logger = slog.New(slog.DiscardHandler)
	}
	if strings.TrimSpace(options.Directory) == "" {
		return errors.New("harness working directory is required")
	}
	if strings.TrimSpace(options.AgentName) == "" {
		return errors.New("harness bridge requires a durable agent name")
	}
	if options.Factory == nil || options.Store == nil || options.Ledger == nil {
		return errors.New("harness bridge is missing a required dependency")
	}
	if options.NewSession && options.RequestedSession != "" {
		return errors.New("starting a new session cannot also resume a session")
	}
	if options.ProjectID != "" && (options.ProjectStore == nil || options.ProjectReady == nil) {
		return errors.New("project harness bridge is missing a required dependency")
	}
	provider := options.Factory.Provider()
	if !provider.Capabilities.IdempotentSubmission && !provider.Capabilities.SubmissionLookup {
		return harness.NewCapabilityError(provider.ID, harness.CapabilitySubmissionLookup)
	}
	terms := normalizeTerminology(options.Terminology, provider)
	namedAgent, err := options.Store.CreateNamedAgent(ctx, options.AgentName, "")
	if err != nil {
		return fmt.Errorf("resolve named agent %s: %w", options.AgentName, err)
	}
	requestedSession := options.RequestedSession
	if options.ProjectID == "" && requestedSession == "" && !options.NewSession && namedAgent.CurrentSessionID != "" {
		if namedAgent.Harness != string(provider.ID) {
			return fmt.Errorf("named agent %s currently selects %s session %s; %s", options.AgentName, namedAgent.Harness, namedAgent.CurrentSessionID, terms.NewSessionHint)
		}
		requestedSession = harness.SessionID(namedAgent.CurrentSessionID)
	}
	stopOwnership, ownershipErrors, err := holdOwnership(ctx, options, logger)
	if err != nil {
		return fmt.Errorf("acquire named agent %s: %w", options.AgentName, err)
	}
	defer stopOwnership()

	var subscription domain.ChangeSubscription
	if options.Updates.Subscribe != nil {
		subscription, err = options.Updates.Subscribe(ctx, domain.TopicMessages, domain.TopicMailboxes)
		if err != nil {
			return fmt.Errorf("subscribe to harness HQ mailbox: %w", err)
		}
		defer subscription.Close()
	}
	reporterContext, stopReporter := context.WithCancel(ctx)
	reporterDone := make(chan struct{})
	go func() {
		reportConnectionUpdates(reporterContext, options.Stderr, options.Updates, strings.ToLower(provider.DisplayName))
		close(reporterDone)
	}()
	defer func() {
		stopReporter()
		<-reporterDone
	}()

	mode := harness.SessionNew
	if requestedSession != "" {
		mode = harness.SessionResume
	}
	launchEnvironment := append([]string(nil), options.Environment...)
	instance, err := options.Factory.Launch(ctx, harness.LaunchConfig{
		InstanceID: harness.InstanceID(options.AgentName), AgentName: options.AgentName, Directory: options.Directory,
		Environment: launchEnvironment, SessionMode: mode, RequestedSession: requestedSession, Options: options.ProviderOptions,
	})
	clearEnvironment(launchEnvironment)
	clearEnvironment(options.Environment)
	options.Environment = nil
	if err != nil {
		if ctx.Err() != nil {
			return nil
		}
		if mode == harness.SessionResume {
			return fmt.Errorf("resume harness session %s failed; %s: %w", requestedSession, terms.NewSessionHint, err)
		}
		return err
	}
	identity := instance.Session().Identity()
	if identity.Provider != provider.ID || identity.ID == "" {
		shutdownInstance(instance)
		return errors.New("harness factory returned an invalid session identity")
	}

	var mailbox model.Mailbox
	var projectBinding ProjectBinding
	if options.ProjectID != "" {
		projectBinding, err = options.ProjectReady(Ready{AgentName: options.AgentName, Session: identity, Directory: options.Directory})
		if err != nil {
			shutdownInstance(instance)
			return fmt.Errorf("activate project harness session: %w", err)
		}
		if projectBinding.ProjectID != options.ProjectID || projectBinding.AssignmentID == "" || projectBinding.ProjectThreadID == "" || projectBinding.MailboxID == "" {
			shutdownInstance(instance)
			return errors.New("project ready callback returned an invalid binding")
		}
		mailbox = model.Mailbox{ID: projectBinding.MailboxID, Kind: model.MailboxProject, Harness: string(provider.ID), Label: options.AgentName + " · " + projectBinding.ProjectName, Context: options.Repository}
	} else {
		selected, selectErr := options.Store.SelectNamedAgentSession(ctx, options.AgentName, model.SessionIdentity{Harness: string(provider.ID), ExternalSessionID: string(identity.ID)}, options.Repository)
		if selectErr != nil {
			shutdownInstance(instance)
			return fmt.Errorf("select harness session for named agent %s: %w", options.AgentName, selectErr)
		}
		namedAgent = selected
		mailbox = model.Mailbox{ID: namedAgent.MailboxID, Kind: model.MailboxAgent, Harness: string(provider.ID), Label: namedAgent.Name, Context: options.Repository}
	}

	workersContext, stopWorkers := context.WithCancel(ctx)
	replies := newReplyRegistry()
	questioner := &questioner{
		store: options.Store, replies: replies, mailbox: mailbox, session: identity, repository: options.Repository,
		sync: options.Sync, subscribe: options.Updates.Subscribe, repairInterval: options.RepairInterval, terms: terms,
	}
	requests := startRequestPump(workersContext, instance, questioner)
	operations := newOperationTracker()
	var projectOutput *domain.ProjectOutputBinding
	if projectBinding.ProjectID != "" {
		projectOutput = &domain.ProjectOutputBinding{
			ProjectID: projectBinding.ProjectID, AssignmentID: projectBinding.AssignmentID, AgentName: options.AgentName,
			ProjectThreadID: projectBinding.ProjectThreadID, ExternalThreadID: string(identity.ID), RuntimeState: "connected",
		}
	}
	// Event ingestion stays alive until the instance closes so shutdown can never
	// deadlock a provider transport behind persistence work or a full source stream.
	events := startEventRelay(context.Background(), instance, options.Store, options.ProjectStore, options.Ledger, options.Sync, mailbox, options.Repository, projectOutput, terms, operations)

	if !options.SuppressStatus && options.PublishStatus != nil {
		if err := options.PublishStatus(ctx, mailbox, identity, terms.ReadyBody, terms.ReadyStatus, events.nextCreatedAt()); err != nil {
			stopWorkers()
			shutdownInstance(instance)
			events.StopAndWait()
			return err
		}
	}
	finishBeforeDispatcher := func(failure error) error {
		stopWorkers()
		shutdownInstance(instance)
		events.StopAndWait()
		terminalStatus := terms.CancelledStatus
		if failure != nil {
			terminalStatus = failure.Error()
		}
		if !options.SuppressStatus && options.PublishStatus != nil {
			_ = options.PublishStatus(context.Background(), mailbox, identity, terms.StoppedBody, terminalStatus, events.nextCreatedAt())
		}
		return failure
	}
	initialMessageID := ""
	if prompt := strings.TrimSpace(options.InitialPrompt); prompt != "" {
		initialID := options.InitialSubmissionID
		if initialID == "" {
			id, idErr := uuid.NewV7()
			if idErr != nil {
				return finishBeforeDispatcher(idErr)
			}
			initialID = harness.SubmissionID(id.String())
		}
		initial := model.Message{
			ID: string(initialID), Context: options.Repository, SenderMailboxID: model.HumanMailboxID, RecipientMailboxID: mailbox.ID,
			Body: prompt, Correlation: model.MessageCorrelation{Provider: string(identity.Provider), SessionID: string(identity.ID)}, CreatedAt: events.nextCreatedAt(),
		}
		if projectBinding.ProjectID != "" {
			initial.Purpose = model.MessagePurposeProjectInput
		}
		existing, getErr := options.Store.Get(ctx, initial.ID)
		switch {
		case getErr == nil:
			if existing.SenderMailboxID != initial.SenderMailboxID || existing.RecipientMailboxID != initial.RecipientMailboxID || existing.Body != initial.Body || model.NormalizeMessagePurpose(existing.Purpose) != model.NormalizeMessagePurpose(initial.Purpose) || existing.Correlation != initial.Correlation {
				return finishBeforeDispatcher(fmt.Errorf("initial harness submission ID %s already belongs to a different message", initial.ID))
			}
		case errors.Is(getErr, domain.ErrNotFound):
			if err := options.Store.Create(ctx, initial); err != nil {
				if ctx.Err() != nil && errors.Is(err, context.Canceled) {
					return finishBeforeDispatcher(nil)
				}
				return finishBeforeDispatcher(fmt.Errorf("persist initial harness submission: %w", err))
			}
		default:
			if ctx.Err() != nil && errors.Is(getErr, context.Canceled) {
				return finishBeforeDispatcher(nil)
			}
			return finishBeforeDispatcher(fmt.Errorf("read initial harness submission: %w", getErr))
		}
		initialMessageID = initial.ID
	}

	dispatcherContext, stopDispatcher := context.WithCancel(workersContext)
	dispatcherDone := make(chan struct{})
	var dispatcherErr error
	dispatcher := &Dispatcher{
		Session: instance.Session(), Provider: provider, Store: options.Store, ProjectStore: options.ProjectStore, Ledger: options.Ledger,
		Replies: replies, Operations: operations, MailboxID: mailbox.ID, RepairInterval: options.RepairInterval, Sync: options.Sync,
		ProjectID: projectBinding.ProjectID, AssignmentID: projectBinding.AssignmentID, ProjectThreadID: projectBinding.ProjectThreadID,
	}
	if subscription != nil {
		dispatcher.Invalidations = subscription.Changes()
	}
	go func() {
		dispatcherErr = dispatcher.Run(dispatcherContext)
		close(dispatcherDone)
	}()
	runtimeDone := make(chan error, 1)
	go func() { runtimeDone <- instance.Wait(context.Background()) }()

	var bridgeErr error
	status := terms.CancelledStatus
	terminated := false
	for initialMessageID != "" && !terminated {
		message, getErr := options.Store.Get(ctx, initialMessageID)
		if getErr != nil {
			bridgeErr, status, terminated = fmt.Errorf("read initial harness submission: %w", getErr), getErr.Error(), true
			break
		}
		if message.CompletedAt != nil {
			break
		}
		timer := time.NewTimer(10 * time.Millisecond)
		select {
		case ownershipErr := <-ownershipErrors:
			bridgeErr, status, terminated = fmt.Errorf("named agent ownership lost: %w", ownershipErr), ownershipErr.Error(), true
		case <-ctx.Done():
			terminated = true
		case runtimeErr := <-runtimeDone:
			if runtimeErr == nil {
				runtimeErr = errors.New("harness instance stopped unexpectedly")
			}
			bridgeErr, status, terminated = runtimeErr, runtimeErr.Error(), true
		case <-dispatcherDone:
			if dispatcherErr == nil {
				dispatcherErr = errors.New("HQ input dispatcher stopped unexpectedly")
			}
			bridgeErr, status, terminated = dispatcherErr, dispatcherErr.Error(), true
		case <-events.Failed():
			bridgeErr, status, terminated = events.Err(), events.Err().Error(), true
		case <-requests.Failed():
			bridgeErr, status, terminated = requests.Err(), requests.Err().Error(), true
		case <-timer.C:
		}
		if !timer.Stop() {
			select {
			case <-timer.C:
			default:
			}
		}
	}
	if !terminated {
		if options.OnReady != nil {
			options.OnReady(Ready{AgentName: options.AgentName, Session: identity, Directory: options.Directory})
		}
		select {
		case ownershipErr := <-ownershipErrors:
			bridgeErr, status = fmt.Errorf("named agent ownership lost: %w", ownershipErr), ownershipErr.Error()
		case <-ctx.Done():
		case runtimeErr := <-runtimeDone:
			if runtimeErr == nil {
				runtimeErr = errors.New("harness instance stopped unexpectedly")
			}
			bridgeErr, status = runtimeErr, runtimeErr.Error()
		case <-dispatcherDone:
			if ctx.Err() == nil {
				if dispatcherErr == nil {
					dispatcherErr = errors.New("HQ input dispatcher stopped unexpectedly")
				}
				bridgeErr, status = dispatcherErr, dispatcherErr.Error()
			}
		case <-events.Failed():
			bridgeErr, status = events.Err(), events.Err().Error()
		case <-requests.Failed():
			bridgeErr, status = requests.Err(), requests.Err().Error()
		}
	}
	if ctx.Err() != nil && (bridgeErr == nil || errors.Is(bridgeErr, context.Canceled)) {
		bridgeErr, status = nil, terms.CancelledStatus
	}

	stopWorkers()
	stopDispatcher()
	<-dispatcherDone
	shutdownInstance(instance)
	events.StopAndWait()
	if !options.SuppressStatus && options.PublishStatus != nil {
		_ = options.PublishStatus(context.Background(), mailbox, identity, terms.StoppedBody, status, events.nextCreatedAt())
	}
	return bridgeErr
}

func clearEnvironment(environment []string) {
	for index := range environment {
		environment[index] = ""
	}
}

func normalizeTerminology(terms Terminology, provider harness.Provider) Terminology {
	if terms.ProviderName == "" {
		terms.ProviderName = provider.DisplayName
	}
	if terms.SessionName == "" {
		terms.SessionName = "session"
	}
	if terms.OperationName == "" {
		terms.OperationName = "operation"
	}
	if terms.ItemName == "" {
		terms.ItemName = "item"
	}
	if terms.StoppedBody == "" {
		terms.StoppedBody = terms.ProviderName + " bridge stopped"
	}
	if terms.CancelledStatus == "" {
		terms.CancelledStatus = "Bridge cancelled; the harness instance is being terminated."
	}
	if terms.NewSessionHint == "" {
		terms.NewSessionHint = "start a new " + provider.DisplayName + " session to replace it"
	}
	return terms
}

func holdOwnership(ctx context.Context, options Options, logger *slog.Logger) (func(), <-chan error, error) {
	duration := options.AgentLeaseDuration
	if duration <= 0 {
		duration = defaultLeaseDuration
	}
	interval := options.AgentRenewInterval
	if interval <= 0 {
		interval = defaultRenewInterval
	}
	token := uuid.NewString()
	if _, err := options.Store.AcquireNamedAgent(ctx, options.AgentName, token, duration); err != nil {
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
				if _, err := options.Store.RenewNamedAgent(leaseContext, options.AgentName, token, duration); err != nil {
					logger.Error("renew named agent ownership", "agent", options.AgentName, "error", err)
					errorsChannel <- err
					return
				}
			}
		}
	}()
	return func() {
		cancel()
		<-done
		releaseContext, releaseCancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer releaseCancel()
		_ = options.Store.ReleaseNamedAgent(releaseContext, options.AgentName, token)
	}, errorsChannel, nil
}

func shutdownInstance(instance harness.Instance) {
	ctx, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
	defer cancel()
	_ = instance.Shutdown(ctx)
	_ = instance.Wait(ctx)
}

func reportConnectionUpdates(ctx context.Context, destination interface{ Write([]byte) (int, error) }, updates domain.ClientUpdates, prefix string) {
	last := ""
	write := func(update domain.ConnectionUpdate) {
		if destination == nil || update.Diagnostic == "" || update.Diagnostic == last {
			return
		}
		last = update.Diagnostic
		_, _ = fmt.Fprintf(destination, "hq %s: %s\n", prefix, update.Diagnostic)
	}
	write(updates.Initial)
	for updates.States != nil {
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
