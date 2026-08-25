package harnessbridge

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/harness"
	"github.com/wbbradley/hq/internal/model"
)

type operationTracker struct {
	mu      sync.Mutex
	active  harness.OperationID
	changed chan struct{}
}

func newOperationTracker() *operationTracker { return &operationTracker{changed: make(chan struct{})} }

func (t *operationTracker) activeID() harness.OperationID {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.active
}

func (t *operationTracker) set(operation harness.OperationID) {
	t.mu.Lock()
	if t.active != operation {
		t.active = operation
		close(t.changed)
		t.changed = make(chan struct{})
	}
	t.mu.Unlock()
}

func (t *operationTracker) apply(event harness.Event) {
	status, ok := event.Payload.(harness.OperationStatusEvent)
	if !ok {
		return
	}
	if status.Status == harness.OperationRunning {
		t.set(event.Operation)
		return
	}
	t.mu.Lock()
	if t.active == event.Operation {
		t.active = ""
		close(t.changed)
		t.changed = make(chan struct{})
	}
	t.mu.Unlock()
}

func (t *operationTracker) waitForChange(ctx context.Context, expected harness.OperationID) error {
	t.mu.Lock()
	if t.active != expected {
		t.mu.Unlock()
		return nil
	}
	changed := t.changed
	t.mu.Unlock()
	select {
	case <-changed:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

type Dispatcher struct {
	Session         harness.Session
	Provider        harness.Provider
	Store           ClaimStore
	ProjectStore    domain.ProjectDeliveryOperations
	Ledger          DeliveryLedger
	Replies         *replyRegistry
	Operations      *operationTracker
	MailboxID       string
	ProjectID       string
	AssignmentID    string
	ProjectThreadID string
	Invalidations   <-chan domain.Invalidation
	RepairInterval  time.Duration
	Sync            func(context.Context) error
}

type claimedDelivery struct {
	message    model.Message
	token      string
	dispatched bool
}

func (d *Dispatcher) Run(ctx context.Context) error {
	if d.Session == nil || d.Store == nil || d.Ledger == nil || d.Operations == nil || d.MailboxID == "" {
		return errors.New("harness inbound dispatcher is missing a required dependency")
	}
	if d.ProjectID != "" && (d.AssignmentID == "" || d.ProjectThreadID == "" || d.ProjectStore == nil) {
		return errors.New("project dispatcher is missing a required dependency")
	}
	interval := d.RepairInterval
	if interval <= 0 {
		interval = defaultRepairInterval
	}
	for {
		if ctx.Err() != nil {
			return nil
		}
		if d.Sync != nil {
			if err := d.Sync(ctx); err != nil && ctx.Err() == nil {
				return fmt.Errorf("sync HQ mailbox: %w", err)
			}
		}
		if d.Replies != nil {
			claimed, err := d.Replies.claimOne(ctx, d.Store, d.MailboxID)
			if err != nil && ctx.Err() == nil {
				return fmt.Errorf("claim structured HQ reply: %w", err)
			}
			if claimed {
				continue
			}
		}
		delivery, err := d.claim(ctx)
		if errors.Is(err, domain.ErrNotReady) {
			if !d.wait(ctx, interval) {
				return nil
			}
			continue
		}
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return fmt.Errorf("claim HQ message: %w", err)
		}
		accepted, dispatchErr := true, error(nil)
		if !delivery.dispatched {
			accepted, dispatchErr = d.deliver(ctx, delivery)
		}
		if dispatchErr != nil || !accepted {
			d.release(delivery)
			if ctx.Err() != nil {
				return nil
			}
			if !d.wait(ctx, interval) {
				return nil
			}
			continue
		}
		if err := d.Store.Complete(ctx, delivery.message.ID, delivery.token); err != nil {
			if ctx.Err() != nil {
				d.release(delivery)
				return nil
			}
			if !errors.Is(err, domain.ErrNotReady) {
				d.release(delivery)
				return fmt.Errorf("complete HQ message %s: %w", delivery.message.ID, err)
			}
		}
	}
}

func (d *Dispatcher) wait(ctx context.Context, interval time.Duration) bool {
	timer := time.NewTimer(interval)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-d.Invalidations:
		return true
	case <-timer.C:
		return true
	}
}

func (d *Dispatcher) claim(ctx context.Context) (claimedDelivery, error) {
	token, err := uuid.NewV7()
	if err != nil {
		return claimedDelivery{}, err
	}
	if d.ProjectID != "" {
		delivery, err := d.ProjectStore.ClaimProjectMessage(ctx, d.ProjectID, d.AssignmentID, d.ProjectThreadID, token.String())
		return claimedDelivery{message: delivery.Message, token: token.String(), dispatched: delivery.Dispatched}, err
	}
	identity := d.Session.Identity()
	claim := domain.Claim{RecipientMailboxID: d.MailboxID, CorrelationProvider: string(identity.Provider), CorrelationSessionID: string(identity.ID)}
	if d.Replies != nil {
		claim.ExcludeReplyTo = d.Replies.outstandingIDs()
	}
	message, err := d.Store.Claim(ctx, claim, token.String())
	return claimedDelivery{message: message, token: token.String()}, err
}

func (d *Dispatcher) release(delivery claimedDelivery) {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if d.ProjectID != "" {
		_ = d.ProjectStore.ReleaseProjectMessage(ctx, delivery.message.ID, delivery.token)
		return
	}
	_ = d.Store.Release(ctx, delivery.message.ID, delivery.token)
}

func (d *Dispatcher) deliver(ctx context.Context, delivery claimedDelivery) (bool, error) {
	sessionID, messageID := d.Session.Identity().Key(), delivery.message.ID
	state, exists, err := d.Ledger.Delivery(sessionID, messageID)
	if err != nil {
		return false, err
	}
	if exists && state == DeliveryAccepted {
		return true, d.recordProjectDispatch(ctx, messageID, delivery.token)
	}
	if exists && state == DeliveryUncertain {
		found, err := d.reconcile(ctx, harness.SubmissionID(messageID))
		if err != nil {
			return false, err
		}
		if found {
			if err := d.recordProjectDispatch(ctx, messageID, delivery.token); err != nil {
				return false, err
			}
			return true, d.Ledger.SetDelivery(sessionID, messageID, DeliveryAccepted)
		}
	}
	if !exists {
		if err := d.Ledger.SetDelivery(sessionID, messageID, DeliveryPending); err != nil {
			return false, err
		}
	}
	if err := d.Ledger.SetDelivery(sessionID, messageID, DeliveryUncertain); err != nil {
		return false, err
	}
	if d.ProjectID != "" {
		if err := d.ProjectStore.MarkProjectDispatchUncertain(ctx, messageID, delivery.token); err != nil {
			return false, err
		}
	}
	if err := d.dispatch(ctx, delivery.message); err != nil {
		return false, err
	}
	if err := d.recordProjectDispatch(ctx, messageID, delivery.token); err != nil {
		return false, err
	}
	return true, d.Ledger.SetDelivery(sessionID, messageID, DeliveryAccepted)
}

func (d *Dispatcher) dispatch(ctx context.Context, message model.Message) error {
	submission := harness.Submission{ID: harness.SubmissionID(message.ID), Input: []harness.InputPart{harness.TextInput{Text: message.Body}}}
	for {
		active := d.Operations.activeID()
		if active == "" {
			result, err := d.Session.Submit(ctx, submission)
			if result.State == harness.DeliveryAccepted {
				d.Operations.set(result.OperationID)
			}
			return deliveryError(result, err)
		}
		steerer, ok := d.Session.(harness.ActiveOperationSubmitter)
		if ok && d.Provider.Capabilities.SteerActiveOperation {
			result, err := steerer.SubmitToActive(ctx, active, submission)
			if err == nil && result.State == harness.DeliveryAccepted {
				return nil
			}
			found, reconcileErr := d.reconcile(ctx, submission.ID)
			if reconcileErr == nil && found {
				return nil
			}
			if reconcileErr != nil && err != nil {
				return err
			}
		}
		if err := d.Operations.waitForChange(ctx, active); err != nil {
			return err
		}
	}
}

func deliveryError(result harness.DeliveryResult, err error) error {
	if validateErr := result.Validate(); validateErr != nil {
		return validateErr
	}
	if result.State == harness.DeliveryAccepted {
		return nil
	}
	if err != nil {
		return err
	}
	return fmt.Errorf("harness submission was %s", result.State)
}

func (d *Dispatcher) reconcile(ctx context.Context, submissionID harness.SubmissionID) (bool, error) {
	reconciler, ok := d.Session.(harness.SubmissionReconciler)
	if !ok {
		if d.Provider.Capabilities.IdempotentSubmission {
			return false, nil
		}
		return false, harness.NewCapabilityError(d.Provider.ID, harness.CapabilitySubmissionLookup)
	}
	result, err := reconciler.Reconcile(ctx, submissionID)
	if err != nil {
		return false, err
	}
	if err := result.Validate(); err != nil {
		return false, err
	}
	if result.State == harness.RecoveryAccepted {
		d.Operations.set(result.OperationID)
		return true, nil
	}
	return false, nil
}

func (d *Dispatcher) recordProjectDispatch(ctx context.Context, messageID, token string) error {
	if d.ProjectID == "" {
		return nil
	}
	return d.ProjectStore.RecordProjectDispatch(ctx, messageID, token)
}
