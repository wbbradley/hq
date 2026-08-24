package store

import (
	"context"
	"database/sql"
	"errors"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
)

func (s *SQLite) BeginProjectActivation(ctx context.Context, operationID, projectID, expectedHead, agentName string) (domain.ProjectActivationOperation, error) {
	if _, err := uuid.Parse(operationID); err != nil {
		return domain.ProjectActivationOperation{}, errors.New("project activation operation ID must be a UUID")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return domain.ProjectActivationOperation{}, err
	}
	defer tx.Rollback()
	if existing, found, err := getProjectActivationTx(ctx, tx, operationID); err != nil {
		return existing, err
	} else if found {
		if existing.ProjectID != projectID || existing.AgentName != agentName {
			return existing, domain.ErrMutationConflict
		}
		return existing, nil
	}
	lifecycle, archived, _, err := checkProjectHeadTx(ctx, tx, projectID, expectedHead)
	if err != nil {
		return domain.ProjectActivationOperation{}, err
	}
	if archived || (lifecycle != domain.ProjectOpen && lifecycle != domain.ProjectClosed) {
		return domain.ProjectActivationOperation{}, domain.ErrProjectState
	}
	var retired bool
	if err := tx.QueryRowContext(ctx, `SELECT retired FROM named_agents WHERE name=?`, agentName).Scan(&retired); errors.Is(err, sql.ErrNoRows) {
		return domain.ProjectActivationOperation{}, domain.ErrAgentNotFound
	} else if err != nil {
		return domain.ProjectActivationOperation{}, err
	}
	if retired {
		return domain.ProjectActivationOperation{}, domain.ErrAgentRetired
	}
	now := s.now().UTC().UnixMilli()
	if _, err := tx.ExecContext(ctx, `INSERT INTO project_activation_operations(id,project_id,agent_name,prior_lifecycle,state,created_at,updated_at) VALUES (?,?,?,?,'preparing',?,?)`, operationID, projectID, agentName, lifecycle, now, now); err != nil {
		return domain.ProjectActivationOperation{}, err
	}
	operation, _, err := getProjectActivationTx(ctx, tx, operationID)
	if err != nil {
		return operation, err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicProjects, domain.TopicAgents})
	if err != nil {
		return operation, err
	}
	if err := tx.Commit(); err != nil {
		return operation, err
	}
	s.notifyChange(change)
	return operation, nil
}

func (s *SQLite) SetProjectActivationAssignment(ctx context.Context, operationID, assignmentID string) error {
	now := s.now().UTC().UnixMilli()
	result, err := s.db.ExecContext(ctx, `UPDATE project_activation_operations SET assignment_id=?,state='configuring',updated_at=? WHERE id=? AND state='preparing' AND EXISTS(SELECT 1 FROM project_assignment_epochs e WHERE e.id=? AND e.project_id=project_activation_operations.project_id AND e.agent_name=project_activation_operations.agent_name AND e.state='configuring' AND e.ended_event_id IS NULL)`, assignmentID, now, operationID, assignmentID)
	if err != nil {
		return err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		return domain.ErrProjectState
	}
	return nil
}

func (s *SQLite) CompleteProjectActivation(ctx context.Context, operationID string) error {
	now := s.now().UTC().UnixMilli()
	result, err := s.db.ExecContext(ctx, `UPDATE project_activation_operations SET state='runnable',updated_at=? WHERE id=? AND state='configuring' AND EXISTS(SELECT 1 FROM project_assignment_epochs e WHERE e.id=project_activation_operations.assignment_id AND e.state='runnable' AND e.ended_event_id IS NULL)`, now, operationID)
	if err != nil {
		return err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		return domain.ErrProjectState
	}
	return nil
}

func (s *SQLite) FailProjectActivation(ctx context.Context, operationID, diagnostic string) error {
	result, err := s.db.ExecContext(ctx, `UPDATE project_activation_operations SET state='failed',last_error=?,updated_at=? WHERE id=? AND state IN ('preparing','configuring')`, diagnostic, s.now().UTC().UnixMilli(), operationID)
	if err != nil {
		return err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		var state string
		if lookupErr := s.db.QueryRowContext(ctx, `SELECT state FROM project_activation_operations WHERE id=?`, operationID).Scan(&state); lookupErr == nil && (state == "failed" || state == "runnable") {
			return nil
		}
		return domain.ErrProjectState
	}
	return nil
}

func (s *SQLite) ListIncompleteProjectActivations(ctx context.Context) ([]domain.ProjectActivationOperation, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT id,project_id,agent_name,prior_lifecycle,assignment_id,state,last_error,created_at,updated_at FROM project_activation_operations WHERE state IN ('preparing','configuring') ORDER BY created_at,id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []domain.ProjectActivationOperation
	for rows.Next() {
		var operation domain.ProjectActivationOperation
		var assignment sql.NullString
		var created, updated int64
		if err := rows.Scan(&operation.ID, &operation.ProjectID, &operation.AgentName, &operation.PriorLifecycle, &assignment, &operation.State, &operation.LastError, &created, &updated); err != nil {
			return nil, err
		}
		operation.AssignmentID = assignment.String
		operation.CreatedAt = time.UnixMilli(created).UTC()
		operation.UpdatedAt = time.UnixMilli(updated).UTC()
		result = append(result, operation)
	}
	return result, rows.Err()
}

func getProjectActivationTx(ctx context.Context, tx *sql.Tx, id string) (domain.ProjectActivationOperation, bool, error) {
	var operation domain.ProjectActivationOperation
	var assignment sql.NullString
	var created, updated int64
	err := tx.QueryRowContext(ctx, `SELECT id,project_id,agent_name,prior_lifecycle,assignment_id,state,last_error,created_at,updated_at FROM project_activation_operations WHERE id=?`, id).Scan(&operation.ID, &operation.ProjectID, &operation.AgentName, &operation.PriorLifecycle, &assignment, &operation.State, &operation.LastError, &created, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		return operation, false, nil
	}
	if err != nil {
		return operation, false, err
	}
	operation.AssignmentID = assignment.String
	operation.CreatedAt = time.UnixMilli(created).UTC()
	operation.UpdatedAt = time.UnixMilli(updated).UTC()
	return operation, true, nil
}

func (s *SQLite) BeginProjectRuntimeOperation(ctx context.Context, requested domain.ProjectRuntimeOperation) (domain.ProjectRuntimeOperation, error) {
	if _, err := uuid.Parse(requested.ID); err != nil {
		return domain.ProjectRuntimeOperation{}, errors.New("project runtime operation ID must be a UUID")
	}
	if requested.Kind != "close" && requested.Kind != "handoff" {
		return domain.ProjectRuntimeOperation{}, errors.New("project runtime operation kind must be close or handoff")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return domain.ProjectRuntimeOperation{}, err
	}
	defer tx.Rollback()
	if existing, found, err := getProjectRuntimeOperationTx(ctx, tx, requested.ID); err != nil {
		return existing, err
	} else if found {
		if existing.Kind != requested.Kind || existing.ProjectID != requested.ProjectID || existing.ExpectedHead != requested.ExpectedHead || existing.TargetAgent != requested.TargetAgent || existing.Force != requested.Force || existing.Archive != requested.Archive {
			return existing, domain.ErrMutationConflict
		}
		return existing, nil
	}
	var unfinished string
	if err := tx.QueryRowContext(ctx, `SELECT id FROM project_runtime_operations WHERE project_id=? AND state IN ('started','closing','unassigned','activating')`, requested.ProjectID).Scan(&unfinished); err == nil {
		return domain.ProjectRuntimeOperation{}, domain.ErrProjectCommandPending
	} else if !errors.Is(err, sql.ErrNoRows) {
		return domain.ProjectRuntimeOperation{}, err
	}
	if _, _, current, err := checkProjectHeadTx(ctx, tx, requested.ProjectID, requested.ExpectedHead); err != nil {
		return domain.ProjectRuntimeOperation{}, err
	} else {
		requested.CurrentHead = current
	}
	now := s.now().UTC().UnixMilli()
	if _, err := tx.ExecContext(ctx, `INSERT INTO project_runtime_operations(id,kind,project_id,expected_head_event_id,current_head_event_id,target_agent,force,archive,state,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,"started",?,?)`, requested.ID, requested.Kind, requested.ProjectID, requested.ExpectedHead, requested.CurrentHead, requested.TargetAgent, requested.Force, requested.Archive, now, now); err != nil {
		return domain.ProjectRuntimeOperation{}, err
	}
	operation, _, err := getProjectRuntimeOperationTx(ctx, tx, requested.ID)
	if err != nil {
		return operation, err
	}
	if err := tx.Commit(); err != nil {
		return operation, err
	}
	return operation, nil
}

func (s *SQLite) AdvanceProjectRuntimeOperation(ctx context.Context, id, state, currentHead, diagnostic string) error {
	switch state {
	case "started", "closing", "unassigned", "activating", "completed", "blocked", "failed":
	default:
		return errors.New("invalid project runtime operation state")
	}
	result, err := s.db.ExecContext(ctx, `UPDATE project_runtime_operations SET state=?,current_head_event_id=?,last_error=?,updated_at=? WHERE id=?`, state, currentHead, diagnostic, s.now().UTC().UnixMilli(), id)
	if err != nil {
		return err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		return domain.ErrProjectState
	}
	return nil
}

func getProjectRuntimeOperationTx(ctx context.Context, tx *sql.Tx, id string) (domain.ProjectRuntimeOperation, bool, error) {
	var operation domain.ProjectRuntimeOperation
	var created, updated int64
	err := tx.QueryRowContext(ctx, `SELECT id,kind,project_id,expected_head_event_id,current_head_event_id,target_agent,force,archive,state,last_error,created_at,updated_at FROM project_runtime_operations WHERE id=?`, id).Scan(&operation.ID, &operation.Kind, &operation.ProjectID, &operation.ExpectedHead, &operation.CurrentHead, &operation.TargetAgent, &operation.Force, &operation.Archive, &operation.State, &operation.LastError, &created, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		return operation, false, nil
	}
	if err != nil {
		return operation, false, err
	}
	operation.CreatedAt = time.UnixMilli(created).UTC()
	operation.UpdatedAt = time.UnixMilli(updated).UTC()
	return operation, true, nil
}

func (s *SQLite) BeginAgentRetirement(ctx context.Context, requested domain.AgentRetirementOperation) (domain.AgentRetirementOperation, error) {
	if _, err := uuid.Parse(requested.ID); err != nil {
		return domain.AgentRetirementOperation{}, errors.New("agent retirement operation ID must be a UUID")
	}
	requested.AgentName = strings.TrimSpace(requested.AgentName)
	if requested.AgentName == "" {
		return domain.AgentRetirementOperation{}, errors.New("agent retirement requires an agent")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return domain.AgentRetirementOperation{}, err
	}
	defer tx.Rollback()
	if existing, found, err := getAgentRetirementTx(ctx, tx, requested.ID); err != nil {
		return existing, err
	} else if found {
		if existing.AgentName != requested.AgentName || requested.ProjectID != "" && existing.ProjectID != requested.ProjectID || existing.Force != requested.Force {
			return existing, domain.ErrMutationConflict
		}
		return existing, nil
	}
	var exists int
	if err := tx.QueryRowContext(ctx, `SELECT count(*) FROM named_agents WHERE name=?`, requested.AgentName).Scan(&exists); err != nil {
		return domain.AgentRetirementOperation{}, err
	}
	if exists == 0 {
		return domain.AgentRetirementOperation{}, domain.ErrAgentNotFound
	}
	var unfinished string
	if err := tx.QueryRowContext(ctx, `SELECT id FROM agent_retirement_operations WHERE agent_name=? AND state IN ('started','quiesced','unassigned')`, requested.AgentName).Scan(&unfinished); err == nil {
		return domain.AgentRetirementOperation{}, domain.ErrProjectCommandPending
	} else if !errors.Is(err, sql.ErrNoRows) {
		return domain.AgentRetirementOperation{}, err
	}
	now := s.now().UTC().UnixMilli()
	if _, err := tx.ExecContext(ctx, `INSERT INTO agent_retirement_operations(id,agent_name,project_id,force,state,created_at,updated_at) VALUES (?,?,?,?,'started',?,?)`, requested.ID, requested.AgentName, requested.ProjectID, requested.Force, now, now); err != nil {
		return domain.AgentRetirementOperation{}, err
	}
	operation, _, err := getAgentRetirementTx(ctx, tx, requested.ID)
	if err != nil {
		return operation, err
	}
	if err := tx.Commit(); err != nil {
		return operation, err
	}
	return operation, nil
}

func (s *SQLite) AdvanceAgentRetirement(ctx context.Context, id, state, diagnostic string) error {
	switch state {
	case "started", "quiesced", "unassigned", "completed", "blocked", "failed":
	default:
		return errors.New("invalid agent retirement state")
	}
	result, err := s.db.ExecContext(ctx, `UPDATE agent_retirement_operations SET state=?,last_error=?,updated_at=? WHERE id=?`, state, diagnostic, s.now().UTC().UnixMilli(), id)
	if err != nil {
		return err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		return domain.ErrProjectState
	}
	return nil
}

func getAgentRetirementTx(ctx context.Context, tx *sql.Tx, id string) (domain.AgentRetirementOperation, bool, error) {
	var operation domain.AgentRetirementOperation
	var created, updated int64
	err := tx.QueryRowContext(ctx, `SELECT id,agent_name,project_id,force,state,last_error,created_at,updated_at FROM agent_retirement_operations WHERE id=?`, id).Scan(&operation.ID, &operation.AgentName, &operation.ProjectID, &operation.Force, &operation.State, &operation.LastError, &created, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		return operation, false, nil
	}
	if err != nil {
		return operation, false, err
	}
	operation.CreatedAt, operation.UpdatedAt = time.UnixMilli(created).UTC(), time.UnixMilli(updated).UTC()
	return operation, true, nil
}
