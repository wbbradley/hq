package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/wbbradley/hq/internal/model"
	_ "modernc.org/sqlite"
)

const schema = `
CREATE TABLE IF NOT EXISTS questions (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) = 36),
    directory TEXT NOT NULL CHECK(length(directory) > 0),
    session_id TEXT NOT NULL CHECK(length(session_id) > 0),
    prompt TEXT NOT NULL CHECK(length(prompt) > 0),
    details TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'answered', 'cancelled')),
    response TEXT,
    created_at INTEGER NOT NULL CHECK(created_at > 0),
    answered_at INTEGER,
    completed_at INTEGER,
    delivery_token TEXT,
    delivery_lease_until INTEGER,
    CHECK((status = 'answered') = (response IS NOT NULL)),
    CHECK((answered_at IS NOT NULL) = (status = 'answered')),
    CHECK(completed_at IS NULL OR status = 'answered'),
    CHECK((delivery_token IS NULL) = (delivery_lease_until IS NULL))
) STRICT;
CREATE INDEX IF NOT EXISTS questions_queue
    ON questions(directory, session_id, status, created_at, id);
CREATE INDEX IF NOT EXISTS questions_answered
    ON questions(status, completed_at, answered_at, id);
CREATE INDEX IF NOT EXISTS questions_history
    ON questions(created_at DESC, id DESC) WHERE status != 'pending';
PRAGMA user_version = 1;
`

type SQLite struct {
	db *sql.DB
}

func DefaultPath() (string, error) {
	if state := os.Getenv("XDG_STATE_HOME"); state != "" {
		return filepath.Join(state, "hq", "hq.db"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("find home directory: %w", err)
	}
	return filepath.Join(home, ".local", "state", "hq", "hq.db"), nil
}

func Open(path string) (*SQLite, error) {
	if path == "" {
		var err error
		path, err = DefaultPath()
		if err != nil {
			return nil, err
		}
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, fmt.Errorf("create state directory: %w", err)
	}
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open database: %w", err)
	}
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	s := &SQLite{db: db}
	if err := s.configure(context.Background()); err != nil {
		db.Close()
		return nil, err
	}
	if err := os.Chmod(path, 0o600); err != nil {
		db.Close()
		return nil, fmt.Errorf("secure database: %w", err)
	}
	return s, nil
}

func (s *SQLite) configure(ctx context.Context) error {
	pragmas := []string{
		"PRAGMA foreign_keys = ON",
		"PRAGMA busy_timeout = 5000",
		"PRAGMA journal_mode = WAL",
		"PRAGMA synchronous = FULL",
		"PRAGMA trusted_schema = OFF",
		"PRAGMA temp_store = MEMORY",
	}
	for _, statement := range pragmas {
		if _, err := s.db.ExecContext(ctx, statement); err != nil {
			return fmt.Errorf("configure sqlite (%s): %w", statement, err)
		}
	}
	if _, err := s.db.ExecContext(ctx, schema); err != nil {
		return fmt.Errorf("create schema: %w", err)
	}
	return nil
}

func (s *SQLite) Close() error { return s.db.Close() }

func (s *SQLite) Create(ctx context.Context, q model.Question) error {
	_, err := s.db.ExecContext(ctx, `
INSERT INTO questions(id, directory, session_id, prompt, details, status, created_at)
VALUES (?, ?, ?, ?, ?, 'pending', ?)`, q.ID, q.Directory, q.SessionID, q.Prompt, q.Details, q.CreatedAt.UnixMilli())
	if err != nil {
		return fmt.Errorf("create question: %w", err)
	}
	return nil
}

const columns = `id, directory, session_id, prompt, details, status, response,
created_at, answered_at, completed_at`

type scanner interface {
	Scan(...any) error
}

func scanQuestion(row scanner) (model.Question, error) {
	var q model.Question
	var response sql.NullString
	var created int64
	var answered, completed sql.NullInt64
	err := row.Scan(&q.ID, &q.Directory, &q.SessionID, &q.Prompt, &q.Details, &q.Status,
		&response, &created, &answered, &completed)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return q, ErrNotFound
		}
		return q, err
	}
	q.CreatedAt = time.UnixMilli(created).UTC()
	if response.Valid {
		q.Response = &response.String
	}
	if answered.Valid {
		t := time.UnixMilli(answered.Int64).UTC()
		q.AnsweredAt = &t
	}
	if completed.Valid {
		t := time.UnixMilli(completed.Int64).UTC()
		q.CompletedAt = &t
	}
	return q, nil
}

func (s *SQLite) Get(ctx context.Context, id string) (model.Question, error) {
	q, err := scanQuestion(s.db.QueryRowContext(ctx, `SELECT `+columns+` FROM questions WHERE id = ?`, id))
	if err != nil && !errors.Is(err, ErrNotFound) {
		return q, fmt.Errorf("get question: %w", err)
	}
	return q, err
}

func (s *SQLite) List(ctx context.Context, f model.Filter) ([]model.Question, error) {
	var where []string
	var args []any
	if f.Directory != "" {
		where = append(where, "directory = ?")
		args = append(args, f.Directory)
	}
	if f.SessionID != "" {
		where = append(where, "session_id = ?")
		args = append(args, f.SessionID)
	}
	if f.Status != "" {
		where = append(where, "status = ?")
		args = append(args, f.Status)
	}
	if f.ExcludeStatus != "" {
		where = append(where, "status != ?")
		args = append(args, f.ExcludeStatus)
	}
	query := `SELECT ` + columns + ` FROM questions`
	if len(where) > 0 {
		query += ` WHERE ` + strings.Join(where, " AND ")
	}
	if f.NewestFirst {
		query += ` ORDER BY created_at DESC, id DESC LIMIT ?`
	} else {
		query += ` ORDER BY created_at, id LIMIT ?`
	}
	limit := f.Limit
	if limit <= 0 || limit > 1000 {
		limit = 100
	}
	args = append(args, limit)
	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("list questions: %w", err)
	}
	defer rows.Close()
	var questions []model.Question
	for rows.Next() {
		q, err := scanQuestion(rows)
		if err != nil {
			return nil, fmt.Errorf("scan question: %w", err)
		}
		questions = append(questions, q)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("list questions: %w", err)
	}
	return questions, nil
}

func (s *SQLite) Answer(ctx context.Context, id, response string) error {
	result, err := s.db.ExecContext(ctx, `
UPDATE questions SET status = 'answered', response = ?, answered_at = ?
WHERE id = ? AND status = 'pending'`, response, time.Now().UTC().UnixMilli(), id)
	return changed(result, err, id, s, ctx)
}

func (s *SQLite) Cancel(ctx context.Context, id string) error {
	result, err := s.db.ExecContext(ctx, `UPDATE questions SET status = 'cancelled' WHERE id = ? AND status = 'pending'`, id)
	return changed(result, err, id, s, ctx)
}

func changed(result sql.Result, err error, id string, s *SQLite, ctx context.Context) error {
	if err != nil {
		return err
	}
	n, err := result.RowsAffected()
	if err != nil {
		return err
	}
	if n == 1 {
		return nil
	}
	if _, err := s.Get(ctx, id); errors.Is(err, ErrNotFound) {
		return ErrNotFound
	}
	return ErrAlreadyHandled
}

func (s *SQLite) ClaimAnswer(ctx context.Context, id, token string) (model.Question, error) {
	now := time.Now().UTC()
	result, err := s.db.ExecContext(ctx, `
UPDATE questions SET delivery_token = ?, delivery_lease_until = ?
WHERE id = ? AND status = 'answered' AND completed_at IS NULL
  AND (delivery_token IS NULL OR delivery_lease_until < ?)`, token, now.Add(30*time.Second).UnixMilli(), id, now.UnixMilli())
	if err != nil {
		return model.Question{}, fmt.Errorf("claim answer: %w", err)
	}
	n, err := result.RowsAffected()
	if err != nil {
		return model.Question{}, fmt.Errorf("claim answer: %w", err)
	}
	if n == 1 {
		return s.Get(ctx, id)
	}
	q, err := s.Get(ctx, id)
	if err != nil {
		return q, err
	}
	if q.Status != model.StatusAnswered || q.CompletedAt != nil {
		return q, ErrNotReady
	}
	return q, ErrClaimed
}

func (s *SQLite) CompleteAnswer(ctx context.Context, id, token string) error {
	result, err := s.db.ExecContext(ctx, `
UPDATE questions SET completed_at = ?, delivery_token = NULL, delivery_lease_until = NULL
WHERE id = ? AND delivery_token = ? AND completed_at IS NULL`, time.Now().UTC().UnixMilli(), id, token)
	if err != nil {
		return fmt.Errorf("complete answer: %w", err)
	}
	n, _ := result.RowsAffected()
	if n != 1 {
		return ErrNotReady
	}
	return nil
}

func (s *SQLite) ReleaseAnswer(ctx context.Context, id, token string) error {
	_, err := s.db.ExecContext(ctx, `
UPDATE questions SET delivery_token = NULL, delivery_lease_until = NULL
WHERE id = ? AND delivery_token = ? AND completed_at IS NULL`, id, token)
	return err
}
