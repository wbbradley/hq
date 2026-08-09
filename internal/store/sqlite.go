package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	_ "modernc.org/sqlite"
)

const schemaVersion = 7

const schema = `
CREATE TABLE canonical_events (
    event_id TEXT PRIMARY KEY CHECK(length(event_id) = 64),
    raw BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    signer_key_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    reduction_status TEXT NOT NULL,
    reduction_reason TEXT NOT NULL DEFAULT ''
) STRICT;
CREATE TABLE causal_edges (
    child_event_id TEXT NOT NULL,
    parent_event_id TEXT NOT NULL,
    PRIMARY KEY(child_event_id, parent_event_id),
    FOREIGN KEY(child_event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE projection_checkpoint (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    event_count INTEGER NOT NULL,
    rebuilt_at INTEGER NOT NULL
) STRICT;
CREATE TABLE mailboxes (
    id TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('human', 'agent')),
    label TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE mailbox_activity (
    mailbox_id TEXT PRIMARY KEY,
    last_seen_at INTEGER NOT NULL,
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id) ON DELETE CASCADE
) STRICT;
CREATE TABLE harness_bindings (
    harness TEXT NOT NULL,
    external_session_id TEXT NOT NULL,
    mailbox_id TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(harness, external_session_id),
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id) ON DELETE CASCADE
) STRICT;
CREATE TABLE mailbox_contexts (
    mailbox_id TEXT NOT NULL,
    directory TEXT NOT NULL,
    git_common_dir TEXT NOT NULL DEFAULT '',
    remote_identity TEXT NOT NULL DEFAULT '',
    worktree TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    first_seen_at INTEGER NOT NULL,
    PRIMARY KEY(mailbox_id, directory, git_common_dir, remote_identity, worktree, branch),
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id) ON DELETE CASCADE
) STRICT;
CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    event_id TEXT NOT NULL UNIQUE,
    thread_event_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    audience_account_id TEXT NOT NULL DEFAULT '',
    directory TEXT NOT NULL DEFAULT '',
    git_common_dir TEXT NOT NULL DEFAULT '',
    remote_identity TEXT NOT NULL DEFAULT '',
    worktree TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    sender_installation_id TEXT NOT NULL,
    recipient_installation_id TEXT NOT NULL,
    sender_mailbox_id TEXT NOT NULL,
    recipient_mailbox_id TEXT NOT NULL,
    actor_label TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL,
    details TEXT NOT NULL DEFAULT '',
    reply_to TEXT,
    created_at INTEGER NOT NULL,
    archived_at INTEGER,
    incomplete INTEGER NOT NULL CHECK(incomplete IN (0, 1)),
    peer_received INTEGER NOT NULL CHECK(peer_received IN (0, 1)),
    rejected INTEGER NOT NULL CHECK(rejected IN (0, 1)),
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id)
) STRICT;
CREATE TABLE threads (
    event_id TEXT PRIMARY KEY,
    answered INTEGER NOT NULL,
    cancelled INTEGER NOT NULL,
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id)
) STRICT;
CREATE TABLE peers (
    installation_id TEXT PRIMARY KEY,
    signer_key_id TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    relays_json TEXT NOT NULL DEFAULT '[]',
    trusted INTEGER NOT NULL CHECK(trusted IN (0, 1))
) STRICT;
CREATE TABLE mailbox_shares (
    mailbox_id TEXT NOT NULL,
    peer_installation_id TEXT NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    PRIMARY KEY(mailbox_id, peer_installation_id)
) STRICT;
CREATE TABLE human_accounts (
    account_id TEXT PRIMARY KEY,
    creator_installation_id TEXT NOT NULL,
    creator_signer_key_id TEXT NOT NULL,
    label TEXT NOT NULL,
    creation_event_id TEXT NOT NULL,
    FOREIGN KEY(creation_event_id) REFERENCES canonical_events(event_id)
) STRICT;
CREATE TABLE human_account_devices (
    account_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    signer_key_id TEXT NOT NULL,
    label TEXT NOT NULL,
    relays_json TEXT NOT NULL DEFAULT '[]',
    state TEXT NOT NULL CHECK(state IN ('active','pending','revoked')),
    grant_event_id TEXT NOT NULL DEFAULT '',
    accept_event_id TEXT NOT NULL DEFAULT '',
    revoke_event_ids_json TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY(account_id, installation_id),
    FOREIGN KEY(account_id) REFERENCES human_accounts(account_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE human_account_default (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    account_id TEXT NOT NULL,
    FOREIGN KEY(account_id) REFERENCES human_accounts(account_id)
) STRICT;
CREATE TABLE outbox (
    event_id TEXT NOT NULL,
    recipient_installation_id TEXT NOT NULL,
    exact_canonical_bytes BLOB NOT NULL,
    recipient_public_key TEXT NOT NULL DEFAULT '',
	recipient_relays_json TEXT NOT NULL DEFAULT '[]',
    gift_wrap_event_id TEXT UNIQUE,
    exact_gift_wrap_bytes BLOB,
    ephemeral_public_key TEXT UNIQUE,
    wrapped_at INTEGER,
    state TEXT NOT NULL DEFAULT 'queued',
    created_at INTEGER NOT NULL,
	PRIMARY KEY(event_id, recipient_installation_id),
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id)
) STRICT;
CREATE TABLE relays (
    url TEXT PRIMARY KEY,
    read_enabled INTEGER NOT NULL CHECK(read_enabled IN (0,1)),
    write_enabled INTEGER NOT NULL CHECK(write_enabled IN (0,1)),
    require_auth INTEGER NOT NULL CHECK(require_auth IN (0,1)),
    unsafe_no_auth INTEGER NOT NULL CHECK(unsafe_no_auth IN (0,1)),
    created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE outbound_relay_attempts (
    event_id TEXT NOT NULL,
    recipient_installation_id TEXT NOT NULL,
    relay_url TEXT NOT NULL,
    state TEXT NOT NULL,
    message TEXT NOT NULL DEFAULT '',
    attempt_count INTEGER NOT NULL,
    last_attempt_at INTEGER NOT NULL,
    next_attempt_at INTEGER NOT NULL,
    accepted_at INTEGER,
	PRIMARY KEY(event_id, recipient_installation_id, relay_url),
	FOREIGN KEY(event_id, recipient_installation_id) REFERENCES outbox(event_id, recipient_installation_id)
) STRICT;
CREATE TABLE inbound_wrappers (
    outer_event_id TEXT PRIMARY KEY,
    ephemeral_public_key TEXT NOT NULL UNIQUE,
    origin_installation_id TEXT NOT NULL,
    canonical_event_id TEXT NOT NULL,
    exact_wrapper BLOB NOT NULL,
    relay_url TEXT NOT NULL,
    status TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    received_at INTEGER NOT NULL
) STRICT;
CREATE INDEX inbound_logical ON inbound_wrappers(origin_installation_id, canonical_event_id);
CREATE TABLE relay_sync_state (
    relay_url TEXT PRIMARY KEY,
    connected INTEGER NOT NULL DEFAULT 0,
    authenticated INTEGER NOT NULL DEFAULT 0,
    last_eose_at INTEGER,
    last_event_at INTEGER,
    last_error TEXT NOT NULL DEFAULT ''
) STRICT;
CREATE TABLE delivery_facts (
    message_id TEXT PRIMARY KEY,
    completed_at INTEGER,
    delivery_token TEXT,
    delivery_lease_until INTEGER,
    CHECK((delivery_token IS NULL) = (delivery_lease_until IS NULL))
) STRICT;
CREATE TABLE inbound_staging (
    id INTEGER PRIMARY KEY,
    raw_wrapper BLOB NOT NULL,
    relay TEXT NOT NULL,
    event_id TEXT NOT NULL DEFAULT '',
    failure_reason TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    retry_at INTEGER NOT NULL
) STRICT;
CREATE TABLE quarantine (
    id INTEGER PRIMARY KEY,
    raw_wrapper BLOB NOT NULL,
    relay TEXT NOT NULL,
    event_id TEXT NOT NULL DEFAULT '',
    rejection_reason TEXT NOT NULL,
    received_at INTEGER NOT NULL
) STRICT;
CREATE INDEX messages_inbox ON messages(recipient_mailbox_id, archived_at, created_at, id);
CREATE INDEX messages_sent ON messages(sender_mailbox_id, created_at DESC, id DESC);
CREATE INDEX messages_reply ON messages(reply_to, recipient_mailbox_id, created_at, id);
CREATE INDEX mailbox_context_search ON mailbox_contexts(directory, git_common_dir, remote_identity, worktree, branch);
PRAGMA user_version = 7;
`

const (
	quarantineMaxRows  = 1000
	quarantineMaxBytes = 16 << 20
	quarantineMaxAge   = 30 * 24 * time.Hour
)

type SQLite struct {
	db       *sql.DB
	signer   identity.Material
	database string
}

var resolveMu sync.Mutex

func DefaultPath() (string, error) { return identity.DefaultDatabasePath() }

func Open(path string) (*SQLite, error) {
	resolved, err := identity.ResolveDatabasePath(path)
	if err != nil {
		return nil, err
	}
	keyPath, err := identity.KeyPath(resolved)
	if err != nil {
		return nil, err
	}
	signer, err := identity.Load(keyPath)
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Dir(resolved), 0o700); err != nil {
		return nil, fmt.Errorf("create state directory: %w", err)
	}
	db, err := sql.Open("sqlite", resolved)
	if err != nil {
		return nil, fmt.Errorf("open database: %w", err)
	}
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	s := &SQLite{db: db, signer: signer, database: resolved}
	if err := s.configure(context.Background()); err != nil {
		db.Close()
		return nil, err
	}
	if err := os.Chmod(resolved, 0o600); err != nil {
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
	var count int
	if err := s.db.QueryRowContext(ctx, `SELECT count(*) FROM canonical_events`).Scan(&count); err != nil {
		return err
	}
	if count == 0 {
		return s.bootstrap(ctx)
	}
	var installationID, signerKeyID string
	if err := s.db.QueryRowContext(ctx, `SELECT installation_id,signer_key_id FROM canonical_events WHERE event_type='installation.create' ORDER BY created_at,event_id LIMIT 1`).Scan(&installationID, &signerKeyID); err != nil {
		return errors.New("database has no signed installation identity")
	}
	if installationID != s.signer.InstallationID || signerKeyID != s.signer.PublicKey() {
		return errors.New("database identity does not match hq.key")
	}
	return s.Rebuild(ctx)
}

func (s *SQLite) resetSchema(ctx context.Context) error {
	rows, err := s.db.QueryContext(ctx, `SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'`)
	if err != nil {
		return err
	}
	var tables []string
	for rows.Next() {
		var table string
		if err := rows.Scan(&table); err != nil {
			rows.Close()
			return err
		}
		tables = append(tables, table)
	}
	rows.Close()
	if _, err := s.db.ExecContext(ctx, `PRAGMA foreign_keys = OFF`); err != nil {
		return err
	}
	for _, table := range tables {
		if _, err := s.db.ExecContext(ctx, `DROP TABLE IF EXISTS "`+strings.ReplaceAll(table, `"`, `""`)+`"`); err != nil {
			return fmt.Errorf("drop old %s table: %w", table, err)
		}
	}
	if _, err := s.db.ExecContext(ctx, schema); err != nil {
		return fmt.Errorf("create schema: %w", err)
	}
	_, err = s.db.ExecContext(ctx, `PRAGMA foreign_keys = ON`)
	return err
}

func (s *SQLite) bootstrap(ctx context.Context) error {
	now := time.Now().UTC()
	accountID, err := uuid.NewV7()
	if err != nil {
		return err
	}
	label, err := os.Hostname()
	if err != nil || strings.TrimSpace(label) == "" {
		label = "hq"
	}
	installationPayload, _ := event.MarshalPayload(event.InstallationPayload{Label: "hq"})
	humanPayload, _ := event.MarshalPayload(event.MailboxPayload{MailboxID: model.HumanMailboxID, Kind: string(model.MailboxHuman), Label: "human"})
	accountPayload, _ := event.MarshalPayload(event.HumanAccountPayload{AccountID: accountID.String(), CreatorInstallationID: s.signer.InstallationID, CreatorSignerKeyID: s.signer.PublicKey(), Label: label})
	contents := []event.Content{
		{Type: event.TypeInstallationCreate, Scope: event.ScopeInstallationPrivate, Payload: installationPayload},
		{Type: event.TypeMailboxCreate, Scope: event.ScopeInstallationPrivate, Payload: humanPayload},
		{Type: event.TypeHumanAccountCreate, Scope: event.ScopeInstallationPrivate, Payload: accountPayload},
	}
	signed, err := s.signContents(ctx, contents, []time.Time{now, now, now})
	if err != nil {
		return err
	}
	selectionPayload, _ := event.MarshalPayload(event.HumanAccountSelectionPayload{AccountID: accountID.String()})
	selection := event.Content{Type: event.TypeHumanAccountSelect, Parents: []string{signed[2].ID()}, Scope: event.ScopeInstallationPrivate, Payload: selectionPayload}
	selected, err := s.signContents(ctx, []event.Content{selection}, []time.Time{now})
	if err != nil {
		return err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if err := s.appendSignedTx(ctx, tx, append(signed, selected...)); err != nil {
		return err
	}
	return tx.Commit()
}

func (s *SQLite) Close() error { return s.db.Close() }

func (s *SQLite) policy() event.Policy {
	return event.Policy{InstallationID: s.signer.InstallationID, RootKeyID: s.signer.PublicKey(), HumanMailboxID: model.HumanMailboxID, SchemaVersions: []int{event.SchemaVersion}}
}

func (s *SQLite) appendContents(ctx context.Context, contents []event.Content, times []time.Time, after func(*sql.Tx) error) error {
	signed, err := s.signContents(ctx, contents, times)
	if err != nil {
		return err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if err := s.appendSignedTx(ctx, tx, signed); err != nil {
		return err
	}
	if after != nil {
		if err := after(tx); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (s *SQLite) signContents(ctx context.Context, contents []event.Content, times []time.Time) ([]event.SignedEvent, error) {
	signed := make([]event.SignedEvent, len(contents))
	for i := range contents {
		if contents[i].InstallationID == "" {
			contents[i].InstallationID = s.signer.InstallationID
		}
		if contents[i].SignerKeyID == "" {
			contents[i].SignerKeyID = s.signer.PublicKey()
		}
		created := time.Now().UTC()
		if i < len(times) && !times[i].IsZero() {
			created = times[i].UTC()
		}
		var err error
		signed[i], err = s.signer.Sign(ctx, contents[i], created)
		if err != nil {
			return nil, err
		}
	}
	return signed, nil
}

func (s *SQLite) appendSignedTx(ctx context.Context, tx *sql.Tx, additions []event.SignedEvent) error {
	return s.appendSignedTxMode(ctx, tx, additions, true)
}

func (s *SQLite) appendSignedTxMode(ctx context.Context, tx *sql.Tx, additions []event.SignedEvent, requireProjected bool) error {
	rows, err := tx.QueryContext(ctx, `SELECT event_id,raw FROM canonical_events ORDER BY event_id`)
	if err != nil {
		return err
	}
	var raw [][]byte
	existing := make(map[string]bool)
	for rows.Next() {
		var id string
		var item []byte
		if err := rows.Scan(&id, &item); err != nil {
			rows.Close()
			return err
		}
		raw = append(raw, item)
		existing[id] = true
	}
	rows.Close()
	for _, item := range additions {
		if !existing[item.ID()] {
			raw = append(raw, item.Wire)
		}
	}
	state := event.Reduce(raw, s.policy())
	for _, item := range additions {
		if existing[item.ID()] {
			continue
		}
		record, ok := state.Records[item.ID()]
		acceptable := ok && (record.Status == event.StatusProjected || (!requireProjected && (record.Status == event.StatusUnresolved || record.Status == event.StatusUnsupported)))
		if !acceptable {
			if ok {
				return fmt.Errorf("new event %s was not projected: %s", item.ID(), record.Reason)
			}
			return fmt.Errorf("new event %s was rejected", item.ID())
		}
		_, err := tx.ExecContext(ctx, `INSERT INTO canonical_events(event_id, raw, created_at, event_type, installation_id, signer_key_id, scope, reduction_status, reduction_reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			item.ID(), item.Wire, item.Nostr.CreatedAt, item.Content.Type, item.Content.InstallationID, item.Content.SignerKeyID, item.Content.Scope, record.Status, record.Reason)
		if err != nil {
			return fmt.Errorf("append canonical event: %w", err)
		}
	}
	return s.rebuildTx(ctx, tx, state)
}

func (s *SQLite) AppendCanonical(ctx context.Context, additions []event.SignedEvent) error {
	if len(additions) == 0 {
		return errors.New("at least one canonical event is required")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if err := s.appendSignedTxMode(ctx, tx, additions, false); err != nil {
		return err
	}
	return tx.Commit()
}

func (s *SQLite) rebuildTx(ctx context.Context, tx *sql.Tx, state event.State) error {
	activity := make(map[string]int64)
	activityRows, err := tx.QueryContext(ctx, `SELECT mailbox_id,last_seen_at FROM mailbox_activity`)
	if err == nil {
		for activityRows.Next() {
			var id string
			var seen int64
			if activityRows.Scan(&id, &seen) == nil {
				activity[id] = seen
			}
		}
		activityRows.Close()
	}
	for id, record := range state.Records {
		if _, err := tx.ExecContext(ctx, `UPDATE canonical_events SET reduction_status = ?, reduction_reason = ? WHERE event_id = ?`, record.Status, record.Reason, id); err != nil {
			return err
		}
	}
	for _, table := range []string{"causal_edges", "threads", "messages", "mailbox_contexts", "harness_bindings", "mailbox_activity", "mailboxes", "peers", "mailbox_shares", "human_account_default", "human_account_devices", "human_accounts"} {
		if _, err := tx.ExecContext(ctx, `DELETE FROM `+table); err != nil {
			return fmt.Errorf("clear %s projection: %w", table, err)
		}
	}
	ids := make([]string, 0, len(state.Records))
	for id := range state.Records {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	for _, id := range ids {
		record := state.Records[id]
		for _, parent := range record.Event.Content.Parents {
			if _, err := tx.ExecContext(ctx, `INSERT INTO causal_edges(child_event_id, parent_event_id) VALUES (?, ?)`, id, parent); err != nil {
				return err
			}
		}
	}
	for _, mailbox := range state.Mailboxes {
		created := firstMailboxTime(state, mailbox.ID)
		if _, err := tx.ExecContext(ctx, `INSERT INTO mailboxes(id, installation_id, kind, label, created_at) VALUES (?, ?, ?, ?, ?)`, mailbox.ID, s.signer.InstallationID, mailbox.Kind, mailbox.Label, created); err != nil {
			return fmt.Errorf("project mailbox: %w", err)
		}
		seen := created
		if activity[mailbox.ID] > seen {
			seen = activity[mailbox.ID]
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO mailbox_activity(mailbox_id, last_seen_at) VALUES (?, ?)`, mailbox.ID, seen); err != nil {
			return err
		}
		if mailbox.Harness != "" {
			if _, err := tx.ExecContext(ctx, `INSERT INTO harness_bindings(harness, external_session_id, mailbox_id, created_at) VALUES (?, ?, ?, ?)`, mailbox.Harness, mailbox.ExternalSessionID, mailbox.ID, created); err != nil {
				return fmt.Errorf("project harness binding: %w", err)
			}
		}
		for _, context := range mailbox.Contexts {
			if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO mailbox_contexts(mailbox_id, directory, git_common_dir, remote_identity, worktree, branch, first_seen_at) VALUES (?, ?, ?, ?, ?, ?, ?)`, mailbox.ID, context.Directory, context.GitCommonDir, context.RemoteIdentity, context.Worktree, context.Branch, created); err != nil {
				return err
			}
		}
	}
	messageIDs := make(map[string]string)
	for eventID, message := range state.Messages {
		id := message.MessageID
		if id == "" {
			id = eventID
		}
		messageIDs[eventID] = id
	}
	for _, eventID := range state.DisplayOrder {
		message, ok := state.Messages[eventID]
		if !ok {
			continue
		}
		id := messageIDs[eventID]
		var repo event.RepositoryContext
		if message.Context != nil {
			repo = *message.Context
		}
		var replyTo any
		if message.Type == event.TypeAnswer && len(message.Parents) > 0 {
			replyTo = messageIDs[message.ThreadID]
		}
		var archived any
		if message.Archived {
			archived = archiveTime(state, eventID)
		}
		_, err := tx.ExecContext(ctx, `INSERT INTO messages(id, event_id, thread_event_id, event_type, audience_account_id, directory, git_common_dir, remote_identity, worktree, branch, sender_installation_id, recipient_installation_id, sender_mailbox_id, recipient_mailbox_id, actor_label, body, details, reply_to, created_at, archived_at, incomplete, peer_received, rejected) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			id, eventID, message.ThreadID, message.Type, message.AudienceAccountID, repo.Directory, repo.GitCommonDir, repo.RemoteIdentity, repo.Worktree, repo.Branch, message.Sender.InstallationID, message.Recipient.InstallationID, message.Sender.MailboxID, message.Recipient.MailboxID, message.ActorLabel, message.Body, message.Details, replyTo, message.CreatedAt.UnixMilli(), archived, boolInt(message.Incomplete), boolInt(message.PeerReceived), boolInt(message.Rejected))
		if err != nil {
			return fmt.Errorf("project message: %w", err)
		}
		if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO delivery_facts(message_id) VALUES (?)`, id); err != nil {
			return err
		}
	}
	for id, thread := range state.Threads {
		if _, err := tx.ExecContext(ctx, `INSERT INTO threads(event_id, answered, cancelled) VALUES (?, ?, ?)`, id, boolInt(thread.Answered), boolInt(thread.Cancelled)); err != nil {
			return err
		}
	}
	for _, peer := range state.Peers {
		relays, _ := json.Marshal(peer.Relays)
		if _, err := tx.ExecContext(ctx, `INSERT INTO peers(installation_id, signer_key_id, name, relays_json, trusted) VALUES (?, ?, ?, ?, ?)`, peer.InstallationID, peer.SignerKeyID, peer.Name, string(relays), boolInt(peer.Trusted)); err != nil {
			return err
		}
	}
	for _, share := range state.Shares {
		if _, err := tx.ExecContext(ctx, `INSERT INTO mailbox_shares(mailbox_id, peer_installation_id, active) VALUES (?, ?, ?)`, share.MailboxID, share.PeerInstallationID, boolInt(share.Active)); err != nil {
			return err
		}
	}
	for _, account := range state.Accounts {
		if _, err := tx.ExecContext(ctx, `INSERT INTO human_accounts(account_id,creator_installation_id,creator_signer_key_id,label,creation_event_id) VALUES (?,?,?,?,?)`, account.ID, account.CreatorInstallationID, account.CreatorSignerKeyID, account.Label, account.CreationEventID); err != nil {
			return err
		}
		for _, device := range account.Devices {
			deviceState := device.State
			if deviceState == "" {
				deviceState = "pending"
			}
			relays, _ := json.Marshal(device.Relays)
			revokes, _ := json.Marshal(device.RevokeEventIDs)
			if _, err := tx.ExecContext(ctx, `INSERT INTO human_account_devices(account_id,installation_id,signer_key_id,label,relays_json,state,grant_event_id,accept_event_id,revoke_event_ids_json) VALUES (?,?,?,?,?,?,?,?,?)`, account.ID, device.InstallationID, device.SignerKeyID, device.Label, string(relays), deviceState, device.GrantEventID, device.AcceptEventID, string(revokes)); err != nil {
				return err
			}
		}
	}
	if state.DefaultAccountID != "" {
		if _, err := tx.ExecContext(ctx, `INSERT INTO human_account_default(id,account_id) VALUES (1,?)`, state.DefaultAccountID); err != nil {
			return err
		}
	}
	desiredAccountDeliveries := make(map[string]bool)
	for _, record := range state.Records {
		content := record.Event.Content
		if record.Status != event.StatusProjected || content.InstallationID != s.signer.InstallationID {
			continue
		}
		recipients := make(map[string]event.HumanDeviceProjection)
		if content.Scope == event.ScopePeerAddressed && content.Recipient != nil {
			peer := state.Peers[content.Recipient.InstallationID]
			recipients[content.Recipient.InstallationID] = event.HumanDeviceProjection{InstallationID: peer.InstallationID, SignerKeyID: peer.SignerKeyID, Relays: peer.Relays}
		}
		if content.Scope == event.ScopeAccountAddressed && content.Audience != nil {
			if account, ok := state.Accounts[content.Audience.HumanAccountID]; ok {
				for _, device := range account.Devices {
					if device.Active && device.InstallationID != s.signer.InstallationID {
						recipients[device.InstallationID] = device
					}
				}
				if content.Recipient != nil && content.Recipient.InstallationID != s.signer.InstallationID {
					if device, exists := account.Devices[content.Recipient.InstallationID]; exists {
						recipients[device.InstallationID] = device
					}
				}
			}
		}
		recipientIDs := make([]string, 0, len(recipients))
		for recipient := range recipients {
			recipientIDs = append(recipientIDs, recipient)
		}
		sort.Strings(recipientIDs)
		for _, recipient := range recipientIDs {
			route := recipients[recipient]
			relays, _ := json.Marshal(route.Relays)
			if _, err := tx.ExecContext(ctx, `INSERT INTO outbox(event_id, recipient_installation_id, exact_canonical_bytes, recipient_public_key, recipient_relays_json, created_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(event_id,recipient_installation_id) DO UPDATE SET recipient_public_key=excluded.recipient_public_key,recipient_relays_json=excluded.recipient_relays_json,state=CASE WHEN outbox.state='revoked' THEN 'queued' ELSE outbox.state END`, record.Event.ID(), recipient, record.Event.Wire, route.SignerKeyID, string(relays), record.Event.Nostr.CreatedAt); err != nil {
				return err
			}
			if content.Scope == event.ScopeAccountAddressed {
				desiredAccountDeliveries[record.Event.ID()+":"+recipient] = true
			}
		}
	}
	rows, err := tx.QueryContext(ctx, `SELECT o.event_id,o.recipient_installation_id FROM outbox o JOIN canonical_events c ON c.event_id=o.event_id WHERE c.scope=?`, event.ScopeAccountAddressed)
	if err != nil {
		return err
	}
	var revokeDeliveries [][2]string
	for rows.Next() {
		var eventID, recipient string
		if err := rows.Scan(&eventID, &recipient); err != nil {
			rows.Close()
			return err
		}
		if !desiredAccountDeliveries[eventID+":"+recipient] {
			revokeDeliveries = append(revokeDeliveries, [2]string{eventID, recipient})
		}
	}
	rows.Close()
	for _, delivery := range revokeDeliveries {
		if _, err := tx.ExecContext(ctx, `UPDATE outbox SET state='revoked' WHERE event_id=? AND recipient_installation_id=? AND state<>'relay-accepted'`, delivery[0], delivery[1]); err != nil {
			return err
		}
	}
	_, err = tx.ExecContext(ctx, `INSERT INTO projection_checkpoint(id, event_count, rebuilt_at) VALUES (1, ?, ?) ON CONFLICT(id) DO UPDATE SET event_count = excluded.event_count, rebuilt_at = excluded.rebuilt_at`, len(state.Records), time.Now().UTC().UnixMilli())
	return err
}

func boolInt(value bool) int {
	if value {
		return 1
	}
	return 0
}

func firstMailboxTime(state event.State, mailboxID string) int64 {
	var first int64
	for _, record := range state.Records {
		if record.Status != event.StatusProjected || record.Event.Content.Type != event.TypeMailboxCreate {
			continue
		}
		var payload event.MailboxPayload
		if json.Unmarshal(record.Event.Content.Payload, &payload) == nil && payload.MailboxID == mailboxID && (first == 0 || record.Event.Nostr.CreatedAt < first) {
			first = record.Event.Nostr.CreatedAt
		}
	}
	return time.Unix(first, 0).UTC().UnixMilli()
}

func archiveTime(state event.State, target string) int64 {
	var latest int64
	for _, record := range state.Records {
		if record.Status != event.StatusProjected || (record.Event.Content.Type != event.TypeMessageArchive && record.Event.Content.Type != event.TypeMessageReject) {
			continue
		}
		var payload event.TargetPayload
		if json.Unmarshal(record.Event.Content.Payload, &payload) == nil && payload.TargetEventID == target && record.Event.Nostr.CreatedAt > latest {
			latest = record.Event.Nostr.CreatedAt
		}
	}
	return time.Unix(latest, 0).UTC().UnixMilli()
}

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

func (s *SQLite) ResolveMailbox(ctx context.Context, session model.SessionIdentity, repo model.RepositoryContext) (model.Mailbox, error) {
	if strings.TrimSpace(session.Harness) == "" || strings.TrimSpace(session.ExternalSessionID) == "" {
		return model.Mailbox{}, errors.New("harness and external session ID are required")
	}
	resolveMu.Lock()
	defer resolveMu.Unlock()
	var id string
	err := s.db.QueryRowContext(ctx, `SELECT mailbox_id FROM harness_bindings WHERE harness = ? AND external_session_id = ?`, session.Harness, session.ExternalSessionID).Scan(&id)
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		return model.Mailbox{}, err
	}
	now := time.Now().UTC()
	var contents []event.Content
	if errors.Is(err, sql.ErrNoRows) {
		generated, err := uuid.NewV7()
		if err != nil {
			return model.Mailbox{}, err
		}
		id = generated.String()
		create, _ := event.MarshalPayload(event.MailboxPayload{MailboxID: id, Kind: string(model.MailboxAgent), Label: session.Harness})
		bind, _ := event.MarshalPayload(event.MailboxBindingPayload{MailboxID: id, Harness: session.Harness, ExternalSessionID: session.ExternalSessionID})
		contents = append(contents,
			event.Content{Type: event.TypeMailboxCreate, Scope: event.ScopeInstallationPrivate, Payload: create},
			event.Content{Type: event.TypeMailboxBind, Scope: event.ScopeInstallationPrivate, Payload: bind})
	}
	if repo.Directory != "" && !s.hasContext(ctx, id, repo) {
		payload, _ := event.MarshalPayload(event.MailboxContextPayload{MailboxID: id, Context: eventContext(repo)})
		contents = append(contents, event.Content{Type: event.TypeMailboxContext, Scope: event.ScopeInstallationPrivate, Payload: payload})
	}
	if len(contents) > 0 {
		times := make([]time.Time, len(contents))
		for i := range times {
			times[i] = now
		}
		if err := s.appendContents(ctx, contents, times, nil); err != nil {
			return model.Mailbox{}, err
		}
	}
	_, _ = s.db.ExecContext(ctx, `UPDATE mailbox_activity SET last_seen_at = ? WHERE mailbox_id = ?`, now.UnixMilli(), id)
	return s.getMailbox(ctx, id)
}

func eventContext(repo model.RepositoryContext) event.RepositoryContext {
	return event.RepositoryContext{Directory: repo.Directory, GitCommonDir: repo.GitCommonDir, RemoteIdentity: repo.RemoteIdentity, Worktree: repo.Worktree, Branch: repo.Branch}
}

func modelContext(repo event.RepositoryContext) model.RepositoryContext {
	return model.RepositoryContext{Directory: repo.Directory, GitCommonDir: repo.GitCommonDir, RemoteIdentity: repo.RemoteIdentity, Worktree: repo.Worktree, Branch: repo.Branch}
}

func (s *SQLite) hasContext(ctx context.Context, mailboxID string, repo model.RepositoryContext) bool {
	var found int
	err := s.db.QueryRowContext(ctx, `SELECT 1 FROM mailbox_contexts WHERE mailbox_id = ? AND directory = ? AND git_common_dir = ? AND remote_identity = ? AND worktree = ? AND branch = ?`, mailboxID, repo.Directory, repo.GitCommonDir, repo.RemoteIdentity, repo.Worktree, repo.Branch).Scan(&found)
	return err == nil
}

func (s *SQLite) getMailbox(ctx context.Context, id string) (model.Mailbox, error) {
	var m model.Mailbox
	var harness sql.NullString
	var created, seen int64
	err := s.db.QueryRowContext(ctx, `SELECT m.id, m.kind, b.harness, m.created_at, COALESCE(a.last_seen_at, m.created_at) FROM mailboxes m LEFT JOIN harness_bindings b ON b.mailbox_id = m.id LEFT JOIN mailbox_activity a ON a.mailbox_id = m.id WHERE m.id = ?`, id).Scan(&m.ID, &m.Kind, &harness, &created, &seen)
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
	for _, field := range []struct{ column, value string }{{"c.directory", repo.Directory}, {"c.git_common_dir", repo.GitCommonDir}, {"c.remote_identity", repo.RemoteIdentity}, {"c.worktree", repo.Worktree}, {"c.branch", repo.Branch}} {
		if field.value != "" {
			clauses, args = append(clauses, field.column+" = ?"), append(args, field.value)
		}
	}
	if len(clauses) == 0 {
		return nil, errors.New("mailbox search needs repository context")
	}
	query := `SELECT m.id, m.kind, b.harness, m.created_at, COALESCE(a.last_seen_at,m.created_at), c.directory, c.git_common_dir, c.remote_identity, c.worktree, c.branch FROM mailboxes m JOIN harness_bindings b ON b.mailbox_id=m.id JOIN mailbox_contexts c ON c.mailbox_id=m.id LEFT JOIN mailbox_activity a ON a.mailbox_id=m.id WHERE ` + strings.Join(clauses, " OR ") + ` ORDER BY COALESCE(a.last_seen_at,m.created_at) DESC, c.first_seen_at DESC`
	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	seenIDs := make(map[string]bool)
	var result []model.Mailbox
	for rows.Next() {
		var m model.Mailbox
		var created, seen int64
		if err := rows.Scan(&m.ID, &m.Kind, &m.Harness, &created, &seen, &m.Context.Directory, &m.Context.GitCommonDir, &m.Context.RemoteIdentity, &m.Context.Worktree, &m.Context.Branch); err != nil {
			return nil, err
		}
		if seenIDs[m.ID] {
			continue
		}
		seenIDs[m.ID] = true
		m.Label = mailboxLabel(m.Kind, m.Harness, m.ID)
		m.CreatedAt, m.LastSeen = time.UnixMilli(created).UTC(), time.UnixMilli(seen).UTC()
		result = append(result, m)
	}
	return result, rows.Err()
}

func (s *SQLite) Create(ctx context.Context, m model.Message) error {
	if m.ID == "" {
		id, err := uuid.NewV7()
		if err != nil {
			return err
		}
		m.ID = id.String()
	}
	sender, err := s.getMailbox(ctx, m.SenderMailboxID)
	if err != nil {
		return err
	}
	actorLabel := sender.Label
	payload, _ := event.MarshalPayload(event.TextPayload{MessageID: m.ID, Body: m.Body, Details: m.Details, Context: contextPointer(m.Context), ActorLabel: actorLabel})
	typeName := event.TypeMessage
	scope := event.ScopeInstallationPrivate
	recipientInstallationID := s.signer.InstallationID
	if m.RecipientInstallationID != "" {
		recipientInstallationID = m.RecipientInstallationID
	}
	var recipient = &event.MailboxAddress{InstallationID: recipientInstallationID, MailboxID: m.RecipientMailboxID}
	var audience *event.Audience
	var parents []string
	if m.RecipientMailboxID == model.HumanMailboxID && m.SenderMailboxID != model.HumanMailboxID {
		typeName = event.TypeQuestion
		account, membership, _, err := s.localAccountAction(ctx, "")
		if err != nil {
			return err
		}
		audience, parents, recipient, scope = &event.Audience{HumanAccountID: account.ID}, membership, nil, event.ScopeAccountAddressed
	} else if m.SenderMailboxID == model.HumanMailboxID && m.RecipientMailboxID != model.HumanMailboxID {
		account, membership, deviceLabel, err := s.localAccountAction(ctx, "")
		if err != nil {
			return err
		}
		actorLabel = deviceLabel
		payload, _ = event.MarshalPayload(event.TextPayload{MessageID: m.ID, Body: m.Body, Details: m.Details, Context: contextPointer(m.Context), ActorLabel: actorLabel})
		audience, parents, scope = &event.Audience{HumanAccountID: account.ID}, membership, event.ScopeAccountAddressed
	}
	content := event.Content{Type: typeName, Sender: s.localAddress(m.SenderMailboxID), Recipient: recipient, Audience: audience, Parents: parents, Scope: scope, Payload: payload}
	return s.appendContents(ctx, []event.Content{content}, []time.Time{m.CreatedAt}, nil)
}

func contextPointer(repo model.RepositoryContext) *event.RepositoryContext {
	context := eventContext(repo)
	return &context
}

func (s *SQLite) localAddress(mailboxID string) *event.MailboxAddress {
	return &event.MailboxAddress{InstallationID: s.signer.InstallationID, MailboxID: mailboxID}
}

func (s *SQLite) Reply(ctx context.Context, originalID string, reply model.Message) error {
	original, err := s.messageRecord(ctx, originalID)
	if err != nil {
		return err
	}
	if original.message.ArchivedAt != nil || original.message.RecipientMailboxID != model.HumanMailboxID || reply.SenderMailboxID != model.HumanMailboxID || reply.RecipientMailboxID != original.message.SenderMailboxID {
		return ErrAlreadyHandled
	}
	scope := event.ScopeInstallationPrivate
	parents := []string{original.eventID}
	var audience *event.Audience
	actorLabel := "human"
	if original.message.AudienceAccountID != "" {
		account, membership, deviceLabel, err := s.localAccountAction(ctx, original.message.AudienceAccountID)
		if err != nil {
			return err
		}
		parents = uniqueSorted(append(parents, membership...))
		audience, scope, actorLabel = &event.Audience{HumanAccountID: account.ID}, event.ScopeAccountAddressed, deviceLabel
	}
	payload, _ := event.MarshalPayload(event.TextPayload{MessageID: reply.ID, Body: reply.Body, Details: reply.Details, Context: contextPointer(reply.Context), ActorLabel: actorLabel})
	answer := event.Content{Type: event.TypeAnswer, Sender: s.localAddress(reply.SenderMailboxID), Recipient: &event.MailboxAddress{InstallationID: original.message.SenderInstallationID, MailboxID: reply.RecipientMailboxID}, Audience: audience, ThreadID: original.eventID, Parents: parents, Scope: scope, Payload: payload}
	archivePayload, _ := event.MarshalPayload(event.TargetPayload{TargetEventID: original.eventID})
	archive := event.Content{Type: event.TypeMessageArchive, Sender: s.localAddress(model.HumanMailboxID), Audience: audience, Parents: parents, Scope: scope, Payload: archivePayload}
	return s.appendContents(ctx, []event.Content{answer, archive}, []time.Time{reply.CreatedAt, reply.CreatedAt}, nil)
}

type messageWithEvent struct {
	message model.Message
	eventID string
}

func (s *SQLite) messageRecord(ctx context.Context, id string) (messageWithEvent, error) {
	m, err := s.Get(ctx, id)
	if err != nil {
		return messageWithEvent{}, err
	}
	var eventID string
	if err := s.db.QueryRowContext(ctx, `SELECT event_id FROM messages WHERE id = ?`, id).Scan(&eventID); err != nil {
		return messageWithEvent{}, err
	}
	return messageWithEvent{message: m, eventID: eventID}, nil
}

const columns = `msg.id, msg.event_id, msg.thread_event_id, msg.incomplete, msg.peer_received, msg.rejected, CASE WHEN msg.rejected=1 OR o.state='rejected' THEN 'rejected' WHEN msg.peer_received=1 THEN 'peer-received' WHEN o.state='relay-accepted' THEN 'relay-accepted' WHEN o.event_id IS NOT NULL THEN 'queued' ELSE 'local' END, msg.audience_account_id, msg.directory, msg.git_common_dir, msg.remote_identity, msg.worktree, msg.branch, msg.sender_installation_id, msg.recipient_installation_id, msg.sender_mailbox_id, msg.recipient_mailbox_id, COALESCE(NULLIF(msg.actor_label,''),CASE WHEN sm.kind='human' THEN 'human' WHEN sm.kind='agent' THEN sb.harness||':'||substr(sm.id,-8) ELSE 'remote:'||substr(msg.sender_mailbox_id,-8) END), COALESCE(hd.label,''), CASE WHEN rm.kind='human' THEN 'human' WHEN rm.kind='agent' THEN rb.harness||':'||substr(rm.id,-8) ELSE 'remote:'||substr(msg.recipient_mailbox_id,-8) END, msg.body, msg.details, msg.reply_to, msg.created_at, msg.archived_at, d.completed_at`
const joins = ` messages msg LEFT JOIN mailboxes sm ON sm.id=msg.sender_mailbox_id AND sm.installation_id=msg.sender_installation_id LEFT JOIN harness_bindings sb ON sb.mailbox_id=sm.id LEFT JOIN mailboxes rm ON rm.id=msg.recipient_mailbox_id AND rm.installation_id=msg.recipient_installation_id LEFT JOIN harness_bindings rb ON rb.mailbox_id=rm.id LEFT JOIN human_account_devices hd ON hd.account_id=msg.audience_account_id AND hd.installation_id=msg.sender_installation_id LEFT JOIN delivery_facts d ON d.message_id=msg.id LEFT JOIN (SELECT event_id,CASE WHEN SUM(CASE WHEN state='queued' THEN 1 ELSE 0 END)>0 THEN 'queued' WHEN SUM(CASE WHEN state='relay-accepted' THEN 1 ELSE 0 END)>0 THEN 'relay-accepted' WHEN SUM(CASE WHEN state='rejected' THEN 1 ELSE 0 END)>0 THEN 'rejected' ELSE '' END AS state FROM outbox GROUP BY event_id) o ON o.event_id=msg.event_id `

type scanner interface{ Scan(...any) error }

func scanMessage(row scanner) (model.Message, error) {
	var m model.Message
	var reply sql.NullString
	var created int64
	var archived, completed sql.NullInt64
	err := row.Scan(&m.ID, &m.EventID, &m.ThreadID, &m.Incomplete, &m.PeerReceived, &m.Rejected, &m.DeliveryState, &m.AudienceAccountID, &m.Context.Directory, &m.Context.GitCommonDir, &m.Context.RemoteIdentity, &m.Context.Worktree, &m.Context.Branch, &m.SenderInstallationID, &m.RecipientInstallationID, &m.SenderMailboxID, &m.RecipientMailboxID, &m.SenderLabel, &m.SourceDeviceLabel, &m.RecipientLabel, &m.Body, &m.Details, &reply, &created, &archived, &completed)
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
			where = append(where, "d.completed_at IS NOT NULL")
		} else {
			where = append(where, "d.completed_at IS NULL")
		}
	}
	query := `SELECT ` + columns + ` FROM ` + joins
	if len(where) > 0 {
		query += ` WHERE ` + strings.Join(where, " AND ")
	}
	if f.NewestFirst {
		query += ` ORDER BY msg.created_at DESC,msg.id DESC LIMIT ?`
	} else {
		query += ` ORDER BY msg.created_at,msg.id LIMIT ?`
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
	record, err := s.messageRecord(ctx, id)
	if err != nil {
		return err
	}
	if record.message.ArchivedAt != nil || record.message.RecipientMailboxID != model.HumanMailboxID {
		return ErrAlreadyHandled
	}
	payload, _ := event.MarshalPayload(event.TargetPayload{TargetEventID: record.eventID})
	parents := []string{record.eventID}
	scope := event.ScopeInstallationPrivate
	var audience *event.Audience
	if record.message.AudienceAccountID != "" {
		account, membership, _, err := s.localAccountAction(ctx, record.message.AudienceAccountID)
		if err != nil {
			return err
		}
		parents = uniqueSorted(append(parents, membership...))
		audience, scope = &event.Audience{HumanAccountID: account.ID}, event.ScopeAccountAddressed
	}
	content := event.Content{Type: event.TypeMessageArchive, Sender: s.localAddress(model.HumanMailboxID), Audience: audience, Parents: parents, Scope: scope, Payload: payload}
	return s.appendContents(ctx, []event.Content{content}, nil, nil)
}

func (s *SQLite) Claim(ctx context.Context, claim Claim, token string) (model.Message, error) {
	now := time.Now().UTC()
	var where []string
	var args []any
	if claim.MessageID != "" {
		where, args = append(where, "m.id = ?"), append(args, claim.MessageID)
	}
	if claim.ReplyTo != "" {
		where, args = append(where, "m.reply_to = ?"), append(args, claim.ReplyTo)
	}
	if claim.RecipientMailboxID != "" {
		where, args = append(where, "m.recipient_mailbox_id = ?"), append(args, claim.RecipientMailboxID)
	}
	where = append(where, "d.completed_at IS NULL", "(d.delivery_token IS NULL OR d.delivery_lease_until < ?)")
	args = append(args, now.UnixMilli())
	query := `UPDATE delivery_facts SET delivery_token=?, delivery_lease_until=? WHERE message_id=(SELECT m.id FROM messages m JOIN delivery_facts d ON d.message_id=m.id WHERE ` + strings.Join(where, " AND ") + ` ORDER BY m.created_at,m.id LIMIT 1) RETURNING message_id`
	queryArgs := append([]any{token, now.Add(30 * time.Second).UnixMilli()}, args...)
	var id string
	if err := s.db.QueryRowContext(ctx, query, queryArgs...).Scan(&id); errors.Is(err, sql.ErrNoRows) {
		return model.Message{}, ErrNotReady
	} else if err != nil {
		return model.Message{}, err
	}
	return s.Get(ctx, id)
}

func (s *SQLite) Complete(ctx context.Context, id, token string) error {
	record, err := s.messageRecord(ctx, id)
	if err != nil {
		return err
	}
	var stored string
	if err := s.db.QueryRowContext(ctx, `SELECT delivery_token FROM delivery_facts WHERE message_id=? AND completed_at IS NULL`, id).Scan(&stored); err != nil || stored != token {
		return ErrNotReady
	}
	payload, _ := event.MarshalPayload(event.TargetPayload{TargetEventID: record.eventID})
	content := event.Content{Type: event.TypeMessageArchive, Sender: s.localAddress(record.message.RecipientMailboxID), Parents: []string{record.eventID}, Scope: event.ScopeInstallationPrivate, Payload: payload}
	now := time.Now().UTC()
	return s.appendContents(ctx, []event.Content{content}, []time.Time{now}, func(tx *sql.Tx) error {
		result, err := tx.ExecContext(ctx, `UPDATE delivery_facts SET completed_at=?,delivery_token=NULL,delivery_lease_until=NULL WHERE message_id=? AND delivery_token=? AND completed_at IS NULL`, now.UnixMilli(), id, token)
		if err != nil {
			return err
		}
		n, _ := result.RowsAffected()
		if n != 1 {
			return ErrNotReady
		}
		return nil
	})
}

func (s *SQLite) Release(ctx context.Context, id, token string) error {
	_, err := s.db.ExecContext(ctx, `UPDATE delivery_facts SET delivery_token=NULL,delivery_lease_until=NULL WHERE message_id=? AND delivery_token=? AND completed_at IS NULL`, id, token)
	return err
}

func (s *SQLite) Rebuild(ctx context.Context) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	rows, err := tx.QueryContext(ctx, `SELECT raw FROM canonical_events ORDER BY event_id`)
	if err != nil {
		return err
	}
	var raw [][]byte
	for rows.Next() {
		var item []byte
		if err := rows.Scan(&item); err != nil {
			rows.Close()
			return err
		}
		raw = append(raw, item)
	}
	rows.Close()
	if err := s.rebuildTx(ctx, tx, event.Reduce(raw, s.policy())); err != nil {
		return err
	}
	return tx.Commit()
}

type Peer struct {
	InstallationID string   `json:"installation_id"`
	SignerKeyID    string   `json:"signer_key_id"`
	Name           string   `json:"name,omitempty"`
	Relays         []string `json:"relays,omitempty"`
	Trusted        bool     `json:"trusted"`
}

func (s *SQLite) TrustPeer(ctx context.Context, peer Peer) error {
	if len(peer.Relays) > 3 {
		return errors.New("a peer may have at most three relay hints")
	}
	for index, relay := range peer.Relays {
		normalized, err := normalizeRelay(relay)
		if err != nil {
			return err
		}
		peer.Relays[index] = normalized
	}
	payload, err := event.MarshalPayload(event.PeerPayload{InstallationID: peer.InstallationID, SignerKeyID: peer.SignerKeyID, Name: peer.Name, Relays: peer.Relays})
	if err != nil {
		return err
	}
	parents := s.peerParents(ctx, peer.InstallationID)
	return s.appendContents(ctx, []event.Content{{Type: event.TypePeerTrust, Parents: parents, Scope: event.ScopeInstallationPrivate, Payload: payload}}, nil, nil)
}

func (s *SQLite) DistrustPeer(ctx context.Context, installationID string) error {
	payload, _ := event.MarshalPayload(event.PeerPayload{InstallationID: installationID})
	return s.appendContents(ctx, []event.Content{{Type: event.TypePeerDistrust, Parents: s.peerParents(ctx, installationID), Scope: event.ScopeInstallationPrivate, Payload: payload}}, nil, nil)
}

func (s *SQLite) peerParents(ctx context.Context, installationID string) []string {
	rows, err := s.db.QueryContext(ctx, `SELECT event_id,raw FROM canonical_events WHERE event_type IN ('peer.trust','peer.distrust')`)
	if err != nil {
		return nil
	}
	defer rows.Close()
	var ids []string
	for rows.Next() {
		var id string
		var raw []byte
		if rows.Scan(&id, &raw) == nil {
			inspected := event.Inspect(raw)
			var p event.PeerPayload
			if json.Unmarshal(inspected.Event.Content.Payload, &p) == nil && p.InstallationID == installationID {
				ids = append(ids, id)
			}
		}
	}
	sort.Strings(ids)
	return ids
}

func (s *SQLite) ListPeers(ctx context.Context) ([]Peer, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT installation_id,signer_key_id,name,relays_json,trusted FROM peers ORDER BY name,installation_id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []Peer
	for rows.Next() {
		var p Peer
		var relays string
		if err := rows.Scan(&p.InstallationID, &p.SignerKeyID, &p.Name, &relays, &p.Trusted); err != nil {
			return nil, err
		}
		_ = json.Unmarshal([]byte(relays), &p.Relays)
		result = append(result, p)
	}
	return result, rows.Err()
}

func (s *SQLite) SetMailboxShare(ctx context.Context, mailboxID, peerID string, active bool) error {
	if _, err := s.getMailbox(ctx, mailboxID); err != nil {
		return err
	}
	payload, _ := event.MarshalPayload(event.MailboxSharePayload{MailboxID: mailboxID, PeerInstallationID: peerID})
	typeName := event.TypeMailboxShare
	if !active {
		typeName = event.TypeMailboxShareRevoke
	}
	rows, _ := s.db.QueryContext(ctx, `SELECT event_id,raw FROM canonical_events WHERE event_type IN ('mailbox.share','mailbox.share.revoke')`)
	var parents []string
	if rows != nil {
		defer rows.Close()
		for rows.Next() {
			var id string
			var raw []byte
			if rows.Scan(&id, &raw) == nil {
				inspected := event.Inspect(raw)
				var p event.MailboxSharePayload
				if json.Unmarshal(inspected.Event.Content.Payload, &p) == nil && p.MailboxID == mailboxID && p.PeerInstallationID == peerID {
					parents = append(parents, id)
				}
			}
		}
	}
	sort.Strings(parents)
	return s.appendContents(ctx, []event.Content{{Type: typeName, Parents: parents, Scope: event.ScopeInstallationPrivate, Payload: payload}}, nil, nil)
}

func (s *SQLite) Quarantine(ctx context.Context, raw []byte, relay, eventID, reason string, received time.Time) error {
	if len(raw) > event.MaxWireBytes*4 {
		return errors.New("quarantine wrapper exceeds the input limit")
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err = tx.ExecContext(ctx, `INSERT INTO quarantine(raw_wrapper,relay,event_id,rejection_reason,received_at) VALUES (?,?,?,?,?)`, raw, relay, eventID, reason, received.UnixMilli()); err != nil {
		return err
	}
	cutoff := received.Add(-quarantineMaxAge).UnixMilli()
	_, _ = tx.ExecContext(ctx, `DELETE FROM quarantine WHERE received_at < ?`, cutoff)
	_, _ = tx.ExecContext(ctx, `DELETE FROM quarantine WHERE id NOT IN (SELECT id FROM quarantine ORDER BY received_at DESC,id DESC LIMIT ?)`, quarantineMaxRows)
	for {
		var total int64
		_ = tx.QueryRowContext(ctx, `SELECT COALESCE(sum(length(raw_wrapper)),0) FROM quarantine`).Scan(&total)
		if total <= quarantineMaxBytes {
			break
		}
		result, e := tx.ExecContext(ctx, `DELETE FROM quarantine WHERE id=(SELECT id FROM quarantine ORDER BY received_at,id LIMIT 1)`)
		if e != nil {
			return e
		}
		n, _ := result.RowsAffected()
		if n == 0 {
			break
		}
	}
	return tx.Commit()
}

func (s *SQLite) Stage(ctx context.Context, raw []byte, relay, eventID, reason string, received, retry time.Time) error {
	_, err := s.db.ExecContext(ctx, `INSERT INTO inbound_staging(raw_wrapper,relay,event_id,failure_reason,received_at,retry_at) VALUES (?,?,?,?,?,?)`, raw, relay, eventID, reason, received.UnixMilli(), retry.UnixMilli())
	return err
}

func (s *SQLite) ReevaluateQuarantine(ctx context.Context, quarantineID int64, retryAt time.Time) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	result, err := tx.ExecContext(ctx, `INSERT INTO inbound_staging(raw_wrapper,relay,event_id,failure_reason,received_at,retry_at) SELECT raw_wrapper,relay,event_id,'explicit re-evaluation',received_at,? FROM quarantine WHERE id=?`, retryAt.UnixMilli(), quarantineID)
	if err != nil {
		return err
	}
	rows, _ := result.RowsAffected()
	if rows != 1 {
		return ErrNotFound
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM quarantine WHERE id=?`, quarantineID); err != nil {
		return err
	}
	return tx.Commit()
}

func (s *SQLite) PendingOutbox(ctx context.Context, limit int) ([]OutboundJob, error) {
	if limit <= 0 || limit > 1000 {
		limit = 100
	}
	rows, err := s.db.QueryContext(ctx, `SELECT event_id,recipient_installation_id,exact_canonical_bytes,state FROM outbox WHERE state='queued' ORDER BY created_at,event_id LIMIT ?`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var jobs []OutboundJob
	for rows.Next() {
		var job OutboundJob
		if err := rows.Scan(&job.EventID, &job.RecipientInstallationID, &job.ExactCanonicalBytes, &job.State); err != nil {
			return nil, err
		}
		jobs = append(jobs, job)
	}
	return jobs, rows.Err()
}
