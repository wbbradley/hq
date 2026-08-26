package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/projectstate"
)

type appliedProjectEvent struct {
	id       string
	previous string
	created  time.Time
	payload  event.ProjectEventPayload
}

type authoritativeProjectProjection struct {
	snapshot   projectstate.Snapshot
	events     []appliedProjectEvent
	diagnostic string
}

func (s *SQLite) rebuildAuthoritativeProjectsTx(ctx context.Context, tx *sql.Tx, state event.State, repair bool) error {
	groups := make(map[string][]replicaProjectEvent)
	affectedProjects := make(map[string]bool)
	for id, record := range state.Records {
		if record.Event.Content.Type != event.TypeProjectEvent || record.Event.Content.InstallationID != s.signer.InstallationID {
			continue
		}
		var payload event.ProjectEventPayload
		if err := json.Unmarshal(record.Event.Content.Payload, &payload); err != nil {
			continue
		}
		affectedProjects[payload.ProjectID] = true
		if record.Status != event.StatusProjected {
			continue
		}
		groups[payload.ProjectID] = append(groups[payload.ProjectID], replicaProjectEvent{id: id, home: record.Event.Content.InstallationID, created: time.Unix(record.Event.Nostr.CreatedAt, 0).UTC(), payload: payload})
	}
	projectIDs := make([]string, 0, len(affectedProjects))
	for projectID := range affectedProjects {
		projectIDs = append(projectIDs, projectID)
	}
	sort.Strings(projectIDs)
	legacyThreads, err := loadProjectThreads(ctx, tx, projectIDs)
	if err != nil {
		return err
	}
	legacyResources, err := loadProjectResources(ctx, tx, projectIDs)
	if err != nil {
		return err
	}
	projections := make([]authoritativeProjectProjection, 0, len(projectIDs))
	for _, projectID := range projectIDs {
		projection := reduceAuthoritativeProject(projectID, groups[projectID], legacyThreads, legacyResources[projectID])
		if projection.snapshot.Project.ID != "" {
			projections = append(projections, projection)
		}
	}
	if repair {
		for _, table := range []string{"project_dispatch_records", "project_message_acceptances", "resource_claim_epochs", "project_resources", "resource_health", "resources", "project_assignment_epochs", "project_threads", "project_events", "projects"} {
			if _, err := tx.ExecContext(ctx, `DELETE FROM `+table); err != nil {
				return fmt.Errorf("clear %s projection: %w", table, err)
			}
		}
	} else {
		for _, projectID := range projectIDs {
			for _, statement := range []string{
				`DELETE FROM project_dispatch_records WHERE project_id=?`,
				`DELETE FROM project_message_acceptances WHERE project_id=?`,
				`DELETE FROM resource_claim_epochs WHERE project_id=?`,
				`DELETE FROM project_resources WHERE project_id=?`,
				`DELETE FROM project_assignment_epochs WHERE project_id=?`,
				`DELETE FROM project_threads WHERE project_id=?`,
				`DELETE FROM project_events WHERE project_id=?`,
				`DELETE FROM projects WHERE id=?`,
			} {
				if _, err := tx.ExecContext(ctx, statement, projectID); err != nil {
					return fmt.Errorf("clear project %s projection: %w", projectID, err)
				}
			}
		}
	}
	for _, projection := range projections {
		if err := insertAuthoritativeProjectProjection(ctx, tx, projection); err != nil {
			return err
		}
		if projection.diagnostic != "" {
			details, _ := json.Marshal(map[string]string{"diagnostic": projection.diagnostic})
			if _, err := tx.ExecContext(ctx, `INSERT INTO project_audit_log(operation,project_id,home_installation_id,expected_head_event_id,current_head_event_id,outcome,details_json,created_at) VALUES ('project.authoritative.reduce',?,?,?,?, 'rejected',?,?)`, projection.snapshot.Project.ID, projection.snapshot.Project.HomeInstallation, projection.snapshot.Project.HeadEventID, projection.snapshot.Project.HeadEventID, string(details), s.now().UTC().UnixMilli()); err != nil {
				return err
			}
		}
	}
	cleanup := []string{
		`DELETE FROM project_dispatch_attempts WHERE %s (NOT EXISTS (SELECT 1 FROM project_message_acceptances a WHERE a.message_id=project_dispatch_attempts.message_id) OR NOT EXISTS (SELECT 1 FROM project_assignment_epochs e WHERE e.id=project_dispatch_attempts.assignment_id) OR NOT EXISTS (SELECT 1 FROM project_threads t WHERE t.id=project_dispatch_attempts.project_thread_id))`,
		`DELETE FROM project_output_provenance WHERE %s (NOT EXISTS (SELECT 1 FROM projects p WHERE p.id=project_output_provenance.project_id) OR NOT EXISTS (SELECT 1 FROM project_assignment_epochs e WHERE e.id=project_output_provenance.assignment_id) OR NOT EXISTS (SELECT 1 FROM project_threads t WHERE t.id=project_output_provenance.project_thread_id))`,
		`DELETE FROM project_activation_operations WHERE %s (NOT EXISTS (SELECT 1 FROM projects p WHERE p.id=project_activation_operations.project_id) OR (assignment_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM project_assignment_epochs e WHERE e.id=project_activation_operations.assignment_id)))`,
		`DELETE FROM project_runtime_operations WHERE %s NOT EXISTS (SELECT 1 FROM projects p WHERE p.id=project_runtime_operations.project_id)`,
	}
	if repair {
		for _, template := range cleanup {
			if _, err := tx.ExecContext(ctx, fmt.Sprintf(template, "")); err != nil {
				return err
			}
		}
	} else {
		for _, projectID := range projectIDs {
			for _, template := range cleanup {
				if _, err := tx.ExecContext(ctx, fmt.Sprintf(template, "project_id=? AND"), projectID); err != nil {
					return err
				}
			}
		}
	}
	return nil
}

func loadProjectResources(ctx context.Context, tx *sql.Tx, projectIDs []string) (map[string]map[string]projectstate.CreatedResource, error) {
	result := make(map[string]map[string]projectstate.CreatedResource)
	if len(projectIDs) == 0 {
		return result, nil
	}
	args := make([]any, len(projectIDs))
	for index, projectID := range projectIDs {
		args[index] = projectID
	}
	rows, err := tx.QueryContext(ctx, `SELECT pr.project_id,r.id,r.kind,r.display_locator,r.canonical_locator,h.state,h.details_json,h.last_checked_at FROM project_resources pr JOIN resources r ON r.id=pr.resource_id JOIN resource_health h ON h.resource_id=r.id WHERE pr.project_id IN (`+strings.TrimRight(strings.Repeat("?,", len(args)), ",")+`)`, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var projectID string
		var resource projectstate.CreatedResource
		var details []byte
		var checked int64
		if err := rows.Scan(&projectID, &resource.ID, &resource.Kind, &resource.DisplayLocator, &resource.CanonicalLocator, &resource.Health, &details, &checked); err != nil {
			return nil, err
		}
		if err := json.Unmarshal(details, &resource.HealthDetails); err != nil {
			resource.HealthDetails = map[string]string{"legacy_details": string(details)}
		}
		resource.LastCheckedAt = time.UnixMilli(checked).UTC()
		if result[projectID] == nil {
			result[projectID] = make(map[string]projectstate.CreatedResource)
		}
		result[projectID][resource.ID] = resource
	}
	return result, rows.Err()
}

func loadProjectThreads(ctx context.Context, tx *sql.Tx, projectIDs []string) (map[string]projectstate.ThreadProjection, error) {
	result := make(map[string]projectstate.ThreadProjection)
	if len(projectIDs) == 0 {
		return result, nil
	}
	args := make([]any, len(projectIDs))
	for index, projectID := range projectIDs {
		args[index] = projectID
	}
	rows, err := tx.QueryContext(ctx, `SELECT id,project_id,agent_name,harness,external_thread_id,launch_directory,created_at FROM project_threads WHERE project_id IN (`+strings.TrimRight(strings.Repeat("?,", len(args)), ",")+`)`, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var thread projectstate.ThreadProjection
		var created int64
		if err := rows.Scan(&thread.ID, &thread.ProjectID, &thread.Agent, &thread.Harness, &thread.ExternalThreadID, &thread.LaunchDirectory, &created); err != nil {
			return nil, err
		}
		thread.CreatedAt = time.UnixMilli(created).UTC()
		result[thread.ID] = thread
	}
	return result, rows.Err()
}

func reduceAuthoritativeProject(projectID string, events []replicaProjectEvent, legacyThreads map[string]projectstate.ThreadProjection, legacyResources map[string]projectstate.CreatedResource) authoritativeProjectProjection {
	var result authoritativeProjectProjection
	var roots []replicaProjectEvent
	children := make(map[string][]replicaProjectEvent)
	for _, item := range events {
		if item.payload.Operation == string(projectstate.OperationCreated) && item.payload.PreviousEventID == "" {
			roots = append(roots, item)
		} else if item.payload.PreviousEventID != "" {
			children[item.payload.PreviousEventID] = append(children[item.payload.PreviousEventID], item)
		}
	}
	if len(roots) != 1 {
		result.diagnostic = fmt.Sprintf("project %s has %d creation roots", projectID, len(roots))
		return result
	}
	root := roots[0]
	data, err := projectstate.DecodeAudit(root.payload.Operation, root.payload.Body)
	if err != nil {
		result.diagnostic = fmt.Sprintf("project event %s: %v", root.id, err)
		return result
	}
	if created, ok := data.(*projectstate.Created); ok && len(created.Resources) == 0 && len(created.ResourceIDs) != 0 {
		for _, resourceID := range created.ResourceIDs {
			resource, exists := legacyResources[resourceID]
			if !exists {
				result.diagnostic = fmt.Sprintf("project creation event %s lacks canonical resource details for %s", root.id, resourceID)
				break
			}
			created.Resources = append(created.Resources, resource)
		}
	}
	result.snapshot, err = projectstate.Apply(projectstate.Snapshot{}, projectstate.Event{ID: root.id, ProjectID: projectID, HomeInstallation: root.home, CreatedAt: root.created, Data: data})
	if err != nil {
		result.diagnostic = fmt.Sprintf("project event %s: %v", root.id, err)
		return result
	}
	result.events = append(result.events, appliedProjectEvent{id: root.id, created: root.created, payload: root.payload})
	for {
		candidates := children[result.snapshot.Project.HeadEventID]
		if len(candidates) == 0 {
			return result
		}
		if len(candidates) != 1 {
			result.diagnostic = fmt.Sprintf("project %s forks at %s", projectID, result.snapshot.Project.HeadEventID)
			return result
		}
		item := candidates[0]
		data, err := projectstate.DecodeAudit(item.payload.Operation, item.payload.Body)
		if err != nil {
			result.diagnostic = fmt.Sprintf("project event %s: %v", item.id, err)
			return result
		}
		if runnable, ok := data.(*projectstate.AssignmentRunnable); ok && runnable.Thread == nil {
			legacy, exists := legacyThreads[runnable.ThreadID]
			if !exists || legacy.ProjectID != projectID || legacy.Agent != runnable.Agent {
				result.diagnostic = fmt.Sprintf("project event %s lacks canonical thread details for %s", item.id, runnable.ThreadID)
				return result
			}
			runnable.Thread = &projectstate.Thread{ID: legacy.ID, Harness: legacy.Harness, ExternalThreadID: legacy.ExternalThreadID, LaunchDirectory: legacy.LaunchDirectory, CreatedAt: legacy.CreatedAt}
		}
		next, err := projectstate.Apply(result.snapshot, projectstate.Event{ID: item.id, ProjectID: projectID, HomeInstallation: item.home, PreviousEventID: item.payload.PreviousEventID, CreatedAt: item.created, Data: data})
		if err != nil {
			result.diagnostic = fmt.Sprintf("project event %s: %v", item.id, err)
			return result
		}
		result.snapshot = next
		result.events = append(result.events, appliedProjectEvent{id: item.id, previous: item.payload.PreviousEventID, created: item.created, payload: item.payload})
	}
}

func insertAuthoritativeProjectProjection(ctx context.Context, tx *sql.Tx, projection authoritativeProjectProjection) error {
	project := projection.snapshot.Project
	if _, err := tx.ExecContext(ctx, `INSERT INTO projects(id,home_installation_id,mailbox_id,predecessor_project_id,name,brief,lifecycle,archived,primary_resource_id,head_event_id,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)`, project.ID, project.HomeInstallation, project.MailboxID, nullString(project.PredecessorProjectID), project.Name, project.Brief, project.Lifecycle, boolInt(project.Archived), nullString(project.PrimaryResourceID), project.HeadEventID, project.CreatedAt.UnixMilli(), project.UpdatedAt.UnixMilli()); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx, `UPDATE mailboxes SET label=? WHERE id=?`, project.Name, project.MailboxID); err != nil {
		return err
	}
	for _, item := range projection.events {
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_events(event_id,project_id,previous_event_id,event_type,payload,created_at) VALUES (?,?,?,?,?,?)`, item.id, project.ID, item.previous, item.payload.Operation, []byte(item.payload.Body), item.created.UnixMilli()); err != nil {
			return err
		}
	}
	for _, item := range projection.snapshot.Resources {
		resource := item.Resource
		if _, err := tx.ExecContext(ctx, `INSERT INTO resources(id,kind,home_installation_id,display_locator,canonical_locator,created_at) VALUES (?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET display_locator=excluded.display_locator`, resource.ID, resource.Kind, project.HomeInstallation, resource.DisplayLocator, resource.CanonicalLocator, item.CreatedAt.UnixMilli()); err != nil {
			return err
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_resources(project_id,resource_id,added_event_id,removed_event_id) VALUES (?,?,?,?)`, project.ID, resource.ID, item.AddedEventID, nullString(item.RemovedEventID)); err != nil {
			return err
		}
		details, _ := json.Marshal(resource.HealthDetails)
		checked := item.CreatedAt
		if resource.LastCheckedAt != nil {
			checked = *resource.LastCheckedAt
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO resource_health(resource_id,state,details_json,last_checked_at) VALUES (?,?,?,?) ON CONFLICT(resource_id) DO UPDATE SET state=excluded.state,details_json=excluded.details_json,last_checked_at=excluded.last_checked_at`, resource.ID, resource.Health, string(details), checked.UnixMilli()); err != nil {
			return err
		}
	}
	for _, claim := range projection.snapshot.Claims {
		var releasedEvent, releasedAt any
		if claim.ReleasedEventID != "" {
			releasedEvent, releasedAt = claim.ReleasedEventID, claim.ReleasedAt.UnixMilli()
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO resource_claim_epochs(id,project_id,resource_id,acquired_event_id,released_event_id,acquired_at,released_at) VALUES (?,?,?,?,?,?,?)`, project.ID+":"+claim.ResourceID+":"+claim.AcquiredEventID, project.ID, claim.ResourceID, claim.AcquiredEventID, releasedEvent, claim.AcquiredAt.UnixMilli(), releasedAt); err != nil {
			return err
		}
	}
	for _, thread := range projection.snapshot.Threads {
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_threads(id,project_id,agent_name,harness,external_thread_id,launch_directory,created_at) VALUES (?,?,?,?,?,?,?)`, thread.ID, thread.ProjectID, thread.Agent, thread.Harness, thread.ExternalThreadID, thread.LaunchDirectory, thread.CreatedAt.UnixMilli()); err != nil {
			return err
		}
	}
	for _, item := range projection.snapshot.Assignments {
		assignment := item.Assignment
		var selected any
		if assignment.SelectedThreadID != "" {
			selected = assignment.SelectedThreadID
		}
		var endedEvent, endedAt any
		if item.EndedEventID != "" {
			endedEvent = item.EndedEventID
		}
		if assignment.EndedAt != nil {
			endedAt = assignment.EndedAt.UnixMilli()
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_assignment_epochs(id,project_id,agent_name,state,selected_thread_id,started_event_id,ended_event_id,started_at,ended_at,forced) VALUES (?,?,?,?,?,?,?,?,?,?)`, assignment.ID, project.ID, assignment.AgentName, assignment.State, selected, item.StartedEventID, endedEvent, assignment.StartedAt.UnixMilli(), endedAt, boolInt(item.Forced)); err != nil {
			return err
		}
	}
	for _, accepted := range projection.snapshot.Acceptances {
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_message_acceptances(project_id,sequence,message_id,message_event_id,acceptance_event_id,accepted_at) VALUES (?,?,?,?,?,?)`, project.ID, accepted.Sequence, accepted.MessageID, accepted.MessageEventID, accepted.EventID, accepted.AcceptedAt.UnixMilli()); err != nil {
			return err
		}
	}
	for _, dispatched := range projection.snapshot.Dispatches {
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_dispatch_records(message_id,project_id,sequence,assignment_id,agent_name,project_thread_id,external_thread_id,dispatch_event_id,dispatched_at) VALUES (?,?,?,?,?,?,?,?,?)`, dispatched.MessageID, project.ID, dispatched.Sequence, dispatched.AssignmentID, dispatched.Agent, dispatched.ProjectThreadID, dispatched.ExternalThreadID, dispatched.EventID, dispatched.DispatchedAt.UnixMilli()); err != nil {
			return err
		}
	}
	return nil
}
