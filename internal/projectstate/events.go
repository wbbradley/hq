package projectstate

import (
	"encoding/json"
	"fmt"
	"time"

	"github.com/wbbradley/hq/internal/domain"
)

type Operation string

const (
	OperationCreated                Operation = "project.created"
	OperationOpened                 Operation = "project.opened"
	OperationClosing                Operation = "project.closing"
	OperationClosed                 Operation = "project.closed"
	OperationArchived               Operation = "project.archived"
	OperationUnarchived             Operation = "project.unarchived"
	OperationMetadataUpdated        Operation = "project.metadata.updated"
	OperationResourceAdded          Operation = "project.resource.added"
	OperationResourceRemoved        Operation = "project.resource.removed"
	OperationResourceReplaced       Operation = "project.resource.replaced"
	OperationPrimaryResourceChanged Operation = "project.primary-resource.changed"
	OperationResourceHealth         Operation = "project.resource.health"
	OperationAssignmentConfiguring  Operation = "project.assignment.configuring"
	OperationAssignmentRunnable     Operation = "project.assignment.runnable"
	OperationAssignmentBlocked      Operation = "project.assignment.blocked"
	OperationAssignmentEnded        Operation = "project.assignment.ended"
	OperationMessageAccepted        Operation = "project.message.accepted"
	OperationMessageDispatched      Operation = "project.message.dispatched"
)

var supportedOperations = []Operation{
	OperationCreated,
	OperationOpened,
	OperationClosing,
	OperationClosed,
	OperationArchived,
	OperationUnarchived,
	OperationMetadataUpdated,
	OperationResourceAdded,
	OperationResourceRemoved,
	OperationResourceReplaced,
	OperationPrimaryResourceChanged,
	OperationResourceHealth,
	OperationAssignmentConfiguring,
	OperationAssignmentRunnable,
	OperationAssignmentBlocked,
	OperationAssignmentEnded,
	OperationMessageAccepted,
	OperationMessageDispatched,
}

func SupportedOperations() []Operation {
	return append([]Operation(nil), supportedOperations...)
}

type Data interface {
	Operation() Operation
	projectEventData()
}

type CreatedResource struct {
	ID               string                     `json:"id"`
	Kind             string                     `json:"kind"`
	DisplayLocator   string                     `json:"display_locator"`
	CanonicalLocator string                     `json:"canonical_locator"`
	Health           domain.ResourceHealthState `json:"health"`
	HealthDetails    map[string]string          `json:"health_details"`
	LastCheckedAt    time.Time                  `json:"last_checked_at"`
}

type Created struct {
	Request     domain.CreateProjectRequest `json:"request"`
	MailboxID   string                      `json:"mailbox_id"`
	ResourceIDs []string                    `json:"resource_ids,omitempty"`
	Resources   []CreatedResource           `json:"resources"`
}

func (Created) Operation() Operation { return OperationCreated }
func (Created) projectEventData()    {}

type Opened struct{}

func (Opened) Operation() Operation { return OperationOpened }
func (Opened) projectEventData()    {}

type Closing struct{}

func (Closing) Operation() Operation { return OperationClosing }
func (Closing) projectEventData()    {}

type Closed struct {
	Forced             bool   `json:"forced"`
	RuntimeObservation string `json:"runtime_observation,omitempty"`
}

func (Closed) Operation() Operation { return OperationClosed }
func (Closed) projectEventData()    {}

type Archived struct{}

func (Archived) Operation() Operation { return OperationArchived }
func (Archived) projectEventData()    {}

type Unarchived struct{}

func (Unarchived) Operation() Operation { return OperationUnarchived }
func (Unarchived) projectEventData()    {}

type MetadataUpdated struct {
	Name  string `json:"name"`
	Brief string `json:"brief"`
}

func (MetadataUpdated) Operation() Operation { return OperationMetadataUpdated }
func (MetadataUpdated) projectEventData()    {}

type ResourceAdded struct {
	ResourceID       string                     `json:"resource_id"`
	Kind             string                     `json:"kind"`
	DisplayLocator   string                     `json:"display_locator"`
	CanonicalLocator string                     `json:"canonical_locator"`
	Primary          bool                       `json:"primary"`
	Health           domain.ResourceHealthState `json:"health"`
	HealthDetails    map[string]string          `json:"health_details"`
	LastCheckedAt    time.Time                  `json:"last_checked_at"`
}

func (ResourceAdded) Operation() Operation { return OperationResourceAdded }
func (ResourceAdded) projectEventData()    {}

type ResourceRemoved struct {
	ResourceID string `json:"resource_id"`
	Assigned   bool   `json:"assigned,omitempty"`
}

func (ResourceRemoved) Operation() Operation { return OperationResourceRemoved }
func (ResourceRemoved) projectEventData()    {}

type ResourceReplaced struct {
	OldResourceID    string                     `json:"old_resource_id"`
	NewResourceID    string                     `json:"new_resource_id"`
	DisplayLocator   string                     `json:"display_locator"`
	CanonicalLocator string                     `json:"canonical_locator"`
	Health           domain.ResourceHealthState `json:"health"`
	HealthDetails    map[string]string          `json:"health_details"`
	LastCheckedAt    time.Time                  `json:"last_checked_at"`
}

func (ResourceReplaced) Operation() Operation { return OperationResourceReplaced }
func (ResourceReplaced) projectEventData()    {}

type PrimaryResourceChanged struct {
	ResourceID string `json:"resource_id"`
}

func (PrimaryResourceChanged) Operation() Operation { return OperationPrimaryResourceChanged }
func (PrimaryResourceChanged) projectEventData()    {}

type ResourceHealth struct {
	ResourceID    string                     `json:"resource_id"`
	Health        domain.ResourceHealthState `json:"health"`
	HealthDetails map[string]string          `json:"health_details"`
	LastCheckedAt time.Time                  `json:"last_checked_at"`
}

func (ResourceHealth) Operation() Operation { return OperationResourceHealth }
func (ResourceHealth) projectEventData()    {}

type AssignmentConfiguring struct {
	AssignmentID string `json:"assignment_id"`
	Agent        string `json:"agent"`
}

func (AssignmentConfiguring) Operation() Operation { return OperationAssignmentConfiguring }
func (AssignmentConfiguring) projectEventData()    {}

type AssignmentRunnable struct {
	AssignmentID string `json:"assignment_id"`
	Agent        string `json:"agent"`
	ThreadID     string `json:"thread_id"`
}

func (AssignmentRunnable) Operation() Operation { return OperationAssignmentRunnable }
func (AssignmentRunnable) projectEventData()    {}

type AssignmentBlocked struct {
	AssignmentID string `json:"assignment_id"`
	Agent        string `json:"agent"`
	Diagnostic   string `json:"diagnostic,omitempty"`
}

func (AssignmentBlocked) Operation() Operation { return OperationAssignmentBlocked }
func (AssignmentBlocked) projectEventData()    {}

type AssignmentEnded struct {
	AssignmentID       string `json:"assignment_id"`
	Agent              string `json:"agent"`
	Forced             bool   `json:"forced"`
	RuntimeObservation string `json:"runtime_observation,omitempty"`
}

func (AssignmentEnded) Operation() Operation { return OperationAssignmentEnded }
func (AssignmentEnded) projectEventData()    {}

type MessageAccepted struct {
	MessageID      string `json:"message_id"`
	MessageEventID string `json:"message_event_id"`
	Sequence       int64  `json:"sequence"`
}

func (MessageAccepted) Operation() Operation { return OperationMessageAccepted }
func (MessageAccepted) projectEventData()    {}

type MessageDispatched struct {
	MessageID        string `json:"message_id"`
	Sequence         int64  `json:"sequence"`
	AssignmentID     string `json:"assignment_id"`
	Agent            string `json:"agent"`
	ProjectThreadID  string `json:"project_thread_id"`
	ExternalThreadID string `json:"external_thread_id"`
}

func (MessageDispatched) Operation() Operation { return OperationMessageDispatched }
func (MessageDispatched) projectEventData()    {}

type auditEnvelope struct {
	RequestID string          `json:"request_id,omitempty"`
	Data      json.RawMessage `json:"data"`
}

func MarshalData(data Data) ([]byte, error) {
	if data == nil {
		return nil, fmt.Errorf("project event data is required")
	}
	return json.Marshal(data)
}

func DecodeAudit(operation string, body json.RawMessage) (Data, error) {
	op := Operation(operation)
	var envelope auditEnvelope
	if err := json.Unmarshal(body, &envelope); err != nil {
		return nil, fmt.Errorf("decode project event audit envelope: %w", err)
	}
	if len(envelope.Data) == 0 {
		return nil, fmt.Errorf("project event %q has no data", op)
	}
	var data Data
	switch op {
	case OperationCreated:
		data = &Created{}
	case OperationOpened:
		data = &Opened{}
	case OperationClosing:
		data = &Closing{}
	case OperationClosed:
		data = &Closed{}
	case OperationArchived:
		data = &Archived{}
	case OperationUnarchived:
		data = &Unarchived{}
	case OperationMetadataUpdated:
		data = &MetadataUpdated{}
	case OperationResourceAdded:
		data = &ResourceAdded{}
	case OperationResourceRemoved:
		data = &ResourceRemoved{}
	case OperationResourceReplaced:
		data = &ResourceReplaced{}
	case OperationPrimaryResourceChanged:
		data = &PrimaryResourceChanged{}
	case OperationResourceHealth:
		data = &ResourceHealth{}
	case OperationAssignmentConfiguring:
		data = &AssignmentConfiguring{}
	case OperationAssignmentRunnable:
		data = &AssignmentRunnable{}
	case OperationAssignmentBlocked:
		data = &AssignmentBlocked{}
	case OperationAssignmentEnded:
		data = &AssignmentEnded{}
	case OperationMessageAccepted:
		data = &MessageAccepted{}
	case OperationMessageDispatched:
		data = &MessageDispatched{}
	default:
		return nil, fmt.Errorf("unsupported project event operation %q", operation)
	}
	if err := json.Unmarshal(envelope.Data, data); err != nil {
		return nil, fmt.Errorf("decode project event %q data: %w", op, err)
	}
	return data, nil
}
