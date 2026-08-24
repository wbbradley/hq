package harnessbridge

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/model"
)

const eventQueueCapacity = 64

type canonicalOutput struct {
	key     string
	body    string
	details string
}

type eventRelay struct {
	store        QuestionStore
	projectStore domain.ProjectOutputOperations
	ledger       DeliveryLedger
	sync         func(context.Context) error
	identity     harness.SessionIdentity
	mailbox      model.Mailbox
	repository   model.RepositoryContext
	project      *domain.ProjectOutputBinding
	terms        Terminology
	operations   *operationTracker
	queue        chan canonicalOutput
	done         chan struct{}
	failed       chan struct{}
	cancel       context.CancelFunc
	now          func() time.Time

	errMu         sync.Mutex
	err           error
	failOnce      sync.Once
	timeMu        sync.Mutex
	lastCreatedAt time.Time
}

func startEventRelay(ctx context.Context, instance harness.Instance, store QuestionStore, projectStore domain.ProjectOutputOperations, ledger DeliveryLedger, syncMailbox func(context.Context) error, mailbox model.Mailbox, repository model.RepositoryContext, project *domain.ProjectOutputBinding, terms Terminology, operations *operationTracker) *eventRelay {
	relayContext, cancel := context.WithCancel(ctx)
	relay := &eventRelay{
		store: store, projectStore: projectStore, ledger: ledger, sync: syncMailbox, identity: instance.Session().Identity(), mailbox: mailbox,
		repository: repository, project: project, terms: terms, operations: operations, queue: make(chan canonicalOutput, eventQueueCapacity), done: make(chan struct{}), failed: make(chan struct{}), cancel: cancel, now: time.Now,
	}
	go relay.publishLoop()
	go relay.ingest(relayContext, instance.Events())
	return relay
}

func (r *eventRelay) ingest(ctx context.Context, events <-chan harness.Event) {
	defer close(r.queue)
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
			output, ok := r.canonicalize(event)
			if !ok {
				continue
			}
			select {
			case r.queue <- output:
			default:
				r.fail(errors.New("harness event persistence queue reached its 64-event bound"))
			}
		}
	}
}

func (r *eventRelay) publishLoop() {
	defer close(r.done)
	for output := range r.queue {
		if err := r.publish(output); err != nil {
			r.fail(err)
		}
	}
}

func (r *eventRelay) canonicalize(event harness.Event) (canonicalOutput, bool) {
	switch payload := event.Payload.(type) {
	case harness.OutputEvent:
		if event.Operation == "" || event.ItemID == "" || strings.TrimSpace(payload.Text) == "" {
			return canonicalOutput{}, false
		}
		kind := "update"
		phase := "commentary"
		if payload.Final {
			kind, phase = "final-answer", "final_answer"
		}
		details := fmt.Sprintf("Kind: %s\n%s %s: %s\n%s %s: %s\n%s %s: %s\nPhase: %s", kind, r.terms.ProviderName, r.terms.SessionName, event.Session.ID, r.terms.ProviderName, r.terms.OperationName, event.Operation, r.terms.ProviderName, r.terms.ItemName, event.ItemID, phase)
		return canonicalOutput{key: event.ItemID, body: payload.Text, details: details}, true
	case harness.OperationStatusEvent:
		if event.Operation == "" {
			return canonicalOutput{}, false
		}
		key := "turn-status:" + string(event.Operation)
		switch payload.Status {
		case harness.OperationFailed:
			errorMessage := strings.TrimSpace(payload.Error)
			if errorMessage == "" {
				errorMessage = "(not provided)"
			}
			details := fmt.Sprintf("Kind: status\n%s %s: %s\n%s %s: %s\nStatus: failed\nError: %s", r.terms.ProviderName, r.terms.SessionName, event.Session.ID, r.terms.ProviderName, r.terms.OperationName, event.Operation, errorMessage)
			return canonicalOutput{key: key, body: r.terms.ProviderName + " " + r.terms.OperationName + " failed", details: details}, true
		case harness.OperationInterrupted:
			details := fmt.Sprintf("Kind: status\n%s %s: %s\n%s %s: %s\nStatus: interrupted", r.terms.ProviderName, r.terms.SessionName, event.Session.ID, r.terms.ProviderName, r.terms.OperationName, event.Operation)
			return canonicalOutput{key: key, body: r.terms.ProviderName + " " + r.terms.OperationName + " interrupted", details: details}, true
		}
	}
	return canonicalOutput{}, false
}

func (r *eventRelay) publish(output canonicalOutput) error {
	if r.store == nil || r.ledger == nil || r.identity.ID == "" || r.mailbox.ID == "" {
		return errors.New("harness output relay is not bound")
	}
	sessionID := string(r.identity.ID)
	sent, err := r.ledger.OutputSent(sessionID, output.key)
	if err != nil {
		return fmt.Errorf("read harness output ledger: %w", err)
	}
	if sent {
		return nil
	}
	message := model.Message{
		ID: r.stableOutputID(output.key), Context: r.repository, SenderMailboxID: r.mailbox.ID, RecipientMailboxID: model.HumanMailboxID,
		Body: output.body, Details: output.details, CreatedAt: r.nextCreatedAt(),
	}
	if r.project != nil {
		message.Purpose = model.MessagePurposeProjectOutput
	}
	existing, err := r.store.Get(context.Background(), message.ID)
	switch {
	case err == nil:
		if !sameOutput(existing, message) {
			return fmt.Errorf("harness output message ID %s collides with different HQ content", message.ID)
		}
		r.advanceCreatedAt(existing.CreatedAt)
	case errors.Is(err, domain.ErrNotFound):
		var createErr error
		if r.project != nil {
			if r.projectStore == nil {
				return errors.New("project harness output store is required")
			}
			createErr = r.projectStore.CreateProjectOutput(context.Background(), *r.project, message)
		} else {
			createErr = r.store.Create(context.Background(), message)
		}
		if createErr != nil {
			return fmt.Errorf("publish harness output: %w", createErr)
		}
	default:
		return fmt.Errorf("reconcile harness output: %w", err)
	}
	if r.sync != nil {
		if err := r.sync(context.Background()); err != nil {
			return fmt.Errorf("sync harness output: %w", err)
		}
	}
	return r.ledger.MarkOutputSent(sessionID, output.key)
}

func (r *eventRelay) stableOutputID(key string) string {
	namespace := r.terms.OutputNamespace
	if namespace == "" {
		namespace = "hq-harness-output"
	}
	return uuid.NewSHA1(uuid.NameSpaceURL, []byte(namespace+"\x00"+string(r.identity.ID)+"\x00"+key)).String()
}

func (r *eventRelay) nextCreatedAt() time.Time {
	r.timeMu.Lock()
	defer r.timeMu.Unlock()
	createdAt := r.now().UTC()
	if !r.lastCreatedAt.IsZero() && createdAt.Unix() <= r.lastCreatedAt.Unix() {
		createdAt = time.Unix(r.lastCreatedAt.Unix()+1, 0).UTC()
	}
	r.lastCreatedAt = createdAt
	return createdAt
}

func (r *eventRelay) advanceCreatedAt(createdAt time.Time) {
	r.timeMu.Lock()
	if createdAt.After(r.lastCreatedAt) {
		r.lastCreatedAt = createdAt
	}
	r.timeMu.Unlock()
}

func (r *eventRelay) fail(err error) {
	r.errMu.Lock()
	if r.err == nil {
		r.err = err
	}
	r.errMu.Unlock()
	r.failOnce.Do(func() { close(r.failed) })
}

func (r *eventRelay) Err() error {
	r.errMu.Lock()
	defer r.errMu.Unlock()
	return r.err
}

func (r *eventRelay) Done() <-chan struct{}   { return r.done }
func (r *eventRelay) Failed() <-chan struct{} { return r.failed }

func (r *eventRelay) StopAndWait() {
	r.cancel()
	<-r.done
}

func sameOutput(existing, expected model.Message) bool {
	return existing.ID == expected.ID && existing.SenderMailboxID == expected.SenderMailboxID && existing.RecipientMailboxID == expected.RecipientMailboxID && existing.Purpose == model.NormalizeMessagePurpose(expected.Purpose) && existing.Body == expected.Body && existing.Details == expected.Details
}
