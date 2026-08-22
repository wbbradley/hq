package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

var agentOwnershipMu sync.Mutex

func validateAgentName(name string) error {
	if name == "self" || name == "human" {
		return fmt.Errorf("agent name %q is reserved", name)
	}
	if len(name) < 1 || len(name) > 63 || name[0] < 'a' || name[0] > 'z' || name[len(name)-1] == '-' {
		return errors.New("agent name must match [a-z](?:[a-z0-9-]{0,61}[a-z0-9])?")
	}
	for index := 1; index < len(name); index++ {
		character := name[index]
		if (character < 'a' || character > 'z') && (character < '0' || character > '9') && character != '-' {
			return errors.New("agent name must match [a-z](?:[a-z0-9-]{0,61}[a-z0-9])?")
		}
	}
	return nil
}

func (s *SQLite) CreateNamedAgent(ctx context.Context, name, mailboxID string) (domain.NamedAgent, error) {
	if err := validateAgentName(name); err != nil {
		return domain.NamedAgent{}, err
	}
	resolveMu.Lock()
	defer resolveMu.Unlock()
	if existing, err := s.GetNamedAgent(ctx, name); err == nil {
		if existing.Retired {
			return domain.NamedAgent{}, fmt.Errorf("%w: %s", domain.ErrAgentRetired, name)
		}
		if mailboxID != "" && existing.MailboxID != mailboxID {
			return domain.NamedAgent{}, fmt.Errorf("%w: %s", domain.ErrAgentNameTaken, name)
		}
		return existing, s.recordMutation(ctx, existing)
	} else if !errors.Is(err, domain.ErrAgentNotFound) {
		return domain.NamedAgent{}, err
	}
	createdMailbox := false
	if mailboxID == "" {
		generated, err := uuid.NewV7()
		if err != nil {
			return domain.NamedAgent{}, err
		}
		mailboxID, createdMailbox = generated.String(), true
	} else {
		var kind, installationID string
		err := s.db.QueryRowContext(ctx, `SELECT kind,installation_id FROM mailboxes WHERE id=?`, mailboxID).Scan(&kind, &installationID)
		if errors.Is(err, sql.ErrNoRows) {
			return domain.NamedAgent{}, fmt.Errorf("adopt mailbox: %w", domain.ErrNotFound)
		}
		if err != nil {
			return domain.NamedAgent{}, err
		}
		if kind != string(model.MailboxAgent) || installationID != s.signer.InstallationID {
			return domain.NamedAgent{}, errors.New("only a local agent mailbox can be adopted")
		}
		var existingName string
		if err := s.db.QueryRowContext(ctx, `SELECT name FROM named_agents WHERE mailbox_id=?`, mailboxID).Scan(&existingName); err == nil {
			return domain.NamedAgent{}, fmt.Errorf("%w: %s belongs to %s", domain.ErrMailboxNamed, mailboxID, existingName)
		} else if !errors.Is(err, sql.ErrNoRows) {
			return domain.NamedAgent{}, err
		}
	}
	now := s.clockNow()
	contents := make([]event.Content, 0, 2)
	if createdMailbox {
		payload, _ := event.MarshalPayload(event.MailboxPayload{MailboxID: mailboxID, Kind: string(model.MailboxAgent), Label: name})
		contents = append(contents, event.Content{Type: event.TypeMailboxCreate, Scope: event.ScopeInstallationPrivate, Payload: payload})
	}
	claim, _ := event.MarshalPayload(event.AgentNamePayload{Name: name, MailboxID: mailboxID})
	contents = append(contents, event.Content{Type: event.TypeAgentNameClaim, Scope: event.ScopeInstallationPrivate, Payload: claim})
	times := make([]time.Time, len(contents))
	for index := range times {
		times[index] = now
	}
	value, err := s.appendContentsResult(ctx, contents, times, func(tx *sql.Tx) (any, error) {
		return getNamedAgentWith(ctx, tx, name, now)
	})
	if err != nil {
		if strings.Contains(err.Error(), "conflicting") || strings.Contains(err.Error(), "UNIQUE") {
			return domain.NamedAgent{}, fmt.Errorf("%w: %s", domain.ErrAgentNameTaken, name)
		}
		return domain.NamedAgent{}, err
	}
	return value.(domain.NamedAgent), nil
}

func (s *SQLite) GetNamedAgent(ctx context.Context, name string) (domain.NamedAgent, error) {
	return getNamedAgentWith(ctx, s.db, name, s.clockNow())
}

func (s *SQLite) ListNamedAgents(ctx context.Context) ([]domain.NamedAgent, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT name FROM named_agents ORDER BY name`)
	if err != nil {
		return nil, err
	}
	var names []string
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			rows.Close()
			return nil, err
		}
		names = append(names, name)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return nil, err
	}
	rows.Close()
	var agents []domain.NamedAgent
	for _, name := range names {
		agent, err := getNamedAgentWith(ctx, s.db, name, s.clockNow())
		if err != nil {
			return nil, err
		}
		agents = append(agents, agent)
	}
	return agents, nil
}

func getNamedAgentWith(ctx context.Context, queryer rowQueryer, name string, now time.Time) (domain.NamedAgent, error) {
	var agent domain.NamedAgent
	var retired int
	var lease, active sql.NullInt64
	err := queryer.QueryRowContext(ctx, `SELECT a.name,a.mailbox_id,a.retired,a.current_harness,a.current_session_id,a.last_active_at,o.lease_expires_at,
COALESCE((SELECT c.directory FROM mailbox_contexts c WHERE c.mailbox_id=a.mailbox_id ORDER BY c.first_seen_at DESC LIMIT 1),''),
COALESCE((SELECT c.git_common_dir FROM mailbox_contexts c WHERE c.mailbox_id=a.mailbox_id ORDER BY c.first_seen_at DESC LIMIT 1),''),
COALESCE((SELECT c.remote_identity FROM mailbox_contexts c WHERE c.mailbox_id=a.mailbox_id ORDER BY c.first_seen_at DESC LIMIT 1),''),
COALESCE((SELECT c.worktree FROM mailbox_contexts c WHERE c.mailbox_id=a.mailbox_id ORDER BY c.first_seen_at DESC LIMIT 1),''),
COALESCE((SELECT c.branch FROM mailbox_contexts c WHERE c.mailbox_id=a.mailbox_id ORDER BY c.first_seen_at DESC LIMIT 1),'')
FROM named_agents a LEFT JOIN agent_ownership o ON o.name=a.name WHERE a.name=?`, name).Scan(
		&agent.Name, &agent.MailboxID, &retired, &agent.Harness, &agent.CurrentSessionID, &active, &lease,
		&agent.Context.Directory, &agent.Context.GitCommonDir, &agent.Context.RemoteIdentity, &agent.Context.Worktree, &agent.Context.Branch)
	if errors.Is(err, sql.ErrNoRows) {
		return agent, domain.ErrAgentNotFound
	}
	if err != nil {
		return agent, err
	}
	agent.Retired = retired != 0
	if active.Valid {
		seen := time.UnixMilli(active.Int64).UTC()
		agent.LastActiveAt = &seen
	}
	if lease.Valid {
		expires := time.UnixMilli(lease.Int64).UTC()
		agent.LeaseExpiresAt = &expires
		agent.Active = !agent.Retired && expires.After(now)
	}
	return agent, nil
}

func (s *SQLite) RetireNamedAgent(ctx context.Context, name string) error {
	agent, err := s.GetNamedAgent(ctx, name)
	if err != nil {
		return err
	}
	if agent.Retired {
		return s.recordMutation(ctx, nil)
	}
	payload, _ := event.MarshalPayload(event.AgentNamePayload{Name: name, MailboxID: agent.MailboxID})
	return s.appendContents(ctx, []event.Content{{Type: event.TypeAgentRetire, Scope: event.ScopeInstallationPrivate, Payload: payload}}, []time.Time{s.clockNow()}, nil)
}

func (s *SQLite) SelectNamedAgentSession(ctx context.Context, name string, session model.SessionIdentity, repository model.RepositoryContext) (domain.NamedAgent, error) {
	if strings.TrimSpace(session.Harness) == "" || strings.TrimSpace(session.ExternalSessionID) == "" {
		return domain.NamedAgent{}, errors.New("harness and external session ID are required")
	}
	resolveMu.Lock()
	defer resolveMu.Unlock()
	agent, err := s.GetNamedAgent(ctx, name)
	if err != nil {
		return domain.NamedAgent{}, err
	}
	if agent.Retired {
		return domain.NamedAgent{}, fmt.Errorf("%w: %s", domain.ErrAgentRetired, name)
	}
	var boundMailbox string
	bindErr := s.db.QueryRowContext(ctx, `SELECT mailbox_id FROM harness_bindings WHERE harness=? AND external_session_id=?`, session.Harness, session.ExternalSessionID).Scan(&boundMailbox)
	if bindErr == nil && boundMailbox != agent.MailboxID {
		return domain.NamedAgent{}, errors.New("harness session is already bound to another mailbox")
	}
	if bindErr != nil && !errors.Is(bindErr, sql.ErrNoRows) {
		return domain.NamedAgent{}, bindErr
	}
	var contents []event.Content
	if errors.Is(bindErr, sql.ErrNoRows) {
		binding, _ := event.MarshalPayload(event.MailboxBindingPayload{MailboxID: agent.MailboxID, Harness: session.Harness, ExternalSessionID: session.ExternalSessionID})
		contents = append(contents, event.Content{Type: event.TypeMailboxBind, Scope: event.ScopeInstallationPrivate, Payload: binding})
	}
	selection, _ := event.MarshalPayload(event.AgentSessionPayload{Name: name, MailboxID: agent.MailboxID, Harness: session.Harness, ExternalSessionID: session.ExternalSessionID})
	selectionContent := event.Content{Type: event.TypeAgentSessionSelect, Scope: event.ScopeInstallationPrivate, Payload: selection}
	if parent, err := s.currentAgentSelectionEvent(ctx, name, agent.Harness, agent.CurrentSessionID); err != nil {
		return domain.NamedAgent{}, err
	} else if parent != "" {
		selectionContent.Parents = []string{parent}
	}
	contents = append(contents, selectionContent)
	if repository.Directory != "" && !s.hasContext(ctx, agent.MailboxID, repository) {
		contextPayload, _ := event.MarshalPayload(event.MailboxContextPayload{MailboxID: agent.MailboxID, Context: eventContext(repository)})
		contents = append(contents, event.Content{Type: event.TypeMailboxContext, Scope: event.ScopeInstallationPrivate, Payload: contextPayload})
	}
	now := s.clockNow()
	times := make([]time.Time, len(contents))
	for index := range times {
		times[index] = now
	}
	value, err := s.appendContentsResult(ctx, contents, times, func(tx *sql.Tx) (any, error) {
		return getNamedAgentWith(ctx, tx, name, now)
	})
	if err != nil {
		return domain.NamedAgent{}, err
	}
	return value.(domain.NamedAgent), nil
}

func (s *SQLite) currentAgentSelectionEvent(ctx context.Context, name, harness, sessionID string) (string, error) {
	if harness == "" || sessionID == "" {
		return "", nil
	}
	rows, err := s.db.QueryContext(ctx, `SELECT raw FROM canonical_events WHERE event_type=?`, event.TypeAgentSessionSelect)
	if err != nil {
		return "", err
	}
	defer rows.Close()
	var selected string
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			return "", err
		}
		inspection := event.Inspect(raw)
		if inspection.Status != event.StatusProjected {
			continue
		}
		var payload event.AgentSessionPayload
		if err := json.Unmarshal(inspection.Event.Content.Payload, &payload); err == nil && payload.Name == name && payload.Harness == harness && payload.ExternalSessionID == sessionID {
			selected = inspection.Event.ID()
		}
	}
	return selected, rows.Err()
}

func (s *SQLite) AcquireNamedAgent(ctx context.Context, name, ownerToken string, duration time.Duration) (domain.NamedAgent, error) {
	return s.changeAgentOwnership(ctx, name, ownerToken, duration, "acquire")
}

func (s *SQLite) RenewNamedAgent(ctx context.Context, name, ownerToken string, duration time.Duration) (domain.NamedAgent, error) {
	return s.changeAgentOwnership(ctx, name, ownerToken, duration, "renew")
}

func (s *SQLite) changeAgentOwnership(ctx context.Context, name, ownerToken string, duration time.Duration, operation string) (domain.NamedAgent, error) {
	if ownerToken == "" || duration <= 0 {
		return domain.NamedAgent{}, errors.New("owner token and positive lease duration are required")
	}
	now := s.clockNow()
	agentOwnershipMu.Lock()
	defer agentOwnershipMu.Unlock()
	var existingExpiry int64
	leaseLookup := s.db.QueryRowContext(ctx, `SELECT lease_expires_at FROM agent_ownership WHERE name=?`, name).Scan(&existingExpiry)
	wasLive := leaseLookup == nil && existingExpiry > now.UnixMilli()
	if leaseLookup != nil && !errors.Is(leaseLookup, sql.ErrNoRows) {
		return domain.NamedAgent{}, leaseLookup
	}
	var topics []domain.ChangeTopic
	if !wasLive {
		topics = []domain.ChangeTopic{domain.TopicAgents}
	}
	value, err := s.commitMutation(ctx, topics, func(tx *sql.Tx) (any, error) {
		agent, err := getNamedAgentWith(ctx, tx, name, now)
		if err != nil {
			return nil, err
		}
		if agent.Retired {
			return nil, fmt.Errorf("%w: %s", domain.ErrAgentRetired, name)
		}
		var token string
		var expiry int64
		leaseErr := tx.QueryRowContext(ctx, `SELECT owner_token,lease_expires_at FROM agent_ownership WHERE name=?`, name).Scan(&token, &expiry)
		live := leaseErr == nil && expiry > now.UnixMilli()
		if leaseErr != nil && !errors.Is(leaseErr, sql.ErrNoRows) {
			return nil, leaseErr
		}
		if live && token != ownerToken {
			return nil, &domain.AgentOwnershipConflict{Name: name, ExpiresAt: time.UnixMilli(expiry).UTC()}
		}
		if operation == "renew" && (!live || token != ownerToken) {
			return nil, fmt.Errorf("%w: no live lease for %s", domain.ErrAgentOwned, name)
		}
		newExpiry := now.Add(duration).UnixMilli()
		if _, err := tx.ExecContext(ctx, `INSERT INTO agent_ownership(name,owner_token,lease_expires_at) VALUES (?,?,?) ON CONFLICT(name) DO UPDATE SET owner_token=excluded.owner_token,lease_expires_at=excluded.lease_expires_at`, name, ownerToken, newExpiry); err != nil {
			return nil, err
		}
		if _, err := tx.ExecContext(ctx, `UPDATE named_agents SET last_active_at=? WHERE name=?`, now.UnixMilli(), name); err != nil {
			return nil, err
		}
		return getNamedAgentWith(ctx, tx, name, now)
	})
	if err != nil {
		return domain.NamedAgent{}, err
	}
	return value.(domain.NamedAgent), nil
}

func (s *SQLite) ReleaseNamedAgent(ctx context.Context, name, ownerToken string) error {
	now := s.clockNow()
	if _, err := s.GetNamedAgent(ctx, name); err != nil {
		return err
	}
	agentOwnershipMu.Lock()
	defer agentOwnershipMu.Unlock()
	var existingExpiry int64
	lookupErr := s.db.QueryRowContext(ctx, `SELECT lease_expires_at FROM agent_ownership WHERE name=?`, name).Scan(&existingExpiry)
	wasLive := lookupErr == nil && existingExpiry > now.UnixMilli()
	if lookupErr != nil && !errors.Is(lookupErr, sql.ErrNoRows) {
		return lookupErr
	}
	var topics []domain.ChangeTopic
	if wasLive {
		topics = []domain.ChangeTopic{domain.TopicAgents}
	}
	_, err := s.commitMutation(ctx, topics, func(tx *sql.Tx) (any, error) {
		var token string
		var expiry int64
		if err := tx.QueryRowContext(ctx, `SELECT owner_token,lease_expires_at FROM agent_ownership WHERE name=?`, name).Scan(&token, &expiry); errors.Is(err, sql.ErrNoRows) {
			return nil, nil
		} else if err != nil {
			return nil, err
		}
		if token != ownerToken && expiry > now.UnixMilli() {
			return nil, &domain.AgentOwnershipConflict{Name: name, ExpiresAt: time.UnixMilli(expiry).UTC()}
		}
		if _, err := tx.ExecContext(ctx, `DELETE FROM agent_ownership WHERE name=?`, name); err != nil {
			return nil, err
		}
		return nil, nil
	})
	return err
}

func (s *SQLite) clockNow() time.Time {
	if s.now != nil {
		return s.now().UTC()
	}
	return time.Now().UTC()
}
