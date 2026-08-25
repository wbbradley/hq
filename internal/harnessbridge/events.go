package harnessbridge

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/model"
)

// The buffer bounds persistence memory while allowing the provider transport
// to continue reading short bursts. Durable work and new replaceable keys apply
// cancellation-aware backpressure; an already-pending replaceable key moves to
// the tail with its latest value.
const eventQueueCapacity = 64

type canonicalOutput struct {
	key               string
	operation         harness.OperationID
	createdAt         time.Time
	body              string
	details           string
	presentation      model.PresentationKind
	correlation       model.MessageCorrelation
	technicalSections []model.TechnicalSection
}

type eventWork struct {
	output   *canonicalOutput
	activity *domain.HarnessActivity
}

func (w eventWork) coalesceKey() string {
	if w.output != nil || w.activity == nil {
		return ""
	}
	activity := w.activity
	switch activity.Kind {
	case domain.HarnessActivityPlan, domain.HarnessActivityDiff, domain.HarnessActivityProgress:
	case domain.HarnessActivityOperation:
		if activity.Status != domain.HarnessActivityRunning {
			return ""
		}
	default:
		return ""
	}
	correlation := activity.Correlation
	return activity.InstallationID + "\x00" + activity.MailboxID + "\x00" + correlation.Provider + "\x00" + correlation.SessionID + "\x00" + correlation.OperationID + "\x00" + string(activity.Kind) + "\x00" + correlation.ItemID
}

type eventRelay struct {
	store         QuestionStore
	projectStore  domain.ProjectOutputOperations
	ledger        DeliveryLedger
	sync          func(context.Context) error
	identity      harness.SessionIdentity
	runtimeID     harness.InstanceID
	mailbox       model.Mailbox
	repository    model.RepositoryContext
	project       *domain.ProjectOutputBinding
	terms         Terminology
	operations    *operationTracker
	activity      domain.HarnessActivityWriter
	queue         *eventBuffer
	done          chan struct{}
	failed        chan struct{}
	cancel        context.CancelFunc
	persistCtx    context.Context
	cancelPersist context.CancelFunc
	now           func() time.Time

	errMu          sync.Mutex
	err            error
	failOnce       sync.Once
	timeMu         sync.Mutex
	lastTimelineAt time.Time
}

func startEventRelay(ctx context.Context, instance harness.Instance, store QuestionStore, projectStore domain.ProjectOutputOperations, ledger DeliveryLedger, syncMailbox func(context.Context) error, mailbox model.Mailbox, repository model.RepositoryContext, project *domain.ProjectOutputBinding, terms Terminology, operations *operationTracker) *eventRelay {
	relayContext, cancel := context.WithCancel(ctx)
	persistContext, cancelPersist := context.WithCancel(context.Background())
	relay := &eventRelay{
		store: store, projectStore: projectStore, ledger: ledger, sync: syncMailbox, identity: instance.Session().Identity(), runtimeID: instance.ID(), mailbox: mailbox,
		repository: repository, project: project, terms: terms, operations: operations, queue: newEventBuffer(eventQueueCapacity), done: make(chan struct{}), failed: make(chan struct{}), cancel: cancel, persistCtx: persistContext, cancelPersist: cancelPersist, now: time.Now,
	}
	relay.activity, _ = store.(domain.HarnessActivityWriter)
	go relay.publishLoop()
	go relay.ingest(relayContext, instance.Events())
	return relay
}

func (r *eventRelay) ingest(ctx context.Context, events <-chan harness.Event) {
	defer r.queue.close()
	for {
		select {
		case <-ctx.Done():
			return
		case event, open := <-events:
			if !open {
				return
			}
			if event.Session != r.identity {
				continue
			}
			r.operations.apply(event)
			work := r.normalize(event)
			if work.output == nil && work.activity == nil {
				continue
			}
			if err := r.queue.enqueue(ctx, work); err != nil {
				return
			}
		}
	}
}

func (r *eventRelay) publishLoop() {
	defer close(r.done)
	for {
		work, ok := r.queue.dequeue()
		if !ok {
			return
		}
		if err := r.publishWork(r.persistCtx, work); err != nil {
			r.fail(err)
		}
	}
}

func (r *eventRelay) normalize(event harness.Event) eventWork {
	work := eventWork{}
	if output, ok := r.canonicalize(event); ok {
		work.output = &output
	}
	if r.activity != nil {
		if activity, ok := r.projectActivity(event); ok {
			work.activity = &activity
		}
	}
	r.assignWorkTime(event.OccurredAt, &work)
	return work
}

func (r *eventRelay) publishWork(ctx context.Context, work eventWork) error {
	if work.output != nil {
		if err := r.publish(ctx, *work.output); err != nil {
			return err
		}
	}
	if work.activity != nil {
		if err := r.activity.UpsertHarnessActivity(ctx, *work.activity); err != nil {
			return fmt.Errorf("persist harness activity: %w", err)
		}
	}
	return nil
}

func (r *eventRelay) projectActivity(event harness.Event) (domain.HarnessActivity, bool) {
	if event.Operation == "" {
		return domain.HarnessActivity{}, false
	}
	activity := domain.HarnessActivity{
		MailboxID: r.mailbox.ID, Harness: string(event.Session.Provider), SessionID: string(event.Session.ID),
		OperationID: string(event.Operation), ItemID: event.ItemID, RuntimeID: string(r.runtimeID), Sequence: event.Sequence,
		Correlation: model.MessageCorrelation{Provider: string(event.Session.Provider), SessionID: string(event.Session.ID), OperationID: string(event.Operation), ItemID: event.ItemID},
	}
	switch payload := event.Payload.(type) {
	case harness.OperationStatusEvent:
		activity.Kind = domain.HarnessActivityOperation
		activity.Status = activityStatus(payload.Status)
		activity.Body = strings.TrimSpace(payload.Error)
	case harness.PlanEvent:
		if strings.TrimSpace(payload.Text) == "" {
			return domain.HarnessActivity{}, false
		}
		activity.Kind, activity.Body = domain.HarnessActivityPlan, payload.Text
	case harness.DiffEvent:
		if strings.TrimSpace(payload.Text) == "" {
			return domain.HarnessActivity{}, false
		}
		activity.Kind, activity.Body = domain.HarnessActivityDiff, payload.Text
	case harness.CommandEvent:
		if event.ItemID == "" || strings.TrimSpace(payload.Command) == "" || !terminalActivityStatus(payload.Status) {
			return domain.HarnessActivity{}, false
		}
		activity.Kind, activity.Title, activity.Body = domain.HarnessActivityCommand, payload.Command, payload.Output
		activity.Status = activityStatus(payload.Status)
		if payload.ExitCode != nil {
			activity.Body = fmt.Sprintf("Exit code: %d\n%s", *payload.ExitCode, activity.Body)
		}
	case harness.FileChangeEvent:
		if event.ItemID == "" || strings.TrimSpace(payload.Path) == "" || !terminalActivityStatus(payload.Status) {
			return domain.HarnessActivity{}, false
		}
		activity.Kind, activity.Title, activity.Body = domain.HarnessActivityFile, payload.Path, payload.Summary
		activity.Status = activityStatus(payload.Status)
	case harness.ToolEvent:
		if event.ItemID == "" || strings.TrimSpace(payload.Name) == "" || !terminalActivityStatus(payload.Status) {
			return domain.HarnessActivity{}, false
		}
		activity.Kind, activity.Title, activity.Body = domain.HarnessActivityTool, payload.Name, payload.Summary
		activity.Status = activityStatus(payload.Status)
	case harness.ProgressEvent:
		if event.ItemID == "" || strings.TrimSpace(payload.Message) == "" {
			return domain.HarnessActivity{}, false
		}
		activity.Kind, activity.Body = domain.HarnessActivityProgress, payload.Message
	default:
		return domain.HarnessActivity{}, false
	}
	var truncated bool
	activity.Title, truncated = boundActivityText(activity.Title, domain.HarnessActivityTitleBytes)
	activity.Truncated = activity.Truncated || truncated
	bodyLimit := domain.HarnessActivityBodyBytes
	if activity.Kind == domain.HarnessActivityCommand {
		bodyLimit = domain.HarnessActivityCommandBodyBytes
	} else if activity.Kind == domain.HarnessActivityProgress {
		bodyLimit = domain.HarnessActivityProgressBytes
	}
	activity.Body, truncated = boundActivityText(activity.Body, bodyLimit)
	activity.Truncated = activity.Truncated || truncated
	valid := activity.MailboxID != "" && activity.Harness != "" && activity.SessionID != "" && activity.RuntimeID != "" && activity.Sequence > 0
	if activity.Kind == domain.HarnessActivityOperation {
		valid = valid && activity.Status != ""
	}
	return activity, valid
}

func boundActivityText(value string, limit int) (string, bool) {
	if len(value) <= limit {
		return value, false
	}
	end := limit
	for end > 0 && !utf8.ValidString(value[:end]) {
		end--
	}
	return value[:end], true
}

func activityStatus(status harness.OperationStatus) domain.HarnessActivityStatus {
	switch status {
	case harness.OperationRunning:
		return domain.HarnessActivityRunning
	case harness.OperationCompleted:
		return domain.HarnessActivityCompleted
	case harness.OperationFailed:
		return domain.HarnessActivityFailed
	case harness.OperationInterrupted:
		return domain.HarnessActivityInterrupted
	default:
		return ""
	}
}

func terminalActivityStatus(status harness.OperationStatus) bool {
	return status == harness.OperationCompleted || status == harness.OperationFailed || status == harness.OperationInterrupted
}

func (r *eventRelay) canonicalize(event harness.Event) (canonicalOutput, bool) {
	switch payload := event.Payload.(type) {
	case harness.OutputEvent:
		if event.Operation == "" || event.ItemID == "" || strings.TrimSpace(payload.Text) == "" {
			return canonicalOutput{}, false
		}
		kind := model.PresentationUpdate
		phase := "commentary"
		if payload.Final {
			kind, phase = model.PresentationFinalAnswer, "final_answer"
		}
		correlation := model.MessageCorrelation{Provider: string(event.Session.Provider), SessionID: string(event.Session.ID), OperationID: string(event.Operation), ItemID: event.ItemID}
		technical := []model.TechnicalSection{{Namespace: "hq.harness.output", Fields: []model.TechnicalField{{Key: "phase", Label: "Phase", Value: phase}}}}
		return canonicalOutput{key: event.ItemID, operation: event.Operation, body: payload.Text, presentation: kind, correlation: correlation, technicalSections: technical}, true
	case harness.OperationStatusEvent:
		if event.Operation == "" {
			return canonicalOutput{}, false
		}
		key := "operation-status:" + string(event.Operation)
		switch payload.Status {
		case harness.OperationFailed:
			errorMessage := strings.TrimSpace(payload.Error)
			if errorMessage == "" {
				errorMessage = "(not provided)"
			}
			correlation := model.MessageCorrelation{Provider: string(event.Session.Provider), SessionID: string(event.Session.ID), OperationID: string(event.Operation)}
			technical := []model.TechnicalSection{{Namespace: "hq.harness.output", Fields: []model.TechnicalField{{Key: "status", Label: "Status", Value: "failed"}}}}
			return canonicalOutput{key: key, operation: event.Operation, body: r.terms.ProviderName + " " + r.terms.OperationName + " failed", details: "Error: " + errorMessage, presentation: model.PresentationStatus, correlation: correlation, technicalSections: technical}, true
		case harness.OperationInterrupted:
			correlation := model.MessageCorrelation{Provider: string(event.Session.Provider), SessionID: string(event.Session.ID), OperationID: string(event.Operation)}
			technical := []model.TechnicalSection{{Namespace: "hq.harness.output", Fields: []model.TechnicalField{{Key: "status", Label: "Status", Value: "interrupted"}}}}
			return canonicalOutput{key: key, operation: event.Operation, body: r.terms.ProviderName + " " + r.terms.OperationName + " interrupted", presentation: model.PresentationStatus, correlation: correlation, technicalSections: technical}, true
		}
	}
	return canonicalOutput{}, false
}

func (r *eventRelay) publish(ctx context.Context, output canonicalOutput) error {
	if r.store == nil || r.ledger == nil || r.identity.ID == "" || r.mailbox.ID == "" {
		return errors.New("harness output relay is not bound")
	}
	ledgerSessionID := r.identity.Key()
	sent, err := r.ledger.OutputSent(ledgerSessionID, output.key)
	if err != nil {
		return fmt.Errorf("read harness output ledger: %w", err)
	}
	if sent {
		return nil
	}
	createdAt := output.createdAt
	if createdAt.IsZero() {
		createdAt = r.nextCreatedAt()
	}
	message := model.Message{
		ID: r.stableOutputID(output.key), Context: r.repository, SenderMailboxID: r.mailbox.ID, RecipientMailboxID: model.HumanMailboxID,
		Body: output.body, Details: output.details, Presentation: output.presentation, Correlation: output.correlation,
		TechnicalSections: output.technicalSections, CreatedAt: createdAt,
	}
	if r.project != nil {
		message.Purpose = model.MessagePurposeProjectOutput
	}
	existing, err := r.store.Get(ctx, message.ID)
	switch {
	case err == nil:
		if r.project != nil {
			if r.projectStore == nil {
				return errors.New("project harness output store is required")
			}
			if createErr := r.projectStore.CreateProjectOutput(ctx, *r.project, message); createErr != nil {
				return fmt.Errorf("reconcile project harness output: %w", createErr)
			}
		} else if !sameOutput(existing, message) {
			return fmt.Errorf("harness output message ID %s collides with different HQ content", message.ID)
		}
		r.advanceCreatedAt(existing.CreatedAt)
	case errors.Is(err, domain.ErrNotFound):
		var createErr error
		if r.project != nil {
			if r.projectStore == nil {
				return errors.New("project harness output store is required")
			}
			createErr = r.projectStore.CreateProjectOutput(ctx, *r.project, message)
		} else {
			createErr = r.store.Create(ctx, message)
		}
		if createErr != nil {
			return fmt.Errorf("publish harness output: %w", createErr)
		}
	default:
		return fmt.Errorf("reconcile harness output: %w", err)
	}
	if r.sync != nil {
		if err := r.sync(ctx); err != nil {
			return fmt.Errorf("sync harness output: %w", err)
		}
	}
	return r.ledger.MarkOutputSent(ledgerSessionID, output.key)
}

func (r *eventRelay) stableOutputID(key string) string {
	namespace := r.terms.OutputNamespace
	if namespace == "" {
		namespace = "hq-harness-output:" + string(r.identity.Provider)
	}
	return uuid.NewSHA1(uuid.NameSpaceURL, []byte(namespace+"\x00"+string(r.identity.ID)+"\x00"+key)).String()
}

func (r *eventRelay) nextCreatedAt() time.Time {
	r.timeMu.Lock()
	defer r.timeMu.Unlock()
	createdAt := r.now().UTC().Truncate(time.Second)
	if !r.lastTimelineAt.IsZero() && createdAt.Unix() <= r.lastTimelineAt.Unix() {
		createdAt = time.Unix(r.lastTimelineAt.Unix()+1, 0).UTC()
	}
	r.lastTimelineAt = createdAt
	return createdAt
}

func (r *eventRelay) advanceCreatedAt(createdAt time.Time) {
	r.timeMu.Lock()
	if createdAt.After(r.lastTimelineAt) {
		r.lastTimelineAt = createdAt
	}
	r.timeMu.Unlock()
}

func (r *eventRelay) assignWorkTime(occurredAt time.Time, work *eventWork) {
	if work == nil || work.output == nil && work.activity == nil {
		return
	}
	r.timeMu.Lock()
	defer r.timeMu.Unlock()
	if occurredAt.IsZero() {
		occurredAt = r.now()
	}
	occurredAt = occurredAt.UTC()
	subsecond := occurredAt.Sub(occurredAt.Truncate(time.Second))
	canonicalSecond := occurredAt.Truncate(time.Second)
	if !r.lastTimelineAt.IsZero() && canonicalSecond.Unix() <= r.lastTimelineAt.Unix() {
		canonicalSecond = time.Unix(r.lastTimelineAt.Unix()+1, 0).UTC()
	}
	r.lastTimelineAt = canonicalSecond
	if work.output != nil {
		work.output.createdAt = canonicalSecond
	}
	if work.activity != nil {
		if work.output != nil && subsecond < time.Millisecond {
			subsecond = time.Millisecond
		}
		work.activity.OccurredAt = canonicalSecond.Add(subsecond)
	}
}

func (r *eventRelay) fail(err error) {
	r.errMu.Lock()
	if r.err == nil {
		r.err = err
	}
	r.errMu.Unlock()
	r.failOnce.Do(func() { close(r.failed) })
	if r.cancel != nil {
		r.cancel()
	}
}

func (r *eventRelay) Err() error {
	r.errMu.Lock()
	defer r.errMu.Unlock()
	return r.err
}

func (r *eventRelay) Done() <-chan struct{}   { return r.done }
func (r *eventRelay) Failed() <-chan struct{} { return r.failed }

func (r *eventRelay) StopAndWait() {
	timer := time.NewTimer(shutdownTimeout)
	defer timer.Stop()
	select {
	case <-r.done:
	case <-timer.C:
		r.fail(errors.New("harness event persistence did not drain before shutdown timeout"))
		if r.cancel != nil {
			r.cancel()
		}
		if r.cancelPersist != nil {
			r.cancelPersist()
		}
		<-r.done
	}
	if r.cancel != nil {
		r.cancel()
	}
	if r.cancelPersist != nil {
		r.cancelPersist()
	}
}

func sameOutput(existing, expected model.Message) bool {
	return existing.ID == expected.ID && existing.SenderMailboxID == expected.SenderMailboxID && existing.RecipientMailboxID == expected.RecipientMailboxID && existing.Purpose == model.NormalizeMessagePurpose(expected.Purpose) && existing.Body == expected.Body && existing.Details == expected.Details && existing.Presentation == expected.Presentation && existing.Correlation == expected.Correlation && reflect.DeepEqual(existing.TechnicalSections, expected.TechnicalSections)
}
