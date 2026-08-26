package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/wbbradley/hq/internal/event"
)

type canonicalResource struct {
	kind string
	id   string
}

func (r canonicalResource) dependencyOnly() bool { return strings.HasSuffix(r.kind, "-support") }

func resourcesForEvent(item event.SignedEvent) []canonicalResource {
	content := item.Content
	resources := make(map[canonicalResource]struct{})
	add := func(kind, id string) {
		if id != "" {
			resources[canonicalResource{kind: kind, id: id}] = struct{}{}
		}
	}
	add("event", item.ID())
	add("installation-support", content.InstallationID)
	if content.Audience != nil {
		add("account-support", content.Audience.HumanAccountID)
	}
	if content.ThreadID != "" {
		add("thread", content.ThreadID)
	}
	switch content.Type {
	case event.TypeInstallationCreate:
		add("installation", content.InstallationID)
	case event.TypeMailboxCreate:
		var payload event.MailboxPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("mailbox", content.InstallationID+":"+payload.MailboxID)
		}
	case event.TypeMailboxBind:
		var payload event.MailboxBindingPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("mailbox", content.InstallationID+":"+payload.MailboxID)
			add("session", payload.Harness+":"+payload.ExternalSessionID)
		}
	case event.TypeMailboxContext:
		var payload event.MailboxContextPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("mailbox", content.InstallationID+":"+payload.MailboxID)
		}
	case event.TypeAgentNameClaim, event.TypeAgentRetire:
		var payload event.AgentNamePayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("agent", payload.Name)
			add("mailbox", content.InstallationID+":"+payload.MailboxID)
		}
	case event.TypeAgentSessionSelect:
		var payload event.AgentSessionPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("agent", payload.Name)
			add("mailbox", content.InstallationID+":"+payload.MailboxID)
			add("session", payload.Harness+":"+payload.ExternalSessionID)
		}
	case event.TypeAgentSessionRename:
		var payload event.AgentSessionRenamePayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("agent", payload.Name)
			add("mailbox", content.InstallationID+":"+payload.MailboxID)
			add("session", payload.Harness+":"+payload.ExternalSessionID)
		}
	case event.TypeQuestion, event.TypeAnswer, event.TypeMessage:
		add("message", item.ID())
		var payload event.TextPayload
		if json.Unmarshal(content.Payload, &payload) == nil && (payload.Correlation.Provider != "" || payload.Correlation.SessionID != "") {
			add("activity-session", payload.Correlation.Provider+":"+payload.Correlation.SessionID)
		}
		root := content.ThreadID
		if root == "" {
			root = item.ID()
		}
		add("thread", root)
	case event.TypeThreadCancel:
		add("thread", content.ThreadID)
	case event.TypeMessageArchive, event.TypeMessageRestore, event.TypeMessageReject:
		var payload event.TargetPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("message", payload.TargetEventID)
		}
	case event.TypePeerBindingSet, event.TypePeerBindingBlock:
		var payload event.PeerPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("peer", payload.InstallationID)
			add("installation", payload.InstallationID)
		}
	case event.TypeMailboxAccessGrant, event.TypeMailboxAccessRevoke:
		var payload event.MailboxAccessPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("mailbox-access-route", content.InstallationID+":"+payload.MailboxID+":"+payload.GranteeInstallationID)
			add("installation", payload.GranteeInstallationID)
		}
	case event.TypeMailboxAccessObserve:
		var payload event.MailboxAccessObservationPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("mailbox-access", payload.GrantEventID)
			add("message", payload.MessageEventID)
		}
	case event.TypeHumanAccountCreate:
		var payload event.HumanAccountPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("account", payload.AccountID)
		}
	case event.TypeHumanAccountSelect:
		var payload event.HumanAccountSelectionPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("account", payload.AccountID)
			add("account-selection", content.InstallationID)
		}
	case event.TypeHumanDeviceGrant, event.TypeHumanDeviceAccept, event.TypeHumanDeviceRevoke:
		var payload event.HumanDevicePayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("account", payload.AccountID)
			add("device", payload.AccountID+":"+payload.InstallationID)
			add("installation", payload.InstallationID)
		}
	case event.TypeProjectEvent:
		var payload event.ProjectEventPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("project", payload.ProjectID)
		}
	case event.TypeProjectCommand:
		var payload event.ProjectCommandPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("project", payload.ProjectID)
			add("project-command", payload.CommandID)
		}
	case event.TypeProjectResult:
		var payload event.ProjectCommandResultPayload
		if json.Unmarshal(content.Payload, &payload) == nil {
			add("project", payload.ProjectID)
			add("project-command", payload.CommandID)
		}
	case event.TypeHarnessActivity:
		var payload event.HarnessActivityPayload
		if json.Unmarshal(content.Payload, &payload) == nil && (payload.Correlation.Provider != "" || payload.Correlation.SessionID != "") {
			add("activity-session", payload.Correlation.Provider+":"+payload.Correlation.SessionID)
		}
	}

	result := make([]canonicalResource, 0, len(resources))
	for resource := range resources {
		result = append(result, resource)
	}
	sort.Slice(result, func(i, j int) bool {
		if result[i].kind == result[j].kind {
			return result[i].id < result[j].id
		}
		return result[i].kind < result[j].kind
	})
	return result
}

func (s *SQLite) reduceCanonicalResources(ctx context.Context, requested ...canonicalResource) (event.State, error) {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return event.State{}, err
	}
	defer tx.Rollback()
	var seeds []string
	for _, resource := range requested {
		rows, err := tx.QueryContext(ctx, `SELECT event_id FROM event_resources WHERE resource_kind=? AND resource_id=?`, resource.kind, resource.id)
		if err != nil {
			return event.State{}, err
		}
		for rows.Next() {
			var id string
			if err := rows.Scan(&id); err != nil {
				rows.Close()
				return event.State{}, err
			}
			seeds = append(seeds, id)
		}
		if err := rows.Close(); err != nil {
			return event.State{}, err
		}
	}
	if len(seeds) == 0 {
		return event.ReduceAffected(nil, s.policy()), nil
	}
	return affectedReductionTx(ctx, tx, uniqueSorted(seeds), s.policy())
}

func layeredReductionStatus(record event.Record) (validation, readiness, authorization string) {
	validation, readiness, authorization = "valid", "ready", "unknown"
	switch record.Status {
	case event.StatusProjected:
		authorization = "authorized"
	case event.StatusUnauthorized:
		authorization = "denied"
	case event.StatusUnresolved:
		readiness = "missing"
	case event.StatusUnsupported:
		readiness = "unsupported"
	case event.StatusInvalid:
		validation, readiness = "invalid", "invalid"
	}
	return validation, readiness, authorization
}

// rebuildCausalIndexesTx is the offline oracle for all rebuildable dependency
// metadata. Ordinary ingestion maintains the same tables incrementally.
func rebuildCausalIndexesTx(ctx context.Context, tx *sql.Tx, state event.State, generation int64) error {
	for _, table := range []string{"projection_support", "aggregate_frontiers", "event_reduction", "unresolved_waiters", "event_resources", "authority_dependencies", "causal_edges"} {
		if _, err := tx.ExecContext(ctx, `DELETE FROM `+table); err != nil {
			return fmt.Errorf("clear %s index: %w", table, err)
		}
	}
	ids := make([]string, 0, len(state.Records))
	known := make(map[string]bool, len(state.Records))
	children := make(map[string][]string)
	resourceEvents := make(map[canonicalResource][]string)
	for id := range state.Records {
		ids = append(ids, id)
		known[id] = true
	}
	sort.Strings(ids)
	for _, id := range ids {
		record := state.Records[id]
		validation, readiness, authorization := layeredReductionStatus(record)
		if _, err := tx.ExecContext(ctx, `INSERT INTO event_reduction(event_id,validation_status,readiness_status,authorization_status,projection_status,reason,generation) VALUES (?,?,?,?,?,?,?)`, id, validation, readiness, authorization, record.Status, record.Reason, generation); err != nil {
			return err
		}
		for _, parent := range record.Event.Content.Parents {
			if _, err := tx.ExecContext(ctx, `INSERT INTO causal_edges(child_event_id,parent_event_id) VALUES (?,?)`, id, parent); err != nil {
				return err
			}
			children[parent] = append(children[parent], id)
			if !known[parent] {
				if _, err := tx.ExecContext(ctx, `INSERT INTO unresolved_waiters(missing_event_id,waiting_event_id) VALUES (?,?)`, parent, id); err != nil {
					return err
				}
			}
		}
		for _, authority := range record.Event.Content.Authorities {
			if _, err := tx.ExecContext(ctx, `INSERT INTO authority_dependencies(event_id,authority_event_id) VALUES (?,?)`, id, authority); err != nil {
				return err
			}
		}
		for _, resource := range resourcesForEvent(record.Event) {
			if _, err := tx.ExecContext(ctx, `INSERT INTO event_resources(event_id,resource_kind,resource_id) VALUES (?,?,?)`, id, resource.kind, resource.id); err != nil {
				return err
			}
			if !resource.dependencyOnly() {
				resourceEvents[resource] = append(resourceEvents[resource], id)
			}
			if resource.kind != "event" && !resource.dependencyOnly() {
				if _, err := tx.ExecContext(ctx, `INSERT INTO projection_support(projection_kind,projection_id,event_id,generation) VALUES (?,?,?,?)`, resource.kind, resource.id, id, generation); err != nil {
					return err
				}
			}
		}
	}
	for resource, eventIDs := range resourceEvents {
		members := make(map[string]bool, len(eventIDs))
		for _, id := range eventIDs {
			members[id] = true
		}
		for _, id := range eventIDs {
			if !hasResourceDescendant(id, members, children) {
				if _, err := tx.ExecContext(ctx, `INSERT INTO aggregate_frontiers(resource_kind,resource_id,event_id,generation) VALUES (?,?,?,?)`, resource.kind, resource.id, id, generation); err != nil {
					return err
				}
			}
		}
	}
	return nil
}

func hasResourceDescendant(id string, members map[string]bool, children map[string][]string) bool {
	seen := map[string]bool{id: true}
	queue := append([]string(nil), children[id]...)
	for len(queue) != 0 {
		current := queue[0]
		queue = queue[1:]
		if seen[current] {
			continue
		}
		seen[current] = true
		if members[current] {
			return true
		}
		queue = append(queue, children[current]...)
	}
	return false
}

func indexCanonicalEventTx(ctx context.Context, tx *sql.Tx, item event.SignedEvent) error {
	id := item.ID()
	for _, parent := range item.Content.Parents {
		if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO causal_edges(child_event_id,parent_event_id) VALUES (?,?)`, id, parent); err != nil {
			return err
		}
		var exists int
		if err := tx.QueryRowContext(ctx, `SELECT EXISTS(SELECT 1 FROM canonical_events WHERE event_id=?)`, parent).Scan(&exists); err != nil {
			return err
		}
		if exists == 0 {
			if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO unresolved_waiters(missing_event_id,waiting_event_id) VALUES (?,?)`, parent, id); err != nil {
				return err
			}
		}
	}
	for _, authority := range item.Content.Authorities {
		if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO authority_dependencies(event_id,authority_event_id) VALUES (?,?)`, id, authority); err != nil {
			return err
		}
	}
	for _, resource := range resourcesForEvent(item) {
		if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO event_resources(event_id,resource_kind,resource_id) VALUES (?,?,?)`, id, resource.kind, resource.id); err != nil {
			return err
		}
	}
	return nil
}

// affectedReductionTx computes the least fixed point containing the seeds,
// their causal ancestors and descendants, authority dependants, unresolved
// waiters, and every fact in the same logical resources.
func affectedReductionTx(ctx context.Context, tx *sql.Tx, seeds []string, policy event.Policy) (event.State, error) {
	if _, err := tx.ExecContext(ctx, `CREATE TEMP TABLE IF NOT EXISTS impacted_canonical_events(event_id TEXT PRIMARY KEY) WITHOUT ROWID; CREATE TEMP TABLE IF NOT EXISTS affected_canonical_events(event_id TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM impacted_canonical_events; DELETE FROM affected_canonical_events`); err != nil {
		return event.State{}, err
	}
	for _, id := range seeds {
		if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO impacted_canonical_events(event_id) VALUES (?)`, id); err != nil {
			return event.State{}, err
		}
		if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO impacted_canonical_events(event_id) SELECT waiting_event_id FROM unresolved_waiters WHERE missing_event_id=?`, id); err != nil {
			return event.State{}, err
		}
	}
	impactStatements := []string{
		`INSERT OR IGNORE INTO impacted_canonical_events SELECT e.child_event_id FROM causal_edges e JOIN impacted_canonical_events a ON a.event_id=e.parent_event_id`,
		`INSERT OR IGNORE INTO impacted_canonical_events SELECT d.event_id FROM authority_dependencies d JOIN impacted_canonical_events a ON a.event_id=d.authority_event_id`,
		`INSERT OR IGNORE INTO impacted_canonical_events SELECT related.event_id FROM event_resources seed JOIN impacted_canonical_events a ON a.event_id=seed.event_id JOIN event_resources related ON related.resource_kind=seed.resource_kind AND related.resource_id=seed.resource_id WHERE seed.resource_kind NOT LIKE '%-support'`,
	}
	for {
		changed := int64(0)
		for _, statement := range impactStatements {
			result, err := tx.ExecContext(ctx, statement)
			if err != nil {
				return event.State{}, err
			}
			rows, err := result.RowsAffected()
			if err != nil {
				return event.State{}, err
			}
			changed += rows
		}
		if changed == 0 {
			break
		}
	}
	if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO affected_canonical_events SELECT event_id FROM impacted_canonical_events`); err != nil {
		return event.State{}, err
	}
	supportStatements := []string{
		`INSERT OR IGNORE INTO affected_canonical_events SELECT provider.event_id FROM event_resources need JOIN affected_canonical_events a ON a.event_id=need.event_id JOIN event_resources provider ON provider.resource_kind='account' AND provider.resource_id=need.resource_id WHERE need.resource_kind='account-support'`,
		`INSERT OR IGNORE INTO affected_canonical_events SELECT provider.event_id FROM event_resources need JOIN affected_canonical_events a ON a.event_id=need.event_id JOIN event_resources provider ON provider.resource_kind='installation' AND provider.resource_id=need.resource_id WHERE need.resource_kind='installation-support'`,
		`INSERT OR IGNORE INTO affected_canonical_events SELECT e.parent_event_id FROM causal_edges e JOIN affected_canonical_events a ON a.event_id=e.child_event_id JOIN canonical_events c ON c.event_id=e.parent_event_id`,
	}
	for {
		changed := int64(0)
		for _, statement := range supportStatements {
			result, err := tx.ExecContext(ctx, statement)
			if err != nil {
				return event.State{}, err
			}
			rows, err := result.RowsAffected()
			if err != nil {
				return event.State{}, err
			}
			changed += rows
		}
		if changed == 0 {
			break
		}
	}
	rows, err := tx.QueryContext(ctx, `SELECT c.raw FROM canonical_events c JOIN affected_canonical_events a ON a.event_id=c.event_id ORDER BY c.event_id`)
	if err != nil {
		return event.State{}, err
	}
	defer rows.Close()
	var raw [][]byte
	for rows.Next() {
		var item []byte
		if err := rows.Scan(&item); err != nil {
			return event.State{}, err
		}
		raw = append(raw, item)
	}
	if err := rows.Err(); err != nil {
		return event.State{}, err
	}
	return event.ReduceAffected(raw, policy), nil
}

func patchCausalIndexesTx(ctx context.Context, tx *sql.Tx, state event.State, generation int64) error {
	children := make(map[string][]string)
	resources := make(map[canonicalResource][]string)
	for id, record := range state.Records {
		validation, readiness, authorization := layeredReductionStatus(record)
		if _, err := tx.ExecContext(ctx, `INSERT INTO event_reduction(event_id,validation_status,readiness_status,authorization_status,projection_status,reason,generation) VALUES (?,?,?,?,?,?,?) ON CONFLICT(event_id) DO UPDATE SET validation_status=excluded.validation_status,readiness_status=excluded.readiness_status,authorization_status=excluded.authorization_status,projection_status=excluded.projection_status,reason=excluded.reason,generation=excluded.generation`, id, validation, readiness, authorization, record.Status, record.Reason, generation); err != nil {
			return err
		}
		if _, err := tx.ExecContext(ctx, `DELETE FROM unresolved_waiters WHERE waiting_event_id=?`, id); err != nil {
			return err
		}
		for _, parent := range record.Event.Content.Parents {
			children[parent] = append(children[parent], id)
			var exists int
			if err := tx.QueryRowContext(ctx, `SELECT EXISTS(SELECT 1 FROM canonical_events WHERE event_id=?)`, parent).Scan(&exists); err != nil {
				return err
			}
			if exists == 0 {
				if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO unresolved_waiters(missing_event_id,waiting_event_id) VALUES (?,?)`, parent, id); err != nil {
					return err
				}
			}
		}
		for _, resource := range resourcesForEvent(record.Event) {
			if !resource.dependencyOnly() {
				resources[resource] = append(resources[resource], id)
			}
		}
	}
	for resource, ids := range resources {
		if _, err := tx.ExecContext(ctx, `DELETE FROM aggregate_frontiers WHERE resource_kind=? AND resource_id=?`, resource.kind, resource.id); err != nil {
			return err
		}
		if _, err := tx.ExecContext(ctx, `DELETE FROM projection_support WHERE projection_kind=? AND projection_id=?`, resource.kind, resource.id); err != nil {
			return err
		}
		members := make(map[string]bool, len(ids))
		for _, id := range ids {
			members[id] = true
			if resource.kind != "event" {
				if _, err := tx.ExecContext(ctx, `INSERT INTO projection_support(projection_kind,projection_id,event_id,generation) VALUES (?,?,?,?)`, resource.kind, resource.id, id, generation); err != nil {
					return err
				}
			}
		}
		for _, id := range ids {
			if !hasResourceDescendant(id, members, children) {
				if _, err := tx.ExecContext(ctx, `INSERT INTO aggregate_frontiers(resource_kind,resource_id,event_id,generation) VALUES (?,?,?,?)`, resource.kind, resource.id, id, generation); err != nil {
					return err
				}
			}
		}
	}
	return nil
}
