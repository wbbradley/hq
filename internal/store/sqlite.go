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

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/model"
	_ "modernc.org/sqlite"
)

const schemaVersion = 3

const schema = `
CREATE TABLE mailboxes (
    id TEXT PRIMARY KEY CHECK(length(id) = 36),
    kind TEXT NOT NULL CHECK(kind IN ('human', 'agent')),
    created_at INTEGER NOT NULL CHECK(created_at > 0),
    last_seen_at INTEGER NOT NULL CHECK(last_seen_at > 0),
    CHECK((id = '00000000-0000-7000-8000-000000000000') = (kind = 'human'))
) STRICT;
CREATE TABLE harness_bindings (
    harness TEXT NOT NULL CHECK(length(harness) > 0),
    external_session_id TEXT NOT NULL CHECK(length(external_session_id) > 0),
    mailbox_id TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL CHECK(created_at > 0),
    PRIMARY KEY(harness, external_session_id),
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id)
) STRICT;
CREATE TABLE mailbox_contexts (
    mailbox_id TEXT NOT NULL,
    directory TEXT NOT NULL CHECK(length(directory) > 0),
    git_common_dir TEXT NOT NULL DEFAULT '',
    remote_identity TEXT NOT NULL DEFAULT '',
    worktree TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    first_seen_at INTEGER NOT NULL CHECK(first_seen_at > 0),
    last_seen_at INTEGER NOT NULL CHECK(last_seen_at > 0),
    PRIMARY KEY(mailbox_id, directory, git_common_dir, remote_identity, worktree, branch),
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id)
) STRICT;
CREATE TABLE messages (
    id TEXT PRIMARY KEY CHECK(length(id) = 36),
    directory TEXT NOT NULL CHECK(length(directory) > 0),
    git_common_dir TEXT NOT NULL DEFAULT '',
    remote_identity TEXT NOT NULL DEFAULT '',
    worktree TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    sender_mailbox_id TEXT NOT NULL,
    recipient_mailbox_id TEXT NOT NULL,
    body TEXT NOT NULL CHECK(length(body) > 0),
    details TEXT NOT NULL DEFAULT '',
    reply_to TEXT,
    created_at INTEGER NOT NULL CHECK(created_at > 0),
    archived_at INTEGER,
    completed_at INTEGER,
    delivery_token TEXT,
    delivery_lease_until INTEGER,
    FOREIGN KEY(sender_mailbox_id) REFERENCES mailboxes(id),
    FOREIGN KEY(recipient_mailbox_id) REFERENCES mailboxes(id),
    FOREIGN KEY(reply_to) REFERENCES messages(id),
    CHECK((delivery_token IS NULL) = (delivery_lease_until IS NULL)),
    CHECK(completed_at IS NULL OR archived_at IS NOT NULL)
) STRICT;
CREATE INDEX messages_inbox ON messages(directory, recipient_mailbox_id, archived_at, created_at, id);
CREATE INDEX messages_sent ON messages(sender_mailbox_id, created_at DESC, id DESC);
CREATE INDEX messages_replies ON messages(reply_to, recipient_mailbox_id, completed_at, created_at, id);
CREATE INDEX messages_delivery ON messages(recipient_mailbox_id, completed_at, delivery_lease_until, created_at, id);
CREATE INDEX mailbox_context_search ON mailbox_contexts(directory, git_common_dir, remote_identity, worktree, branch, last_seen_at DESC);
PRAGMA user_version = 3;
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
	for _, statement := range []string{"PRAGMA foreign_keys = ON", "PRAGMA busy_timeout = 5000", "PRAGMA journal_mode = WAL", "PRAGMA synchronous = FULL", "PRAGMA trusted_schema = OFF", "PRAGMA temp_store = MEMORY"} {
		if _, err := s.db.ExecContext(ctx, statement); err != nil {
			return fmt.Errorf("configure sqlite (%s): %w", statement, err)
		}
	}
	var version int
	if err := s.db.QueryRowContext(ctx, "PRAGMA user_version").Scan(&version); err != nil {
		return fmt.Errorf("read schema version: %w", err)
	}
	if version != schemaVersion {
		if err := s.resetSchema(ctx); err != nil {
			return err
		}
	}
	now := time.Now().UTC().UnixMilli()
	_, err := s.db.ExecContext(ctx, `INSERT INTO mailboxes(id, kind, created_at, last_seen_at) VALUES (?, 'human', ?, ?) ON CONFLICT(id) DO NOTHING`, model.HumanMailboxID, now, now)
	if err != nil {
		return fmt.Errorf("seed human mailbox: %w", err)
	}
	return nil
}

func (s *SQLite) resetSchema(ctx context.Context) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	for _, table := range []string{"messages", "mailbox_contexts", "harness_bindings", "mailboxes", "questions", "legacy_questions_v1"} {
		if _, err := tx.ExecContext(ctx, `DROP TABLE IF EXISTS `+table); err != nil {
			return fmt.Errorf("drop old %s table: %w", table, err)
		}
	}
	if _, err := tx.ExecContext(ctx, schema); err != nil {
		return fmt.Errorf("create schema: %w", err)
	}
	return tx.Commit()
}

func (s *SQLite) Close() error { return s.db.Close() }

func mailboxLabel(kind model.MailboxKind, harness, id string) string {
	if kind == model.MailboxHuman {
		return "human"
	}
	short := id
	if len(short) > 8 {
		short = short[len(short)-8:]
	}
	return harness + ":" + short
}

func (s *SQLite) HumanMailbox(ctx context.Context) (model.Mailbox, error) {
	return s.getMailbox(ctx, model.HumanMailboxID)
}

func (s *SQLite) ResolveMailbox(ctx context.Context, identity model.SessionIdentity, repo model.RepositoryContext) (model.Mailbox, error) {
	if strings.TrimSpace(identity.Harness) == "" || strings.TrimSpace(identity.ExternalSessionID) == "" {
		return model.Mailbox{}, errors.New("harness and external session ID are required")
	}
	now := time.Now().UTC().UnixMilli()
	generated, err := uuid.NewV7()
	if err != nil {
		return model.Mailbox{}, err
	}
	candidateID := generated.String()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return model.Mailbox{}, err
	}
	defer tx.Rollback()
	if _, err = tx.ExecContext(ctx, `INSERT INTO mailboxes(id, kind, created_at, last_seen_at) VALUES (?, 'agent', ?, ?)`, candidateID, now, now); err != nil {
		return model.Mailbox{}, err
	}
	if _, err = tx.ExecContext(ctx, `INSERT INTO harness_bindings(harness, external_session_id, mailbox_id, created_at) VALUES (?, ?, ?, ?) ON CONFLICT(harness, external_session_id) DO NOTHING`, identity.Harness, identity.ExternalSessionID, candidateID, now); err != nil {
		return model.Mailbox{}, err
	}
	var id string
	if err = tx.QueryRowContext(ctx, `SELECT mailbox_id FROM harness_bindings WHERE harness = ? AND external_session_id = ?`, identity.Harness, identity.ExternalSessionID).Scan(&id); err != nil {
		return model.Mailbox{}, err
	}
	if id != candidateID {
		if _, err = tx.ExecContext(ctx, `DELETE FROM mailboxes WHERE id = ?`, candidateID); err != nil {
			return model.Mailbox{}, err
		}
	}
	if _, err = tx.ExecContext(ctx, `UPDATE mailboxes SET last_seen_at = ? WHERE id = ?`, now, id); err != nil {
		return model.Mailbox{}, err
	}
	if repo.Directory != "" {
		_, err = tx.ExecContext(ctx, `INSERT INTO mailbox_contexts(mailbox_id, directory, git_common_dir, remote_identity, worktree, branch, first_seen_at, last_seen_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(mailbox_id, directory, git_common_dir, remote_identity, worktree, branch) DO UPDATE SET last_seen_at = excluded.last_seen_at`,
			id, repo.Directory, repo.GitCommonDir, repo.RemoteIdentity, repo.Worktree, repo.Branch, now, now)
		if err != nil {
			return model.Mailbox{}, err
		}
	}
	if err := tx.Commit(); err != nil {
		return model.Mailbox{}, err
	}
	return s.getMailbox(ctx, id)
}

func (s *SQLite) getMailbox(ctx context.Context, id string) (model.Mailbox, error) {
	var m model.Mailbox
	var created, seen int64
	var harness sql.NullString
	err := s.db.QueryRowContext(ctx, `SELECT m.id, m.kind, b.harness, m.created_at, m.last_seen_at FROM mailboxes m LEFT JOIN harness_bindings b ON b.mailbox_id = m.id WHERE m.id = ?`, id).Scan(&m.ID, &m.Kind, &harness, &created, &seen)
	if errors.Is(err, sql.ErrNoRows) {
		return m, ErrNotFound
	}
	if err != nil {
		return m, err
	}
	m.Harness = harness.String
	m.Label = mailboxLabel(m.Kind, m.Harness, m.ID)
	m.CreatedAt, m.LastSeen = time.UnixMilli(created).UTC(), time.UnixMilli(seen).UTC()
	return m, nil
}

func (s *SQLite) FindMailboxes(ctx context.Context, repo model.RepositoryContext) ([]model.Mailbox, error) {
	var clauses []string
	var args []any
	for _, field := range []struct{ column, value string }{
		{"c.directory", repo.Directory},
		{"c.git_common_dir", repo.GitCommonDir},
		{"c.remote_identity", repo.RemoteIdentity},
		{"c.worktree", repo.Worktree},
		{"c.branch", repo.Branch},
	} {
		if field.value != "" {
			clauses = append(clauses, field.column+" = ?")
			args = append(args, field.value)
		}
	}
	if len(clauses) == 0 {
		return nil, errors.New("mailbox search needs repository context")
	}
	query := `SELECT m.id, m.kind, b.harness, m.created_at, m.last_seen_at, c.directory, c.git_common_dir, c.remote_identity, c.worktree, c.branch
FROM mailboxes m JOIN harness_bindings b ON b.mailbox_id = m.id JOIN mailbox_contexts c ON c.mailbox_id = m.id
WHERE ` + strings.Join(clauses, " OR ") + ` ORDER BY m.last_seen_at DESC, c.last_seen_at DESC`
	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []model.Mailbox
	seenIDs := make(map[string]bool)
	for rows.Next() {
		var m model.Mailbox
		var created, seenAt int64
		if err := rows.Scan(&m.ID, &m.Kind, &m.Harness, &created, &seenAt, &m.Context.Directory, &m.Context.GitCommonDir, &m.Context.RemoteIdentity, &m.Context.Worktree, &m.Context.Branch); err != nil {
			return nil, err
		}
		if seenIDs[m.ID] {
			continue
		}
		seenIDs[m.ID] = true
		m.Label = mailboxLabel(m.Kind, m.Harness, m.ID)
		m.CreatedAt, m.LastSeen = time.UnixMilli(created).UTC(), time.UnixMilli(seenAt).UTC()
		result = append(result, m)
	}
	return result, rows.Err()
}

func (s *SQLite) Create(ctx context.Context, m model.Message) error {
	_, err := s.db.ExecContext(ctx, `INSERT INTO messages(id, directory, git_common_dir, remote_identity, worktree, branch, sender_mailbox_id, recipient_mailbox_id, body, details, reply_to, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		m.ID, m.Context.Directory, m.Context.GitCommonDir, m.Context.RemoteIdentity, m.Context.Worktree, m.Context.Branch, m.SenderMailboxID, m.RecipientMailboxID, m.Body, m.Details, nullableString(m.ReplyTo), m.CreatedAt.UnixMilli())
	if err != nil {
		return fmt.Errorf("create message: %w", err)
	}
	return nil
}

func nullableString(value *string) any {
	if value == nil {
		return nil
	}
	return *value
}

func (s *SQLite) Reply(ctx context.Context, originalID string, reply model.Message) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var directory, sender, recipient string
	if err := tx.QueryRowContext(ctx, `SELECT directory, sender_mailbox_id, recipient_mailbox_id FROM messages WHERE id = ?`, originalID).Scan(&directory, &sender, &recipient); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return ErrNotFound
		}
		return err
	}
	if reply.Context.Directory != directory || recipient != model.HumanMailboxID || reply.SenderMailboxID != model.HumanMailboxID || reply.RecipientMailboxID != sender || reply.ReplyTo == nil || *reply.ReplyTo != originalID {
		return errors.New("reply does not match the inbound message")
	}
	result, err := tx.ExecContext(ctx, `UPDATE messages SET archived_at = ? WHERE id = ? AND recipient_mailbox_id = ? AND archived_at IS NULL`, reply.CreatedAt.UnixMilli(), originalID, model.HumanMailboxID)
	if err != nil {
		return fmt.Errorf("archive inbound message: %w", err)
	}
	n, _ := result.RowsAffected()
	if n != 1 {
		return ErrAlreadyHandled
	}
	_, err = tx.ExecContext(ctx, `INSERT INTO messages(id, directory, git_common_dir, remote_identity, worktree, branch, sender_mailbox_id, recipient_mailbox_id, body, details, reply_to, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`, reply.ID, reply.Context.Directory, reply.Context.GitCommonDir, reply.Context.RemoteIdentity, reply.Context.Worktree, reply.Context.Branch, reply.SenderMailboxID, reply.RecipientMailboxID, reply.Body, reply.Details, nullableString(reply.ReplyTo), reply.CreatedAt.UnixMilli())
	if err != nil {
		return fmt.Errorf("create reply: %w", err)
	}
	return tx.Commit()
}

const columns = `msg.id, msg.directory, msg.git_common_dir, msg.remote_identity, msg.worktree, msg.branch, msg.sender_mailbox_id, msg.recipient_mailbox_id,
CASE WHEN sm.kind = 'human' THEN 'human' ELSE sb.harness || ':' || substr(sm.id, -8) END,
CASE WHEN rm.kind = 'human' THEN 'human' ELSE rb.harness || ':' || substr(rm.id, -8) END,
msg.body, msg.details, msg.reply_to, msg.created_at, msg.archived_at, msg.completed_at`

const joins = ` messages msg JOIN mailboxes sm ON sm.id = msg.sender_mailbox_id LEFT JOIN harness_bindings sb ON sb.mailbox_id = sm.id JOIN mailboxes rm ON rm.id = msg.recipient_mailbox_id LEFT JOIN harness_bindings rb ON rb.mailbox_id = rm.id `

type scanner interface{ Scan(...any) error }

func scanMessage(row scanner) (model.Message, error) {
	var m model.Message
	var reply sql.NullString
	var created int64
	var archived, completed sql.NullInt64
	err := row.Scan(&m.ID, &m.Context.Directory, &m.Context.GitCommonDir, &m.Context.RemoteIdentity, &m.Context.Worktree, &m.Context.Branch, &m.SenderMailboxID, &m.RecipientMailboxID, &m.SenderLabel, &m.RecipientLabel, &m.Body, &m.Details, &reply, &created, &archived, &completed)
	if errors.Is(err, sql.ErrNoRows) {
		return m, ErrNotFound
	}
	if err != nil {
		return m, err
	}
	m.CreatedAt = time.UnixMilli(created).UTC()
	if reply.Valid {
		m.ReplyTo = &reply.String
	}
	if archived.Valid {
		value := time.UnixMilli(archived.Int64).UTC()
		m.ArchivedAt = &value
	}
	if completed.Valid {
		value := time.UnixMilli(completed.Int64).UTC()
		m.CompletedAt = &value
	}
	return m, nil
}

func (s *SQLite) Get(ctx context.Context, id string) (model.Message, error) {
	m, err := scanMessage(s.db.QueryRowContext(ctx, `SELECT `+columns+` FROM `+joins+` WHERE msg.id = ?`, id))
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
		add("msg.directory = ?", f.Directory)
	}
	if f.SenderMailboxID != "" {
		add("msg.sender_mailbox_id = ?", f.SenderMailboxID)
	}
	if f.RecipientMailboxID != "" {
		add("msg.recipient_mailbox_id = ?", f.RecipientMailboxID)
	}
	if f.ReplyTo != "" {
		add("msg.reply_to = ?", f.ReplyTo)
	}
	if f.Archived != nil {
		if *f.Archived {
			where = append(where, "msg.archived_at IS NOT NULL")
		} else {
			where = append(where, "msg.archived_at IS NULL")
		}
	}
	if f.Completed != nil {
		if *f.Completed {
			where = append(where, "msg.completed_at IS NOT NULL")
		} else {
			where = append(where, "msg.completed_at IS NULL")
		}
	}
	query := `SELECT ` + columns + ` FROM ` + joins
	if len(where) > 0 {
		query += ` WHERE ` + strings.Join(where, " AND ")
	}
	if f.NewestFirst {
		query += ` ORDER BY msg.created_at DESC, msg.id DESC LIMIT ?`
	} else {
		query += ` ORDER BY msg.created_at, msg.id LIMIT ?`
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
			return nil, err
		}
		messages = append(messages, m)
	}
	return messages, rows.Err()
}

func (s *SQLite) Archive(ctx context.Context, id string) error {
	result, err := s.db.ExecContext(ctx, `UPDATE messages SET archived_at = ? WHERE id = ? AND recipient_mailbox_id = ? AND archived_at IS NULL`, time.Now().UTC().UnixMilli(), id, model.HumanMailboxID)
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
	if claim.RecipientMailboxID != "" {
		where, args = append(where, "recipient_mailbox_id = ?"), append(args, claim.RecipientMailboxID)
	}
	where = append(where, "completed_at IS NULL", "(delivery_token IS NULL OR delivery_lease_until < ?)")
	args = append(args, now.UnixMilli())
	query := `UPDATE messages SET delivery_token = ?, delivery_lease_until = ? WHERE id = (SELECT id FROM messages WHERE ` + strings.Join(where, " AND ") + ` ORDER BY created_at, id LIMIT 1) RETURNING id`
	queryArgs := append([]any{token, now.Add(30 * time.Second).UnixMilli()}, args...)
	var id string
	if err := s.db.QueryRowContext(ctx, query, queryArgs...).Scan(&id); errors.Is(err, sql.ErrNoRows) {
		return model.Message{}, ErrNotReady
	} else if err != nil {
		return model.Message{}, fmt.Errorf("claim message: %w", err)
	}
	return s.Get(ctx, id)
}

func (s *SQLite) Complete(ctx context.Context, id, token string) error {
	now := time.Now().UTC().UnixMilli()
	result, err := s.db.ExecContext(ctx, `UPDATE messages SET completed_at = ?, archived_at = COALESCE(archived_at, ?), delivery_token = NULL, delivery_lease_until = NULL WHERE id = ? AND delivery_token = ? AND completed_at IS NULL`, now, now, id, token)
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
	_, err := s.db.ExecContext(ctx, `UPDATE messages SET delivery_token = NULL, delivery_lease_until = NULL WHERE id = ? AND delivery_token = ? AND completed_at IS NULL`, id, token)
	return err
}
