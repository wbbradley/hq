package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"sort"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
)

type replicaProjectEvent struct {
	id      string
	home    string
	created time.Time
	payload event.ProjectEventPayload
}

func (s *SQLite) rebuildProjectReplicasTx(ctx context.Context, tx *sql.Tx, state event.State) error {
	if _, err := tx.ExecContext(ctx, `DELETE FROM project_replicas`); err != nil {
		return err
	}
	groups := make(map[string][]replicaProjectEvent)
	for id, record := range state.Records {
		if record.Status != event.StatusProjected || record.Event.Content.Type != event.TypeProjectEvent {
			continue
		}
		var payload event.ProjectEventPayload
		if json.Unmarshal(record.Event.Content.Payload, &payload) != nil {
			continue
		}
		groups[payload.ProjectID] = append(groups[payload.ProjectID], replicaProjectEvent{id: id, home: record.Event.Content.InstallationID, created: time.Unix(record.Event.Nostr.CreatedAt, 0).UTC(), payload: payload})
	}
	for projectID, events := range groups {
		project, ok := reduceReplicaProject(projectID, events)
		if !ok || project.HomeInstallation == s.signer.InstallationID {
			continue
		}
		raw, err := json.Marshal(project)
		if err != nil {
			return err
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_replicas(id,home_installation_id,head_event_id,state_json,updated_at) VALUES (?,?,?,?,?)`, project.ID, project.HomeInstallation, project.HeadEventID, raw, project.UpdatedAt.UnixMilli()); err != nil {
			return err
		}
	}
	return nil
}

func reduceReplicaProject(projectID string, events []replicaProjectEvent) (domain.Project, bool) {
	sort.Slice(events, func(i, j int) bool {
		if events[i].created.Equal(events[j].created) {
			return events[i].id < events[j].id
		}
		return events[i].created.Before(events[j].created)
	})
	var creation *replicaProjectEvent
	creationCount := 0
	for index := range events {
		if events[index].payload.Operation == "project.created" && events[index].payload.PreviousEventID == "" {
			creation, creationCount = &events[index], creationCount+1
		}
	}
	if creation == nil || creationCount != 1 {
		return domain.Project{}, false
	}
	var audit struct {
		Data struct {
			Request   domain.CreateProjectRequest `json:"request"`
			MailboxID string                      `json:"mailbox_id"`
			Resources []struct {
				ID               string                     `json:"id"`
				Kind             string                     `json:"kind"`
				DisplayLocator   string                     `json:"display_locator"`
				CanonicalLocator string                     `json:"canonical_locator"`
				Health           domain.ResourceHealthState `json:"health"`
				HealthDetails    map[string]string          `json:"health_details"`
				LastCheckedAt    time.Time                  `json:"last_checked_at"`
			} `json:"resources"`
		} `json:"data"`
	}
	if json.Unmarshal(creation.payload.Body, &audit) != nil || audit.Data.MailboxID == "" {
		return domain.Project{}, false
	}
	lifecycle := domain.ProjectClosed
	if audit.Data.Request.Open {
		lifecycle = domain.ProjectOpen
	}
	project := domain.Project{ID: projectID, HomeInstallation: creation.home, MailboxID: audit.Data.MailboxID, PredecessorProjectID: audit.Data.Request.PredecessorProjectID, Name: audit.Data.Request.Name, Brief: audit.Data.Request.Brief, Lifecycle: lifecycle, HeadEventID: creation.id, CreatedAt: creation.created, UpdatedAt: creation.created, ReadOnlyReplica: true}
	for _, resource := range audit.Data.Resources {
		checked := resource.LastCheckedAt
		project.Resources = append(project.Resources, domain.ProjectResource{ID: resource.ID, Kind: resource.Kind, HomeInstallation: creation.home, DisplayLocator: resource.DisplayLocator, CanonicalLocator: resource.CanonicalLocator, Health: resource.Health, HealthDetails: resource.HealthDetails, LastCheckedAt: &checked})
	}
	if len(project.Resources) > 0 && audit.Data.Request.PrimaryPath >= 0 && audit.Data.Request.PrimaryPath < len(project.Resources) {
		project.PrimaryResourceID = project.Resources[audit.Data.Request.PrimaryPath].ID
	}
	children := make(map[string][]replicaProjectEvent)
	for _, item := range events {
		if item.home == project.HomeInstallation && item.payload.PreviousEventID != "" {
			children[item.payload.PreviousEventID] = append(children[item.payload.PreviousEventID], item)
		}
	}
	for {
		candidates := children[project.HeadEventID]
		if len(candidates) == 0 {
			break
		}
		if len(candidates) != 1 {
			// A home fork is never guessed through. Retain the last
			// unambiguous projection until authoritative repair arrives.
			break
		}
		sort.Slice(candidates, func(i, j int) bool { return candidates[i].id < candidates[j].id })
		item := candidates[0]
		applyReplicaProjectEvent(&project, item)
		project.HeadEventID, project.UpdatedAt = item.id, item.created
	}
	return project, true
}

func applyReplicaProjectEvent(project *domain.Project, item replicaProjectEvent) {
	var audit struct {
		Data json.RawMessage `json:"data"`
	}
	if json.Unmarshal(item.payload.Body, &audit) != nil {
		return
	}
	switch item.payload.Operation {
	case "project.opened":
		project.Lifecycle, project.Archived = domain.ProjectOpen, false
	case "project.closing":
		project.Lifecycle = domain.ProjectClosing
	case "project.closed":
		if project.Assignment != nil {
			project.SuggestedAgentName = project.Assignment.AgentName
		}
		project.Lifecycle, project.Assignment = domain.ProjectClosed, nil
	case "project.archived":
		project.Archived = true
	case "project.unarchived":
		project.Archived = false
	case "project.metadata.updated":
		var data struct{ Name, Brief string }
		if json.Unmarshal(audit.Data, &data) == nil {
			project.Name, project.Brief = data.Name, data.Brief
		}
	case "project.resource.added":
		var data struct {
			ResourceID       string                     `json:"resource_id"`
			Kind             string                     `json:"kind"`
			DisplayLocator   string                     `json:"display_locator"`
			CanonicalLocator string                     `json:"canonical_locator"`
			Primary          bool                       `json:"primary"`
			Health           domain.ResourceHealthState `json:"health"`
			HealthDetails    map[string]string          `json:"health_details"`
			LastCheckedAt    time.Time                  `json:"last_checked_at"`
		}
		if json.Unmarshal(audit.Data, &data) == nil {
			checked := data.LastCheckedAt
			project.Resources = append(project.Resources, domain.ProjectResource{ID: data.ResourceID, Kind: data.Kind, HomeInstallation: project.HomeInstallation, DisplayLocator: data.DisplayLocator, CanonicalLocator: data.CanonicalLocator, Health: data.Health, HealthDetails: data.HealthDetails, LastCheckedAt: &checked})
			if data.Primary {
				project.PrimaryResourceID = data.ResourceID
			}
		}
	case "project.resource.removed":
		var data struct {
			ResourceID string `json:"resource_id"`
		}
		if json.Unmarshal(audit.Data, &data) == nil {
			project.Resources = removeReplicaResource(project.Resources, data.ResourceID)
			if project.PrimaryResourceID == data.ResourceID {
				project.PrimaryResourceID = ""
				if len(project.Resources) != 0 {
					project.PrimaryResourceID = project.Resources[0].ID
				}
			}
		}
	case "project.resource.replaced":
		var data struct {
			OldResourceID    string                     `json:"old_resource_id"`
			NewResourceID    string                     `json:"new_resource_id"`
			DisplayLocator   string                     `json:"display_locator"`
			CanonicalLocator string                     `json:"canonical_locator"`
			Health           domain.ResourceHealthState `json:"health"`
			HealthDetails    map[string]string          `json:"health_details"`
			LastCheckedAt    time.Time                  `json:"last_checked_at"`
		}
		if json.Unmarshal(audit.Data, &data) == nil {
			for index := range project.Resources {
				if project.Resources[index].ID == data.OldResourceID {
					checked := data.LastCheckedAt
					project.Resources[index] = domain.ProjectResource{ID: data.NewResourceID, Kind: "path", HomeInstallation: project.HomeInstallation, DisplayLocator: data.DisplayLocator, CanonicalLocator: data.CanonicalLocator, Health: data.Health, HealthDetails: data.HealthDetails, LastCheckedAt: &checked}
				}
			}
			if project.PrimaryResourceID == data.OldResourceID {
				project.PrimaryResourceID = data.NewResourceID
			}
		}
	case "project.primary-resource.changed":
		var data struct {
			ResourceID string `json:"resource_id"`
		}
		if json.Unmarshal(audit.Data, &data) == nil {
			project.PrimaryResourceID = data.ResourceID
		}
	case "project.resource.health":
		var data struct {
			ResourceID    string                     `json:"resource_id"`
			Health        domain.ResourceHealthState `json:"health"`
			HealthDetails map[string]string          `json:"health_details"`
			LastCheckedAt time.Time                  `json:"last_checked_at"`
		}
		if json.Unmarshal(audit.Data, &data) == nil {
			for index := range project.Resources {
				if project.Resources[index].ID == data.ResourceID {
					project.Resources[index].Health, project.Resources[index].HealthDetails, project.Resources[index].LastCheckedAt = data.Health, data.HealthDetails, &data.LastCheckedAt
				}
			}
		}
	case "project.assignment.configuring":
		var data struct {
			AssignmentID string `json:"assignment_id"`
			Agent        string `json:"agent"`
		}
		if json.Unmarshal(audit.Data, &data) == nil {
			project.Assignment = &domain.ProjectAssignment{ID: data.AssignmentID, AgentName: data.Agent, State: domain.AssignmentConfiguring, StartedAt: item.created}
			project.SuggestedAgentName = ""
		}
	case "project.assignment.runnable":
		var data struct {
			AssignmentID string `json:"assignment_id"`
			Agent        string `json:"agent"`
			ThreadID     string `json:"thread_id"`
		}
		if json.Unmarshal(audit.Data, &data) == nil && project.Assignment != nil && project.Assignment.ID == data.AssignmentID {
			project.Assignment.State, project.Assignment.SelectedThreadID = domain.AssignmentRunnable, data.ThreadID
		}
	case "project.assignment.blocked":
		if project.Assignment != nil {
			project.Assignment.State = domain.AssignmentBlocked
		}
	case "project.assignment.ended":
		if project.Assignment != nil {
			project.SuggestedAgentName = project.Assignment.AgentName
		}
		project.Assignment = nil
	}
}

func removeReplicaResource(resources []domain.ProjectResource, id string) []domain.ProjectResource {
	result := resources[:0]
	for _, resource := range resources {
		if resource.ID != id {
			result = append(result, resource)
		}
	}
	return result
}

func getProjectReplica(ctx context.Context, q projectQueryer, id string) (domain.Project, error) {
	var raw []byte
	if err := q.QueryRowContext(ctx, `SELECT state_json FROM project_replicas WHERE id=?`, id).Scan(&raw); errors.Is(err, sql.ErrNoRows) {
		return domain.Project{}, domain.ErrProjectNotFound
	} else if err != nil {
		return domain.Project{}, err
	}
	var project domain.Project
	if err := json.Unmarshal(raw, &project); err != nil {
		return project, err
	}
	if project.SuggestedAgentName != "" {
		rows, err := q.QueryContext(ctx, `SELECT state_json FROM project_replicas WHERE id<>?`, id)
		if err != nil {
			return project, err
		}
		for rows.Next() {
			var otherRaw []byte
			var other domain.Project
			if rows.Scan(&otherRaw) == nil && json.Unmarshal(otherRaw, &other) == nil && other.HomeInstallation == project.HomeInstallation && other.Assignment != nil && other.Assignment.AgentName == project.SuggestedAgentName {
				project.SuggestedAgentName = ""
			}
		}
		if err := rows.Err(); err != nil {
			rows.Close()
			return project, err
		}
		if err := rows.Close(); err != nil {
			return project, err
		}
	}
	pending, err := latestProjectCommand(ctx, q, id)
	if err != nil {
		return project, err
	}
	if pending != nil && pending.Stage != domain.ProjectCommandCommitted && pending.Stage != domain.ProjectCommandRejected {
		project.PendingCommand = pending
	}
	project.LatestCommand = pending
	return project, nil
}
