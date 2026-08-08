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

const schemaV2 = `
CREATE TABLE IF NOT EXISTS mailboxes (
    directory TEXT NOT NULL CHECK(length(directory) > 0),
    session_id TEXT NOT NULL CHECK(length(session_id) > 0),
    kind TEXT NOT NULL CHECK(kind IN ('human', 'agent')),
    created_at INTEGER NOT NULL CHECK(created_at > 0),
    PRIMARY KEY(directory, session_id),
    CHECK((session_id = 'human') = (kind = 'human'))
) STRICT;
CREATE TABLE IF NOT EXISTS messages (
    directory TEXT NOT NULL CHECK(length(directory) > 0),
    recipient_session TEXT NOT NULL CHECK(length(recipient_session) > 0),
    id TEXT NOT NULL CHECK(length(id) = 36),
    sender_session TEXT NOT NULL CHECK(length(sender_session) > 0),
    body TEXT NOT NULL CHECK(length(body) > 0),
    details TEXT NOT NULL DEFAULT '',
    reply_to TEXT,
    created_at INTEGER NOT NULL CHECK(created_at > 0),
    archived_at INTEGER,
    completed_at INTEGER,
    delivery_token TEXT,
    delivery_lease_until INTEGER,
    PRIMARY KEY(directory, recipient_session, id),
    UNIQUE(id),
    FOREIGN KEY(directory, sender_session) REFERENCES mailboxes(directory, session_id),
    FOREIGN KEY(directory, recipient_session) REFERENCES mailboxes(directory, session_id),
    FOREIGN KEY(reply_to) REFERENCES messages(id),
    CHECK((delivery_token IS NULL) = (delivery_lease_until IS NULL)),
    CHECK(completed_at IS NULL OR archived_at IS NOT NULL)
) STRICT;
CREATE INDEX IF NOT EXISTS messages_inbox
    ON messages(directory, recipient_session, archived_at, created_at, id);
CREATE INDEX IF NOT EXISTS messages_sent
    ON messages(directory, sender_session, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS messages_replies
    ON messages(reply_to, recipient_session, completed_at, created_at, id);
CREATE INDEX IF NOT EXISTS messages_delivery
    ON messages(recipient_session, completed_at, delivery_lease_until, created_at, id);
PRAGMA user_version = 2;
`

type SQLite struct{ db *sql.DB }

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
	for _, statement := range []string{
		"PRAGMA foreign_keys = ON", "PRAGMA busy_timeout = 5000", "PRAGMA journal_mode = WAL",
		"PRAGMA synchronous = FULL", "PRAGMA trusted_schema = OFF", "PRAGMA temp_store = MEMORY",
	} {
		if _, err := s.db.ExecContext(ctx, statement); err != nil {
			return fmt.Errorf("configure sqlite (%s): %w", statement, err)
		}
	}
	var oldTable int
	if err := s.db.QueryRowContext(ctx, `SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'questions'`).Scan(&oldTable); err != nil {
		return fmt.Errorf("inspect schema: %w", err)
	}
	if oldTable == 1 {
		if _, err := s.db.ExecContext(ctx, `ALTER TABLE questions RENAME TO legacy_questions_v1`); err != nil {
			return fmt.Errorf("preserve legacy questions: %w", err)
		}
	}
	if _, err := s.db.ExecContext(ctx, schemaV2); err != nil {
		return fmt.Errorf("create schema: %w", err)
	}
	return nil
}

func (s *SQLite) Close() error { return s.db.Close() }

func mailboxKind(session string) model.MailboxKind {
	if session == model.HumanSession {
		return model.MailboxHuman
	}
	return model.MailboxAgent
}

func ensureMailbox(ctx context.Context, tx *sql.Tx, directory, session string, created int64) error {
	_, err := tx.ExecContext(ctx, `INSERT INTO mailboxes(directory, session_id, kind, created_at)
VALUES (?, ?, ?, ?) ON CONFLICT(directory, session_id) DO NOTHING`, directory, session, mailboxKind(session), created)
	return err
}

func insertMessage(ctx context.Context, tx *sql.Tx, m model.Message) error {
	created := m.CreatedAt.UnixMilli()
	if err := ensureMailbox(ctx, tx, m.Directory, m.SenderSession, created); err != nil {
		return err
	}
	if err := ensureMailbox(ctx, tx, m.Directory, m.RecipientSession, created); err != nil {
		return err
	}
	var replyTo any
	if m.ReplyTo != nil {
		replyTo = *m.ReplyTo
	}
	_, err := tx.ExecContext(ctx, `INSERT INTO messages(
directory, recipient_session, id, sender_session, body, details, reply_to, created_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)`, m.Directory, m.RecipientSession, m.ID, m.SenderSession, m.Body, m.Details, replyTo, created)
	return err
}

func (s *SQLite) Create(ctx context.Context, m model.Message) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if err := insertMessage(ctx, tx, m); err != nil {
		return fmt.Errorf("create message: %w", err)
	}
	return tx.Commit()
}

func (s *SQLite) Reply(ctx context.Context, originalID string, reply model.Message) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var directory, sender string
	if err := tx.QueryRowContext(ctx, `SELECT directory, sender_session FROM messages WHERE id = ?`, originalID).Scan(&directory, &sender); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return ErrNotFound
		}
		return err
	}
	if reply.Directory != directory || reply.SenderSession != model.HumanSession || reply.RecipientSession != sender || reply.ReplyTo == nil || *reply.ReplyTo != originalID {
		return errors.New("reply does not match the inbound message")
	}
	result, err := tx.ExecContext(ctx, `UPDATE messages SET archived_at = ?
WHERE id = ? AND recipient_session = 'human' AND archived_at IS NULL`, reply.CreatedAt.UnixMilli(), originalID)
	if err != nil {
		return fmt.Errorf("archive inbound message: %w", err)
	}
	n, _ := result.RowsAffected()
	if n != 1 {
		var exists int
		if err := tx.QueryRowContext(ctx, `SELECT count(*) FROM messages WHERE id = ?`, originalID).Scan(&exists); err != nil {
			return err
		}
		if exists == 0 {
			return ErrNotFound
		}
		return ErrAlreadyHandled
	}
	if err := insertMessage(ctx, tx, reply); err != nil {
		return fmt.Errorf("create reply: %w", err)
	}
	return tx.Commit()
}

const columns = `id, directory, sender_session, recipient_session, body, details,
reply_to, created_at, archived_at, completed_at`

type scanner interface{ Scan(...any) error }

func scanMessage(row scanner) (model.Message, error) {
	var m model.Message
	var reply sql.NullString
	var created int64
	var archived, completed sql.NullInt64
	err := row.Scan(&m.ID, &m.Directory, &m.SenderSession, &m.RecipientSession, &m.Body, &m.Details,
		&reply, &created, &archived, &completed)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return m, ErrNotFound
		}
		return m, err
	}
	m.CreatedAt = time.UnixMilli(created).UTC()
	if reply.Valid {
		m.ReplyTo = &reply.String
	}
	if archived.Valid {
		t := time.UnixMilli(archived.Int64).UTC()
		m.ArchivedAt = &t
	}
	if completed.Valid {
		t := time.UnixMilli(completed.Int64).UTC()
		m.CompletedAt = &t
	}
	return m, nil
}

func (s *SQLite) Get(ctx context.Context, id string) (model.Message, error) {
	m, err := scanMessage(s.db.QueryRowContext(ctx, `SELECT `+columns+` FROM messages WHERE id = ?`, id))
	if err != nil && !errors.Is(err, ErrNotFound) {
		return m, fmt.Errorf("get message: %w", err)
	}
	return m, err
}

func (s *SQLite) List(ctx context.Context, f model.Filter) ([]model.Message, error) {
	var where []string
	var args []any
	add := func(clause string, value any) { where, args = append(where, clause), append(args, value) }
	if f.Directory != "" {
		add("directory = ?", f.Directory)
	}
	if f.SenderSession != "" {
		add("sender_session = ?", f.SenderSession)
	}
	if f.RecipientSession != "" {
		add("recipient_session = ?", f.RecipientSession)
	}
	if f.ReplyTo != "" {
		add("reply_to = ?", f.ReplyTo)
	}
	if f.Archived != nil {
		if *f.Archived {
			where = append(where, "archived_at IS NOT NULL")
		} else {
			where = append(where, "archived_at IS NULL")
		}
	}
	if f.Completed != nil {
		if *f.Completed {
			where = append(where, "completed_at IS NOT NULL")
		} else {
			where = append(where, "completed_at IS NULL")
		}
	}
	query := `SELECT ` + columns + ` FROM messages`
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
		return nil, fmt.Errorf("list messages: %w", err)
	}
	defer rows.Close()
	var messages []model.Message
	for rows.Next() {
		m, err := scanMessage(rows)
		if err != nil {
			return nil, fmt.Errorf("scan message: %w", err)
		}
		messages = append(messages, m)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("list messages: %w", err)
	}
	return messages, nil
}

func (s *SQLite) Archive(ctx context.Context, id string) error {
	result, err := s.db.ExecContext(ctx, `UPDATE messages SET archived_at = ?
WHERE id = ? AND recipient_session = 'human' AND archived_at IS NULL`, time.Now().UTC().UnixMilli(), id)
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

func (s *SQLite) Claim(ctx context.Context, claim Claim, token string) (model.Message, error) {
	now := time.Now().UTC()
	var where []string
	var args []any
	if claim.MessageID != "" {
		where, args = append(where, "id = ?"), append(args, claim.MessageID)
	}
	if claim.ReplyTo != "" {
		where, args = append(where, "reply_to = ?"), append(args, claim.ReplyTo)
	}
	if claim.Directory != "" {
		where, args = append(where, "directory = ?"), append(args, claim.Directory)
	}
	if claim.RecipientSession != "" {
		where, args = append(where, "recipient_session = ?"), append(args, claim.RecipientSession)
	}
	where = append(where, "completed_at IS NULL", "(delivery_token IS NULL OR delivery_lease_until < ?)")
	args = append(args, now.UnixMilli())
	query := `UPDATE messages SET delivery_token = ?, delivery_lease_until = ? WHERE id = (
SELECT id FROM messages WHERE ` + strings.Join(where, " AND ") + ` ORDER BY created_at, id LIMIT 1)
RETURNING ` + columns
	queryArgs := append([]any{token, now.Add(30 * time.Second).UnixMilli()}, args...)
	m, err := scanMessage(s.db.QueryRowContext(ctx, query, queryArgs...))
	if errors.Is(err, ErrNotFound) {
		return m, ErrNotReady
	}
	if err != nil {
		return m, fmt.Errorf("claim message: %w", err)
	}
	return m, nil
}

func (s *SQLite) Complete(ctx context.Context, id, token string) error {
	now := time.Now().UTC().UnixMilli()
	result, err := s.db.ExecContext(ctx, `UPDATE messages SET completed_at = ?, archived_at = COALESCE(archived_at, ?),
delivery_token = NULL, delivery_lease_until = NULL WHERE id = ? AND delivery_token = ? AND completed_at IS NULL`, now, now, id, token)
	if err != nil {
		return fmt.Errorf("complete message: %w", err)
	}
	n, _ := result.RowsAffected()
	if n != 1 {
		return ErrNotReady
	}
	return nil
}

func (s *SQLite) Release(ctx context.Context, id, token string) error {
	_, err := s.db.ExecContext(ctx, `UPDATE messages SET delivery_token = NULL, delivery_lease_until = NULL
WHERE id = ? AND delivery_token = ? AND completed_at IS NULL`, id, token)
	return err
}
