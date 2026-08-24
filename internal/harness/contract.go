// Package harness defines the vendor-neutral runtime boundary used by HQ.
package harness

import (
	"context"
	"fmt"
	"strings"
	"time"
)

type ProviderID string
type InstanceID string
type SessionID string
type OperationID string
type SubmissionID string
type RequestID string

type Capability string

const (
	CapabilityResume               Capability = "resume"
	CapabilitySteerActiveOperation Capability = "steer-active-operation"
	CapabilityInterrupt            Capability = "interrupt"
	CapabilityStructuredInput      Capability = "structured-input"
	CapabilityApprovals            Capability = "approvals"
	CapabilityPlans                Capability = "plans"
	CapabilityDiffs                Capability = "diffs"
	CapabilityToolLifecycle        Capability = "tool-lifecycle"
	CapabilityStreaming            Capability = "streaming"
	CapabilityIdempotentSubmission Capability = "idempotent-submission"
	CapabilitySubmissionLookup     Capability = "submission-lookup"
)

type Capabilities struct {
	Resume               bool
	SteerActiveOperation bool
	Interrupt            bool
	StructuredInput      bool
	Approvals            bool
	Plans                bool
	Diffs                bool
	ToolLifecycle        bool
	Streaming            bool
	IdempotentSubmission bool
	SubmissionLookup     bool
}

func (c Capabilities) Supports(capability Capability) bool {
	switch capability {
	case CapabilityResume:
		return c.Resume
	case CapabilitySteerActiveOperation:
		return c.SteerActiveOperation
	case CapabilityInterrupt:
		return c.Interrupt
	case CapabilityStructuredInput:
		return c.StructuredInput
	case CapabilityApprovals:
		return c.Approvals
	case CapabilityPlans:
		return c.Plans
	case CapabilityDiffs:
		return c.Diffs
	case CapabilityToolLifecycle:
		return c.ToolLifecycle
	case CapabilityStreaming:
		return c.Streaming
	case CapabilityIdempotentSubmission:
		return c.IdempotentSubmission
	case CapabilitySubmissionLookup:
		return c.SubmissionLookup
	default:
		return false
	}
}

type Provider struct {
	ID           ProviderID
	DisplayName  string
	Capabilities Capabilities
}

// ProviderOptions is implemented by a provider's typed launch configuration.
// Generic orchestration may carry it but must not inspect its concrete fields.
type ProviderOptions interface {
	Provider() ProviderID
}

type SessionMode string

const (
	SessionNew    SessionMode = "new"
	SessionResume SessionMode = "resume"
)

type LaunchConfig struct {
	InstanceID InstanceID
	AgentName  string
	Directory  string
	// Environment is sensitive transient input. A factory that retains it
	// beyond Launch must copy it, must never persist or log values, and must
	// clear its copy when the logical instance no longer needs it.
	Environment      []string
	SessionMode      SessionMode
	RequestedSession SessionID
	Options          ProviderOptions
}

func (c LaunchConfig) Validate(provider Provider) error {
	if strings.TrimSpace(string(c.InstanceID)) == "" {
		return fmt.Errorf("instance ID is required")
	}
	if strings.TrimSpace(c.AgentName) == "" {
		return fmt.Errorf("agent name is required")
	}
	if strings.TrimSpace(c.Directory) == "" {
		return fmt.Errorf("working directory is required")
	}
	switch c.SessionMode {
	case SessionNew:
		if c.RequestedSession != "" {
			return fmt.Errorf("new session cannot include a requested session ID")
		}
	case SessionResume:
		if c.RequestedSession == "" {
			return fmt.Errorf("resumed session ID is required")
		}
		if !provider.Capabilities.Resume {
			return NewCapabilityError(provider.ID, CapabilityResume)
		}
	default:
		return fmt.Errorf("invalid session mode %q", c.SessionMode)
	}
	if c.Options != nil && c.Options.Provider() != provider.ID {
		return fmt.Errorf("options for provider %q cannot launch provider %q", c.Options.Provider(), provider.ID)
	}
	return nil
}

type RuntimePhase string

const (
	RuntimeStarting RuntimePhase = "starting"
	RuntimeRunning  RuntimePhase = "running"
	RuntimeStopping RuntimePhase = "stopping"
	RuntimeStopped  RuntimePhase = "stopped"
	RuntimeFailed   RuntimePhase = "failed"
)

type RuntimeState struct {
	Phase RuntimePhase
	Since time.Time
	Err   error
}

type SessionIdentity struct {
	Provider ProviderID
	ID       SessionID
}

type Factory interface {
	Provider() Provider
	// Launch returns only after the logical instance has a validated session
	// identity and is ready to accept submissions.
	Launch(context.Context, LaunchConfig) (Instance, error)
}

type Instance interface {
	ID() InstanceID
	Provider() ProviderID
	State() RuntimeState
	Session() Session
	// Events returns one stream whose Sequence values are strictly increasing
	// from one for this logical instance. It closes before Wait returns.
	Events() <-chan Event
	// Requests is independent from Events so a blocking interactive request
	// cannot prevent event ingestion. It closes before Wait returns.
	Requests() <-chan Request
	// Shutdown is safe to call concurrently and initiates orderly teardown.
	Shutdown(context.Context) error
	// Wait returns the terminal provider failure, or nil after clean shutdown.
	Wait(context.Context) error
}

type Session interface {
	Identity() SessionIdentity
	Submit(context.Context, Submission) (DeliveryResult, error)
	// Respond completes a pending request exactly once. A duplicate response
	// must return ErrRequestCompleted without invoking the provider again.
	Respond(context.Context, Response) error
}

// SubmissionReconciler is mandatory when a provider does not guarantee
// idempotent submission for repeated stable Submission IDs.
type SubmissionReconciler interface {
	Reconcile(context.Context, SubmissionID) (RecoveryResult, error)
}

type ActiveOperationSubmitter interface {
	SubmitToActive(context.Context, OperationID, Submission) (DeliveryResult, error)
}

type Interrupter interface {
	Interrupt(context.Context, OperationID) error
}

type InputPart interface {
	isInputPart()
}

type TextInput struct {
	Text string
}

func (TextInput) isInputPart() {}

type StructuredInput struct {
	MediaType string
	Data      []byte
}

func (StructuredInput) isInputPart() {}

type Submission struct {
	ID    SubmissionID
	Input []InputPart
}

func (s Submission) Validate(provider Provider) error {
	if strings.TrimSpace(string(s.ID)) == "" {
		return fmt.Errorf("submission ID is required")
	}
	if len(s.Input) == 0 {
		return fmt.Errorf("submission input is required")
	}
	for _, part := range s.Input {
		switch value := part.(type) {
		case TextInput:
			if strings.TrimSpace(value.Text) == "" {
				return fmt.Errorf("text input is empty")
			}
		case StructuredInput:
			if !provider.Capabilities.StructuredInput {
				return NewCapabilityError(provider.ID, CapabilityStructuredInput)
			}
			if strings.TrimSpace(value.MediaType) == "" || len(value.Data) == 0 {
				return fmt.Errorf("structured input requires media type and data")
			}
		default:
			return fmt.Errorf("unsupported input type %T", part)
		}
	}
	return nil
}

type DeliveryState string

const (
	// DeliveryRejected proves the submission was not accepted and may be
	// attempted again.
	DeliveryRejected DeliveryState = "rejected"
	// DeliveryAccepted proves one operation accepted the stable submission ID.
	DeliveryAccepted DeliveryState = "accepted"
	// DeliveryUncertain requires lookup before retry, unless repeated Submit
	// with the same ID is guaranteed idempotent by the provider.
	DeliveryUncertain DeliveryState = "uncertain"
)

type DeliveryResult struct {
	State       DeliveryState
	OperationID OperationID
}

// Validate enforces the delivery result carried even when a call also returns
// context cancellation or a transport error. Unknown acceptance is never the
// zero value: it must be DeliveryUncertain.
func (r DeliveryResult) Validate() error {
	switch r.State {
	case DeliveryRejected:
		if r.OperationID != "" {
			return fmt.Errorf("rejected delivery cannot identify an operation")
		}
	case DeliveryAccepted:
		if r.OperationID == "" {
			return fmt.Errorf("accepted delivery requires an operation ID")
		}
	case DeliveryUncertain:
	default:
		return fmt.Errorf("invalid delivery state %q", r.State)
	}
	return nil
}

type RecoveryState string

const (
	// RecoveryNotFound proves that the stable submission ID is absent and may
	// be submitted again.
	RecoveryNotFound RecoveryState = "not-found"
	// RecoveryAccepted proves that the stable submission ID belongs to the
	// returned operation.
	RecoveryAccepted RecoveryState = "accepted"
)

type RecoveryResult struct {
	State       RecoveryState
	OperationID OperationID
}

func (r RecoveryResult) Validate() error {
	switch r.State {
	case RecoveryNotFound:
		if r.OperationID != "" {
			return fmt.Errorf("missing submission cannot identify an operation")
		}
	case RecoveryAccepted:
		if r.OperationID == "" {
			return fmt.Errorf("accepted recovery requires an operation ID")
		}
	default:
		return fmt.Errorf("invalid recovery state %q", r.State)
	}
	return nil
}

type Event struct {
	// Sequence is scoped to one logical instance and starts at one.
	Sequence   uint64
	Session    SessionIdentity
	Operation  OperationID
	ItemID     string
	OccurredAt time.Time
	Payload    EventPayload
}

type EventPayload interface {
	isEventPayload()
}

type OperationStatus string

const (
	OperationRunning     OperationStatus = "running"
	OperationCompleted   OperationStatus = "completed"
	OperationFailed      OperationStatus = "failed"
	OperationInterrupted OperationStatus = "interrupted"
)

type OperationStatusEvent struct {
	Status OperationStatus
	Error  string
}

func (OperationStatusEvent) isEventPayload() {}

type OutputEvent struct {
	Text  string
	Final bool
}

func (OutputEvent) isEventPayload() {}

type ProgressEvent struct {
	Message string
}

func (ProgressEvent) isEventPayload() {}

type Request struct {
	ID        RequestID
	Session   SessionIdentity
	Operation OperationID
	ItemID    string
	Payload   RequestPayload
}

type RequestPayload interface {
	isRequestPayload()
}

type QuestionOption struct {
	Label       string
	Description string
}

type QuestionRequest struct {
	Prompt     string
	Options    []QuestionOption
	AllowOther bool
	Secret     bool
}

func (QuestionRequest) isRequestPayload() {}

type ApprovalRequest struct {
	Kind       string
	Summary    string
	Choices    []string
	Persistent bool
}

func (ApprovalRequest) isRequestPayload() {}

type Response struct {
	RequestID RequestID
	Payload   ResponsePayload
}

type ResponsePayload interface {
	isResponsePayload()
}

type AnswerResponse struct {
	Answers []string
}

func (AnswerResponse) isResponsePayload() {}

type DecisionResponse struct {
	Decision string
}

func (DecisionResponse) isResponsePayload() {}

type CancelResponse struct {
	Reason string
}

func (CancelResponse) isResponsePayload() {}
