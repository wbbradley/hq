package harness

import (
	"errors"
	"fmt"
)

var (
	ErrUnknownProvider       = errors.New("unknown harness provider")
	ErrProviderUnavailable   = errors.New("harness provider unavailable")
	ErrCapabilityUnavailable = errors.New("harness capability unavailable")
	ErrSessionNotFound       = errors.New("harness session not found")
	ErrOperationMismatch     = errors.New("harness operation mismatch")
	ErrRequestNotFound       = errors.New("interactive request not found")
	ErrRequestCompleted      = errors.New("interactive request already completed")
	ErrInstanceStopped       = errors.New("harness instance stopped")
)

type ProviderError struct {
	Provider   ProviderID
	Capability Capability
	Operation  string
	Cause      error
}

func (e *ProviderError) Error() string {
	switch {
	case e.Capability != "":
		return fmt.Sprintf("harness provider %q does not support %s", e.Provider, e.Capability)
	case e.Operation != "" && e.Cause != nil:
		return fmt.Sprintf("harness provider %q could not %s: %v", e.Provider, e.Operation, e.Cause)
	case e.Cause != nil:
		return fmt.Sprintf("harness provider %q: %v", e.Provider, e.Cause)
	default:
		return fmt.Sprintf("harness provider %q failed", e.Provider)
	}
}

func (e *ProviderError) Unwrap() error { return e.Cause }

func NewCapabilityError(provider ProviderID, capability Capability) error {
	return &ProviderError{Provider: provider, Capability: capability, Cause: ErrCapabilityUnavailable}
}

type RuntimeError struct {
	Provider  ProviderID
	Session   SessionID
	Operation OperationID
	Action    string
	Cause     error
}

func (e *RuntimeError) Error() string {
	return fmt.Sprintf("harness %q session %q %s: %v", e.Provider, e.Session, e.Action, e.Cause)
}

func (e *RuntimeError) Unwrap() error { return e.Cause }
