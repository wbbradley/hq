package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
)

func (s *SQLite) BeginProjectWorktreeProvision(ctx context.Context, request domain.ProjectWorktreeRequest) (domain.ProjectWorktreeOperation, error) {
	if _, err := uuid.Parse(request.RequestID); err != nil {
		return domain.ProjectWorktreeOperation{}, errors.New("worktree provisioning request ID must be a UUID")
	}
	if request.ProjectID == "" {
		request.ProjectID = uuid.NewString()
	}
	if _, err := uuid.Parse(request.ProjectID); err != nil {
		return domain.ProjectWorktreeOperation{}, errors.New("worktree project ID must be a UUID")
	}
	request.Name, request.Repository, request.Destination, request.Branch = strings.TrimSpace(request.Name), strings.TrimSpace(request.Repository), strings.TrimSpace(request.Destination), strings.TrimSpace(request.Branch)
	if request.Name == "" || request.Repository == "" || request.Destination == "" || request.Branch == "" {
		return domain.ProjectWorktreeOperation{}, errors.New("worktree provisioning requires project name, repository, destination, and branch")
	}
	if request.MergeBase == "" {
		request.MergeBase = "HEAD"
	}
	repository, err := canonicalExistingDirectory(request.Repository)
	if err != nil {
		return domain.ProjectWorktreeOperation{}, fmt.Errorf("canonicalize worktree repository: %w", err)
	}
	destination, err := canonicalizeProjectPath(request.Destination)
	if err != nil {
		return domain.ProjectWorktreeOperation{}, fmt.Errorf("canonicalize worktree destination: %w", err)
	}
	request.Repository, request.Destination = repository.display, destination.display
	raw, err := json.Marshal(request)
	if err != nil {
		return domain.ProjectWorktreeOperation{}, err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return domain.ProjectWorktreeOperation{}, err
	}
	defer tx.Rollback()
	if existing, found, err := getProjectWorktreeOperationTx(ctx, tx, request.RequestID); err != nil {
		return existing, err
	} else if found {
		existingRaw, _ := json.Marshal(existing.Request)
		if string(existingRaw) != string(raw) {
			return existing, domain.ErrMutationConflict
		}
		return existing, nil
	}
	if err := checkPathConflictsTx(ctx, tx, request.ProjectID, []canonicalPath{destination}); err != nil {
		return domain.ProjectWorktreeOperation{}, err
	}
	now := s.now().UTC().UnixMilli()
	if _, err := tx.ExecContext(ctx, `INSERT INTO project_worktree_operations(id,project_id,request_json,canonical_repository,canonical_destination,state,created_at,updated_at) VALUES (?,?,?,?,?,'reserved',?,?)`, request.RequestID, request.ProjectID, string(raw), repository.canonical, destination.canonical, now, now); err != nil {
		return domain.ProjectWorktreeOperation{}, err
	}
	operation, _, err := getProjectWorktreeOperationTx(ctx, tx, request.RequestID)
	if err != nil {
		return operation, err
	}
	if err := tx.Commit(); err != nil {
		return operation, err
	}
	return operation, nil
}

func (s *SQLite) AdvanceProjectWorktreeProvision(ctx context.Context, id, state, diagnostic string) error {
	switch state {
	case "reserved", "worktree-created", "completed", "failed":
	default:
		return errors.New("invalid worktree provisioning state")
	}
	result, err := s.db.ExecContext(ctx, `UPDATE project_worktree_operations SET state=?,last_error=?,updated_at=? WHERE id=?`, state, diagnostic, s.now().UTC().UnixMilli(), id)
	if err != nil {
		return err
	}
	if count, _ := result.RowsAffected(); count != 1 {
		return domain.ErrProjectState
	}
	return nil
}

func getProjectWorktreeOperationTx(ctx context.Context, tx *sql.Tx, id string) (domain.ProjectWorktreeOperation, bool, error) {
	var operation domain.ProjectWorktreeOperation
	var raw string
	var created, updated int64
	err := tx.QueryRowContext(ctx, `SELECT id,project_id,request_json,canonical_repository,canonical_destination,state,last_error,created_at,updated_at FROM project_worktree_operations WHERE id=?`, id).Scan(&operation.ID, &operation.ProjectID, &raw, &operation.CanonicalRepository, &operation.CanonicalDestination, &operation.State, &operation.LastError, &created, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		return operation, false, nil
	}
	if err != nil {
		return operation, false, err
	}
	if err := json.Unmarshal([]byte(raw), &operation.Request); err != nil {
		return operation, false, err
	}
	operation.CreatedAt, operation.UpdatedAt = time.UnixMilli(created).UTC(), time.UnixMilli(updated).UTC()
	return operation, true, nil
}

func canonicalExistingDirectory(value string) (canonicalPath, error) {
	abs, err := filepath.Abs(value)
	if err != nil {
		return canonicalPath{}, err
	}
	display := filepath.Clean(abs)
	info, err := os.Stat(display)
	if err != nil {
		return canonicalPath{}, err
	}
	if !info.IsDir() {
		return canonicalPath{}, errors.New("path is not a directory")
	}
	canonical, err := filepath.EvalSymlinks(display)
	if err != nil {
		return canonicalPath{}, err
	}
	return canonicalPath{display: display, canonical: filepath.Clean(canonical), health: domain.ResourceHealthy, details: map[string]string{}}, nil
}

func checkProvisioningReservationsTx(ctx context.Context, tx *sql.Tx, projectID, skipOperation string, paths []canonicalPath) error {
	rows, err := tx.QueryContext(ctx, `SELECT id,project_id,canonical_destination,request_json FROM project_worktree_operations WHERE state IN ('reserved','worktree-created') AND id<>?`, skipOperation)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var id, otherProject, canonical, raw string
		if err := rows.Scan(&id, &otherProject, &canonical, &raw); err != nil {
			return err
		}
		var request domain.ProjectWorktreeRequest
		_ = json.Unmarshal([]byte(raw), &request)
		for _, path := range paths {
			if overlap := pathOverlap(path.canonical, canonical); overlap != "" {
				return &domain.ProjectConflict{RequestedProjectID: projectID, RequestedDisplay: path.display, RequestedPath: path.canonical, ConflictingProject: otherProject, ConflictingDisplay: request.Destination, ConflictingPath: canonical, Overlap: overlap}
			}
		}
	}
	return rows.Err()
}
