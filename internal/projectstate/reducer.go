package projectstate

import (
	"fmt"
	"time"

	"github.com/wbbradley/hq/internal/domain"
)

type Event struct {
	ID               string
	ProjectID        string
	HomeInstallation string
	PreviousEventID  string
	CreatedAt        time.Time
	Data             Data
}

type Acceptance struct {
	MessageID      string
	MessageEventID string
	Sequence       int64
	EventID        string
	AcceptedAt     time.Time
}

type Dispatch struct {
	MessageID        string
	Sequence         int64
	AssignmentID     string
	Agent            string
	ProjectThreadID  string
	ExternalThreadID string
	EventID          string
	DispatchedAt     time.Time
}

type ResourceProjection struct {
	Resource       domain.ProjectResource
	AddedEventID   string
	RemovedEventID string
	CreatedAt      time.Time
	RemovedAt      time.Time
}

type ClaimProjection struct {
	ResourceID      string
	AcquiredEventID string
	ReleasedEventID string
	AcquiredAt      time.Time
	ReleasedAt      time.Time
}

type AssignmentProjection struct {
	Assignment     domain.ProjectAssignment
	StartedEventID string
	EndedEventID   string
	Forced         bool
}

type ThreadProjection struct {
	ID               string
	ProjectID        string
	Agent            string
	Harness          string
	ExternalThreadID string
	LaunchDirectory  string
	CreatedAt        time.Time
}

type Snapshot struct {
	Project     domain.Project
	Resources   []ResourceProjection
	Claims      []ClaimProjection
	Assignments []AssignmentProjection
	Threads     []ThreadProjection
	Acceptances []Acceptance
	Dispatches  []Dispatch
}

func Apply(current Snapshot, item Event) (Snapshot, error) {
	if item.Data == nil {
		return current, fmt.Errorf("project event %q has no typed data", item.ID)
	}
	next := cloneSnapshot(current)
	if item.Data.Operation() == OperationCreated {
		if next.Project.ID != "" || item.PreviousEventID != "" {
			return current, fmt.Errorf("project creation must be the unique root")
		}
		var data Created
		switch value := item.Data.(type) {
		case Created:
			data = value
		case *Created:
			data = *value
		default:
			return current, fmt.Errorf("project creation has data type %T", item.Data)
		}
		if item.ProjectID == "" || item.HomeInstallation == "" || data.MailboxID == "" || data.Request.Name == "" {
			return current, fmt.Errorf("project creation data is incomplete")
		}
		lifecycle := domain.ProjectClosed
		if data.Request.Open {
			lifecycle = domain.ProjectOpen
		}
		next.Project = domain.Project{ID: item.ProjectID, HomeInstallation: item.HomeInstallation, MailboxID: data.MailboxID, PredecessorProjectID: data.Request.PredecessorProjectID, Name: data.Request.Name, Brief: data.Request.Brief, Lifecycle: lifecycle, HeadEventID: item.ID, CreatedAt: item.CreatedAt, UpdatedAt: item.CreatedAt}
		for _, resource := range data.Resources {
			checked := resource.LastCheckedAt
			projectResource := domain.ProjectResource{ID: resource.ID, Kind: resource.Kind, HomeInstallation: item.HomeInstallation, DisplayLocator: resource.DisplayLocator, CanonicalLocator: resource.CanonicalLocator, Health: resource.Health, HealthDetails: cloneMap(resource.HealthDetails), LastCheckedAt: &checked}
			next.Project.Resources = append(next.Project.Resources, projectResource)
			next.Resources = append(next.Resources, ResourceProjection{Resource: projectResource, AddedEventID: item.ID, CreatedAt: item.CreatedAt})
			if data.Request.Open {
				next.Claims = append(next.Claims, ClaimProjection{ResourceID: resource.ID, AcquiredEventID: item.ID, AcquiredAt: item.CreatedAt})
			}
		}
		if len(next.Project.Resources) > 0 {
			if data.Request.PrimaryPath < 0 || data.Request.PrimaryPath >= len(next.Project.Resources) {
				return current, fmt.Errorf("project creation primary path is out of range")
			}
			next.Project.PrimaryResourceID = next.Project.Resources[data.Request.PrimaryPath].ID
		}
		return next, nil
	}
	if next.Project.ID == "" {
		return current, fmt.Errorf("project event %q has no creation root", item.ID)
	}
	if item.ProjectID != next.Project.ID || item.HomeInstallation != next.Project.HomeInstallation {
		return current, fmt.Errorf("project event identity does not match its root")
	}
	if item.PreviousEventID != next.Project.HeadEventID {
		return current, fmt.Errorf("project event previous head %q does not match %q", item.PreviousEventID, next.Project.HeadEventID)
	}
	if err := applyData(&next, item); err != nil {
		return current, err
	}
	next.Project.HeadEventID, next.Project.UpdatedAt = item.ID, item.CreatedAt
	return next, nil
}

func applyData(next *Snapshot, item Event) error {
	switch data := item.Data.(type) {
	case *Opened:
		if next.Project.Lifecycle != domain.ProjectClosed || next.Project.Archived {
			return fmt.Errorf("open requires an unarchived closed project")
		}
		next.Project.Lifecycle = domain.ProjectOpen
		for _, resource := range next.Project.Resources {
			next.Claims = append(next.Claims, ClaimProjection{ResourceID: resource.ID, AcquiredEventID: item.ID, AcquiredAt: item.CreatedAt})
		}
	case *Closing:
		if next.Project.Lifecycle != domain.ProjectOpen {
			return fmt.Errorf("closing requires an open project")
		}
		next.Project.Lifecycle = domain.ProjectClosing
	case *Closed:
		if next.Project.Lifecycle != domain.ProjectClosing {
			return fmt.Errorf("closed requires a closing project")
		}
		if next.Project.Assignment != nil {
			next.Project.SuggestedAgentName = next.Project.Assignment.AgentName
			ended := item.CreatedAt
			next.Project.Assignment.State, next.Project.Assignment.EndedAt = domain.AssignmentEnded, &ended
			next.Assignments[len(next.Assignments)-1].Assignment = *next.Project.Assignment
			next.Assignments[len(next.Assignments)-1].EndedEventID, next.Assignments[len(next.Assignments)-1].Forced = item.ID, data.Forced
		}
		next.Project.Lifecycle, next.Project.Assignment = domain.ProjectClosed, nil
		releaseClaims(next, item)
	case *Archived:
		if next.Project.Lifecycle != domain.ProjectClosed || next.Project.Archived {
			return fmt.Errorf("archive requires an unarchived closed project")
		}
		next.Project.Archived = true
	case *Unarchived:
		if next.Project.Lifecycle != domain.ProjectClosed || !next.Project.Archived {
			return fmt.Errorf("unarchive requires an archived closed project")
		}
		next.Project.Archived = false
	case *MetadataUpdated:
		if data.Name == "" {
			return fmt.Errorf("project metadata name is required")
		}
		next.Project.Name, next.Project.Brief = data.Name, data.Brief
	case *ResourceAdded:
		if data.ResourceID == "" || data.Kind == "" || data.CanonicalLocator == "" || findResource(next.Project.Resources, data.ResourceID) >= 0 {
			return fmt.Errorf("project resource addition is invalid")
		}
		checked := data.LastCheckedAt
		next.Project.Resources = append(next.Project.Resources, domain.ProjectResource{ID: data.ResourceID, Kind: data.Kind, HomeInstallation: next.Project.HomeInstallation, DisplayLocator: data.DisplayLocator, CanonicalLocator: data.CanonicalLocator, Health: data.Health, HealthDetails: cloneMap(data.HealthDetails), LastCheckedAt: &checked})
		next.Resources = append(next.Resources, ResourceProjection{Resource: next.Project.Resources[len(next.Project.Resources)-1], AddedEventID: item.ID, CreatedAt: item.CreatedAt})
		if next.Project.Lifecycle == domain.ProjectOpen {
			next.Claims = append(next.Claims, ClaimProjection{ResourceID: data.ResourceID, AcquiredEventID: item.ID, AcquiredAt: item.CreatedAt})
		}
		if data.Primary {
			next.Project.PrimaryResourceID = data.ResourceID
		}
	case *ResourceRemoved:
		index := findResource(next.Project.Resources, data.ResourceID)
		if index < 0 {
			return fmt.Errorf("removed project resource %q does not exist", data.ResourceID)
		}
		next.Project.Resources = append(next.Project.Resources[:index], next.Project.Resources[index+1:]...)
		projectionIndex := findCurrentResourceProjection(next.Resources, data.ResourceID)
		next.Resources[projectionIndex].RemovedEventID, next.Resources[projectionIndex].RemovedAt = item.ID, item.CreatedAt
		releaseResourceClaims(next, data.ResourceID, item)
		if next.Project.PrimaryResourceID == data.ResourceID {
			next.Project.PrimaryResourceID = ""
			if len(next.Project.Resources) > 0 {
				next.Project.PrimaryResourceID = next.Project.Resources[0].ID
			}
		}
	case *ResourceReplaced:
		index := findResource(next.Project.Resources, data.OldResourceID)
		if index < 0 || data.NewResourceID == "" || findResource(next.Project.Resources, data.NewResourceID) >= 0 {
			return fmt.Errorf("project resource replacement is invalid")
		}
		checked := data.LastCheckedAt
		next.Project.Resources[index] = domain.ProjectResource{ID: data.NewResourceID, Kind: "path", HomeInstallation: next.Project.HomeInstallation, DisplayLocator: data.DisplayLocator, CanonicalLocator: data.CanonicalLocator, Health: data.Health, HealthDetails: cloneMap(data.HealthDetails), LastCheckedAt: &checked}
		projectionIndex := findCurrentResourceProjection(next.Resources, data.OldResourceID)
		next.Resources[projectionIndex].RemovedEventID, next.Resources[projectionIndex].RemovedAt = item.ID, item.CreatedAt
		releaseResourceClaims(next, data.OldResourceID, item)
		next.Resources = append(next.Resources, ResourceProjection{Resource: next.Project.Resources[index], AddedEventID: item.ID, CreatedAt: item.CreatedAt})
		if next.Project.Lifecycle == domain.ProjectOpen {
			next.Claims = append(next.Claims, ClaimProjection{ResourceID: data.NewResourceID, AcquiredEventID: item.ID, AcquiredAt: item.CreatedAt})
		}
		if next.Project.PrimaryResourceID == data.OldResourceID {
			next.Project.PrimaryResourceID = data.NewResourceID
		}
	case *PrimaryResourceChanged:
		if findResource(next.Project.Resources, data.ResourceID) < 0 {
			return fmt.Errorf("primary project resource %q does not exist", data.ResourceID)
		}
		next.Project.PrimaryResourceID = data.ResourceID
	case *ResourceHealth:
		index := findResource(next.Project.Resources, data.ResourceID)
		if index < 0 {
			return fmt.Errorf("health project resource %q does not exist", data.ResourceID)
		}
		checked := data.LastCheckedAt
		next.Project.Resources[index].Health, next.Project.Resources[index].HealthDetails, next.Project.Resources[index].LastCheckedAt = data.Health, cloneMap(data.HealthDetails), &checked
		projectionIndex := findCurrentResourceProjection(next.Resources, data.ResourceID)
		next.Resources[projectionIndex].Resource = next.Project.Resources[index]
	case *AssignmentConfiguring:
		if next.Project.Lifecycle != domain.ProjectOpen || next.Project.Assignment != nil || data.AssignmentID == "" || data.Agent == "" {
			return fmt.Errorf("project assignment configuration is invalid")
		}
		next.Project.Assignment = &domain.ProjectAssignment{ID: data.AssignmentID, AgentName: data.Agent, State: domain.AssignmentConfiguring, StartedAt: item.CreatedAt}
		next.Assignments = append(next.Assignments, AssignmentProjection{Assignment: *next.Project.Assignment, StartedEventID: item.ID})
		next.Project.SuggestedAgentName = ""
	case *AssignmentRunnable:
		if next.Project.Assignment == nil || next.Project.Assignment.ID != data.AssignmentID || next.Project.Assignment.AgentName != data.Agent || next.Project.Assignment.State != domain.AssignmentConfiguring || data.ThreadID == "" {
			return fmt.Errorf("runnable project assignment does not match configuring assignment")
		}
		next.Project.Assignment.State, next.Project.Assignment.SelectedThreadID = domain.AssignmentRunnable, data.ThreadID
		next.Assignments[len(next.Assignments)-1].Assignment = *next.Project.Assignment
		if data.Thread != nil {
			if data.Thread.ID != data.ThreadID || data.Thread.Harness == "" || data.Thread.ExternalThreadID == "" || data.Thread.LaunchDirectory == "" {
				return fmt.Errorf("runnable project assignment thread data is invalid")
			}
			found := false
			for _, thread := range next.Threads {
				if thread.ID == data.Thread.ID {
					found = thread.ProjectID == next.Project.ID && thread.Agent == data.Agent
				}
			}
			if !found {
				createdAt := data.Thread.CreatedAt
				if createdAt.IsZero() {
					createdAt = item.CreatedAt
				}
				next.Threads = append(next.Threads, ThreadProjection{ID: data.Thread.ID, ProjectID: next.Project.ID, Agent: data.Agent, Harness: data.Thread.Harness, ExternalThreadID: data.Thread.ExternalThreadID, LaunchDirectory: data.Thread.LaunchDirectory, CreatedAt: createdAt})
			}
		}
	case *AssignmentBlocked:
		if next.Project.Assignment == nil || next.Project.Assignment.ID != data.AssignmentID || next.Project.Assignment.AgentName != data.Agent || next.Project.Assignment.State == domain.AssignmentBlocked {
			return fmt.Errorf("blocked project assignment does not match active assignment")
		}
		next.Project.Assignment.State = domain.AssignmentBlocked
		next.Assignments[len(next.Assignments)-1].Assignment = *next.Project.Assignment
	case *AssignmentEnded:
		if next.Project.Assignment == nil || next.Project.Assignment.ID != data.AssignmentID || next.Project.Assignment.AgentName != data.Agent {
			return fmt.Errorf("ended project assignment does not match active assignment")
		}
		ended := item.CreatedAt
		next.Project.Assignment.State, next.Project.Assignment.EndedAt = domain.AssignmentEnded, &ended
		next.Assignments[len(next.Assignments)-1].Assignment = *next.Project.Assignment
		next.Assignments[len(next.Assignments)-1].EndedEventID, next.Assignments[len(next.Assignments)-1].Forced = item.ID, data.Forced
		next.Project.SuggestedAgentName, next.Project.Assignment = next.Project.Assignment.AgentName, nil
	case *MessageAccepted:
		if data.MessageID == "" || data.MessageEventID == "" || data.Sequence != int64(len(next.Acceptances)+1) {
			return fmt.Errorf("project message acceptance sequence or identity is invalid")
		}
		for _, accepted := range next.Acceptances {
			if accepted.MessageID == data.MessageID || accepted.MessageEventID == data.MessageEventID {
				return fmt.Errorf("project message was accepted more than once")
			}
		}
		next.Acceptances = append(next.Acceptances, Acceptance{MessageID: data.MessageID, MessageEventID: data.MessageEventID, Sequence: data.Sequence, EventID: item.ID, AcceptedAt: item.CreatedAt})
	case *MessageDispatched:
		if data.MessageID == "" || data.AssignmentID == "" || data.ProjectThreadID == "" || data.ExternalThreadID == "" {
			return fmt.Errorf("project message dispatch data is incomplete")
		}
		found := false
		for _, accepted := range next.Acceptances {
			if accepted.MessageID == data.MessageID && accepted.Sequence == data.Sequence {
				found = true
			}
		}
		for _, dispatched := range next.Dispatches {
			if dispatched.MessageID == data.MessageID || dispatched.Sequence == data.Sequence {
				return fmt.Errorf("project message was dispatched more than once")
			}
		}
		if !found {
			return fmt.Errorf("project message dispatch has no matching acceptance")
		}
		next.Dispatches = append(next.Dispatches, Dispatch{MessageID: data.MessageID, Sequence: data.Sequence, AssignmentID: data.AssignmentID, Agent: data.Agent, ProjectThreadID: data.ProjectThreadID, ExternalThreadID: data.ExternalThreadID, EventID: item.ID, DispatchedAt: item.CreatedAt})
	default:
		return fmt.Errorf("unsupported typed project event %T", item.Data)
	}
	return nil
}

func cloneSnapshot(current Snapshot) Snapshot {
	next := current
	next.Project.Resources = append([]domain.ProjectResource(nil), current.Project.Resources...)
	next.Resources = append([]ResourceProjection(nil), current.Resources...)
	for index := range next.Resources {
		next.Resources[index].Resource.HealthDetails = cloneMap(next.Resources[index].Resource.HealthDetails)
	}
	next.Claims = append([]ClaimProjection(nil), current.Claims...)
	next.Assignments = append([]AssignmentProjection(nil), current.Assignments...)
	next.Threads = append([]ThreadProjection(nil), current.Threads...)
	for index := range next.Project.Resources {
		next.Project.Resources[index].HealthDetails = cloneMap(next.Project.Resources[index].HealthDetails)
		if current.Project.Resources[index].LastCheckedAt != nil {
			checked := *current.Project.Resources[index].LastCheckedAt
			next.Project.Resources[index].LastCheckedAt = &checked
		}
	}
	if current.Project.Assignment != nil {
		assignment := *current.Project.Assignment
		next.Project.Assignment = &assignment
	}
	next.Acceptances = append([]Acceptance(nil), current.Acceptances...)
	next.Dispatches = append([]Dispatch(nil), current.Dispatches...)
	return next
}

func findCurrentResourceProjection(resources []ResourceProjection, id string) int {
	for index := range resources {
		if resources[index].Resource.ID == id && resources[index].RemovedEventID == "" {
			return index
		}
	}
	return -1
}

func releaseClaims(next *Snapshot, item Event) {
	for index := range next.Claims {
		if next.Claims[index].ReleasedEventID == "" {
			next.Claims[index].ReleasedEventID, next.Claims[index].ReleasedAt = item.ID, item.CreatedAt
		}
	}
}

func releaseResourceClaims(next *Snapshot, resourceID string, item Event) {
	for index := range next.Claims {
		if next.Claims[index].ResourceID == resourceID && next.Claims[index].ReleasedEventID == "" {
			next.Claims[index].ReleasedEventID, next.Claims[index].ReleasedAt = item.ID, item.CreatedAt
		}
	}
}

func cloneMap(values map[string]string) map[string]string {
	if values == nil {
		return nil
	}
	result := make(map[string]string, len(values))
	for key, value := range values {
		result[key] = value
	}
	return result
}

func findResource(resources []domain.ProjectResource, id string) int {
	for index := range resources {
		if resources[index].ID == id {
			return index
		}
	}
	return -1
}
