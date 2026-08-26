package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/projectstate"
)

type replicaProjectEvent struct {
	id      string
	home    string
	created time.Time
	payload event.ProjectEventPayload
}

func (s *SQLite) rebuildProjectReplicasTx(ctx context.Context, tx *sql.Tx, state event.State, repair bool) error {
	if repair {
		if _, err := tx.ExecContext(ctx, `DELETE FROM project_replicas`); err != nil {
			return err
		}
	}
	groups := make(map[string][]replicaProjectEvent)
	affectedProjects := make(map[string]bool)
	for id, record := range state.Records {
		if record.Event.Content.Type != event.TypeProjectEvent {
			continue
		}
		var payload event.ProjectEventPayload
		if json.Unmarshal(record.Event.Content.Payload, &payload) != nil {
			continue
		}
		affectedProjects[payload.ProjectID] = true
		if record.Status != event.StatusProjected {
			continue
		}
		groups[payload.ProjectID] = append(groups[payload.ProjectID], replicaProjectEvent{id: id, home: record.Event.Content.InstallationID, created: time.Unix(record.Event.Nostr.CreatedAt, 0).UTC(), payload: payload})
	}
	if !repair {
		for projectID := range affectedProjects {
			if _, err := tx.ExecContext(ctx, `DELETE FROM project_replicas WHERE id=?`, projectID); err != nil {
				return err
			}
		}
	}
	for projectID, events := range groups {
		project, ok, diagnostic := reduceReplicaProject(projectID, events)
		if project.HomeInstallation == s.signer.InstallationID {
			continue
		}
		if diagnostic != "" {
			details, _ := json.Marshal(map[string]string{"diagnostic": diagnostic})
			if _, err := tx.ExecContext(ctx, `INSERT INTO project_audit_log(operation,project_id,home_installation_id,expected_head_event_id,current_head_event_id,outcome,details_json,created_at) VALUES ('project.replica.reduce',?,?,?,?, 'rejected',?,?)`, projectID, project.HomeInstallation, project.HeadEventID, project.HeadEventID, string(details), s.now().UTC().UnixMilli()); err != nil {
				return err
			}
		}
		if !ok {
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

func reduceReplicaProject(projectID string, events []replicaProjectEvent) (domain.Project, bool, string) {
	sort.Slice(events, func(i, j int) bool {
		if events[i].created.Equal(events[j].created) {
			return events[i].id < events[j].id
		}
		return events[i].created.Before(events[j].created)
	})
	var creation *replicaProjectEvent
	creationCount := 0
	for index := range events {
		if events[index].payload.Operation == string(projectstate.OperationCreated) && events[index].payload.PreviousEventID == "" {
			creation, creationCount = &events[index], creationCount+1
		}
	}
	if creation == nil || creationCount != 1 {
		return domain.Project{}, false, fmt.Sprintf("project %s has %d creation roots", projectID, creationCount)
	}
	data, err := projectstate.DecodeAudit(creation.payload.Operation, creation.payload.Body)
	if err != nil {
		return domain.Project{ID: projectID, HomeInstallation: creation.home}, false, err.Error()
	}
	snapshot, err := projectstate.Apply(projectstate.Snapshot{}, projectstate.Event{ID: creation.id, ProjectID: projectID, HomeInstallation: creation.home, PreviousEventID: creation.payload.PreviousEventID, CreatedAt: creation.created, Data: data})
	if err != nil {
		return domain.Project{ID: projectID, HomeInstallation: creation.home}, false, err.Error()
	}
	children := make(map[string][]replicaProjectEvent)
	for _, item := range events {
		if item.home == snapshot.Project.HomeInstallation && item.payload.PreviousEventID != "" {
			children[item.payload.PreviousEventID] = append(children[item.payload.PreviousEventID], item)
		}
	}
	for {
		candidates := children[snapshot.Project.HeadEventID]
		if len(candidates) == 0 {
			snapshot.Project.ReadOnlyReplica = true
			return snapshot.Project, true, fmt.Sprintf("project %s forks at %s", projectID, snapshot.Project.HeadEventID)
		}
		if len(candidates) != 1 {
			// A home fork is never guessed through. Retain the last
			// unambiguous projection until authoritative repair arrives.
			break
		}
		sort.Slice(candidates, func(i, j int) bool { return candidates[i].id < candidates[j].id })
		item := candidates[0]
		data, err := projectstate.DecodeAudit(item.payload.Operation, item.payload.Body)
		if err != nil {
			snapshot.Project.ReadOnlyReplica = true
			return snapshot.Project, true, fmt.Sprintf("project event %s: %v", item.id, err)
		}
		next, err := projectstate.Apply(snapshot, projectstate.Event{ID: item.id, ProjectID: projectID, HomeInstallation: item.home, PreviousEventID: item.payload.PreviousEventID, CreatedAt: item.created, Data: data})
		if err != nil {
			snapshot.Project.ReadOnlyReplica = true
			return snapshot.Project, true, fmt.Sprintf("project event %s: %v", item.id, err)
		}
		snapshot = next
	}
	snapshot.Project.ReadOnlyReplica = true
	return snapshot.Project, true, ""
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
