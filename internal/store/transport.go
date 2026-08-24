package store

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"slices"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
	"github.com/wbbradley/hq/internal/nostrwire"
)

type RelayJob struct {
	CanonicalEventID      string
	GiftWrapEventID       string
	RecipientInstallation string
	ExactGiftWrapBytes    []byte
	RelayURL              string
}

type ReceiveResult struct {
	OuterEventID     string
	CanonicalEventID string
	Status           string
}

type RelayAttempt struct {
	EventID                 string
	RecipientInstallationID string
	RelayURL                string
	State                   string
	Message                 string
	AttemptCount            int
	AcceptedAt              *time.Time
}

func (s *SQLite) InstallationIdentity() (string, string) {
	return s.signer.InstallationID, s.signer.PublicKey()
}

func (s *SQLite) WireCodec(random io.Reader, now func() time.Time) *nostrwire.Codec {
	return nostrwire.New(s.signer.SecretKey, random, now)
}

func (s *SQLite) CreatePeerMessage(ctx context.Context, message model.Message, recipientInstallationID, recipientMailboxID string) error {
	if _, err := s.getMailbox(ctx, message.SenderMailboxID); err != nil {
		return err
	}
	if message.ID == "" {
		id, err := uuid.NewV7()
		if err != nil {
			return err
		}
		message.ID = id.String()
	}
	if !message.Purpose.Valid() {
		return fmt.Errorf("unsupported message purpose %q", message.Purpose)
	}
	message.Purpose = model.NormalizeMessagePurpose(message.Purpose)
	payload, err := event.MarshalPayload(event.TextPayload{MessageID: message.ID, Body: message.Body, Details: message.Details, Purpose: message.Purpose, Context: contextPointer(message.Context)})
	if err != nil {
		return err
	}
	typeName := event.TypeMessage
	if message.SenderMailboxID != model.HumanMailboxID {
		typeName = event.TypeQuestion
	}
	content := event.Content{Type: typeName, Sender: s.localAddress(message.SenderMailboxID), Recipient: &event.MailboxAddress{InstallationID: recipientInstallationID, MailboxID: recipientMailboxID}, Scope: event.ScopePeerAddressed, Payload: payload}
	return s.appendContents(ctx, []event.Content{content}, []time.Time{message.CreatedAt}, nil)
}

func (s *SQLite) AddRelay(ctx context.Context, config RelayConfig) error {
	normalized, err := normalizeRelay(config.URL)
	if err != nil {
		return err
	}
	if !config.Read && !config.Write {
		return errors.New("relay must enable reads, writes, or both")
	}
	if config.Read && !config.RequireAuth && !config.UnsafeNoAuth {
		return errors.New("private relay reads require auth; set the unsafe development override explicitly")
	}
	config.URL = normalized
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err = tx.ExecContext(ctx, `INSERT INTO relays(url,read_enabled,write_enabled,require_auth,unsafe_no_auth,created_at) VALUES (?,?,?,?,?,?) ON CONFLICT(url) DO UPDATE SET read_enabled=excluded.read_enabled,write_enabled=excluded.write_enabled,require_auth=excluded.require_auth,unsafe_no_auth=excluded.unsafe_no_auth`, config.URL, boolInt(config.Read), boolInt(config.Write), boolInt(config.RequireAuth), boolInt(config.UnsafeNoAuth), time.Now().UTC().UnixMilli()); err != nil {
		return err
	}
	if err := recordMutationTx(ctx, tx, nil); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicRelays, domain.TopicNetwork})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) RemoveRelay(ctx context.Context, relayURL string) error {
	normalized, err := normalizeRelay(relayURL)
	if err != nil {
		return err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err = tx.ExecContext(ctx, `DELETE FROM relays WHERE url=?`, normalized); err != nil {
		return err
	}
	if err := recordMutationTx(ctx, tx, nil); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicRelays, domain.TopicNetwork})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) ListRelays(ctx context.Context) ([]RelayConfig, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT url,read_enabled,write_enabled,require_auth,unsafe_no_auth FROM relays ORDER BY url`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []RelayConfig
	for rows.Next() {
		var item RelayConfig
		if err := rows.Scan(&item.URL, &item.Read, &item.Write, &item.RequireAuth, &item.UnsafeNoAuth); err != nil {
			return nil, err
		}
		result = append(result, item)
	}
	return result, rows.Err()
}

func (s *SQLite) OutboundRelays(ctx context.Context) ([]string, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT relays_json FROM peers WHERE trusted=1 UNION ALL SELECT recipient_relays_json FROM outbox WHERE state<>'revoked'`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	seen := make(map[string]bool)
	var result []string
	for rows.Next() {
		var raw string
		if err := rows.Scan(&raw); err != nil {
			return nil, err
		}
		var hints []string
		if json.Unmarshal([]byte(raw), &hints) != nil {
			continue
		}
		for _, hint := range hints {
			if !seen[hint] {
				seen[hint] = true
				result = append(result, hint)
			}
		}
	}
	slices.Sort(result)
	return result, rows.Err()
}

func normalizeRelay(value string) (string, error) {
	parsed, err := url.Parse(strings.TrimSpace(value))
	if err != nil || (parsed.Scheme != "ws" && parsed.Scheme != "wss") || parsed.Host == "" || parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" {
		return "", fmt.Errorf("invalid relay URL %q", value)
	}
	parsed.Scheme = strings.ToLower(parsed.Scheme)
	parsed.Host = strings.ToLower(parsed.Host)
	parsed.Path = strings.TrimSuffix(parsed.Path, "/")
	return parsed.String(), nil
}

func (s *SQLite) PrepareOutbound(ctx context.Context, limit int) (int, error) {
	return s.prepareOutbound(ctx, limit, rand.Reader, time.Now)
}

func (s *SQLite) prepareOutbound(ctx context.Context, limit int, random io.Reader, now func() time.Time) (int, error) {
	if limit <= 0 || limit > 1000 {
		limit = 100
	}
	rows, err := s.db.QueryContext(ctx, `SELECT event_id,recipient_installation_id,exact_canonical_bytes,recipient_public_key FROM outbox WHERE gift_wrap_event_id IS NULL AND state<>'revoked' AND recipient_public_key<>'' ORDER BY created_at,event_id,recipient_installation_id LIMIT ?`, limit)
	if err != nil {
		return 0, err
	}
	type pending struct {
		id, recipientID, recipientKey string
		raw                           []byte
	}
	var items []pending
	for rows.Next() {
		var item pending
		if err := rows.Scan(&item.id, &item.recipientID, &item.raw, &item.recipientKey); err != nil {
			rows.Close()
			return 0, err
		}
		items = append(items, item)
	}
	rows.Close()
	codec := nostrwire.New(s.signer.SecretKey, random, now)
	prepared := 0
	for _, item := range items {
		inspection := event.Inspect(item.raw)
		if inspection.Status == event.StatusInvalid {
			return prepared, fmt.Errorf("outbox canonical event %s is invalid", item.id)
		}
		wrapped, err := codec.Wrap(inspection.Event, item.recipientKey)
		if err != nil {
			return prepared, err
		}
		result, err := s.db.ExecContext(ctx, `UPDATE outbox SET recipient_public_key=?,gift_wrap_event_id=?,exact_gift_wrap_bytes=?,ephemeral_public_key=?,wrapped_at=? WHERE event_id=? AND recipient_installation_id=? AND gift_wrap_event_id IS NULL`, item.recipientKey, wrapped.EventID, wrapped.ExactWire, wrapped.EphemeralKey, now().UTC().UnixMilli(), item.id, item.recipientID)
		if err != nil {
			return prepared, fmt.Errorf("persist exact gift wrap: %w", err)
		}
		changed, _ := result.RowsAffected()
		prepared += int(changed)
	}
	return prepared, nil
}

func (s *SQLite) RelayJobs(ctx context.Context, relayURL string, limit int, now time.Time) ([]RelayJob, error) {
	normalized, err := normalizeRelay(relayURL)
	if err != nil {
		return nil, err
	}
	if limit <= 0 || limit > 1000 {
		limit = 100
	}
	var configuredWrite bool
	if err := s.db.QueryRowContext(ctx, `SELECT write_enabled FROM relays WHERE url=?`, normalized).Scan(&configuredWrite); err != nil && !errors.Is(err, sql.ErrNoRows) {
		return nil, err
	}
	rows, err := s.db.QueryContext(ctx, `SELECT o.event_id,o.gift_wrap_event_id,o.recipient_installation_id,o.exact_gift_wrap_bytes,o.recipient_relays_json,COALESCE(a.state,''),COALESCE(a.next_attempt_at,0) FROM outbox o LEFT JOIN outbound_relay_attempts a ON a.event_id=o.event_id AND a.recipient_installation_id=o.recipient_installation_id AND a.relay_url=? WHERE o.gift_wrap_event_id IS NOT NULL AND o.state<>'revoked' ORDER BY o.created_at,o.event_id,o.recipient_installation_id`, normalized)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var jobs []RelayJob
	for rows.Next() {
		var job RelayJob
		var relaysJSON, state string
		var retryAt int64
		if err := rows.Scan(&job.CanonicalEventID, &job.GiftWrapEventID, &job.RecipientInstallation, &job.ExactGiftWrapBytes, &relaysJSON, &state, &retryAt); err != nil {
			return nil, err
		}
		var hints []string
		if err := jsonUnmarshal([]byte(relaysJSON), &hints); err != nil || (!configuredWrite && !slices.Contains(hints, normalized)) {
			continue
		}
		if state == "accepted" || (retryAt > 0 && retryAt > now.UnixMilli()) {
			continue
		}
		job.RelayURL = normalized
		jobs = append(jobs, job)
		if len(jobs) == limit {
			break
		}
	}
	return jobs, rows.Err()
}

func (s *SQLite) RecordPublish(ctx context.Context, eventID, recipientInstallationID, relayURL string, accepted, rejected bool, message string, now, retryAt time.Time) error {
	normalized, err := normalizeRelay(relayURL)
	if err != nil {
		return err
	}
	state := "retry"
	var acceptedAt any
	if accepted {
		state, acceptedAt = "accepted", now.UTC().UnixMilli()
	} else if rejected {
		state = "rejected"
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	_, err = tx.ExecContext(ctx, `INSERT INTO outbound_relay_attempts(event_id,recipient_installation_id,relay_url,state,message,attempt_count,last_attempt_at,next_attempt_at,accepted_at) VALUES (?,?,?,?,?,1,?,?,?) ON CONFLICT(event_id,recipient_installation_id,relay_url) DO UPDATE SET state=excluded.state,message=excluded.message,attempt_count=outbound_relay_attempts.attempt_count+1,last_attempt_at=excluded.last_attempt_at,next_attempt_at=excluded.next_attempt_at,accepted_at=COALESCE(outbound_relay_attempts.accepted_at,excluded.accepted_at)`, eventID, recipientInstallationID, normalized, state, message, now.UTC().UnixMilli(), retryAt.UTC().UnixMilli(), acceptedAt)
	if err != nil {
		return err
	}
	if accepted {
		if _, err := tx.ExecContext(ctx, `UPDATE outbox SET state='relay-accepted' WHERE event_id=? AND recipient_installation_id=?`, eventID, recipientInstallationID); err != nil {
			return err
		}
	} else if rejected {
		if _, err := tx.ExecContext(ctx, `UPDATE outbox SET state='rejected' WHERE event_id=? AND recipient_installation_id=? AND state<>'relay-accepted'`, eventID, recipientInstallationID); err != nil {
			return err
		}
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicNetwork, domain.TopicMessages})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) ReceiveGiftWrap(ctx context.Context, raw []byte, relayURL string, received time.Time) (ReceiveResult, error) {
	normalized, err := normalizeRelay(relayURL)
	if err != nil {
		return ReceiveResult{}, err
	}
	codec := nostrwire.New(s.signer.SecretKey, nil, nil)
	unwrapped, err := codec.Unwrap(raw)
	if err != nil {
		_ = s.Quarantine(context.Background(), raw, normalized, "", err.Error(), received)
		return ReceiveResult{}, err
	}
	result := ReceiveResult{OuterEventID: unwrapped.Outer.ID, CanonicalEventID: unwrapped.CanonicalEvent.ID(), Status: "projected"}
	content := unwrapped.CanonicalEvent.Content
	sourceDeviceState := ""
	localRoute := content.Recipient != nil && content.Recipient.InstallationID == s.signer.InstallationID
	if content.Scope == event.ScopeAccountAddressed && content.Audience != nil {
		var active int
		_ = s.db.QueryRowContext(ctx, `SELECT count(*) FROM human_account_devices WHERE account_id=? AND installation_id=? AND state='active'`, content.Audience.HumanAccountID, s.signer.InstallationID).Scan(&active)
		_ = s.db.QueryRowContext(ctx, `SELECT state FROM human_account_devices WHERE account_id=? AND installation_id=?`, content.Audience.HumanAccountID, content.InstallationID).Scan(&sourceDeviceState)
		localRoute = active > 0
	}
	if !localRoute {
		reason := "canonical event is not addressed to the local installation or human account"
		if content.Scope == event.ScopeAccountAddressed {
			reason = "account traffic: " + reason
		}
		err := errors.New(reason)
		_ = s.Quarantine(context.Background(), raw, normalized, unwrapped.Outer.ID, err.Error(), received)
		return ReceiveResult{}, err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		_ = s.Stage(context.Background(), raw, normalized, unwrapped.Outer.ID, err.Error(), received, received.Add(time.Minute))
		return ReceiveResult{}, err
	}
	defer tx.Rollback()
	var existing string
	if err := tx.QueryRowContext(ctx, `SELECT status FROM inbound_wrappers WHERE outer_event_id=?`, unwrapped.Outer.ID).Scan(&existing); err == nil {
		return ReceiveResult{OuterEventID: unwrapped.Outer.ID, CanonicalEventID: unwrapped.CanonicalEvent.ID(), Status: "duplicate-wrapper"}, nil
	}
	var usedBy string
	if err := tx.QueryRowContext(ctx, `SELECT outer_event_id FROM inbound_wrappers WHERE ephemeral_public_key=?`, unwrapped.Outer.PubKey).Scan(&usedBy); err == nil && usedBy != unwrapped.Outer.ID {
		tx.Rollback()
		err := errors.New("gift wrap reused an ephemeral public key")
		_ = s.Quarantine(context.Background(), raw, normalized, unwrapped.Outer.ID, err.Error(), received)
		return ReceiveResult{}, err
	}
	var logical int
	_ = tx.QueryRowContext(ctx, `SELECT count(*) FROM inbound_wrappers WHERE origin_installation_id=? AND canonical_event_id=?`, unwrapped.Envelope.OriginInstallationID, unwrapped.CanonicalEvent.ID()).Scan(&logical)
	if logical > 0 {
		result.Status = "duplicate-logical"
	}
	_, err = tx.ExecContext(ctx, `INSERT INTO inbound_wrappers(outer_event_id,ephemeral_public_key,origin_installation_id,canonical_event_id,exact_wrapper,relay_url,status,received_at) VALUES (?,?,?,?,?,?,?,?)`, unwrapped.Outer.ID, unwrapped.Outer.PubKey, unwrapped.Envelope.OriginInstallationID, unwrapped.CanonicalEvent.ID(), raw, normalized, result.Status, received.UTC().UnixMilli())
	if err != nil {
		return ReceiveResult{}, err
	}
	var canonicalCommit canonicalIngest
	var canonicalChange domain.Invalidation
	if logical == 0 {
		var ingestErr error
		canonicalCommit, ingestErr = s.ingestCanonicalTx(ctx, tx, []event.SignedEvent{unwrapped.CanonicalEvent}, false)
		if ingestErr != nil {
			tx.Rollback()
			if strings.Contains(strings.ToLower(ingestErr.Error()), "locked") || strings.Contains(strings.ToLower(ingestErr.Error()), "busy") {
				_ = s.Stage(context.Background(), raw, normalized, unwrapped.Outer.ID, ingestErr.Error(), received, received.Add(time.Minute))
			} else {
				reason := ingestErr.Error()
				if sourceDeviceState == "revoked" {
					reason = "revoked account device: " + reason
				} else if content.Scope == event.ScopeAccountAddressed {
					reason = "account traffic: " + reason
				}
				_ = s.Quarantine(context.Background(), raw, normalized, unwrapped.Outer.ID, reason, received)
			}
			return ReceiveResult{}, ingestErr
		}
		if len(canonicalCommit.EventIDs) > 0 {
			canonicalChange, err = recordChangeTx(ctx, tx, canonicalChangeTopics)
			if err != nil {
				return ReceiveResult{}, err
			}
		}
	}
	if err := tx.Commit(); err != nil {
		_ = s.Stage(context.Background(), raw, normalized, unwrapped.Outer.ID, err.Error(), received, received.Add(time.Minute))
		return ReceiveResult{}, err
	}
	if len(canonicalCommit.EventIDs) > 0 {
		s.notifyChange(canonicalChange)
	}
	return result, nil
}

func (s *SQLite) SetRelaySyncState(ctx context.Context, relayURL string, connected, authenticated bool, lastError string, eose, eventAt *time.Time) error {
	var eoseValue, eventValue any
	if eose != nil {
		eoseValue = eose.UTC().UnixMilli()
	}
	if eventAt != nil {
		eventValue = eventAt.UTC().UnixMilli()
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `INSERT INTO relay_sync_state(relay_url,connected,authenticated,last_eose_at,last_event_at,last_error) VALUES (?,?,?,?,?,?) ON CONFLICT(relay_url) DO UPDATE SET connected=excluded.connected,authenticated=excluded.authenticated,last_eose_at=COALESCE(excluded.last_eose_at,relay_sync_state.last_eose_at),last_event_at=COALESCE(excluded.last_event_at,relay_sync_state.last_event_at),last_error=excluded.last_error`, relayURL, boolInt(connected), boolInt(authenticated), eoseValue, eventValue, lastError); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicNetwork})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) RelayAttempts(ctx context.Context, eventID string) ([]RelayAttempt, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT event_id,recipient_installation_id,relay_url,state,message,attempt_count,accepted_at FROM outbound_relay_attempts WHERE event_id=? ORDER BY recipient_installation_id,relay_url`, eventID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []RelayAttempt
	for rows.Next() {
		var item RelayAttempt
		var accepted sql.NullInt64
		if err := rows.Scan(&item.EventID, &item.RecipientInstallationID, &item.RelayURL, &item.State, &item.Message, &item.AttemptCount, &accepted); err != nil {
			return nil, err
		}
		if accepted.Valid {
			value := time.UnixMilli(accepted.Int64).UTC()
			item.AcceptedAt = &value
		}
		result = append(result, item)
	}
	return result, rows.Err()
}

func (s *SQLite) NetworkStatus(ctx context.Context) (NetworkStatus, error) {
	var status NetworkStatus
	queries := []struct {
		query  string
		target *int
	}{
		{`SELECT count(*) FROM outbox WHERE state NOT IN ('relay-accepted','revoked')`, &status.Queued},
		{`SELECT count(*) FROM outbox WHERE state='relay-accepted'`, &status.RelayAccepted},
		{`SELECT count(*) FROM outbound_relay_attempts WHERE state='rejected'`, &status.Rejected},
		{`SELECT count(*) FROM canonical_events WHERE reduction_status='unresolved'`, &status.Unresolved},
		{`SELECT count(*) FROM canonical_events WHERE reduction_status='unsupported'`, &status.Unsupported},
		{`SELECT count(*) FROM inbound_staging`, &status.Staged},
		{`SELECT count(*) FROM quarantine`, &status.Quarantined},
		{`SELECT count(*) FROM human_account_devices d JOIN human_account_default a ON a.account_id=d.account_id WHERE d.state='active'`, &status.AccountMembers},
		{`SELECT count(*) FROM outbox o JOIN canonical_events c ON c.event_id=o.event_id WHERE c.scope='account-addressed' AND o.state NOT IN ('relay-accepted','revoked')`, &status.PendingAccountFanout},
		{`SELECT count(*) FROM quarantine WHERE rejection_reason LIKE 'account traffic:%'`, &status.InvalidAccountTraffic},
		{`SELECT count(*) FROM quarantine WHERE rejection_reason LIKE 'revoked account device:%'`, &status.RevokedDeviceTraffic},
	}
	for _, item := range queries {
		if err := s.db.QueryRowContext(ctx, item.query).Scan(item.target); err != nil {
			return status, err
		}
	}
	rows, err := s.db.QueryContext(ctx, `SELECT relay_url,connected,authenticated,last_eose_at,last_event_at,last_error FROM relay_sync_state ORDER BY relay_url`)
	if err != nil {
		return status, err
	}
	defer rows.Close()
	for rows.Next() {
		var relay RelayHealth
		var eose, eventAt sql.NullInt64
		if err := rows.Scan(&relay.URL, &relay.Connected, &relay.Authenticated, &eose, &eventAt, &relay.LastError); err != nil {
			return status, err
		}
		if eose.Valid {
			value := time.UnixMilli(eose.Int64).UTC()
			relay.LastEOSE = &value
		}
		if eventAt.Valid {
			value := time.UnixMilli(eventAt.Int64).UTC()
			relay.LastEvent = &value
		}
		status.Relays = append(status.Relays, relay)
	}
	return status, rows.Err()
}

// jsonUnmarshal is a small seam for relay-hint parsing tests.
var jsonUnmarshal = func(raw []byte, target any) error {
	return json.Unmarshal(raw, target)
}
