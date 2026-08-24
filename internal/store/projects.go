package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

type canonicalPath struct {
	display   string
	canonical string
	health    domain.ResourceHealthState
	details   map[string]string
}

// canonicalizeProjectPath preserves the spelling chosen by the human while
// resolving identity through the nearest existing ancestor. Thus a path can
// be reserved before it exists without changing identity when it is created.
func canonicalizeProjectPath(value string) (canonicalPath, error) {
	if strings.TrimSpace(value) == "" {
		return canonicalPath{}, errors.New("path resource is empty")
	}
	abs, err := filepath.Abs(value)
	if err != nil {
		return canonicalPath{}, fmt.Errorf("make project path absolute: %w", err)
	}
	display := filepath.Clean(abs)
	ancestor := display
	var suffix []string
	for {
		_, statErr := os.Lstat(ancestor)
		if statErr == nil {
			break
		}
		if !errors.Is(statErr, os.ErrNotExist) {
			return canonicalPath{display: display, canonical: display, health: domain.ResourceInaccessible, details: map[string]string{"error": statErr.Error()}}, nil
		}
		parent := filepath.Dir(ancestor)
		if parent == ancestor {
			return canonicalPath{}, fmt.Errorf("find existing ancestor for %q", display)
		}
		suffix = append(suffix, filepath.Base(ancestor))
		ancestor = parent
	}
	resolved, err := filepath.EvalSymlinks(ancestor)
	if err != nil {
		return canonicalPath{display: display, canonical: display, health: domain.ResourceInaccessible, details: map[string]string{"error": err.Error()}}, nil
	}
	for i := len(suffix) - 1; i >= 0; i-- {
		resolved = filepath.Join(resolved, suffix[i])
	}
	health := domain.ResourceHealthy
	if len(suffix) > 0 {
		health = domain.ResourceMissing
	}
	return canonicalPath{display: display, canonical: filepath.Clean(resolved), health: health, details: map[string]string{}}, nil
}

func pathOverlap(a, b string) string {
	if a == b {
		return "equal"
	}
	if rel, err := filepath.Rel(b, a); err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "descendant"
	}
	if rel, err := filepath.Rel(a, b); err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "ancestor"
	}
	return ""
}

func (s *SQLite) appendProjectEventTx(ctx context.Context, tx *sql.Tx, projectID, previous, eventType string, payload any, now int64) (string, error) {
	signed, raw, err := s.signProjectEventTx(ctx, tx, projectID, previous, eventType, payload, time.UnixMilli(now).UTC())
	if err != nil {
		return "", err
	}
	if _, err := s.ingestCanonicalTx(ctx, tx, []event.SignedEvent{signed}, true); err != nil {
		return "", err
	}
	id := signed.ID()
	_, err = tx.ExecContext(ctx, `INSERT INTO project_events(event_id,project_id,previous_event_id,event_type,payload,created_at) VALUES (?,?,?,?,?,?)`, id, projectID, previous, eventType, raw, now)
	return id, err
}

func (s *SQLite) signProjectEvent(ctx context.Context, projectID, previous, eventType string, payload any, createdAt time.Time, accountID string, membershipParents []string) (event.SignedEvent, []byte, error) {
	raw, err := marshalProjectAuditPayload(ctx, payload)
	if err != nil {
		return event.SignedEvent{}, nil, err
	}
	projectPayload, err := event.MarshalPayload(event.ProjectEventPayload{ProjectID: projectID, PreviousEventID: previous, Operation: eventType, Body: raw})
	if err != nil {
		return event.SignedEvent{}, nil, err
	}
	parents := append([]string(nil), membershipParents...)
	if previous != "" {
		parents = append(parents, previous)
	}
	sort.Strings(parents)
	parents = slices.Compact(parents)
	content := event.Content{Type: event.TypeProjectEvent, Parents: parents, Scope: event.ScopeAccountAddressed, Audience: &event.Audience{HumanAccountID: accountID}, Payload: projectPayload}
	signed, err := s.signContents(ctx, []event.Content{content}, []time.Time{createdAt})
	if err != nil {
		return event.SignedEvent{}, nil, err
	}
	return signed[0], raw, nil
}

func (s *SQLite) signProjectEventTx(ctx context.Context, tx *sql.Tx, projectID, previous, eventType string, payload any, createdAt time.Time) (event.SignedEvent, []byte, error) {
	accountID, parents, err := projectAccountRouteTx(ctx, tx, s.signer.InstallationID)
	if err != nil {
		return event.SignedEvent{}, nil, err
	}
	return s.signProjectEvent(ctx, projectID, previous, eventType, payload, createdAt, accountID, parents)
}

func projectAccountRouteTx(ctx context.Context, tx *sql.Tx, installationID string) (string, []string, error) {
	var accountID, creatorInstallation, creationEvent, state, acceptEvent string
	if err := tx.QueryRowContext(ctx, `SELECT d.account_id,a.creator_installation_id,a.creation_event_id,d.state,d.accept_event_id FROM human_account_default x JOIN human_accounts a ON a.account_id=x.account_id JOIN human_account_devices d ON d.account_id=a.account_id AND d.installation_id=? WHERE x.id=1`, installationID).Scan(&accountID, &creatorInstallation, &creationEvent, &state, &acceptEvent); err != nil {
		return "", nil, err
	}
	if state != "active" {
		return "", nil, errors.New("local installation is not an active human account device")
	}
	if installationID == creatorInstallation {
		return accountID, []string{creationEvent}, nil
	}
	if acceptEvent == "" {
		return "", nil, errors.New("active human device has no acceptance event")
	}
	return accountID, []string{acceptEvent}, nil
}

func marshalProjectAuditPayload(ctx context.Context, payload any) ([]byte, error) {
	requestID := ""
	if mutation, ok := domain.MutationFromContext(ctx); ok {
		requestID = mutation.ID
	}
	return json.Marshal(struct {
		RequestID string `json:"request_id,omitempty"`
		Data      any    `json:"data"`
	}{RequestID: requestID, Data: payload})
}

func checkProjectHeadTx(ctx context.Context, tx *sql.Tx, projectID, expected string) (domain.ProjectLifecycle, bool, string, error) {
	var lifecycle domain.ProjectLifecycle
	var archived bool
	var current string
	if err := tx.QueryRowContext(ctx, `SELECT lifecycle,archived,head_event_id FROM projects WHERE id=?`, projectID).Scan(&lifecycle, &archived, &current); errors.Is(err, sql.ErrNoRows) {
		return "", false, "", domain.ErrProjectNotFound
	} else if err != nil {
		return "", false, "", err
	}
	if expected == "" || expected != current {
		return "", false, current, &domain.StaleProjectHead{ProjectID: projectID, Expected: expected, Current: current}
	}
	return lifecycle, archived, current, nil
}

func (s *SQLite) CreateProject(ctx context.Context, request domain.CreateProjectRequest) (domain.Project, error) {
	request.Name = strings.TrimSpace(request.Name)
	if request.Name == "" {
		return domain.Project{}, errors.New("project name is required")
	}
	if len(request.Name) > 200 || strings.ContainsAny(request.Name, "\r\n\x00") {
		return domain.Project{}, errors.New("project name must be at most 200 bytes without line breaks")
	}
	if request.ID == "" {
		request.ID = uuid.NewString()
	}
	if _, err := uuid.Parse(request.ID); err != nil {
		return domain.Project{}, errors.New("project ID must be a UUID")
	}
	if request.HomeInstallation != "" && request.HomeInstallation != s.signer.InstallationID {
		return s.queueRemoteProjectCreation(ctx, request)
	}
	if request.PredecessorProjectID != "" {
		if _, err := uuid.Parse(request.PredecessorProjectID); err != nil {
			return domain.Project{}, errors.New("predecessor project ID must be a UUID")
		}
	}
	paths := make([]canonicalPath, len(request.Paths))
	for i, input := range request.Paths {
		path, err := canonicalizeProjectPath(input.DisplayPath)
		if err != nil {
			return domain.Project{}, err
		}
		paths[i] = path
	}
	if len(paths) == 0 {
		request.PrimaryPath = 0
	} else if request.PrimaryPath < 0 || request.PrimaryPath >= len(paths) {
		return domain.Project{}, errors.New("primary path index is out of range")
	}
	mailboxID := uuid.NewString()
	resourceIDs := make([]string, len(paths))
	for i := range resourceIDs {
		resourceIDs[i] = uuid.NewString()
	}
	operationTime := s.now().UTC()
	mailboxPayload, _ := event.MarshalPayload(event.MailboxPayload{MailboxID: mailboxID, Kind: "project", Label: request.Name})
	resourceDescriptors := make([]map[string]any, len(paths))
	for index, path := range paths {
		resourceDescriptors[index] = map[string]any{"id": resourceIDs[index], "kind": "path", "display_locator": path.display, "canonical_locator": path.canonical, "health": path.health, "health_details": path.details, "last_checked_at": operationTime}
	}
	request.HomeInstallation = s.signer.InstallationID
	createData := map[string]any{"request": request, "mailbox_id": mailboxID, "resource_ids": resourceIDs, "resources": resourceDescriptors}
	createBody, _ := marshalProjectAuditPayload(ctx, createData)
	account, membership, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return domain.Project{}, err
	}
	mailboxEvents, err := s.signContents(ctx, []event.Content{{Type: event.TypeMailboxCreate, Scope: event.ScopeInstallationPrivate, Payload: mailboxPayload}}, []time.Time{operationTime})
	if err != nil {
		return domain.Project{}, err
	}
	projectEvent, _, err := s.signProjectEvent(ctx, request.ID, "", "project.created", createData, operationTime, account.ID, membership)
	if err != nil {
		return domain.Project{}, err
	}
	mailboxEvents = append(mailboxEvents, projectEvent)

	value, err := s.commitMutation(ctx, []domain.ChangeTopic{domain.TopicProjects, domain.TopicMailboxes}, func(tx *sql.Tx) (any, error) {
		now := operationTime.UnixMilli()
		if request.Open {
			if err := checkPathConflictsTx(ctx, tx, request.ID, paths); err != nil {
				return nil, err
			}
		}
		eventID := mailboxEvents[1].ID()
		lifecycle := domain.ProjectClosed
		if request.Open {
			lifecycle = domain.ProjectOpen
		}
		primaryID := ""
		if len(resourceIDs) > 0 {
			primaryID = resourceIDs[request.PrimaryPath]
		}
		if _, err := s.ingestCanonicalTx(ctx, tx, mailboxEvents, true); err != nil {
			return nil, err
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO projects(id,home_installation_id,mailbox_id,predecessor_project_id,name,brief,lifecycle,archived,primary_resource_id,head_event_id,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)`, request.ID, s.signer.InstallationID, mailboxID, nullString(request.PredecessorProjectID), request.Name, request.Brief, lifecycle, false, nullString(primaryID), eventID, now, now); err != nil {
			return nil, err
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_events(event_id,project_id,previous_event_id,event_type,payload,created_at) VALUES (?,?,?,?,?,?)`, eventID, request.ID, "", "project.created", createBody, now); err != nil {
			return nil, err
		}
		for i, path := range paths {
			details, _ := json.Marshal(path.details)
			if _, err := tx.ExecContext(ctx, `INSERT INTO resources(id,kind,home_installation_id,display_locator,canonical_locator,created_at) VALUES (?,'path',?,?,?,?)`, resourceIDs[i], s.signer.InstallationID, path.display, path.canonical, now); err != nil {
				return nil, err
			}
			if _, err := tx.ExecContext(ctx, `INSERT INTO project_resources(project_id,resource_id,added_event_id) VALUES (?,?,?)`, request.ID, resourceIDs[i], eventID); err != nil {
				return nil, err
			}
			if _, err := tx.ExecContext(ctx, `INSERT INTO resource_health(resource_id,state,details_json,last_checked_at) VALUES (?,?,?,?)`, resourceIDs[i], path.health, string(details), now); err != nil {
				return nil, err
			}
			if request.Open {
				if _, err := tx.ExecContext(ctx, `INSERT INTO resource_claim_epochs(id,project_id,resource_id,acquired_event_id,acquired_at) VALUES (?,?,?,?,?)`, uuid.NewString(), request.ID, resourceIDs[i], eventID, now); err != nil {
					return nil, err
				}
			}
		}
		return getProjectTx(ctx, tx, request.ID)
	})
	if err != nil {
		s.recordProjectAuditFailure(ctx, request.ID, "", err)
		return domain.Project{}, err
	}
	return value.(domain.Project), nil
}

func nullString(value string) any {
	if value == "" {
		return nil
	}
	return value
}

func checkPathConflictsTx(ctx context.Context, tx *sql.Tx, projectID string, paths []canonicalPath) error {
	rows, err := tx.QueryContext(ctx, `SELECT c.project_id,r.display_locator,r.canonical_locator FROM resource_claim_epochs c JOIN resources r ON r.id=c.resource_id WHERE c.released_event_id IS NULL AND r.kind='path' AND c.project_id<>?`, projectID)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var otherProject, otherDisplay, otherPath string
		if err := rows.Scan(&otherProject, &otherDisplay, &otherPath); err != nil {
			return err
		}
		for _, path := range paths {
			if overlap := pathOverlap(path.canonical, otherPath); overlap != "" {
				return &domain.ProjectConflict{RequestedProjectID: projectID, RequestedDisplay: path.display, RequestedPath: path.canonical, ConflictingProject: otherProject, ConflictingDisplay: otherDisplay, ConflictingPath: otherPath, Overlap: overlap}
			}
		}
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return err
	}
	if err := rows.Close(); err != nil {
		return err
	}
	return checkProvisioningReservationsTx(ctx, tx, projectID, domain.ProjectProvisioningFromContext(ctx), paths)
}

func (s *SQLite) GetProject(ctx context.Context, id string) (domain.Project, error) {
	project, err := getProjectQuery(ctx, s.db, id)
	if errors.Is(err, domain.ErrProjectNotFound) {
		return getProjectReplica(ctx, s.db, id)
	}
	return project, err
}

type projectQueryer interface {
	QueryRowContext(context.Context, string, ...any) *sql.Row
	QueryContext(context.Context, string, ...any) (*sql.Rows, error)
}

func getProjectTx(ctx context.Context, tx *sql.Tx, id string) (domain.Project, error) {
	return getProjectQuery(ctx, tx, id)
}

func getProjectQuery(ctx context.Context, q projectQueryer, id string) (domain.Project, error) {
	var project domain.Project
	var predecessor, primary sql.NullString
	var created, updated int64
	err := q.QueryRowContext(ctx, `SELECT id,home_installation_id,mailbox_id,predecessor_project_id,name,brief,lifecycle,archived,primary_resource_id,head_event_id,created_at,updated_at FROM projects WHERE id=?`, id).Scan(&project.ID, &project.HomeInstallation, &project.MailboxID, &predecessor, &project.Name, &project.Brief, &project.Lifecycle, &project.Archived, &primary, &project.HeadEventID, &created, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		return project, domain.ErrProjectNotFound
	}
	if err != nil {
		return project, err
	}
	project.PredecessorProjectID, project.PrimaryResourceID = predecessor.String, primary.String
	project.CreatedAt, project.UpdatedAt = time.UnixMilli(created).UTC(), time.UnixMilli(updated).UTC()
	rows, err := q.QueryContext(ctx, `SELECT r.id,r.kind,r.home_installation_id,r.display_locator,r.canonical_locator,h.state,h.details_json,h.last_checked_at FROM project_resources pr JOIN resources r ON r.id=pr.resource_id LEFT JOIN resource_health h ON h.resource_id=r.id WHERE pr.project_id=? AND pr.removed_event_id IS NULL ORDER BY pr.rowid`, id)
	if err != nil {
		return project, err
	}
	defer rows.Close()
	for rows.Next() {
		var resource domain.ProjectResource
		var state sql.NullString
		var details sql.NullString
		var checked sql.NullInt64
		if err := rows.Scan(&resource.ID, &resource.Kind, &resource.HomeInstallation, &resource.DisplayLocator, &resource.CanonicalLocator, &state, &details, &checked); err != nil {
			return project, err
		}
		resource.Health = domain.ResourceHealthState(state.String)
		if resource.Health == "" {
			resource.Health = domain.ResourceUnknown
		}
		if details.Valid {
			_ = json.Unmarshal([]byte(details.String), &resource.HealthDetails)
		}
		if checked.Valid {
			value := time.UnixMilli(checked.Int64).UTC()
			resource.LastCheckedAt = &value
		}
		project.Resources = append(project.Resources, resource)
	}
	if err := rows.Err(); err != nil {
		return project, err
	}
	var assignment domain.ProjectAssignment
	var started int64
	var selected sql.NullString
	err = q.QueryRowContext(ctx, `SELECT id,agent_name,state,selected_thread_id,started_at FROM project_assignment_epochs WHERE project_id=? AND ended_event_id IS NULL`, id).Scan(&assignment.ID, &assignment.AgentName, &assignment.State, &selected, &started)
	if err == nil {
		assignment.SelectedThreadID = selected.String
		assignment.StartedAt = time.UnixMilli(started).UTC()
		project.Assignment = &assignment
	} else if !errors.Is(err, sql.ErrNoRows) {
		return project, err
	}
	if project.Assignment == nil {
		_ = q.QueryRowContext(ctx, `SELECT e.agent_name FROM project_assignment_epochs e JOIN named_agents a ON a.name=e.agent_name WHERE e.project_id=? AND e.ended_event_id IS NOT NULL AND a.retired=0 AND NOT EXISTS(SELECT 1 FROM project_assignment_epochs current WHERE current.agent_name=e.agent_name AND current.ended_event_id IS NULL) ORDER BY e.ended_at DESC,e.id DESC LIMIT 1`, id).Scan(&project.SuggestedAgentName)
	}
	return project, nil
}

func (s *SQLite) ListProjects(ctx context.Context, includeArchived bool) ([]domain.Project, error) {
	query := `SELECT id FROM projects`
	if !includeArchived {
		query += ` WHERE archived=0`
	}
	query += ` ORDER BY updated_at DESC,id`
	rows, err := s.db.QueryContext(ctx, query)
	if err != nil {
		return nil, err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return nil, err
		}
		ids = append(ids, id)
	}
	if err := rows.Close(); err != nil {
		return nil, err
	}
	projects := make([]domain.Project, 0, len(ids))
	for _, id := range ids {
		project, err := s.GetProject(ctx, id)
		if err != nil {
			return nil, err
		}
		projects = append(projects, project)
	}
	replicaRows, err := s.db.QueryContext(ctx, `SELECT state_json FROM project_replicas ORDER BY updated_at DESC,id`)
	if err != nil {
		return nil, err
	}
	defer replicaRows.Close()
	for replicaRows.Next() {
		var raw []byte
		var project domain.Project
		if err := replicaRows.Scan(&raw); err != nil {
			return nil, err
		}
		if err := json.Unmarshal(raw, &project); err != nil {
			return nil, err
		}
		if includeArchived || !project.Archived {
			projects = append(projects, project)
		}
	}
	pendingCreations, err := listPendingProjectCreations(ctx, s.db)
	if err != nil {
		return nil, err
	}
	projects = append(projects, pendingCreations...)
	busyAgents := make(map[string]bool)
	for _, project := range projects {
		if project.Assignment != nil {
			busyAgents[project.HomeInstallation+"\x00"+project.Assignment.AgentName] = true
		}
	}
	for index := range projects {
		if projects[index].SuggestedAgentName != "" && busyAgents[projects[index].HomeInstallation+"\x00"+projects[index].SuggestedAgentName] {
			projects[index].SuggestedAgentName = ""
		}
	}
	return projects, nil
}

func (s *SQLite) ListProjectThreads(ctx context.Context, projectID string) ([]domain.ProjectThread, error) {
	var exists int
	if err := s.db.QueryRowContext(ctx, `SELECT count(*) FROM projects WHERE id=?`, projectID).Scan(&exists); err != nil {
		return nil, err
	}
	if exists == 0 {
		if _, err := getProjectReplica(ctx, s.db, projectID); err != nil {
			return nil, domain.ErrProjectNotFound
		}
		return nil, nil
	}
	rows, err := s.db.QueryContext(ctx, `SELECT t.id,t.project_id,t.agent_name,t.harness,t.external_thread_id,t.launch_directory,t.created_at,a.retired FROM project_threads t JOIN named_agents a ON a.name=t.agent_name WHERE t.project_id=? ORDER BY t.created_at DESC,t.id`, projectID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []domain.ProjectThread
	for rows.Next() {
		var thread domain.ProjectThread
		var created int64
		if err := rows.Scan(&thread.ID, &thread.ProjectID, &thread.AgentName, &thread.Harness, &thread.ExternalID, &thread.LaunchDir, &created, &thread.RetiredAgent); err != nil {
			return nil, err
		}
		thread.CreatedAt = time.UnixMilli(created).UTC()
		result = append(result, thread)
	}
	return result, rows.Err()
}

func (s *SQLite) OpenProject(ctx context.Context, id, expected string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.open", map[string]any{}); remote {
		return project, err
	}
	initial, err := s.GetProject(ctx, id)
	if err != nil {
		return initial, err
	}
	if initial.HeadEventID != expected {
		return initial, &domain.StaleProjectHead{ProjectID: initial.ID, Expected: expected, Current: initial.HeadEventID}
	}
	if initial.Lifecycle != domain.ProjectClosed || initial.Archived {
		return initial, fmt.Errorf("open: %w", domain.ErrProjectState)
	}
	observed, err := s.ObserveProjectResources(ctx, id, expected)
	if err != nil {
		return domain.Project{}, err
	}
	expected = observed.HeadEventID
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, archived bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectClosed || archived {
			return "", fmt.Errorf("open: %w", domain.ErrProjectState)
		}
		rows, err := tx.QueryContext(ctx, `SELECT r.id,r.display_locator,r.canonical_locator FROM project_resources pr JOIN resources r ON r.id=pr.resource_id WHERE pr.project_id=? AND pr.removed_event_id IS NULL`, id)
		if err != nil {
			return "", err
		}
		var resourceIDs []string
		var paths []canonicalPath
		for rows.Next() {
			var rid string
			var path canonicalPath
			if err := rows.Scan(&rid, &path.display, &path.canonical); err != nil {
				rows.Close()
				return "", err
			}
			resourceIDs = append(resourceIDs, rid)
			paths = append(paths, path)
		}
		if err := rows.Close(); err != nil {
			return "", err
		}
		if err := checkPathConflictsTx(ctx, tx, id, paths); err != nil {
			return "", err
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.opened", map[string]any{}, now)
		if err != nil {
			return "", err
		}
		if _, err := tx.ExecContext(ctx, `UPDATE projects SET lifecycle='open',head_event_id=?,updated_at=? WHERE id=?`, eventID, now, id); err != nil {
			return "", err
		}
		for _, rid := range resourceIDs {
			if _, err := tx.ExecContext(ctx, `INSERT INTO resource_claim_epochs(id,project_id,resource_id,acquired_event_id,acquired_at) VALUES (?,?,?,?,?)`, uuid.NewString(), id, rid, eventID, now); err != nil {
				return "", err
			}
		}
		return eventID, nil
	})
}

func (s *SQLite) ObserveProjectResources(ctx context.Context, id, expected string) (domain.Project, error) {
	project, err := s.GetProject(ctx, id)
	if err != nil {
		return project, err
	}
	if project.HeadEventID != expected {
		return project, &domain.StaleProjectHead{ProjectID: id, Expected: expected, Current: project.HeadEventID}
	}
	observationCtx := domain.WithoutMutation(ctx)
	for _, resource := range project.Resources {
		if _, err := s.CheckProjectResource(observationCtx, id, resource.ID); err != nil {
			return project, err
		}
	}
	current, err := s.GetProject(ctx, id)
	if err != nil {
		return current, err
	}
	for head := current.HeadEventID; head != expected; {
		var previous, eventType string
		if err := s.db.QueryRowContext(ctx, `SELECT previous_event_id,event_type FROM project_events WHERE event_id=? AND project_id=?`, head, id).Scan(&previous, &eventType); err != nil {
			return current, &domain.StaleProjectHead{ProjectID: id, Expected: expected, Current: current.HeadEventID}
		}
		if eventType != "project.resource.health" {
			return current, &domain.StaleProjectHead{ProjectID: id, Expected: expected, Current: current.HeadEventID}
		}
		head = previous
	}
	return current, nil
}

func (s *SQLite) BeginCloseProject(ctx context.Context, id, expected string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.close.begin", map[string]any{}); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, archived bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectOpen {
			return "", fmt.Errorf("begin close: %w", domain.ErrProjectState)
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.closing", map[string]any{}, now)
		if err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET lifecycle='closing',head_event_id=?,updated_at=? WHERE id=?`, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) FinalizeCloseProject(ctx context.Context, id, expected string, forced bool, runtimeObservation string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.close.finalize", map[string]any{"forced": forced, "runtime_observation": runtimeObservation}); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, archived bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectClosing {
			return "", fmt.Errorf("finalize close: %w", domain.ErrProjectState)
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.closed", map[string]any{"forced": forced, "runtime_observation": runtimeObservation}, now)
		if err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE resource_claim_epochs SET released_event_id=?,released_at=? WHERE project_id=? AND released_event_id IS NULL`, eventID, now, id); err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE project_assignment_epochs SET state='ended',ended_event_id=?,ended_at=?,forced=? WHERE project_id=? AND ended_event_id IS NULL`, eventID, now, forced, id); err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET lifecycle='closed',head_event_id=?,updated_at=? WHERE id=?`, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) SetProjectArchived(ctx context.Context, id, expected string, archived bool) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.archive.set", map[string]any{"archived": archived}); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, current bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectClosed {
			return "", fmt.Errorf("archive: %w", domain.ErrProjectState)
		}
		if current == archived {
			return "", fmt.Errorf("archive: %w", domain.ErrProjectState)
		}
		typeName := "project.archived"
		if !archived {
			typeName = "project.unarchived"
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, typeName, map[string]any{}, now)
		if err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET archived=?,head_event_id=?,updated_at=? WHERE id=?`, archived, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) UpdateProjectMetadata(ctx context.Context, id, expected, name, brief string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.metadata.update", map[string]any{"name": name, "brief": brief}); remote {
		return project, err
	}
	name = strings.TrimSpace(name)
	if name == "" || len(name) > 200 || strings.ContainsAny(name, "\r\n\x00") {
		return domain.Project{}, errors.New("project name must be non-empty and at most 200 bytes without line breaks")
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, _ domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.metadata.updated", map[string]any{"name": name, "brief": brief}, now)
		if err != nil {
			return "", err
		}
		if _, err := tx.ExecContext(ctx, `UPDATE projects SET name=?,brief=?,head_event_id=?,updated_at=? WHERE id=?`, name, brief, eventID, now, id); err != nil {
			return "", err
		}
		if _, err := tx.ExecContext(ctx, `UPDATE mailboxes SET label=? WHERE id=(SELECT mailbox_id FROM projects WHERE id=?)`, name, id); err != nil {
			return "", err
		}
		return eventID, nil
	})
}

func (s *SQLite) AddProjectPath(ctx context.Context, id, expected string, input domain.ProjectPathInput, makePrimary bool) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.resource.add", map[string]any{"path": input, "primary": makePrimary}); remote {
		return project, err
	}
	path, err := canonicalizeProjectPath(input.DisplayPath)
	if err != nil {
		s.recordProjectAuditFailure(ctx, id, expected, err)
		return domain.Project{}, err
	}
	resourceID := uuid.NewString()
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		if lifecycle == domain.ProjectClosing || lifecycle == domain.ProjectPreparing {
			return "", fmt.Errorf("add resource: %w", domain.ErrProjectState)
		}
		if lifecycle == domain.ProjectOpen {
			if err := checkPathConflictsTx(ctx, tx, id, []canonicalPath{path}); err != nil {
				return "", err
			}
		}
		var priorPrimary sql.NullString
		if err := tx.QueryRowContext(ctx, `SELECT primary_resource_id FROM projects WHERE id=?`, id).Scan(&priorPrimary); err != nil {
			return "", err
		}
		effectivePrimary := makePrimary || !priorPrimary.Valid
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.resource.added", map[string]any{"resource_id": resourceID, "kind": "path", "display_locator": path.display, "canonical_locator": path.canonical, "primary": effectivePrimary, "health": path.health, "health_details": path.details, "last_checked_at": time.UnixMilli(now).UTC()}, now)
		if err != nil {
			return "", err
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO resources(id,kind,home_installation_id,display_locator,canonical_locator,created_at) SELECT ?,'path',home_installation_id,?,?,? FROM projects WHERE id=?`, resourceID, path.display, path.canonical, now, id); err != nil {
			return "", err
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO project_resources(project_id,resource_id,added_event_id) VALUES (?,?,?)`, id, resourceID, eventID); err != nil {
			return "", err
		}
		details, _ := json.Marshal(path.details)
		if _, err := tx.ExecContext(ctx, `INSERT INTO resource_health(resource_id,state,details_json,last_checked_at) VALUES (?,?,?,?)`, resourceID, path.health, string(details), now); err != nil {
			return "", err
		}
		if lifecycle == domain.ProjectOpen {
			if _, err := tx.ExecContext(ctx, `INSERT INTO resource_claim_epochs(id,project_id,resource_id,acquired_event_id,acquired_at) VALUES (?,?,?,?,?)`, uuid.NewString(), id, resourceID, eventID, now); err != nil {
				return "", err
			}
		}
		if effectivePrimary {
			_, err = tx.ExecContext(ctx, `UPDATE projects SET primary_resource_id=?,head_event_id=?,updated_at=? WHERE id=?`, resourceID, eventID, now, id)
		} else {
			_, err = tx.ExecContext(ctx, `UPDATE projects SET primary_resource_id=COALESCE(primary_resource_id,?),head_event_id=?,updated_at=? WHERE id=?`, resourceID, eventID, now, id)
		}
		return eventID, err
	})
}

func (s *SQLite) recordProjectAuditFailure(ctx context.Context, projectID, expected string, failure error) {
	operation, requestID := "project.mutation", ""
	if mutation, ok := domain.MutationFromContext(ctx); ok {
		operation, requestID = mutation.Method, mutation.ID
	}
	current := ""
	_ = s.db.QueryRowContext(context.Background(), `SELECT head_event_id FROM projects WHERE id=?`, projectID).Scan(&current)
	details := map[string]any{"error": failure.Error()}
	var conflict *domain.ProjectConflict
	if errors.As(failure, &conflict) {
		details["requested_display_path"], details["requested_path"], details["conflicting_project_id"], details["conflicting_display_path"], details["conflicting_path"], details["overlap"] = conflict.RequestedDisplay, conflict.RequestedPath, conflict.ConflictingProject, conflict.ConflictingDisplay, conflict.ConflictingPath, conflict.Overlap
	}
	if stale := new(domain.StaleProjectHead); errors.As(failure, &stale) {
		details["stale_expected"], details["stale_current"] = stale.Expected, stale.Current
	}
	var lifecycle, assignedAgent, assignmentState string
	_ = s.db.QueryRowContext(context.Background(), `SELECT lifecycle FROM projects WHERE id=?`, projectID).Scan(&lifecycle)
	_ = s.db.QueryRowContext(context.Background(), `SELECT agent_name,state FROM project_assignment_epochs WHERE project_id=? AND ended_event_id IS NULL`, projectID).Scan(&assignedAgent, &assignmentState)
	details["requesting_human_device"] = s.signer.InstallationID
	details["proposed_agent"] = ""
	details["project_lifecycle"] = lifecycle
	details["assigned_agent"] = assignedAgent
	details["assignment_state"] = assignmentState
	details["runtime_observation"] = "unknown"
	raw, _ := json.Marshal(details)
	_, _ = s.db.ExecContext(context.Background(), `INSERT INTO project_audit_log(operation,request_id,project_id,home_installation_id,expected_head_event_id,current_head_event_id,outcome,details_json,created_at) VALUES (?,?,?,?,?,?, 'rejected',?,?)`, operation, requestID, projectID, s.signer.InstallationID, expected, current, string(raw), s.now().UTC().UnixMilli())
}

func (s *SQLite) RemoveProjectResource(ctx context.Context, id, expected, resourceID string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.resource.remove", map[string]any{"resource_id": resourceID}); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		if lifecycle == domain.ProjectClosing || lifecycle == domain.ProjectPreparing {
			return "", fmt.Errorf("remove resource: %w", domain.ErrProjectState)
		}
		var current int
		if err := tx.QueryRowContext(ctx, `SELECT count(*) FROM project_resources WHERE project_id=? AND resource_id=? AND removed_event_id IS NULL`, id, resourceID).Scan(&current); err != nil {
			return "", err
		}
		if current == 0 {
			return "", domain.ErrResourceNotFound
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.resource.removed", map[string]any{"resource_id": resourceID, "assigned": projectHasAssignmentTx(ctx, tx, id)}, now)
		if err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE project_resources SET removed_event_id=? WHERE project_id=? AND resource_id=? AND removed_event_id IS NULL`, eventID, id, resourceID); err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE resource_claim_epochs SET released_event_id=?,released_at=? WHERE project_id=? AND resource_id=? AND released_event_id IS NULL`, eventID, now, id, resourceID); err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET primary_resource_id=CASE WHEN primary_resource_id=? THEN (SELECT resource_id FROM project_resources WHERE project_id=? AND removed_event_id IS NULL ORDER BY rowid LIMIT 1) ELSE primary_resource_id END,head_event_id=?,updated_at=? WHERE id=?`, resourceID, id, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) ReplaceProjectPath(ctx context.Context, id, expected, oldResourceID string, input domain.ProjectPathInput) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.resource.replace", map[string]any{"resource_id": oldResourceID, "path": input}); remote {
		return project, err
	}
	path, err := canonicalizeProjectPath(input.DisplayPath)
	if err != nil {
		return domain.Project{}, err
	}
	newResourceID := uuid.NewString()
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		if lifecycle == domain.ProjectClosing || lifecycle == domain.ProjectPreparing {
			return "", fmt.Errorf("replace resource: %w", domain.ErrProjectState)
		}
		var current int
		if err := tx.QueryRowContext(ctx, `SELECT count(*) FROM project_resources WHERE project_id=? AND resource_id=? AND removed_event_id IS NULL`, id, oldResourceID).Scan(&current); err != nil {
			return "", err
		}
		if current == 0 {
			return "", domain.ErrResourceNotFound
		}
		if lifecycle == domain.ProjectOpen {
			if err := checkPathConflictsTx(ctx, tx, id, []canonicalPath{path}); err != nil {
				return "", err
			}
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.resource.replaced", map[string]any{"old_resource_id": oldResourceID, "new_resource_id": newResourceID, "display_locator": path.display, "canonical_locator": path.canonical, "health": path.health, "health_details": path.details, "last_checked_at": time.UnixMilli(now).UTC()}, now)
		if err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `INSERT INTO resources(id,kind,home_installation_id,display_locator,canonical_locator,created_at) SELECT ?,'path',home_installation_id,?,?,? FROM projects WHERE id=?`, newResourceID, path.display, path.canonical, now, id); err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `INSERT INTO project_resources(project_id,resource_id,added_event_id) VALUES (?,?,?)`, id, newResourceID, eventID); err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE project_resources SET removed_event_id=? WHERE project_id=? AND resource_id=? AND removed_event_id IS NULL`, eventID, id, oldResourceID); err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE resource_claim_epochs SET released_event_id=?,released_at=? WHERE project_id=? AND resource_id=? AND released_event_id IS NULL`, eventID, now, id, oldResourceID); err != nil {
			return "", err
		}
		if lifecycle == domain.ProjectOpen {
			if _, err = tx.ExecContext(ctx, `INSERT INTO resource_claim_epochs(id,project_id,resource_id,acquired_event_id,acquired_at) VALUES (?,?,?,?,?)`, uuid.NewString(), id, newResourceID, eventID, now); err != nil {
				return "", err
			}
		}
		details, _ := json.Marshal(path.details)
		if _, err = tx.ExecContext(ctx, `INSERT INTO resource_health(resource_id,state,details_json,last_checked_at) VALUES (?,?,?,?)`, newResourceID, path.health, string(details), now); err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET primary_resource_id=CASE WHEN primary_resource_id=? THEN ? ELSE primary_resource_id END,head_event_id=?,updated_at=? WHERE id=?`, oldResourceID, newResourceID, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) SetProjectPrimaryResource(ctx context.Context, id, expected, resourceID string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.resource.primary", map[string]any{"resource_id": resourceID}); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, _ domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		var current int
		if err := tx.QueryRowContext(ctx, `SELECT count(*) FROM project_resources pr JOIN resources r ON r.id=pr.resource_id WHERE pr.project_id=? AND pr.resource_id=? AND pr.removed_event_id IS NULL AND r.kind='path'`, id, resourceID).Scan(&current); err != nil {
			return "", err
		}
		if current == 0 {
			return "", domain.ErrResourceNotFound
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.primary-resource.changed", map[string]any{"resource_id": resourceID}, now)
		if err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET primary_resource_id=?,head_event_id=?,updated_at=? WHERE id=?`, resourceID, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) CheckProjectResource(ctx context.Context, projectID, resourceID string) (domain.ProjectResource, error) {
	if replica, err := getProjectReplica(ctx, s.db, projectID); err == nil {
		var resource domain.ProjectResource
		for _, candidate := range replica.Resources {
			if candidate.ID == resourceID {
				resource = candidate
				break
			}
		}
		if resource.ID == "" {
			return resource, domain.ErrResourceNotFound
		}
		queued, err := s.QueueProjectCommand(ctx, domain.ProjectCommand{ProjectID: projectID, ExpectedHead: replica.HeadEventID, Operation: "project.resource.check", Body: []byte(`{"resource_id":"` + resourceID + `"}`)})
		if queued.PendingCommand != nil {
			resource.PendingCommand = queued.PendingCommand
		}
		return resource, err
	}
	var resource domain.ProjectResource
	err := s.db.QueryRowContext(ctx, `SELECT r.id,r.kind,r.home_installation_id,r.display_locator,r.canonical_locator FROM project_resources pr JOIN resources r ON r.id=pr.resource_id WHERE pr.project_id=? AND pr.resource_id=? AND pr.removed_event_id IS NULL`, projectID, resourceID).Scan(&resource.ID, &resource.Kind, &resource.HomeInstallation, &resource.DisplayLocator, &resource.CanonicalLocator)
	if errors.Is(err, sql.ErrNoRows) {
		return resource, domain.ErrResourceNotFound
	}
	if err != nil {
		return resource, err
	}
	if resource.Kind != "path" {
		return resource, fmt.Errorf("unknown resource kind %q", resource.Kind)
	}
	checked, checkErr := canonicalizeProjectPath(resource.DisplayLocator)
	now := s.now().UTC()
	details := checked.details
	if checkErr != nil {
		checked.health = domain.ResourceMalformed
		details = map[string]string{"error": checkErr.Error()}
	}
	if checked.canonical != "" && checked.canonical != resource.CanonicalLocator {
		checked.health = domain.ResourceMalformed
		details = map[string]string{"expected_canonical": resource.CanonicalLocator, "observed_canonical": checked.canonical}
	}
	raw, _ := json.Marshal(details)
	account, parents, _, err := s.localAccountAction(ctx, "")
	if err != nil {
		return resource, err
	}
	topics := append(append([]domain.ChangeTopic(nil), canonicalChangeTopics...), domain.TopicProjects)
	result, err := s.commitMutation(ctx, topics, func(tx *sql.Tx) (any, error) {
		var priorState domain.ResourceHealthState
		var priorDetails, projectName, mailboxID string
		if err := tx.QueryRowContext(ctx, `SELECT h.state,h.details_json,p.name,p.mailbox_id FROM resource_health h JOIN project_resources pr ON pr.resource_id=h.resource_id AND pr.removed_event_id IS NULL JOIN projects p ON p.id=pr.project_id WHERE p.id=? AND h.resource_id=?`, projectID, resourceID).Scan(&priorState, &priorDetails, &projectName, &mailboxID); errors.Is(err, sql.ErrNoRows) {
			return nil, domain.ErrResourceNotFound
		} else if err != nil {
			return nil, err
		}
		if _, err := tx.ExecContext(ctx, `UPDATE resource_health SET state=?,details_json=?,last_checked_at=? WHERE resource_id=?`, checked.health, string(raw), now.UnixMilli(), resourceID); err != nil {
			return nil, err
		}
		if priorState != checked.health || priorDetails != string(raw) {
			messageID, idErr := uuid.NewV7()
			if idErr != nil {
				return nil, idErr
			}
			body := fmt.Sprintf("Project resource condition changed to %s: %s", checked.health, resource.DisplayLocator)
			if checked.health == domain.ResourceHealthy {
				body = "Project resource recovered: " + resource.DisplayLocator
			} else if priorState == domain.ResourceHealthy {
				body = fmt.Sprintf("Project resource degraded (%s): %s", checked.health, resource.DisplayLocator)
			}
			noticeDetails := fmt.Sprintf("Kind: notice\nProject: %s\nResource: %s\nPrevious health: %s\nCurrent health: %s", projectID, resourceID, priorState, checked.health)
			if len(details) != 0 {
				noticeDetails += "\nHealth details: " + string(raw)
			}
			payload, _ := event.MarshalPayload(event.TextPayload{MessageID: messageID.String(), Body: body, Details: noticeDetails, Purpose: model.MessagePurposeSystemNotice, ActorLabel: "HQ · " + projectName})
			content := event.Content{Type: event.TypeQuestion, Sender: s.localAddress(mailboxID), Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: payload}
			signed, signErr := s.signContents(ctx, []event.Content{content}, []time.Time{now})
			if signErr != nil {
				return nil, signErr
			}
			if _, ingestErr := s.ingestCanonicalTx(ctx, tx, signed, true); ingestErr != nil {
				return nil, ingestErr
			}
			var head string
			if err := tx.QueryRowContext(ctx, `SELECT head_event_id FROM projects WHERE id=?`, projectID).Scan(&head); err != nil {
				return nil, err
			}
			healthEvent, err := s.appendProjectEventTx(ctx, tx, projectID, head, "project.resource.health", map[string]any{"resource_id": resourceID, "health": checked.health, "health_details": details, "last_checked_at": now}, now.UnixMilli())
			if err != nil {
				return nil, err
			}
			if _, err := tx.ExecContext(ctx, `UPDATE projects SET head_event_id=?,updated_at=? WHERE id=?`, healthEvent, now.UnixMilli(), projectID); err != nil {
				return nil, err
			}
		}
		return nil, nil
	})
	_ = result
	if err != nil {
		return resource, err
	}
	resource.Health, resource.HealthDetails = checked.health, details
	resource.LastCheckedAt = &now
	return resource, nil
}

func projectHasAssignmentTx(ctx context.Context, tx *sql.Tx, id string) bool {
	var count int
	_ = tx.QueryRowContext(ctx, `SELECT count(*) FROM project_assignment_epochs WHERE project_id=? AND ended_event_id IS NULL`, id).Scan(&count)
	return count != 0
}

func (s *SQLite) AssignProject(ctx context.Context, id, expected, agent string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.assignment.assign", map[string]any{"agent": agent}); remote {
		return project, err
	}
	agent = strings.TrimSpace(agent)
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectOpen {
			return "", fmt.Errorf("assign: %w", domain.ErrProjectState)
		}
		var retired bool
		if err := tx.QueryRowContext(ctx, `SELECT retired FROM named_agents WHERE name=?`, agent).Scan(&retired); errors.Is(err, sql.ErrNoRows) {
			return "", domain.ErrAgentNotFound
		} else if err != nil {
			return "", err
		}
		if retired {
			return "", domain.ErrAgentRetired
		}
		if projectHasAssignmentTx(ctx, tx, id) {
			return "", domain.ErrProjectAssigned
		}
		var busy int
		if err := tx.QueryRowContext(ctx, `SELECT count(*) FROM project_assignment_epochs WHERE agent_name=? AND ended_event_id IS NULL`, agent).Scan(&busy); err != nil {
			return "", err
		}
		if busy != 0 {
			return "", domain.ErrAgentAssigned
		}
		assignmentID := uuid.NewString()
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.assignment.configuring", map[string]any{"assignment_id": assignmentID, "agent": agent}, now)
		if err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `INSERT INTO project_assignment_epochs(id,project_id,agent_name,state,started_event_id,started_at) VALUES (?,?,?,'configuring',?,?)`, assignmentID, id, agent, eventID, now); err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET head_event_id=?,updated_at=? WHERE id=?`, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) ActivateProjectAssignment(ctx context.Context, id, expected string, request domain.ActivateProjectAssignmentRequest) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.assignment.activate", request); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectOpen {
			return "", fmt.Errorf("activate assignment: %w", domain.ErrProjectState)
		}
		var assignmentID, agent string
		var state domain.AssignmentState
		if err := tx.QueryRowContext(ctx, `SELECT id,agent_name,state FROM project_assignment_epochs WHERE project_id=? AND ended_event_id IS NULL`, id).Scan(&assignmentID, &agent, &state); errors.Is(err, sql.ErrNoRows) {
			return "", domain.ErrProjectAssigned
		} else if err != nil {
			return "", err
		}
		if state != domain.AssignmentConfiguring {
			return "", fmt.Errorf("activate assignment: %w", domain.ErrProjectState)
		}
		threadID := request.ThreadID
		if threadID != "" {
			var threadProject, threadAgent string
			if err := tx.QueryRowContext(ctx, `SELECT project_id,agent_name FROM project_threads WHERE id=?`, threadID).Scan(&threadProject, &threadAgent); errors.Is(err, sql.ErrNoRows) {
				return "", domain.ErrProjectThreadMismatch
			} else if err != nil {
				return "", err
			}
			if threadProject != id || threadAgent != agent {
				return "", domain.ErrProjectThreadMismatch
			}
		}
		if threadID == "" {
			if strings.TrimSpace(request.Harness) == "" || strings.TrimSpace(request.ExternalThread) == "" {
				return "", errors.New("new project thread needs harness and external thread ID")
			}
			launch, err := filepath.Abs(request.LaunchDirectory)
			if err != nil {
				return "", err
			}
			threadID = uuid.NewString()
			if _, err = tx.ExecContext(ctx, `INSERT INTO project_threads(id,project_id,agent_name,harness,external_thread_id,launch_directory,created_at) VALUES (?,?,?,?,?,?,?)`, threadID, id, agent, request.Harness, request.ExternalThread, filepath.Clean(launch), now); err != nil {
				return "", err
			}
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.assignment.runnable", map[string]any{"assignment_id": assignmentID, "agent": agent, "thread_id": threadID}, now)
		if err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE project_assignment_epochs SET state='runnable',selected_thread_id=? WHERE id=?`, threadID, assignmentID); err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET head_event_id=?,updated_at=? WHERE id=?`, eventID, now, id)
		return eventID, err
	})
}

func (s *SQLite) AbortProjectAssignment(ctx context.Context, id, expected, diagnostic string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.assignment.abort", map[string]any{"diagnostic": diagnostic}); remote {
		return project, err
	}
	return s.UnassignProject(ctx, id, expected, false, diagnostic)
}

func (s *SQLite) BlockProjectAssignment(ctx context.Context, id, expected, diagnostic string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.assignment.block", map[string]any{"diagnostic": diagnostic}); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectOpen {
			return "", fmt.Errorf("block handoff: %w", domain.ErrProjectState)
		}
		var assignmentID, agent string
		var state domain.AssignmentState
		if err := tx.QueryRowContext(ctx, `SELECT id,agent_name,state FROM project_assignment_epochs WHERE project_id=? AND ended_event_id IS NULL`, id).Scan(&assignmentID, &agent, &state); errors.Is(err, sql.ErrNoRows) {
			return "", domain.ErrProjectAssigned
		} else if err != nil {
			return "", err
		}
		if state == domain.AssignmentBlocked {
			return "", fmt.Errorf("block handoff: %w", domain.ErrProjectState)
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.assignment.blocked", map[string]any{"assignment_id": assignmentID, "agent": agent, "diagnostic": diagnostic}, now)
		if err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE project_assignment_epochs SET state='blocked' WHERE id=?`, assignmentID); err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET head_event_id=?,updated_at=? WHERE id=?`, eventID, now, id)
		return eventID, err
	})
}
func (s *SQLite) UnassignProject(ctx context.Context, id, expected string, forced bool, runtimeObservation string) (domain.Project, error) {
	if project, remote, err := s.queueReplicaCommand(ctx, id, expected, "project.assignment.unassign", map[string]any{"forced": forced, "runtime_observation": runtimeObservation}); remote {
		return project, err
	}
	return s.mutateProject(ctx, id, expected, func(tx *sql.Tx, lifecycle domain.ProjectLifecycle, _ bool, head string, now int64) (string, error) {
		if lifecycle != domain.ProjectOpen && lifecycle != domain.ProjectClosing {
			return "", fmt.Errorf("unassign: %w", domain.ErrProjectState)
		}
		var assignmentID, agent string
		if err := tx.QueryRowContext(ctx, `SELECT id,agent_name FROM project_assignment_epochs WHERE project_id=? AND ended_event_id IS NULL`, id).Scan(&assignmentID, &agent); errors.Is(err, sql.ErrNoRows) {
			return "", domain.ErrProjectAssigned
		} else if err != nil {
			return "", err
		}
		eventID, err := s.appendProjectEventTx(ctx, tx, id, head, "project.assignment.ended", map[string]any{"assignment_id": assignmentID, "agent": agent, "forced": forced, "runtime_observation": runtimeObservation}, now)
		if err != nil {
			return "", err
		}
		if _, err = tx.ExecContext(ctx, `UPDATE project_assignment_epochs SET state='ended',ended_event_id=?,ended_at=?,forced=? WHERE id=?`, eventID, now, forced, assignmentID); err != nil {
			return "", err
		}
		_, err = tx.ExecContext(ctx, `UPDATE projects SET head_event_id=?,updated_at=? WHERE id=?`, eventID, now, id)
		return eventID, err
	})
}

type projectMutation func(*sql.Tx, domain.ProjectLifecycle, bool, string, int64) (string, error)

func (s *SQLite) mutateProject(ctx context.Context, id, expected string, action projectMutation) (domain.Project, error) {
	value, err := s.commitMutation(ctx, []domain.ChangeTopic{domain.TopicProjects, domain.TopicAgents}, func(tx *sql.Tx) (any, error) {
		lifecycle, archived, head, err := checkProjectHeadTx(ctx, tx, id, expected)
		if err != nil {
			return nil, err
		}
		now := s.now().UTC().UnixMilli()
		if _, err := action(tx, lifecycle, archived, head, now); err != nil {
			return nil, err
		}
		return getProjectTx(ctx, tx, id)
	})
	if err != nil {
		s.recordProjectAuditFailure(ctx, id, expected, err)
		return domain.Project{}, err
	}
	return value.(domain.Project), nil
}

// Keep deterministic ordering available to callers that assemble resources
// from maps while retaining the human-selected membership order in storage.
func sortProjectResources(resources []domain.ProjectResource) {
	sort.Slice(resources, func(i, j int) bool { return resources[i].ID < resources[j].ID })
}
