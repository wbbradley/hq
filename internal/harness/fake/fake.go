// Package fake provides a deterministic harness implementation for generic
// orchestration and conformance tests.
package fake

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/wbbradley/hq/internal/harness"
)

type submissionOutcome struct {
	state    harness.DeliveryState
	accepted bool
}

type Factory struct {
	mu              sync.Mutex
	provider        harness.Provider
	sessions        map[harness.SessionID]*sessionRecord
	nextSession     uint64
	nextOperation   uint64
	nextRequest     uint64
	nextLaunchError error
	nextOutcome     *submissionOutcome
}

func NewFactory(providerID harness.ProviderID) *Factory {
	return &Factory{
		provider: harness.Provider{
			ID: providerID, DisplayName: "Deterministic fake harness",
			Capabilities: harness.Capabilities{
				Resume: true, SteerActiveOperation: true, Interrupt: true, StructuredInput: true, Approvals: true,
				IdempotentSubmission: true, SubmissionLookup: true, Plans: true, Diffs: true, ToolLifecycle: true, Streaming: true,
			},
		},
		sessions: make(map[harness.SessionID]*sessionRecord),
	}
}

func (f *Factory) Provider() harness.Provider {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.provider
}

func (f *Factory) SetCapabilities(capabilities harness.Capabilities) {
	f.mu.Lock()
	f.provider.Capabilities = capabilities
	f.mu.Unlock()
}

func (f *Factory) FailNextLaunch(err error) {
	f.mu.Lock()
	f.nextLaunchError = err
	f.mu.Unlock()
}

// SetNextSubmissionOutcome controls the next submission result. accepted
// specifies whether lookup observes the submission when the returned state is
// uncertain.
func (f *Factory) SetNextSubmissionOutcome(state harness.DeliveryState, accepted bool) {
	f.mu.Lock()
	if state == harness.DeliveryAccepted {
		accepted = true
	} else if state == harness.DeliveryRejected {
		accepted = false
	}
	f.nextOutcome = &submissionOutcome{state: state, accepted: accepted}
	f.mu.Unlock()
}

func (f *Factory) Launch(ctx context.Context, config harness.LaunchConfig) (harness.Instance, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	provider := f.Provider()
	if err := config.Validate(provider); err != nil {
		return nil, err
	}
	f.mu.Lock()
	if err := f.nextLaunchError; err != nil {
		f.nextLaunchError = nil
		f.mu.Unlock()
		return nil, &harness.ProviderError{Provider: provider.ID, Operation: "launch instance", Cause: errors.Join(harness.ErrProviderUnavailable, err)}
	}
	var record *sessionRecord
	switch config.SessionMode {
	case harness.SessionNew:
		f.nextSession++
		identity := harness.SessionIdentity{Provider: provider.ID, ID: harness.SessionID(fmt.Sprintf("session-%d", f.nextSession))}
		record = &sessionRecord{identity: identity, submissions: make(map[harness.SubmissionID]harness.DeliveryResult)}
		f.sessions[identity.ID] = record
	case harness.SessionResume:
		record = f.sessions[config.RequestedSession]
		if record == nil {
			f.mu.Unlock()
			return nil, &harness.RuntimeError{Provider: provider.ID, Session: config.RequestedSession, Action: "resume", Cause: harness.ErrSessionNotFound}
		}
	}
	f.mu.Unlock()
	now := deterministicTime(0)
	instance := &instance{
		factory: f, id: config.InstanceID, provider: provider.ID, events: make(chan harness.Event, 32), requests: make(chan harness.Request, 32),
		state: harness.RuntimeState{Phase: harness.RuntimeRunning, Since: now}, done: make(chan struct{}), pending: make(map[harness.RequestID]*pendingRequest),
	}
	instance.session = &session{factory: f, instance: instance, record: record}
	return instance, nil
}

func (f *Factory) Emit(target harness.Instance, operation harness.OperationID, itemID string, payload harness.EventPayload) error {
	instance, ok := target.(*instance)
	if !ok || instance.factory != f {
		return fmt.Errorf("fake harness instance does not belong to this factory")
	}
	return instance.emit(operation, itemID, payload)
}

func (f *Factory) Ask(target harness.Instance, operation harness.OperationID, itemID string, payload harness.RequestPayload) (harness.RequestID, <-chan harness.Response, error) {
	instance, ok := target.(*instance)
	if !ok || instance.factory != f {
		return "", nil, fmt.Errorf("fake harness instance does not belong to this factory")
	}
	if payload == nil {
		return "", nil, fmt.Errorf("interactive request payload is required")
	}
	f.mu.Lock()
	f.nextRequest++
	requestID := harness.RequestID(fmt.Sprintf("request-%d", f.nextRequest))
	f.mu.Unlock()
	response := make(chan harness.Response, 1)
	instance.mu.Lock()
	if instance.closed {
		instance.mu.Unlock()
		return "", nil, harness.ErrInstanceStopped
	}
	instance.pending[requestID] = &pendingRequest{response: response}
	request := harness.Request{ID: requestID, Session: instance.session.Identity(), Operation: operation, ItemID: itemID, Payload: payload}
	instance.requests <- request
	instance.mu.Unlock()
	return requestID, response, nil
}

func (f *Factory) Crash(target harness.Instance, err error) error {
	instance, ok := target.(*instance)
	if !ok || instance.factory != f {
		return fmt.Errorf("fake harness instance does not belong to this factory")
	}
	if err == nil {
		err = harness.ErrProviderUnavailable
	}
	instance.finish(err)
	return nil
}

func (f *Factory) nextOperationID() harness.OperationID {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.nextOperation++
	return harness.OperationID(fmt.Sprintf("operation-%d", f.nextOperation))
}

func (f *Factory) consumeOutcome() submissionOutcome {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.nextOutcome == nil {
		return submissionOutcome{state: harness.DeliveryAccepted, accepted: true}
	}
	outcome := *f.nextOutcome
	f.nextOutcome = nil
	return outcome
}

type sessionRecord struct {
	mu          sync.Mutex
	identity    harness.SessionIdentity
	submissions map[harness.SubmissionID]harness.DeliveryResult
	active      harness.OperationID
}

type session struct {
	factory  *Factory
	instance *instance
	record   *sessionRecord
}

func (s *session) Identity() harness.SessionIdentity { return s.record.identity }

func (s *session) Submit(ctx context.Context, submission harness.Submission) (harness.DeliveryResult, error) {
	if err := ctx.Err(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	provider := s.factory.Provider()
	capabilities := provider.Capabilities
	if err := submission.Validate(provider); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	if err := s.instance.running(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	s.record.mu.Lock()
	if prior, ok := s.record.submissions[submission.ID]; ok && capabilities.IdempotentSubmission {
		s.record.mu.Unlock()
		return prior, nil
	}
	operationID := s.factory.nextOperationID()
	return s.applyOutcomeLocked(submission, operationID)
}

func (s *session) SubmitToActive(ctx context.Context, expected harness.OperationID, submission harness.Submission) (harness.DeliveryResult, error) {
	provider := s.factory.Provider()
	if !provider.Capabilities.SteerActiveOperation {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, harness.NewCapabilityError(provider.ID, harness.CapabilitySteerActiveOperation)
	}
	if err := ctx.Err(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	if err := s.instance.running(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	if err := submission.Validate(provider); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	s.record.mu.Lock()
	active := s.record.active
	if prior, ok := s.record.submissions[submission.ID]; ok && provider.Capabilities.IdempotentSubmission {
		s.record.mu.Unlock()
		return prior, nil
	}
	if active == "" || active != expected {
		s.record.mu.Unlock()
		return harness.DeliveryResult{State: harness.DeliveryRejected}, &harness.RuntimeError{
			Provider: provider.ID, Session: s.record.identity.ID, Operation: expected, Action: "submit to active operation", Cause: harness.ErrOperationMismatch,
		}
	}
	return s.applyOutcomeLocked(submission, active)
}

// applyOutcomeLocked consumes the configured result while record.mu is held so
// concurrent retries of one stable submission ID cannot create two operations.
func (s *session) applyOutcomeLocked(submission harness.Submission, operationID harness.OperationID) (harness.DeliveryResult, error) {
	outcome := s.factory.consumeOutcome()
	result := harness.DeliveryResult{State: outcome.state}
	if outcome.state == harness.DeliveryAccepted || outcome.accepted {
		result.OperationID = operationID
		s.record.submissions[submission.ID] = harness.DeliveryResult{State: harness.DeliveryAccepted, OperationID: operationID}
		s.record.active = operationID
	}
	s.record.mu.Unlock()
	if err := result.Validate(); err != nil {
		return harness.DeliveryResult{State: harness.DeliveryRejected}, err
	}
	if result.OperationID != "" {
		_ = s.instance.emit(operationID, "", harness.OperationStatusEvent{Status: harness.OperationRunning})
	}
	return result, nil
}

func (s *session) Reconcile(ctx context.Context, submissionID harness.SubmissionID) (harness.RecoveryResult, error) {
	provider := s.factory.Provider()
	if !provider.Capabilities.SubmissionLookup {
		return harness.RecoveryResult{}, harness.NewCapabilityError(provider.ID, harness.CapabilitySubmissionLookup)
	}
	if err := ctx.Err(); err != nil {
		return harness.RecoveryResult{}, err
	}
	if err := s.instance.running(); err != nil {
		return harness.RecoveryResult{}, err
	}
	s.record.mu.Lock()
	accepted, ok := s.record.submissions[submissionID]
	s.record.mu.Unlock()
	if !ok {
		return harness.RecoveryResult{State: harness.RecoveryNotFound}, nil
	}
	return harness.RecoveryResult{State: harness.RecoveryAccepted, OperationID: accepted.OperationID}, nil
}

func (s *session) Respond(ctx context.Context, response harness.Response) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if response.RequestID == "" || response.Payload == nil {
		return fmt.Errorf("interactive response requires request ID and payload")
	}
	s.instance.mu.Lock()
	if s.instance.closed {
		s.instance.mu.Unlock()
		return harness.ErrInstanceStopped
	}
	pending := s.instance.pending[response.RequestID]
	if pending == nil {
		s.instance.mu.Unlock()
		return harness.ErrRequestNotFound
	}
	if pending.completed {
		s.instance.mu.Unlock()
		return harness.ErrRequestCompleted
	}
	pending.completed = true
	pending.response <- response
	s.instance.mu.Unlock()
	return nil
}

func (s *session) Interrupt(ctx context.Context, operationID harness.OperationID) error {
	provider := s.factory.Provider()
	if !provider.Capabilities.Interrupt {
		return harness.NewCapabilityError(provider.ID, harness.CapabilityInterrupt)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := s.instance.running(); err != nil {
		return err
	}
	s.record.mu.Lock()
	if s.record.active != operationID {
		s.record.mu.Unlock()
		return &harness.RuntimeError{Provider: provider.ID, Session: s.record.identity.ID, Operation: operationID, Action: "interrupt", Cause: harness.ErrOperationMismatch}
	}
	s.record.active = ""
	s.record.mu.Unlock()
	return s.instance.emit(operationID, "", harness.OperationStatusEvent{Status: harness.OperationInterrupted})
}

type pendingRequest struct {
	response  chan harness.Response
	completed bool
}

type instance struct {
	factory  *Factory
	id       harness.InstanceID
	provider harness.ProviderID
	session  *session
	events   chan harness.Event
	requests chan harness.Request
	done     chan struct{}

	mu       sync.Mutex
	state    harness.RuntimeState
	sequence uint64
	pending  map[harness.RequestID]*pendingRequest
	closed   bool
	waitErr  error
}

func (i *instance) ID() harness.InstanceID           { return i.id }
func (i *instance) Provider() harness.ProviderID     { return i.provider }
func (i *instance) Session() harness.Session         { return i.session }
func (i *instance) Events() <-chan harness.Event     { return i.events }
func (i *instance) Requests() <-chan harness.Request { return i.requests }

func (i *instance) State() harness.RuntimeState {
	i.mu.Lock()
	defer i.mu.Unlock()
	return i.state
}

func (i *instance) Shutdown(context.Context) error {
	i.finish(nil)
	return nil
}

func (i *instance) Wait(ctx context.Context) error {
	select {
	case <-i.done:
		i.mu.Lock()
		err := i.waitErr
		i.mu.Unlock()
		return err
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (i *instance) running() error {
	i.mu.Lock()
	defer i.mu.Unlock()
	if i.closed {
		return harness.ErrInstanceStopped
	}
	return nil
}

func (i *instance) emit(operation harness.OperationID, itemID string, payload harness.EventPayload) error {
	if payload == nil {
		return fmt.Errorf("event payload is required")
	}
	i.mu.Lock()
	defer i.mu.Unlock()
	if i.closed {
		return harness.ErrInstanceStopped
	}
	i.sequence++
	i.events <- harness.Event{
		Sequence: i.sequence, Session: i.session.Identity(), Operation: operation, ItemID: itemID,
		OccurredAt: deterministicTime(i.sequence), Payload: payload,
	}
	return nil
}

func (i *instance) finish(err error) {
	i.mu.Lock()
	if i.closed {
		i.mu.Unlock()
		return
	}
	i.state = harness.RuntimeState{Phase: harness.RuntimeStopping, Since: deterministicTime(i.sequence + 1)}
	i.closed = true
	i.waitErr = err
	for _, pending := range i.pending {
		close(pending.response)
	}
	close(i.requests)
	close(i.events)
	if err != nil {
		i.state = harness.RuntimeState{Phase: harness.RuntimeFailed, Since: deterministicTime(i.sequence + 2), Err: err}
	} else {
		i.state = harness.RuntimeState{Phase: harness.RuntimeStopped, Since: deterministicTime(i.sequence + 2)}
	}
	close(i.done)
	i.mu.Unlock()
}

func deterministicTime(sequence uint64) time.Time {
	return time.Unix(1_700_000_000+int64(sequence), 0).UTC()
}

var (
	_ harness.Factory                  = (*Factory)(nil)
	_ harness.Instance                 = (*instance)(nil)
	_ harness.Session                  = (*session)(nil)
	_ harness.SubmissionReconciler     = (*session)(nil)
	_ harness.ActiveOperationSubmitter = (*session)(nil)
	_ harness.Interrupter              = (*session)(nil)
)
