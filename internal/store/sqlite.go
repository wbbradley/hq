package store

import (
	"context"
	"database/sql"
	"encoding/base64"
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
	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
	_ "modernc.org/sqlite"
)

const (
	schemaVersion  = 33
	reducerVersion = 1
)

const tuiDraftSchema = `CREATE TABLE IF NOT EXISTS tui_drafts (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK(version > 0),
    body TEXT NOT NULL,
    reply_to_message_id TEXT NOT NULL DEFAULT '',
    conversation_json TEXT NOT NULL DEFAULT '{}',
    recipient_mailbox_id TEXT NOT NULL DEFAULT '',
    recipient_label TEXT NOT NULL DEFAULT '',
    recipient_address_json TEXT NOT NULL DEFAULT '{}',
    recipient_named INTEGER NOT NULL CHECK(recipient_named IN (0,1)),
    repository_json TEXT NOT NULL DEFAULT '{}',
    activation_json TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;`

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
CREATE INDEX causal_edges_by_parent ON causal_edges(parent_event_id,child_event_id);
CREATE TABLE authority_dependencies (
    event_id TEXT NOT NULL,
    authority_event_id TEXT NOT NULL,
    PRIMARY KEY(event_id, authority_event_id),
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX authority_dependencies_by_authority ON authority_dependencies(authority_event_id,event_id);
CREATE TABLE unresolved_waiters (
    missing_event_id TEXT NOT NULL,
    waiting_event_id TEXT NOT NULL,
    PRIMARY KEY(missing_event_id, waiting_event_id),
    FOREIGN KEY(waiting_event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX unresolved_waiters_by_waiting ON unresolved_waiters(waiting_event_id,missing_event_id);
CREATE TABLE event_resources (
    event_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    PRIMARY KEY(event_id,resource_kind,resource_id),
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX event_resources_by_resource ON event_resources(resource_kind,resource_id,event_id);
CREATE TABLE aggregate_frontiers (
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY(resource_kind,resource_id,event_id),
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE projection_support (
    projection_kind TEXT NOT NULL,
    projection_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY(projection_kind,projection_id,event_id),
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX projection_support_by_event ON projection_support(event_id,projection_kind,projection_id);
CREATE TABLE event_reduction (
    event_id TEXT PRIMARY KEY,
    validation_status TEXT NOT NULL,
    readiness_status TEXT NOT NULL,
    authorization_status TEXT NOT NULL,
    projection_status TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    generation INTEGER NOT NULL,
    FOREIGN KEY(event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX event_reduction_by_projection ON event_reduction(projection_status,event_id);
CREATE TABLE projection_metadata (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    reducer_version INTEGER NOT NULL,
    event_count INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    repaired_at INTEGER NOT NULL
) STRICT;
CREATE TABLE mailboxes (
    id TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('human', 'agent', 'project')),
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
    mailbox_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(harness, external_session_id),
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id) ON DELETE CASCADE
) STRICT;
CREATE TABLE named_agents (
    name TEXT PRIMARY KEY,
    mailbox_id TEXT NOT NULL UNIQUE,
    retired INTEGER NOT NULL CHECK(retired IN (0,1)),
    current_harness TEXT NOT NULL DEFAULT '',
    current_session_id TEXT NOT NULL DEFAULT '',
    last_active_at INTEGER,
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id) ON DELETE CASCADE
) STRICT;
CREATE TABLE agent_ownership (
    name TEXT PRIMARY KEY,
    owner_token TEXT NOT NULL,
    lease_expires_at INTEGER NOT NULL,
    FOREIGN KEY(name) REFERENCES named_agents(name) ON DELETE CASCADE
) STRICT;
CREATE TABLE agent_sessions (
    agent_name TEXT NOT NULL,
    mailbox_id TEXT NOT NULL,
    harness TEXT NOT NULL,
    external_session_id TEXT NOT NULL,
    thread_name TEXT NOT NULL DEFAULT '',
    directory TEXT NOT NULL,
    git_common_dir TEXT NOT NULL DEFAULT '',
    remote_identity TEXT NOT NULL DEFAULT '',
    worktree TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    last_selected_at INTEGER NOT NULL,
    PRIMARY KEY(harness, external_session_id),
    FOREIGN KEY(agent_name) REFERENCES named_agents(name) ON DELETE CASCADE,
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
	purpose TEXT NOT NULL DEFAULT 'conversation',
	presentation TEXT NOT NULL DEFAULT '',
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
	harness_provider TEXT NOT NULL DEFAULT '',
	harness_session_id TEXT NOT NULL DEFAULT '',
	harness_operation_id TEXT NOT NULL DEFAULT '',
	correlation_item_id TEXT NOT NULL DEFAULT '',
	correlation_request_id TEXT NOT NULL DEFAULT '',
	technical_sections_json TEXT NOT NULL DEFAULT '[]',
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
CREATE TABLE mailbox_access (
    grant_event_id TEXT PRIMARY KEY,
    mailbox_id TEXT NOT NULL,
    grantor_installation_id TEXT NOT NULL,
    grantee_installation_id TEXT NOT NULL,
    grantee_signer_key_id TEXT NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    FOREIGN KEY(grant_event_id) REFERENCES canonical_events(event_id)
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
` + tuiDraftSchema + `
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
CREATE TABLE mutation_receipts (
    mutation_id TEXT PRIMARY KEY,
    method TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    result BLOB NOT NULL,
    committed_at INTEGER NOT NULL
) STRICT;
CREATE TABLE change_revision (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    revision INTEGER NOT NULL
) STRICT;
INSERT INTO change_revision(id,revision) VALUES (1,0);
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    home_installation_id TEXT NOT NULL,
    mailbox_id TEXT NOT NULL UNIQUE,
    predecessor_project_id TEXT,
    name TEXT NOT NULL,
    brief TEXT NOT NULL DEFAULT '',
    lifecycle TEXT NOT NULL CHECK(lifecycle IN ('preparing','open','closing','closed')),
    archived INTEGER NOT NULL CHECK(archived IN (0,1)),
    primary_resource_id TEXT,
    head_event_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK(archived = 0 OR lifecycle = 'closed'),
    FOREIGN KEY(mailbox_id) REFERENCES mailboxes(id)
) STRICT;
CREATE TABLE project_events (
    event_id TEXT PRIMARY KEY CHECK(length(event_id) = 64),
    project_id TEXT NOT NULL,
    previous_event_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(project_id) REFERENCES projects(id)
) STRICT;
CREATE TABLE resources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    home_installation_id TEXT NOT NULL,
    display_locator TEXT NOT NULL,
    canonical_locator TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(id, kind, home_installation_id, canonical_locator)
) STRICT;
CREATE TABLE project_resources (
    project_id TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    added_event_id TEXT NOT NULL,
    removed_event_id TEXT,
    PRIMARY KEY(project_id, resource_id, added_event_id),
    FOREIGN KEY(project_id) REFERENCES projects(id),
    FOREIGN KEY(resource_id) REFERENCES resources(id),
    FOREIGN KEY(added_event_id) REFERENCES project_events(event_id),
    FOREIGN KEY(removed_event_id) REFERENCES project_events(event_id)
) STRICT;
CREATE UNIQUE INDEX project_resources_current ON project_resources(project_id, resource_id) WHERE removed_event_id IS NULL;
CREATE TABLE resource_claim_epochs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    acquired_event_id TEXT NOT NULL,
    released_event_id TEXT,
    acquired_at INTEGER NOT NULL,
    released_at INTEGER,
    FOREIGN KEY(project_id) REFERENCES projects(id),
    FOREIGN KEY(resource_id) REFERENCES resources(id),
    FOREIGN KEY(acquired_event_id) REFERENCES project_events(event_id),
    FOREIGN KEY(released_event_id) REFERENCES project_events(event_id)
) STRICT;
CREATE UNIQUE INDEX resource_claim_active ON resource_claim_epochs(resource_id) WHERE released_event_id IS NULL;
CREATE TABLE resource_health (
    resource_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK(state IN ('healthy','missing','inaccessible','malformed','unknown')),
    details_json TEXT NOT NULL DEFAULT '{}',
    last_checked_at INTEGER NOT NULL,
    FOREIGN KEY(resource_id) REFERENCES resources(id)
) STRICT;
CREATE TABLE project_assignment_epochs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('configuring','runnable','blocked','ended')),
    selected_thread_id TEXT,
    started_event_id TEXT NOT NULL,
    ended_event_id TEXT,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    forced INTEGER NOT NULL DEFAULT 0 CHECK(forced IN (0,1)),
    FOREIGN KEY(project_id) REFERENCES projects(id),
    FOREIGN KEY(agent_name) REFERENCES named_agents(name),
    FOREIGN KEY(started_event_id) REFERENCES project_events(event_id),
    FOREIGN KEY(ended_event_id) REFERENCES project_events(event_id)
) STRICT;
CREATE UNIQUE INDEX project_assignment_current_project ON project_assignment_epochs(project_id) WHERE ended_event_id IS NULL;
CREATE UNIQUE INDEX project_assignment_current_agent ON project_assignment_epochs(agent_name) WHERE ended_event_id IS NULL;
CREATE TABLE project_threads (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    harness TEXT NOT NULL,
    external_thread_id TEXT NOT NULL,
    launch_directory TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(harness, external_thread_id),
    FOREIGN KEY(project_id) REFERENCES projects(id),
    FOREIGN KEY(agent_name) REFERENCES named_agents(name)
) STRICT;
CREATE TABLE project_message_acceptances (
    project_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    message_id TEXT NOT NULL UNIQUE,
    message_event_id TEXT NOT NULL UNIQUE,
    acceptance_event_id TEXT NOT NULL UNIQUE,
    accepted_at INTEGER NOT NULL,
    PRIMARY KEY(project_id, sequence),
    FOREIGN KEY(project_id) REFERENCES projects(id),
    FOREIGN KEY(message_id) REFERENCES messages(id),
    FOREIGN KEY(message_event_id) REFERENCES canonical_events(event_id),
    FOREIGN KEY(acceptance_event_id) REFERENCES project_events(event_id)
) STRICT;
CREATE TABLE project_dispatch_records (
    message_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    assignment_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    project_thread_id TEXT NOT NULL,
    external_thread_id TEXT NOT NULL,
    dispatch_event_id TEXT NOT NULL UNIQUE,
    dispatched_at INTEGER NOT NULL,
    UNIQUE(project_id, sequence),
    FOREIGN KEY(project_id,sequence) REFERENCES project_message_acceptances(project_id,sequence),
    FOREIGN KEY(assignment_id) REFERENCES project_assignment_epochs(id),
    FOREIGN KEY(project_thread_id) REFERENCES project_threads(id),
    FOREIGN KEY(dispatch_event_id) REFERENCES project_events(event_id)
) STRICT;
CREATE TABLE project_dispatch_attempts (
    message_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    assignment_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    project_thread_id TEXT NOT NULL,
    external_thread_id TEXT NOT NULL,
    owner_token TEXT,
    lease_until INTEGER,
    state TEXT NOT NULL CHECK(state IN ('pending','uncertain','accepted')),
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK((owner_token IS NULL) = (lease_until IS NULL)),
    FOREIGN KEY(project_id,sequence) REFERENCES project_message_acceptances(project_id,sequence),
    FOREIGN KEY(assignment_id) REFERENCES project_assignment_epochs(id),
    FOREIGN KEY(project_thread_id) REFERENCES project_threads(id)
) STRICT;
CREATE TABLE project_activation_operations (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    prior_lifecycle TEXT NOT NULL CHECK(prior_lifecycle IN ('open','closed')),
    assignment_id TEXT,
    state TEXT NOT NULL CHECK(state IN ('preparing','configuring','runnable','failed')),
    last_error TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(project_id) REFERENCES projects(id),
    FOREIGN KEY(agent_name) REFERENCES named_agents(name),
    FOREIGN KEY(assignment_id) REFERENCES project_assignment_epochs(id)
) STRICT;
CREATE UNIQUE INDEX project_activation_incomplete ON project_activation_operations(project_id) WHERE state IN ('preparing','configuring');
CREATE TABLE project_output_provenance (
    message_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    project_thread_id TEXT NOT NULL,
    external_thread_id TEXT NOT NULL,
    late INTEGER NOT NULL CHECK(late IN (0,1)),
    current_assignment_id TEXT NOT NULL DEFAULT '',
    current_agent_name TEXT NOT NULL DEFAULT '',
    current_project_thread_id TEXT NOT NULL DEFAULT '',
    runtime_owner_token TEXT NOT NULL DEFAULT '',
    observed_runtime_state TEXT NOT NULL DEFAULT '',
    forced_transition INTEGER NOT NULL CHECK(forced_transition IN (0,1)),
    recorded_at INTEGER NOT NULL,
    FOREIGN KEY(message_id) REFERENCES messages(id),
    FOREIGN KEY(project_id) REFERENCES projects(id),
    FOREIGN KEY(assignment_id) REFERENCES project_assignment_epochs(id),
    FOREIGN KEY(project_thread_id) REFERENCES project_threads(id)
) STRICT;
CREATE TABLE project_replicas (
    id TEXT PRIMARY KEY,
    home_installation_id TEXT NOT NULL,
    head_event_id TEXT NOT NULL,
    state_json BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE TABLE project_audit_log (
    id INTEGER PRIMARY KEY,
    operation TEXT NOT NULL,
    request_id TEXT NOT NULL DEFAULT '',
    project_id TEXT NOT NULL DEFAULT '',
    home_installation_id TEXT NOT NULL,
    expected_head_event_id TEXT NOT NULL DEFAULT '',
    current_head_event_id TEXT NOT NULL DEFAULT '',
    outcome TEXT NOT NULL,
    details_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE project_runtime_operations (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('close','handoff')),
    project_id TEXT NOT NULL,
    expected_head_event_id TEXT NOT NULL,
    current_head_event_id TEXT NOT NULL,
    target_agent TEXT NOT NULL DEFAULT '',
    force INTEGER NOT NULL CHECK(force IN (0,1)),
    archive INTEGER NOT NULL CHECK(archive IN (0,1)),
    state TEXT NOT NULL CHECK(state IN ('started','closing','unassigned','activating','completed','blocked','failed')),
    last_error TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(project_id) REFERENCES projects(id)
) STRICT;
CREATE UNIQUE INDEX project_runtime_operation_incomplete ON project_runtime_operations(project_id) WHERE state IN ('started','closing','unassigned','activating');
CREATE TABLE project_worktree_operations (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL UNIQUE,
    request_json TEXT NOT NULL,
    canonical_repository TEXT NOT NULL,
    canonical_destination TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('reserved','worktree-created','completed','failed')),
    last_error TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE INDEX project_worktree_reservations ON project_worktree_operations(canonical_destination) WHERE state IN ('reserved','worktree-created');
CREATE TABLE agent_retirement_operations (
    id TEXT PRIMARY KEY,
    agent_name TEXT NOT NULL,
    project_id TEXT NOT NULL DEFAULT '',
    force INTEGER NOT NULL CHECK(force IN (0,1)),
    state TEXT NOT NULL CHECK(state IN ('started','quiesced','unassigned','completed','blocked','failed')),
    last_error TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(agent_name) REFERENCES named_agents(name)
) STRICT;
CREATE UNIQUE INDEX agent_retirement_incomplete ON agent_retirement_operations(agent_name) WHERE state IN ('started','quiesced','unassigned');
CREATE TRIGGER project_identity_immutable BEFORE UPDATE OF home_installation_id,mailbox_id,predecessor_project_id ON projects BEGIN SELECT RAISE(ABORT,'project identity is immutable'); END;
CREATE TRIGGER resource_identity_immutable BEFORE UPDATE OF kind,home_installation_id,canonical_locator ON resources BEGIN SELECT RAISE(ABORT,'resource identity is immutable'); END;
CREATE TRIGGER project_thread_scope_immutable BEFORE UPDATE OF project_id,agent_name,harness,external_thread_id ON project_threads BEGIN SELECT RAISE(ABORT,'project thread scope is immutable'); END;
CREATE TRIGGER active_claim_requires_open BEFORE INSERT ON resource_claim_epochs WHEN NEW.released_event_id IS NULL AND (SELECT lifecycle FROM projects WHERE id=NEW.project_id) NOT IN ('open','closing') BEGIN SELECT RAISE(ABORT,'active resource claim requires open or closing project'); END;
CREATE TRIGGER active_assignment_requires_open BEFORE INSERT ON project_assignment_epochs WHEN NEW.ended_event_id IS NULL AND (SELECT lifecycle FROM projects WHERE id=NEW.project_id) NOT IN ('open','closing') BEGIN SELECT RAISE(ABORT,'active assignment requires open or closing project'); END;
CREATE TRIGGER closed_project_requires_release BEFORE UPDATE OF lifecycle ON projects WHEN NEW.lifecycle='closed' AND (EXISTS(SELECT 1 FROM resource_claim_epochs WHERE project_id=NEW.id AND released_event_id IS NULL) OR EXISTS(SELECT 1 FROM project_assignment_epochs WHERE project_id=NEW.id AND ended_event_id IS NULL)) BEGIN SELECT RAISE(ABORT,'closed project cannot retain claims or assignment'); END;
CREATE INDEX messages_inbox ON messages(recipient_mailbox_id, archived_at, created_at, id);
CREATE INDEX messages_sent ON messages(sender_mailbox_id, created_at DESC, id DESC);
CREATE INDEX messages_reply ON messages(reply_to, recipient_mailbox_id, created_at, id);
CREATE INDEX messages_harness_conversation ON messages(harness_provider, harness_session_id, created_at, id);
CREATE INDEX messages_harness_operation ON messages(harness_provider, harness_session_id, harness_operation_id, created_at, id);
CREATE INDEX messages_conversation_order ON messages(sender_mailbox_id,recipient_mailbox_id,harness_provider,harness_session_id,created_at,event_id);
CREATE INDEX mailbox_context_search ON mailbox_contexts(directory, git_common_dir, remote_identity, worktree, branch);
CREATE TABLE harness_activities (
	 event_id TEXT PRIMARY KEY CHECK(length(event_id) = 64),
	 source_installation_id TEXT NOT NULL,
    mailbox_id TEXT NOT NULL,
	 audience_account_id TEXT NOT NULL DEFAULT '',
	 harness TEXT NOT NULL,
    session_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('operation-status','plan','diff','command','file-change','tool-call','progress')),
    item_id TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '' CHECK(status IN ('','running','completed','failed','interrupted')),
    title TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    truncated INTEGER NOT NULL CHECK(truncated IN (0,1)),
    occurred_at INTEGER NOT NULL,
	 runtime_id TEXT NOT NULL,
	 source_sequence TEXT NOT NULL,
	 UNIQUE(source_installation_id,mailbox_id,harness,session_id,operation_id,kind,item_id),
	 FOREIGN KEY(event_id) REFERENCES canonical_events(event_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX harness_activities_mailbox_time ON harness_activities(source_installation_id,mailbox_id,occurred_at,event_id);
CREATE INDEX harness_activities_progress_retention ON harness_activities(source_installation_id,mailbox_id,harness,session_id,occurred_at,event_id) WHERE kind='progress';
PRAGMA user_version = 33;
`

const (
	quarantineMaxRows  = 1000
	quarantineMaxBytes = 16 << 20
	quarantineMaxAge   = 30 * 24 * time.Hour
)

type SQLite struct {
	db                    *sql.DB
	signer                identity.Material
	database              string
	afterChange           func(domain.Invalidation)
	now                   func() time.Time
	projectCommandHandler func(context.Context, domain.ProjectCommand, domain.ProjectCommandData) (domain.Project, error)
}

func (s *SQLite) SetProjectCommandHandler(handler func(context.Context, domain.ProjectCommand, domain.ProjectCommandData) (domain.Project, error)) {
	s.projectCommandHandler = handler
}

type canonicalIngest struct {
	EventIDs                  []string
	ProjectInputAcceptanceIDs []string
}

var canonicalChangeTopics = []domain.ChangeTopic{
	domain.TopicMessages, domain.TopicMailboxes, domain.TopicNetwork, domain.TopicPeers, domain.TopicHuman,
	domain.TopicAgents, domain.TopicActivities,
}

func (s *SQLite) SetChangeObserver(observer func(domain.Invalidation)) {
	s.afterChange = observer
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
	s := &SQLite{db: db, signer: signer, database: resolved, now: time.Now}
	if err := s.configure(context.Background()); err != nil {
		db.Close()
		return nil, err
	}
	if err := s.reconcileProjectInputs(context.Background()); err != nil {
		db.Close()
		return nil, fmt.Errorf("reconcile project inputs: %w", err)
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
	if version != 0 && version != schemaVersion {
		return fmt.Errorf("unsupported HQ database schema %d; archive or remove the database, then reinitialize and pair this installation", version)
	}
	if version == schemaVersion {
		if _, err := s.db.ExecContext(ctx, tuiDraftSchema); err != nil {
			return fmt.Errorf("ensure local TUI draft storage: %w", err)
		}
	}
	if version == 0 {
		var existingTables int
		if err := s.db.QueryRowContext(ctx, `SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'`).Scan(&existingTables); err != nil {
			return fmt.Errorf("inspect unversioned database: %w", err)
		}
		if existingTables != 0 {
			return errors.New("unsupported unversioned HQ database; archive or remove the database, then reinitialize and pair this installation")
		}
		if err := s.createFreshSchema(ctx); err != nil {
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
	var projectedVersion, projectedCount, reducedCount int
	metadataErr := s.db.QueryRowContext(ctx, `SELECT reducer_version,event_count FROM projection_metadata WHERE id=1`).Scan(&projectedVersion, &projectedCount)
	reductionErr := s.db.QueryRowContext(ctx, `SELECT count(*) FROM event_reduction`).Scan(&reducedCount)
	if metadataErr == nil && reductionErr == nil && projectedVersion == reducerVersion && projectedCount == count && reducedCount == count {
		return nil
	}
	return s.Rebuild(ctx)
}

func (s *SQLite) createFreshSchema(ctx context.Context) error {
	if _, err := s.db.ExecContext(ctx, schema); err != nil {
		return fmt.Errorf("create schema: %w", err)
	}
	return nil
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
	selection := event.Content{Type: event.TypeHumanAccountSelect, Parents: []string{signed[2].ID()}, Authorities: []string{signed[2].ID()}, Scope: event.ScopeInstallationPrivate, Payload: selectionPayload}
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
	return event.Policy{InstallationID: s.signer.InstallationID, RootKeyID: s.signer.PublicKey(), HumanMailboxID: model.HumanMailboxID}
}

func (s *SQLite) CurrentRevision(ctx context.Context) (uint64, error) {
	var revision uint64
	err := s.db.QueryRowContext(ctx, `SELECT revision FROM change_revision WHERE id=1`).Scan(&revision)
	return revision, err
}

func recordChangeTx(ctx context.Context, tx *sql.Tx, topics []domain.ChangeTopic) (domain.Invalidation, error) {
	if len(topics) == 0 {
		return domain.Invalidation{}, nil
	}
	seen := make(map[domain.ChangeTopic]bool)
	unique := make([]domain.ChangeTopic, 0, len(topics))
	for _, topic := range topics {
		if topic != "" && !seen[topic] {
			seen[topic] = true
			unique = append(unique, topic)
		}
	}
	var revision uint64
	if err := tx.QueryRowContext(ctx, `UPDATE change_revision SET revision=revision+1 WHERE id=1 RETURNING revision`).Scan(&revision); err != nil {
		return domain.Invalidation{}, err
	}
	return domain.Invalidation{Revision: revision, Topics: unique}, nil
}

func (s *SQLite) notifyChange(change domain.Invalidation) {
	if change.Revision != 0 && s.afterChange != nil {
		change.Topics = append([]domain.ChangeTopic(nil), change.Topics...)
		s.afterChange(change)
	}
}

var errMutationAlreadyCommitted = errors.New("mutation was already committed")

func (s *SQLite) MutationResult(ctx context.Context, mutation domain.Mutation) (json.RawMessage, bool, error) {
	var method, digest string
	var result []byte
	err := s.db.QueryRowContext(ctx, `SELECT method,request_digest,result FROM mutation_receipts WHERE mutation_id=?`, mutation.ID).Scan(&method, &digest, &result)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, false, nil
	}
	if err != nil {
		return nil, false, err
	}
	if method != mutation.Method || digest != mutation.RequestDigest {
		return nil, false, fmt.Errorf("%w: mutation_id was already used for a different request", domain.ErrMutationConflict)
	}
	return json.RawMessage(result), true, nil
}

func recordMutationTx(ctx context.Context, tx *sql.Tx, result any) error {
	mutation, ok := domain.MutationFromContext(ctx)
	if !ok {
		return nil
	}
	raw, err := json.Marshal(result)
	if err != nil {
		return fmt.Errorf("encode mutation result: %w", err)
	}
	_, err = tx.ExecContext(ctx, `INSERT INTO mutation_receipts(mutation_id,method,request_digest,result,committed_at) VALUES (?,?,?,?,?)`, mutation.ID, mutation.Method, mutation.RequestDigest, raw, time.Now().UTC().UnixMilli())
	if err == nil {
		return nil
	}
	var method, digest string
	if lookupErr := tx.QueryRowContext(ctx, `SELECT method,request_digest FROM mutation_receipts WHERE mutation_id=?`, mutation.ID).Scan(&method, &digest); lookupErr == nil {
		if method != mutation.Method || digest != mutation.RequestDigest {
			return fmt.Errorf("%w: mutation_id was already used for a different request", domain.ErrMutationConflict)
		}
		return errMutationAlreadyCommitted
	}
	return err
}

func (s *SQLite) recordMutation(ctx context.Context, result any) error {
	if _, ok := domain.MutationFromContext(ctx); !ok {
		return nil
	}
	_, err := s.commitMutation(ctx, nil, func(*sql.Tx) (any, error) { return result, nil })
	return err
}

func (s *SQLite) commitMutation(ctx context.Context, topics []domain.ChangeTopic, action func(*sql.Tx) (any, error)) (any, error) {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	result, err := action(tx)
	if err != nil {
		return nil, err
	}
	if err := recordMutationTx(ctx, tx, result); err != nil {
		return nil, err
	}
	change, err := recordChangeTx(ctx, tx, topics)
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	s.notifyChange(change)
	return result, nil
}

func (s *SQLite) appendContents(ctx context.Context, contents []event.Content, times []time.Time, after func(*sql.Tx) error) error {
	_, err := s.appendContentsResult(ctx, contents, times, func(tx *sql.Tx) (any, error) {
		if after == nil {
			return nil, nil
		}
		return nil, after(tx)
	})
	return err
}

func (s *SQLite) appendContentsResult(ctx context.Context, contents []event.Content, times []time.Time, result func(*sql.Tx) (any, error)) (any, error) {
	signed, err := s.signContents(ctx, contents, times)
	if err != nil {
		return nil, err
	}
	if outer, ok := ctx.Value(canonicalTransactionContextKey{}).(*canonicalTransactionContext); ok {
		commit, err := s.ingestCanonicalTx(ctx, outer.tx, signed, true)
		if err != nil {
			return nil, err
		}
		var value any
		if result != nil {
			value, err = result(outer.tx)
			if err != nil {
				return nil, err
			}
		}
		if len(commit.EventIDs) != 0 {
			outer.addTopics(canonicalChangeTopics...)
		}
		if len(commit.ProjectInputAcceptanceIDs) != 0 {
			outer.addTopics(domain.TopicProjects)
		}
		return value, nil
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	commit, err := s.ingestCanonicalTx(ctx, tx, signed, true)
	if err != nil {
		return nil, err
	}
	var value any
	if result != nil {
		value, err = result(tx)
		if err != nil {
			return nil, err
		}
	}
	if err := recordMutationTx(ctx, tx, value); err != nil {
		return nil, err
	}
	var change domain.Invalidation
	if len(commit.EventIDs) > 0 {
		topics := canonicalChangeTopics
		if len(commit.ProjectInputAcceptanceIDs) != 0 {
			topics = append(append([]domain.ChangeTopic(nil), canonicalChangeTopics...), domain.TopicProjects)
		}
		change, err = recordChangeTx(ctx, tx, topics)
		if err != nil {
			return nil, err
		}
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	s.notifyChange(change)
	return value, nil
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
	_, err := s.ingestCanonicalTx(ctx, tx, additions, requireProjected)
	return err
}

func (s *SQLite) ingestCanonicalTx(ctx context.Context, tx *sql.Tx, additions []event.SignedEvent, requireProjected bool) (canonicalIngest, error) {
	commit, err := s.ingestCanonicalProjectionTx(ctx, tx, additions, requireProjected)
	if err != nil {
		return commit, err
	}
	observations, err := s.signMailboxAccessObservationsTx(ctx, tx, additions)
	if err != nil {
		return commit, err
	}
	if len(observations) != 0 {
		observed, observeErr := s.ingestCanonicalProjectionTx(ctx, tx, observations, true)
		commit.EventIDs = append(commit.EventIDs, observed.EventIDs...)
		if observeErr != nil {
			return commit, observeErr
		}
	}
	accepted, err := s.reconcileProjectInputsTx(ctx, tx, true)
	commit.EventIDs = append(commit.EventIDs, accepted...)
	commit.ProjectInputAcceptanceIDs = append(commit.ProjectInputAcceptanceIDs, accepted...)
	return commit, err
}

func (s *SQLite) signMailboxAccessObservationsTx(ctx context.Context, tx *sql.Tx, additions []event.SignedEvent) ([]event.SignedEvent, error) {
	type storedObservation struct {
		id      string
		grantID string
		message string
		parents []string
	}
	var stored []storedObservation
	rows, err := tx.QueryContext(ctx, `SELECT raw FROM canonical_events WHERE event_type=? AND installation_id=?`, event.TypeMailboxAccessObserve, s.signer.InstallationID)
	if err != nil {
		return nil, err
	}
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			rows.Close()
			return nil, err
		}
		inspection := event.Inspect(raw)
		var payload event.MailboxAccessObservationPayload
		if inspection.Status == event.StatusProjected && json.Unmarshal(inspection.Event.Content.Payload, &payload) == nil {
			stored = append(stored, storedObservation{
				id: inspection.Event.ID(), grantID: payload.GrantEventID,
				message: payload.MessageEventID, parents: inspection.Event.Content.Parents,
			})
		}
	}
	if err := rows.Close(); err != nil {
		return nil, err
	}
	observedMessages := make(map[string]bool, len(stored))
	frontiers := make(map[string]map[string]bool)
	for _, observation := range stored {
		observedMessages[observation.message] = true
		if frontiers[observation.grantID] == nil {
			frontiers[observation.grantID] = make(map[string]bool)
		}
		frontiers[observation.grantID][observation.id] = true
	}
	for _, observation := range stored {
		for _, parent := range observation.parents {
			for _, grantFrontier := range frontiers {
				delete(grantFrontier, parent)
			}
		}
	}

	var observations []event.SignedEvent
	for _, item := range additions {
		content := item.Content
		if content.Scope != event.ScopePeerAddressed || content.InstallationID == s.signer.InstallationID || content.Recipient == nil || content.Recipient.InstallationID != s.signer.InstallationID || len(content.Authorities) != 1 {
			continue
		}
		if content.Type != event.TypeQuestion && content.Type != event.TypeAnswer && content.Type != event.TypeMessage {
			continue
		}
		if observedMessages[item.ID()] {
			continue
		}
		grantID := content.Authorities[0]
		parents := []string{grantID, item.ID()}
		for observationID := range frontiers[grantID] {
			parents = append(parents, observationID)
		}
		parents = uniqueSorted(parents)
		if len(parents) > event.MaxParents {
			return nil, errors.New("mailbox access observation frontier exceeds the causal parent limit")
		}
		payload, err := event.MarshalPayload(event.MailboxAccessObservationPayload{GrantEventID: grantID, MessageEventID: item.ID()})
		if err != nil {
			return nil, err
		}
		signed, err := s.signContents(ctx, []event.Content{{
			Type: event.TypeMailboxAccessObserve, Sender: s.localAddress(model.HumanMailboxID),
			Recipient: &event.MailboxAddress{InstallationID: content.InstallationID, MailboxID: model.HumanMailboxID},
			Parents:   parents, Authorities: []string{grantID}, Scope: event.ScopePeerAddressed, Payload: payload,
		}}, nil)
		if err != nil {
			return nil, err
		}
		observation := signed[0]
		observations = append(observations, observation)
		observedMessages[item.ID()] = true
		frontiers[grantID] = map[string]bool{observation.ID(): true}
	}
	return observations, nil
}

// ingestCanonicalProjectionTx projects canonical facts without invoking the
// project-input invariant. Only the invariant itself uses this lower boundary
// while appending acceptance events and their idempotent pending notices.
func (s *SQLite) ingestCanonicalProjectionTx(ctx context.Context, tx *sql.Tx, additions []event.SignedEvent, requireProjected bool) (canonicalIngest, error) {
	var commit canonicalIngest
	existing := make(map[string]bool, len(additions))
	var seeds []string
	for _, item := range additions {
		var exists int
		if err := tx.QueryRowContext(ctx, `SELECT EXISTS(SELECT 1 FROM canonical_events WHERE event_id=?)`, item.ID()).Scan(&exists); err != nil {
			return commit, err
		}
		if exists != 0 {
			existing[item.ID()] = true
			continue
		}
		inspection := event.Inspect(item.Wire)
		reason := ""
		if inspection.Err != nil {
			reason = inspection.Err.Error()
		}
		_, err := tx.ExecContext(ctx, `INSERT INTO canonical_events(event_id, raw, created_at, event_type, installation_id, signer_key_id, scope, reduction_status, reduction_reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			item.ID(), item.Wire, item.Nostr.CreatedAt, item.Content.Type, item.Content.InstallationID, item.Content.SignerKeyID, item.Content.Scope, inspection.Status, reason)
		if err != nil {
			return commit, fmt.Errorf("append canonical event: %w", err)
		}
		if err := indexCanonicalEventTx(ctx, tx, item); err != nil {
			return commit, err
		}
		seeds = append(seeds, item.ID())
	}
	if len(seeds) == 0 {
		return commit, nil
	}
	state, err := affectedReductionTx(ctx, tx, seeds, s.policy())
	if err != nil {
		return commit, err
	}
	for _, item := range additions {
		if existing[item.ID()] {
			continue
		}
		record, ok := state.Records[item.ID()]
		acceptable := ok && (record.Status == event.StatusProjected || (!requireProjected && (record.Status == event.StatusUnresolved || record.Status == event.StatusUnsupported)))
		if !acceptable {
			if ok {
				return commit, fmt.Errorf("new event %s was not projected: %s", item.ID(), record.Reason)
			}
			return commit, fmt.Errorf("new event %s was rejected", item.ID())
		}
		commit.EventIDs = append(commit.EventIDs, item.ID())
	}
	var generation int64
	if err := tx.QueryRowContext(ctx, `SELECT generation FROM projection_metadata WHERE id=1`).Scan(&generation); err != nil && !errors.Is(err, sql.ErrNoRows) {
		return commit, err
	}
	generation++
	if err := patchCausalIndexesTx(ctx, tx, state, generation); err != nil {
		return commit, err
	}
	return commit, s.projectStateTx(ctx, tx, state, false)
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
	commit, err := s.ingestCanonicalTx(ctx, tx, additions, false)
	if err != nil {
		return err
	}
	var change domain.Invalidation
	if len(commit.EventIDs) > 0 {
		topics := append(append([]domain.ChangeTopic(nil), canonicalChangeTopics...), domain.TopicProjects)
		change, err = recordChangeTx(ctx, tx, topics)
		if err != nil {
			return err
		}
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	if len(commit.EventIDs) > 0 {
		s.notifyChange(change)
	}
	return s.ProcessProjectCommands(ctx)
}

func (s *SQLite) rebuildTx(ctx context.Context, tx *sql.Tx, state event.State) error {
	return s.projectStateTx(ctx, tx, state, true)
}

// projectStateTx applies a pure reduction. Repair mode is the only path that
// clears rebuildable state; ordinary ingestion upserts its affected result.
func (s *SQLite) projectStateTx(ctx context.Context, tx *sql.Tx, state event.State, repair bool) error {
	// Project and assignment tables reference rebuildable projections. Their
	// canonical rows are recreated before commit, so defer those foreign-key
	// checks across the projection rebuild transaction.
	if _, err := tx.ExecContext(ctx, `PRAGMA defer_foreign_keys = ON`); err != nil {
		return err
	}
	var generation int64
	metadataErr := tx.QueryRowContext(ctx, `SELECT generation FROM projection_metadata WHERE id=1`).Scan(&generation)
	if metadataErr != nil && !errors.Is(metadataErr, sql.ErrNoRows) {
		return metadataErr
	}
	repair = repair || errors.Is(metadataErr, sql.ErrNoRows)
	accountAffected, projectAffected := repair, repair
	if !repair {
		if err := tx.QueryRowContext(ctx, `SELECT EXISTS(SELECT 1 FROM impacted_canonical_events i JOIN canonical_events c ON c.event_id=i.event_id WHERE c.event_type IN (?,?,?,?,?))`, event.TypeHumanAccountCreate, event.TypeHumanAccountSelect, event.TypeHumanDeviceGrant, event.TypeHumanDeviceAccept, event.TypeHumanDeviceRevoke).Scan(&accountAffected); err != nil {
			return err
		}
		if err := tx.QueryRowContext(ctx, `SELECT EXISTS(SELECT 1 FROM impacted_canonical_events i JOIN canonical_events c ON c.event_id=i.event_id WHERE c.event_type IN (?,?,?))`, event.TypeProjectEvent, event.TypeProjectCommand, event.TypeProjectResult).Scan(&projectAffected); err != nil {
			return err
		}
	}
	generation++
	if repair {
		if err := rebuildCausalIndexesTx(ctx, tx, state, generation); err != nil {
			return err
		}
	}
	activity := make(map[string]int64)
	agentActivity := make(map[string]int64)
	type ownershipLease struct {
		token  string
		expiry int64
	}
	ownership := make(map[string]ownershipLease)
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
	agentRows, agentErr := tx.QueryContext(ctx, `SELECT name,last_active_at FROM named_agents WHERE last_active_at IS NOT NULL`)
	if agentErr == nil {
		for agentRows.Next() {
			var name string
			var seen int64
			if agentRows.Scan(&name, &seen) == nil {
				agentActivity[name] = seen
			}
		}
		agentRows.Close()
	}
	ownershipRows, ownershipErr := tx.QueryContext(ctx, `SELECT name,owner_token,lease_expires_at FROM agent_ownership`)
	if ownershipErr == nil {
		for ownershipRows.Next() {
			var name string
			var lease ownershipLease
			if ownershipRows.Scan(&name, &lease.token, &lease.expiry) == nil {
				ownership[name] = lease
			}
		}
		ownershipRows.Close()
	}
	for id, record := range state.Records {
		if _, err := tx.ExecContext(ctx, `UPDATE canonical_events SET reduction_status=?,reduction_reason=? WHERE event_id=? AND (reduction_status<>? OR reduction_reason<>?)`, record.Status, record.Reason, id, record.Status, record.Reason); err != nil {
			return err
		}
	}
	if !repair {
		// A newly arrived revoke, block, or competing fact can remove an
		// already-projected item from the pure state. Upserts alone cannot
		// retract that stale row, so invalidate affected typed projections whose
		// canonical record is no longer projected before applying the new state.
		for _, table := range []string{"messages", "harness_activities", "threads"} {
			if _, err := tx.ExecContext(ctx, `DELETE FROM `+table+` WHERE event_id IN (
SELECT i.event_id FROM impacted_canonical_events i
JOIN canonical_events c ON c.event_id=i.event_id
WHERE c.reduction_status<>?
)`, event.StatusProjected); err != nil {
				return fmt.Errorf("invalidate stale %s projection: %w", table, err)
			}
		}
	}
	if repair {
		for _, table := range []string{"threads", "messages", "harness_activities", "mailbox_contexts", "harness_bindings", "agent_sessions", "agent_ownership", "named_agents", "mailbox_activity", "mailboxes", "peers", "mailbox_access", "human_account_default", "human_account_devices", "human_accounts"} {
			if _, err := tx.ExecContext(ctx, `DELETE FROM `+table); err != nil {
				return fmt.Errorf("clear %s projection: %w", table, err)
			}
		}
	}
	for _, mailbox := range state.Mailboxes {
		created := firstMailboxTime(state, mailbox.ID)
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO mailboxes(id, installation_id, kind, label, created_at) VALUES (?, ?, ?, ?, ?)`, mailbox.ID, s.signer.InstallationID, mailbox.Kind, mailbox.Label, created); err != nil {
			return fmt.Errorf("project mailbox: %w", err)
		}
		seen := created
		if activity[mailbox.ID] > seen {
			seen = activity[mailbox.ID]
		}
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO mailbox_activity(mailbox_id, last_seen_at) VALUES (?, ?)`, mailbox.ID, seen); err != nil {
			return err
		}
		for _, binding := range mailbox.Bindings {
			if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO harness_bindings(harness, external_session_id, mailbox_id, created_at) VALUES (?, ?, ?, ?)`, binding.Harness, binding.ExternalSessionID, mailbox.ID, created); err != nil {
				return fmt.Errorf("project harness binding: %w", err)
			}
		}
		for _, context := range mailbox.Contexts {
			if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO mailbox_contexts(mailbox_id, directory, git_common_dir, remote_identity, worktree, branch, first_seen_at) VALUES (?, ?, ?, ?, ?, ?, ?)`, mailbox.ID, context.Directory, context.GitCommonDir, context.RemoteIdentity, context.Worktree, context.Branch, created); err != nil {
				return err
			}
		}
	}
	for _, agent := range state.NamedAgents {
		var lastActive any
		if seen := agentActivity[agent.Name]; seen != 0 {
			lastActive = seen
		}
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO named_agents(name,mailbox_id,retired,current_harness,current_session_id,last_active_at) VALUES (?,?,?,?,?,?)`, agent.Name, agent.MailboxID, boolInt(agent.Retired), agent.Harness, agent.ExternalSessionID, lastActive); err != nil {
			return fmt.Errorf("project named agent: %w", err)
		}
		if lease, exists := ownership[agent.Name]; exists && !agent.Retired {
			if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO agent_ownership(name,owner_token,lease_expires_at) VALUES (?,?,?)`, agent.Name, lease.token, lease.expiry); err != nil {
				return fmt.Errorf("restore agent ownership: %w", err)
			}
		}
		for _, session := range agent.Sessions {
			if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO agent_sessions(agent_name,mailbox_id,harness,external_session_id,thread_name,directory,git_common_dir,remote_identity,worktree,branch,created_at,last_selected_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)`, agent.Name, agent.MailboxID, session.Harness, session.ExternalSessionID, session.ThreadName, session.Context.Directory, session.Context.GitCommonDir, session.Context.RemoteIdentity, session.Context.Worktree, session.Context.Branch, time.Unix(session.CreatedAt, 0).UTC().UnixMilli(), time.Unix(session.LastSelectedAt, 0).UTC().UnixMilli()); err != nil {
				return fmt.Errorf("project named agent session: %w", err)
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
			archived = message.ArchivedAt.UnixMilli()
		}
		correlation := message.Correlation
		technicalJSON, _ := json.Marshal(message.TechnicalSections)
		if len(message.TechnicalSections) == 0 {
			technicalJSON = []byte("[]")
		}
		_, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO messages(id, event_id, thread_event_id, event_type, purpose, presentation, audience_account_id, directory, git_common_dir, remote_identity, worktree, branch, sender_installation_id, recipient_installation_id, sender_mailbox_id, recipient_mailbox_id, actor_label, body, details, technical_sections_json, harness_provider, harness_session_id, harness_operation_id, correlation_item_id, correlation_request_id, reply_to, created_at, archived_at, incomplete, peer_received, rejected) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			id, eventID, message.ThreadID, message.Type, model.NormalizeMessagePurpose(message.Purpose), message.Presentation, message.AudienceAccountID, repo.Directory, repo.GitCommonDir, repo.RemoteIdentity, repo.Worktree, repo.Branch, message.Sender.InstallationID, message.Recipient.InstallationID, message.Sender.MailboxID, message.Recipient.MailboxID, message.ActorLabel, message.Body, message.Details, string(technicalJSON), correlation.Provider, correlation.SessionID, correlation.OperationID, correlation.ItemID, correlation.RequestID, replyTo, message.CreatedAt.UnixMilli(), archived, boolInt(message.Incomplete), boolInt(message.PeerReceived), boolInt(message.Rejected))
		if err != nil {
			return fmt.Errorf("project message: %w", err)
		}
		if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO delivery_facts(message_id) VALUES (?)`, id); err != nil {
			return err
		}
	}
	for _, key := range state.HarnessActivityOrder {
		activity := state.HarnessActivities[key]
		correlation := activity.Correlation
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO harness_activities(event_id,source_installation_id,mailbox_id,audience_account_id,harness,session_id,operation_id,kind,item_id,status,title,body,truncated,occurred_at,runtime_id,source_sequence) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`,
			activity.EventID, activity.Sender.InstallationID, activity.Sender.MailboxID, activity.AudienceAccountID,
			correlation.Provider, correlation.SessionID, correlation.OperationID, activity.Kind, correlation.ItemID,
			activity.Status, activity.Title, activity.Body, boolInt(activity.Truncated), activity.OccurredAt.UnixMilli(),
			activity.RuntimeID, fmt.Sprintf("%d", activity.Sequence)); err != nil {
			return fmt.Errorf("project harness activity: %w", err)
		}
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM harness_activities WHERE event_id IN (
SELECT event_id FROM (
  SELECT event_id,row_number() OVER (
    PARTITION BY source_installation_id,mailbox_id,harness,session_id
    ORDER BY occurred_at DESC,event_id DESC
  ) AS retention_rank
  FROM harness_activities WHERE kind='progress'
) WHERE retention_rank > ?
)`, domain.HarnessActivityProgressRetained); err != nil {
		return fmt.Errorf("prune projected harness activity progress: %w", err)
	}
	for id, thread := range state.Threads {
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO threads(event_id, answered, cancelled) VALUES (?, ?, ?)`, id, boolInt(thread.Answered), boolInt(thread.Cancelled)); err != nil {
			return err
		}
	}
	for _, peer := range state.Peers {
		relays, _ := json.Marshal(peer.Relays)
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO peers(installation_id, signer_key_id, name, relays_json, trusted) VALUES (?, ?, ?, ?, ?)`, peer.InstallationID, peer.SignerKeyID, peer.Name, string(relays), boolInt(peer.Trusted)); err != nil {
			return err
		}
	}
	for _, access := range state.MailboxAccess {
		if access.GrantorInstallationID != s.signer.InstallationID {
			continue
		}
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO mailbox_access(grant_event_id,mailbox_id,grantor_installation_id,grantee_installation_id,grantee_signer_key_id,active) VALUES (?, ?, ?, ?, ?, ?)`, access.GrantEventID, access.MailboxID, access.GrantorInstallationID, access.GranteeInstallationID, access.GranteeSignerKeyID, boolInt(access.Active)); err != nil {
			return err
		}
	}
	for _, account := range state.Accounts {
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO human_accounts(account_id,creator_installation_id,creator_signer_key_id,label,creation_event_id) VALUES (?,?,?,?,?)`, account.ID, account.CreatorInstallationID, account.CreatorSignerKeyID, account.Label, account.CreationEventID); err != nil {
			return err
		}
		for _, device := range account.Devices {
			deviceState := device.State
			if deviceState == "" {
				deviceState = "pending"
			}
			relays, _ := json.Marshal(device.Relays)
			revokes, _ := json.Marshal(device.RevokeEventIDs)
			if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO human_account_devices(account_id,installation_id,signer_key_id,label,relays_json,state,grant_event_id,accept_event_id,revoke_event_ids_json) VALUES (?,?,?,?,?,?,?,?,?)`, account.ID, device.InstallationID, device.SignerKeyID, device.Label, string(relays), deviceState, device.GrantEventID, device.AcceptEventID, string(revokes)); err != nil {
				return err
			}
		}
	}
	if !repair && accountAffected {
		if _, err := tx.ExecContext(ctx, `DELETE FROM human_account_default WHERE id=1 AND account_id<>?`, state.DefaultAccountID); err != nil {
			return err
		}
	}
	if state.DefaultAccountID != "" {
		if _, err := tx.ExecContext(ctx, `INSERT OR REPLACE INTO human_account_default(id,account_id) VALUES (1,?)`, state.DefaultAccountID); err != nil {
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
	if accountAffected {
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
	}
	if projectAffected {
		if err := s.rebuildAuthoritativeProjectsTx(ctx, tx, state, repair); err != nil {
			return err
		}
		if err := s.rebuildProjectReplicasTx(ctx, tx, state, repair); err != nil {
			return err
		}
	}
	if repair {
		_, err = tx.ExecContext(ctx, `INSERT INTO projection_metadata(id,reducer_version,event_count,generation,repaired_at) VALUES (1,?,?,?,?) ON CONFLICT(id) DO UPDATE SET reducer_version=excluded.reducer_version,event_count=excluded.event_count,generation=excluded.generation,repaired_at=excluded.repaired_at`, reducerVersion, len(state.Records), generation, time.Now().UTC().UnixMilli())
	} else {
		var eventCount int
		if err := tx.QueryRowContext(ctx, `SELECT count(*) FROM canonical_events`).Scan(&eventCount); err != nil {
			return err
		}
		_, err = tx.ExecContext(ctx, `INSERT INTO projection_metadata(id,reducer_version,event_count,generation,repaired_at) VALUES (1,?,?,?,?) ON CONFLICT(id) DO UPDATE SET reducer_version=excluded.reducer_version,event_count=excluded.event_count,generation=excluded.generation`, reducerVersion, eventCount, generation, time.Now().UTC().UnixMilli())
	}
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

func mailboxLabel(kind model.MailboxKind, harness, label, id string) string {
	switch kind {
	case model.MailboxHuman:
		return "human"
	case model.MailboxProject:
		if label != "" {
			return label
		}
		return "project"
	case model.MailboxRemote:
		return "remote"
	case model.MailboxAgent:
	default:
		return "remote"
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
		value, err := s.appendContentsResult(ctx, contents, times, func(tx *sql.Tx) (any, error) {
			if _, err := tx.ExecContext(ctx, `UPDATE mailbox_activity SET last_seen_at = ? WHERE mailbox_id = ?`, now.UnixMilli(), id); err != nil {
				return nil, err
			}
			return getMailboxWith(ctx, tx, id)
		})
		if err != nil {
			return model.Mailbox{}, err
		}
		return value.(model.Mailbox), nil
	}
	value, err := s.commitMutation(ctx, []domain.ChangeTopic{domain.TopicMailboxes}, func(tx *sql.Tx) (any, error) {
		if _, err := tx.ExecContext(ctx, `UPDATE mailbox_activity SET last_seen_at = ? WHERE mailbox_id = ?`, now.UnixMilli(), id); err != nil {
			return nil, err
		}
		return getMailboxWith(ctx, tx, id)
	})
	if err != nil {
		return model.Mailbox{}, err
	}
	return value.(model.Mailbox), nil
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
	return getMailboxWith(ctx, s.queryer(ctx), id)
}

func (s *SQLite) queryer(ctx context.Context) rowQueryer {
	if outer, ok := ctx.Value(canonicalTransactionContextKey{}).(*canonicalTransactionContext); ok {
		return outer.tx
	}
	return s.db
}

type rowQueryer interface {
	QueryRowContext(context.Context, string, ...any) *sql.Row
}

func getMailboxWith(ctx context.Context, queryer rowQueryer, id string) (model.Mailbox, error) {
	var m model.Mailbox
	var harness, agentName, storedLabel sql.NullString
	var created, seen int64
	err := queryer.QueryRowContext(ctx, `SELECT m.id,m.kind,COALESCE(n.current_harness,(SELECT b.harness FROM harness_bindings b WHERE b.mailbox_id=m.id ORDER BY b.created_at DESC,b.harness,b.external_session_id LIMIT 1)),m.label,m.created_at,COALESCE(a.last_seen_at,m.created_at),n.name FROM mailboxes m LEFT JOIN named_agents n ON n.mailbox_id=m.id LEFT JOIN mailbox_activity a ON a.mailbox_id=m.id WHERE m.id=?`, id).Scan(&m.ID, &m.Kind, &harness, &storedLabel, &created, &seen, &agentName)
	if errors.Is(err, sql.ErrNoRows) {
		return m, ErrNotFound
	}
	if err != nil {
		return m, err
	}
	m.Harness = harness.String
	if agentName.String != "" {
		m.Label = agentName.String
	} else {
		m.Label = mailboxLabel(m.Kind, m.Harness, storedLabel.String, m.ID)
	}
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
		m.Label = mailboxLabel(m.Kind, m.Harness, "", m.ID)
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
	if !m.Purpose.Valid() {
		return fmt.Errorf("unsupported message purpose %q", m.Purpose)
	}
	m.Purpose = model.NormalizeMessagePurpose(m.Purpose)
	actorLabel := sender.Label
	payload, _ := event.MarshalPayload(textPayloadForMessage(m, actorLabel))
	typeName := event.TypeMessage
	scope := event.ScopeInstallationPrivate
	recipientInstallationID := s.signer.InstallationID
	var remoteProjectID, remoteProjectHome string
	_ = s.queryer(ctx).QueryRowContext(ctx, `SELECT id,home_installation_id FROM project_replicas WHERE json_extract(state_json,'$.mailbox_id')=?`, m.RecipientMailboxID).Scan(&remoteProjectID, &remoteProjectHome)
	if remoteProjectID != "" && m.SenderMailboxID == model.HumanMailboxID && m.Purpose == model.MessagePurposeConversation {
		m.Purpose = model.MessagePurposeProjectInput
		payload, _ = event.MarshalPayload(textPayloadForMessage(m, actorLabel))
	}
	if m.RecipientInstallationID != "" {
		recipientInstallationID = m.RecipientInstallationID
	} else if remoteProjectHome != "" {
		recipientInstallationID = remoteProjectHome
	}
	var recipient = &event.MailboxAddress{InstallationID: recipientInstallationID, MailboxID: m.RecipientMailboxID}
	var audience *event.Audience
	var parents []string
	var authorities []string
	remoteRecipient := recipientInstallationID != s.signer.InstallationID
	if remoteProjectID != "" && m.SenderMailboxID == model.HumanMailboxID {
		account, membership, deviceLabel, err := s.localAccountAction(ctx, "")
		if err != nil {
			return err
		}
		actorLabel = deviceLabel
		payload, _ = event.MarshalPayload(textPayloadForMessage(m, actorLabel))
		audience, parents, authorities, scope = &event.Audience{HumanAccountID: account.ID}, membership, uniqueSorted(membership), event.ScopeAccountAddressed
	} else if remoteRecipient {
		scope = event.ScopePeerAddressed
		authority, err := s.outboundMailboxAuthority(ctx, recipientInstallationID, m.RecipientMailboxID)
		if err != nil {
			return err
		}
		parents, authorities = []string{authority}, []string{authority}
		if m.SenderMailboxID != model.HumanMailboxID {
			typeName = event.TypeQuestion
		}
	} else if m.RecipientMailboxID == model.HumanMailboxID && m.SenderMailboxID != model.HumanMailboxID {
		typeName = event.TypeQuestion
		account, membership, _, err := s.localAccountAction(ctx, "")
		if err != nil {
			return err
		}
		audience, parents, authorities, recipient, scope = &event.Audience{HumanAccountID: account.ID}, membership, uniqueSorted(membership), nil, event.ScopeAccountAddressed
	} else if m.SenderMailboxID == model.HumanMailboxID && m.RecipientMailboxID != model.HumanMailboxID {
		account, membership, deviceLabel, err := s.localAccountAction(ctx, "")
		if err != nil {
			return err
		}
		actorLabel = deviceLabel
		payload, _ = event.MarshalPayload(textPayloadForMessage(m, actorLabel))
		audience, parents, authorities, scope = &event.Audience{HumanAccountID: account.ID}, membership, uniqueSorted(membership), event.ScopeAccountAddressed
	}
	content := event.Content{Schema: event.MessageSchemaVersion, Type: typeName, Sender: s.localAddress(m.SenderMailboxID), Recipient: recipient, Audience: audience, Parents: parents, Authorities: authorities, Scope: scope, Payload: payload}
	var projectID string
	if recipientInstallationID == s.signer.InstallationID {
		err := s.queryer(ctx).QueryRowContext(ctx, `SELECT id FROM projects WHERE mailbox_id=?`, m.RecipientMailboxID).Scan(&projectID)
		if err != nil && !errors.Is(err, sql.ErrNoRows) {
			return err
		}
	}
	if projectID != "" && m.SenderMailboxID == model.HumanMailboxID && m.Purpose == model.MessagePurposeConversation {
		m.Purpose = model.MessagePurposeProjectInput
		payload, _ = event.MarshalPayload(textPayloadForMessage(m, actorLabel))
		content.Payload = payload
	}
	return s.appendContents(ctx, []event.Content{content}, []time.Time{m.CreatedAt}, nil)
}

func textPayloadForMessage(message model.Message, actorLabel string) event.TextPayload {
	correlation := message.Correlation
	if correlation.Empty() {
		correlation = model.MessageCorrelation{
			Provider: message.HarnessProvider, SessionID: message.HarnessSessionID, OperationID: message.HarnessOperationID,
		}
	}
	return event.TextPayload{
		MessageID: message.ID, Body: message.Body, Details: message.Details, Purpose: message.Purpose,
		Context: contextPointer(message.Context), ActorLabel: actorLabel, Presentation: message.Presentation,
		Correlation: correlation, TechnicalSections: message.TechnicalSections,
	}
}

func contextPointer(repo model.RepositoryContext) *event.RepositoryContext {
	if repo == (model.RepositoryContext{}) {
		return nil
	}
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
	if !reply.Purpose.Valid() {
		return fmt.Errorf("unsupported message purpose %q", reply.Purpose)
	}
	if reply.Purpose == "" {
		if original.message.Purpose == model.MessagePurposeProtocolQuestion {
			reply.Purpose = model.MessagePurposeProtocolAnswer
		} else {
			reply.Purpose = model.MessagePurposeConversation
		}
	}
	if reply.Correlation.Empty() && reply.HarnessProvider == "" && reply.HarnessSessionID == "" && reply.HarnessOperationID == "" {
		reply.Correlation = original.message.Correlation
	}
	scope := event.ScopeInstallationPrivate
	parents := []string{original.eventID}
	var audience *event.Audience
	var authorities []string
	actorLabel := "human"
	if original.message.AudienceAccountID != "" {
		account, membership, deviceLabel, err := s.localAccountAction(ctx, original.message.AudienceAccountID)
		if err != nil {
			return err
		}
		parents = uniqueSorted(append(parents, membership...))
		authorities = uniqueSorted(membership)
		audience, scope, actorLabel = &event.Audience{HumanAccountID: account.ID}, event.ScopeAccountAddressed, deviceLabel
	} else if original.message.SenderInstallationID != s.signer.InstallationID {
		authority, err := s.outboundMailboxAuthority(ctx, original.message.SenderInstallationID, reply.RecipientMailboxID)
		if err != nil {
			return err
		}
		parents = uniqueSorted(append(parents, authority))
		authorities, scope = []string{authority}, event.ScopePeerAddressed
	}
	payload, _ := event.MarshalPayload(textPayloadForMessage(reply, actorLabel))
	answer := event.Content{Schema: event.MessageSchemaVersion, Type: event.TypeAnswer, Sender: s.localAddress(reply.SenderMailboxID), Recipient: &event.MailboxAddress{InstallationID: original.message.SenderInstallationID, MailboxID: reply.RecipientMailboxID}, Audience: audience, ThreadID: original.eventID, Parents: parents, Authorities: authorities, Scope: scope, Payload: payload}
	archivePayload, _ := event.MarshalPayload(event.TargetPayload{TargetEventID: original.eventID})
	archive := event.Content{Type: event.TypeMessageArchive, Sender: s.localAddress(model.HumanMailboxID), Audience: audience, Parents: parents, Authorities: authorities, Scope: scope, Payload: archivePayload}
	if scope == event.ScopePeerAddressed {
		archive.Audience, archive.Authorities, archive.Scope = nil, nil, event.ScopeInstallationPrivate
	}
	if original.message.SenderInstallationID == s.signer.InstallationID {
		var projectID string
		err := s.queryer(ctx).QueryRowContext(ctx, `SELECT id FROM projects WHERE mailbox_id=? AND home_installation_id=?`, original.message.SenderMailboxID, s.signer.InstallationID).Scan(&projectID)
		if err != nil && !errors.Is(err, sql.ErrNoRows) {
			return err
		}
		if projectID != "" {
			if reply.Purpose == model.MessagePurposeProtocolAnswer {
				return s.appendContents(ctx, []event.Content{answer, archive}, []time.Time{reply.CreatedAt, reply.CreatedAt}, nil)
			}
			if reply.Purpose == model.MessagePurposeConversation {
				reply.Purpose = model.MessagePurposeProjectInput
				payload, _ = event.MarshalPayload(textPayloadForMessage(reply, actorLabel))
				answer.Payload = payload
			}
			return s.appendContents(ctx, []event.Content{answer, archive}, []time.Time{reply.CreatedAt, reply.CreatedAt}, nil)
		}
	}
	return s.appendContents(ctx, []event.Content{answer, archive}, []time.Time{reply.CreatedAt, reply.CreatedAt}, nil)
}

func (s *SQLite) outboundMailboxAuthority(ctx context.Context, recipientInstallationID, recipientMailboxID string) (string, error) {
	state, err := s.reduceCanonicalResources(ctx, canonicalResource{kind: "mailbox-access-route", id: recipientInstallationID + ":" + recipientMailboxID + ":" + s.signer.InstallationID})
	if err != nil {
		return "", err
	}
	var grants []string
	for _, access := range state.MailboxAccess {
		if access.Active && access.GrantorInstallationID == recipientInstallationID && access.GranteeInstallationID == s.signer.InstallationID && access.GranteeSignerKeyID == s.signer.PublicKey() && access.MailboxID == recipientMailboxID {
			grants = append(grants, access.GrantEventID)
		}
	}
	sort.Strings(grants)
	if len(grants) == 0 {
		return "", fmt.Errorf("peer %s has not granted access to mailbox %s", recipientInstallationID, recipientMailboxID)
	}
	return grants[0], nil
}

type messageWithEvent struct {
	message model.Message
	eventID string
}

func (s *SQLite) messageRecord(ctx context.Context, id string) (messageWithEvent, error) {
	queryer := s.queryer(ctx)
	m, err := getMessageWith(ctx, queryer, id)
	if err != nil {
		return messageWithEvent{}, err
	}
	var eventID string
	if err := queryer.QueryRowContext(ctx, `SELECT event_id FROM messages WHERE id = ?`, id).Scan(&eventID); err != nil {
		return messageWithEvent{}, err
	}
	return messageWithEvent{message: m, eventID: eventID}, nil
}

const columns = `msg.id, msg.event_id, msg.thread_event_id, msg.purpose, msg.presentation, msg.harness_provider, msg.harness_session_id, msg.harness_operation_id, msg.correlation_item_id, msg.correlation_request_id, msg.technical_sections_json, msg.incomplete, msg.peer_received, msg.rejected, CASE WHEN msg.rejected=1 OR o.state='rejected' THEN 'rejected' WHEN msg.peer_received=1 THEN 'peer-received' WHEN o.state='relay-accepted' THEN 'relay-accepted' WHEN o.event_id IS NOT NULL THEN 'queued' ELSE 'local' END, msg.audience_account_id, msg.directory, msg.git_common_dir, msg.remote_identity, msg.worktree, msg.branch, msg.sender_installation_id, msg.recipient_installation_id, msg.sender_mailbox_id, msg.recipient_mailbox_id, COALESCE(NULLIF(msg.actor_label,''),CASE WHEN sm.kind='human' THEN 'human' WHEN sm.kind='agent' THEN COALESCE(sn.name,NULLIF(sn.current_harness,''),(SELECT b.harness FROM harness_bindings b WHERE b.mailbox_id=sm.id ORDER BY b.created_at DESC,b.harness,b.external_session_id LIMIT 1))||':'||substr(sm.id,-8) WHEN sm.kind='project' THEN COALESCE(NULLIF(sm.label,''),'project:'||substr(sm.id,-8)) ELSE 'remote:'||substr(msg.sender_mailbox_id,-8) END), COALESCE(sm.kind,'remote'), COALESCE(sn.current_harness,(SELECT b.harness FROM harness_bindings b WHERE b.mailbox_id=sm.id ORDER BY b.created_at DESC,b.harness,b.external_session_id LIMIT 1),''), COALESCE(sn.name,''), COALESCE(hd.label,''), CASE WHEN rm.kind='human' THEN 'human' WHEN rm.kind='agent' THEN COALESCE(rn.name,NULLIF(rn.current_harness,''),(SELECT b.harness FROM harness_bindings b WHERE b.mailbox_id=rm.id ORDER BY b.created_at DESC,b.harness,b.external_session_id LIMIT 1))||CASE WHEN rn.name IS NULL THEN ':'||substr(rm.id,-8) ELSE '' END WHEN rm.kind='project' THEN COALESCE(NULLIF(rm.label,''),'project:'||substr(rm.id,-8)) ELSE 'remote:'||substr(msg.recipient_mailbox_id,-8) END, COALESCE(rm.kind,'remote'), COALESCE(rn.current_harness,(SELECT b.harness FROM harness_bindings b WHERE b.mailbox_id=rm.id ORDER BY b.created_at DESC,b.harness,b.external_session_id LIMIT 1),''), COALESCE(rn.name,''), msg.body, msg.details, msg.reply_to, msg.created_at, msg.archived_at, d.completed_at`
const joins = ` messages msg LEFT JOIN mailboxes sm ON sm.id=msg.sender_mailbox_id AND sm.installation_id=msg.sender_installation_id LEFT JOIN named_agents sn ON sn.mailbox_id=sm.id LEFT JOIN mailboxes rm ON rm.id=msg.recipient_mailbox_id AND rm.installation_id=msg.recipient_installation_id LEFT JOIN named_agents rn ON rn.mailbox_id=rm.id LEFT JOIN human_account_devices hd ON hd.account_id=msg.audience_account_id AND hd.installation_id=msg.sender_installation_id LEFT JOIN delivery_facts d ON d.message_id=msg.id LEFT JOIN (SELECT event_id,CASE WHEN SUM(CASE WHEN state='queued' THEN 1 ELSE 0 END)>0 THEN 'queued' WHEN SUM(CASE WHEN state='relay-accepted' THEN 1 ELSE 0 END)>0 THEN 'relay-accepted' WHEN SUM(CASE WHEN state='rejected' THEN 1 ELSE 0 END)>0 THEN 'rejected' ELSE '' END AS state FROM outbox GROUP BY event_id) o ON o.event_id=msg.event_id `

type scanner interface{ Scan(...any) error }

func scanMessage(row scanner) (model.Message, error) {
	var m model.Message
	var senderKind, senderHarness, senderName, recipientKind, recipientHarness, recipientName string
	var reply sql.NullString
	var created int64
	var archived, completed sql.NullInt64
	var correlationItemID, correlationRequestID, technicalJSON string
	err := row.Scan(&m.ID, &m.EventID, &m.ThreadID, &m.Purpose, &m.Presentation, &m.HarnessProvider, &m.HarnessSessionID, &m.HarnessOperationID, &correlationItemID, &correlationRequestID, &technicalJSON, &m.Incomplete, &m.PeerReceived, &m.Rejected, &m.DeliveryState, &m.AudienceAccountID, &m.Context.Directory, &m.Context.GitCommonDir, &m.Context.RemoteIdentity, &m.Context.Worktree, &m.Context.Branch, &m.SenderInstallationID, &m.RecipientInstallationID, &m.SenderMailboxID, &m.RecipientMailboxID, &m.SenderLabel, &senderKind, &senderHarness, &senderName, &m.SourceDeviceLabel, &m.RecipientLabel, &recipientKind, &recipientHarness, &recipientName, &m.Body, &m.Details, &reply, &created, &archived, &completed)
	if errors.Is(err, sql.ErrNoRows) {
		return m, ErrNotFound
	}
	if err != nil {
		return m, err
	}
	m.Purpose = model.NormalizeMessagePurpose(m.Purpose)
	m.Correlation = model.MessageCorrelation{Provider: m.HarnessProvider, SessionID: m.HarnessSessionID, OperationID: m.HarnessOperationID, ItemID: correlationItemID, RequestID: correlationRequestID}
	if err := json.Unmarshal([]byte(technicalJSON), &m.TechnicalSections); err != nil {
		return m, fmt.Errorf("decode message technical sections: %w", err)
	}
	m.SenderAddress = messageAddress(m.SenderInstallationID, m.SenderMailboxID, model.MailboxKind(senderKind), m.SenderLabel, senderHarness, senderName)
	m.RecipientAddress = messageAddress(m.RecipientInstallationID, m.RecipientMailboxID, model.MailboxKind(recipientKind), m.RecipientLabel, recipientHarness, recipientName)
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

func messageAddress(installationID, mailboxID string, kind model.MailboxKind, fallbackLabel, harness, name string) model.MessageAddress {
	address := model.MessageAddress{InstallationID: installationID, MailboxID: mailboxID, Kind: kind, Label: fallbackLabel, Harness: harness, Name: name}
	switch kind {
	case model.MailboxHuman:
		address.Label = "human"
	case model.MailboxAgent:
		if name != "" {
			address.Label = name
		} else if harness != "" {
			address.Label = harness
		} else {
			address.Label = "agent"
		}
	case model.MailboxProject:
		if address.Label == "" {
			address.Label = "project"
		}
	case model.MailboxRemote:
		if address.Label == "" {
			address.Label = "remote"
		}
	default:
		address.Kind, address.Label = model.MailboxRemote, "remote"
	}
	return address
}

func (s *SQLite) Get(ctx context.Context, id string) (model.Message, error) {
	return getMessageWith(ctx, s.queryer(ctx), id)
}

func getMessageWith(ctx context.Context, queryer rowQueryer, id string) (model.Message, error) {
	m, err := scanMessage(queryer.QueryRowContext(ctx, `SELECT `+columns+` FROM `+joins+` WHERE msg.id = ?`, id))
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
	if f.CounterpartyMailboxID != "" {
		where = append(where, "((msg.sender_mailbox_id = ? AND msg.recipient_mailbox_id = ?) OR (msg.sender_mailbox_id = ? AND msg.recipient_mailbox_id = ?))")
		args = append(args, f.CounterpartyMailboxID, model.HumanMailboxID, model.HumanMailboxID, f.CounterpartyMailboxID)
	}
	if f.ThreadID != "" {
		add("msg.thread_event_id = ?", f.ThreadID)
	}
	if f.HarnessProvider != "" {
		add("msg.harness_provider = ?", f.HarnessProvider)
	}
	if f.HarnessSessionID != "" {
		add("msg.harness_session_id = ?", f.HarnessSessionID)
	}
	if f.HarnessOperationID != "" {
		add("msg.harness_operation_id = ?", f.HarnessOperationID)
	}
	if f.ReplyTo != "" {
		add("msg.reply_to = ?", f.ReplyTo)
	}
	if f.Purpose != "" {
		add("msg.purpose = ?", f.Purpose)
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

type conversationCursor struct {
	Activity int64  `json:"activity"`
	SortKey  string `json:"sort_key"`
}

type historyCursor struct {
	CreatedAt int64  `json:"created_at"`
	ID        string `json:"id"`
}

func encodePageCursor(value any) (string, error) {
	raw, err := json.Marshal(value)
	if err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(raw), nil
}

func decodePageCursor(raw string, value any) error {
	decoded, err := base64.RawURLEncoding.DecodeString(raw)
	if err != nil {
		return fmt.Errorf("decode cursor: %w", err)
	}
	if err := json.Unmarshal(decoded, value); err != nil {
		return fmt.Errorf("decode cursor: %w", err)
	}
	return nil
}

func pageLimit(limit int) int {
	if limit <= 0 {
		return 50
	}
	return min(limit, 200)
}

func (s *SQLite) ListConversations(ctx context.Context, filter model.ConversationFilter) (model.ConversationPage, error) {
	limit := pageLimit(filter.Limit)
	queryArgs := []any{
		model.HumanMailboxID, model.HumanMailboxID, model.HumanMailboxID,
		model.HumanMailboxID, model.HumanMailboxID, model.HumanMailboxID,
		model.HumanMailboxID, boolInt(filter.IncludeSent), model.HumanMailboxID,
		boolInt(filter.IncludeArchived), model.HumanMailboxID, model.HumanMailboxID,
	}
	cursorClause := ""
	if filter.Cursor != "" {
		var cursor conversationCursor
		if err := decodePageCursor(filter.Cursor, &cursor); err != nil || cursor.Activity <= 0 || cursor.SortKey == "" {
			if err == nil {
				err = errors.New("cursor is incomplete")
			}
			return model.ConversationPage{}, fmt.Errorf("list conversations: %w", err)
		}
		cursorClause = `WHERE (last_activity < ? OR (last_activity = ? AND sort_key > ?))`
		queryArgs = append(queryArgs, cursor.Activity, cursor.Activity, cursor.SortKey)
	}
	queryArgs = append(queryArgs, limit+1)
	query := `WITH identified AS (
 SELECT id,created_at,sender_mailbox_id,recipient_mailbox_id,archived_at,
  CASE WHEN sender_mailbox_id=? THEN recipient_mailbox_id ELSE sender_mailbox_id END AS counterparty,
  CASE WHEN harness_session_id<>'' THEN 'harness' ELSE 'thread' END AS conversation_kind,
  CASE WHEN harness_session_id<>'' THEN harness_provider ELSE '' END AS conversation_provider,
  CASE WHEN harness_session_id<>'' THEN harness_session_id ELSE thread_event_id END AS conversation_id
 FROM messages
 WHERE (sender_mailbox_id=? OR recipient_mailbox_id=?) AND sender_mailbox_id<>recipient_mailbox_id
), eligible AS (
 SELECT counterparty,conversation_kind,conversation_provider,conversation_id,
  counterparty||char(31)||conversation_kind||char(31)||conversation_provider||char(31)||conversation_id AS sort_key,
  MAX(created_at) AS last_activity,
  SUM(CASE WHEN recipient_mailbox_id=? AND archived_at IS NULL THEN 1 ELSE 0 END) AS open_count,
  MAX(CASE WHEN sender_mailbox_id=? THEN 1 ELSE 0 END) AS has_sent,
  MAX(CASE WHEN recipient_mailbox_id=? AND archived_at IS NOT NULL THEN 1 ELSE 0 END) AS has_archived
 FROM identified
 GROUP BY counterparty,conversation_kind,conversation_provider,conversation_id
 HAVING SUM(CASE WHEN recipient_mailbox_id=? AND archived_at IS NULL THEN 1 ELSE 0 END)>0
   OR (?=1 AND MAX(CASE WHEN sender_mailbox_id=? THEN 1 ELSE 0 END)>0)
   OR (?=1 AND MAX(CASE WHEN recipient_mailbox_id=? AND archived_at IS NOT NULL THEN 1 ELSE 0 END)>0)
)
SELECT counterparty,conversation_kind,conversation_provider,conversation_id,sort_key,last_activity,open_count,has_sent,has_archived,
 (SELECT i.id FROM identified i WHERE i.counterparty=e.counterparty AND i.conversation_kind=e.conversation_kind AND i.conversation_provider=e.conversation_provider AND i.conversation_id=e.conversation_id ORDER BY i.created_at DESC,i.id DESC LIMIT 1),
 (SELECT i.id FROM identified i WHERE i.counterparty=e.counterparty AND i.conversation_kind=e.conversation_kind AND i.conversation_provider=e.conversation_provider AND i.conversation_id=e.conversation_id AND i.recipient_mailbox_id=? AND i.archived_at IS NULL ORDER BY i.created_at,i.id LIMIT 1)
FROM eligible e ` + cursorClause + ` ORDER BY last_activity DESC,sort_key LIMIT ?`
	rows, err := s.db.QueryContext(ctx, query, queryArgs...)
	if err != nil {
		return model.ConversationPage{}, fmt.Errorf("list conversations: %w", err)
	}
	defer rows.Close()
	type summaryRow struct {
		summary model.ConversationSummary
		sortKey string
		latest  string
		open    sql.NullString
		at      int64
	}
	var summaries []summaryRow
	for rows.Next() {
		var item summaryRow
		var kind, provider, identifier string
		var hasSent, hasArchived int
		if err := rows.Scan(&item.summary.Key.CounterpartyMailboxID, &kind, &provider, &identifier, &item.sortKey, &item.at, &item.summary.OpenCount, &hasSent, &hasArchived, &item.latest, &item.open); err != nil {
			return model.ConversationPage{}, fmt.Errorf("list conversations: %w", err)
		}
		if kind == "harness" {
			item.summary.Key.HarnessProvider = provider
			item.summary.Key.HarnessSessionID = identifier
		} else {
			item.summary.Key.ThreadID = identifier
		}
		item.summary.HasSent = hasSent != 0
		item.summary.HasArchived = hasArchived != 0
		item.summary.LastActivity = time.UnixMilli(item.at).UTC()
		summaries = append(summaries, item)
	}
	if err := rows.Err(); err != nil {
		return model.ConversationPage{}, fmt.Errorf("list conversations: %w", err)
	}
	page := model.ConversationPage{}
	if len(summaries) > limit {
		last := summaries[limit-1]
		page.NextCursor, err = encodePageCursor(conversationCursor{Activity: last.at, SortKey: last.sortKey})
		if err != nil {
			return model.ConversationPage{}, fmt.Errorf("list conversations: %w", err)
		}
		summaries = summaries[:limit]
	}
	page.Conversations = make([]model.ConversationSummary, 0, len(summaries))
	for _, item := range summaries {
		item.summary.Latest, err = s.Get(ctx, item.latest)
		if err != nil {
			return model.ConversationPage{}, fmt.Errorf("list conversations: latest message: %w", err)
		}
		if item.open.Valid {
			openMessage, getErr := s.Get(ctx, item.open.String)
			if getErr != nil {
				return model.ConversationPage{}, fmt.Errorf("list conversations: oldest open message: %w", getErr)
			}
			item.summary.OldestOpen = &openMessage
		}
		page.Conversations = append(page.Conversations, item.summary)
	}
	return page, nil
}

func (s *SQLite) ListConversationHistory(ctx context.Context, filter model.ConversationHistoryFilter) (model.MessagePage, error) {
	if !filter.Key.Valid() {
		return model.MessagePage{}, errors.New("list conversation history: invalid conversation key")
	}
	limit := pageLimit(filter.Limit)
	where := []string{`((msg.sender_mailbox_id=? AND msg.recipient_mailbox_id=?) OR (msg.sender_mailbox_id=? AND msg.recipient_mailbox_id=?))`}
	args := []any{filter.Key.CounterpartyMailboxID, model.HumanMailboxID, model.HumanMailboxID, filter.Key.CounterpartyMailboxID}
	if filter.Key.HarnessSessionID != "" {
		where = append(where, `msg.harness_provider=?`, `msg.harness_session_id=?`)
		args = append(args, filter.Key.HarnessProvider, filter.Key.HarnessSessionID)
	} else {
		where = append(where, `msg.harness_session_id=''`, `msg.thread_event_id=?`)
		args = append(args, filter.Key.ThreadID)
	}
	if filter.Cursor != "" {
		var cursor historyCursor
		if err := decodePageCursor(filter.Cursor, &cursor); err != nil || cursor.CreatedAt <= 0 || cursor.ID == "" {
			if err == nil {
				err = errors.New("cursor is incomplete")
			}
			return model.MessagePage{}, fmt.Errorf("list conversation history: %w", err)
		}
		where = append(where, `(msg.created_at>? OR (msg.created_at=? AND msg.id>?))`)
		args = append(args, cursor.CreatedAt, cursor.CreatedAt, cursor.ID)
	}
	args = append(args, limit+1)
	rows, err := s.db.QueryContext(ctx, `SELECT `+columns+` FROM `+joins+` WHERE `+strings.Join(where, " AND ")+` ORDER BY msg.created_at,msg.id LIMIT ?`, args...)
	if err != nil {
		return model.MessagePage{}, fmt.Errorf("list conversation history: %w", err)
	}
	defer rows.Close()
	var messages []model.Message
	for rows.Next() {
		message, scanErr := scanMessage(rows)
		if scanErr != nil {
			return model.MessagePage{}, scanErr
		}
		messages = append(messages, message)
	}
	if err := rows.Err(); err != nil {
		return model.MessagePage{}, err
	}
	page := model.MessagePage{}
	if len(messages) > limit {
		last := messages[limit-1]
		page.NextCursor, err = encodePageCursor(historyCursor{CreatedAt: last.CreatedAt.UnixMilli(), ID: last.ID})
		if err != nil {
			return model.MessagePage{}, fmt.Errorf("list conversation history: %w", err)
		}
		messages = messages[:limit]
	}
	page.Messages = messages
	return page, nil
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
	stateParents, err := s.messageStateParents(ctx, record.eventID)
	if err != nil {
		return err
	}
	parents := uniqueSorted(append([]string{record.eventID}, stateParents...))
	scope := event.ScopeInstallationPrivate
	var audience *event.Audience
	var authorities []string
	if record.message.AudienceAccountID != "" {
		account, membership, _, err := s.localAccountAction(ctx, record.message.AudienceAccountID)
		if err != nil {
			return err
		}
		parents = uniqueSorted(append(parents, membership...))
		authorities = uniqueSorted(membership)
		audience, scope = &event.Audience{HumanAccountID: account.ID}, event.ScopeAccountAddressed
	}
	content := event.Content{Type: event.TypeMessageArchive, Sender: s.localAddress(model.HumanMailboxID), Audience: audience, Parents: parents, Authorities: authorities, Scope: scope, Payload: payload}
	return s.appendContents(ctx, []event.Content{content}, nil, nil)
}

func (s *SQLite) Restore(ctx context.Context, id string) error {
	record, err := s.messageRecord(ctx, id)
	if err != nil {
		return err
	}
	if record.message.ArchivedAt == nil || record.message.Rejected || record.message.RecipientMailboxID != model.HumanMailboxID {
		return ErrAlreadyHandled
	}
	stateParents, err := s.messageStateParents(ctx, record.eventID)
	if err != nil {
		return err
	}
	parents := uniqueSorted(append([]string{record.eventID}, stateParents...))
	payload, _ := event.MarshalPayload(event.TargetPayload{TargetEventID: record.eventID})
	scope := event.ScopeInstallationPrivate
	var audience *event.Audience
	var authorities []string
	if record.message.AudienceAccountID != "" {
		account, membership, _, err := s.localAccountAction(ctx, record.message.AudienceAccountID)
		if err != nil {
			return err
		}
		parents = uniqueSorted(append(parents, membership...))
		authorities = uniqueSorted(membership)
		audience, scope = &event.Audience{HumanAccountID: account.ID}, event.ScopeAccountAddressed
	}
	content := event.Content{Type: event.TypeMessageRestore, Sender: s.localAddress(model.HumanMailboxID), Audience: audience, Parents: parents, Authorities: authorities, Scope: scope, Payload: payload}
	return s.appendContents(ctx, []event.Content{content}, nil, nil)
}

func (s *SQLite) messageStateParents(ctx context.Context, targetEventID string) ([]string, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT raw FROM canonical_events WHERE reduction_status = ?`, event.StatusProjected)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	parents := make(map[string][]string)
	var candidates []string
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			return nil, err
		}
		inspection := event.Inspect(raw)
		if inspection.Status != event.StatusProjected {
			continue
		}
		item := inspection.Event
		parents[item.ID()] = item.Content.Parents
		if item.Content.Type != event.TypeMessageArchive && item.Content.Type != event.TypeMessageRestore && item.Content.Type != event.TypeMessageReject {
			continue
		}
		var payload event.TargetPayload
		if json.Unmarshal(item.Content.Payload, &payload) == nil && payload.TargetEventID == targetEventID {
			candidates = append(candidates, item.ID())
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	isAncestor := func(ancestor, descendant string) bool {
		seen := make(map[string]bool)
		stack := append([]string(nil), parents[descendant]...)
		for len(stack) > 0 {
			id := stack[len(stack)-1]
			stack = stack[:len(stack)-1]
			if id == ancestor {
				return true
			}
			if seen[id] {
				continue
			}
			seen[id] = true
			stack = append(stack, parents[id]...)
		}
		return false
	}
	var maximal []string
	for _, candidate := range candidates {
		shadowed := false
		for _, other := range candidates {
			if candidate != other && isAncestor(candidate, other) {
				shadowed = true
				break
			}
		}
		if !shadowed {
			maximal = append(maximal, candidate)
		}
	}
	return uniqueSorted(maximal), nil
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
	if len(claim.ExcludeReplyTo) > 0 {
		placeholders := make([]string, len(claim.ExcludeReplyTo))
		for index, replyTo := range claim.ExcludeReplyTo {
			placeholders[index] = "?"
			args = append(args, replyTo)
		}
		where = append(where, "(m.reply_to IS NULL OR m.reply_to NOT IN ("+strings.Join(placeholders, ",")+"))")
	}
	if claim.RecipientMailboxID != "" {
		where, args = append(where, "m.recipient_mailbox_id = ?"), append(args, claim.RecipientMailboxID)
	}
	if claim.Purpose != "" {
		where, args = append(where, "m.purpose = ?"), append(args, claim.Purpose)
	}
	if claim.CorrelationSessionID != "" {
		where = append(where, `(m.reply_to IS NULL OR (m.harness_provider=? AND m.harness_session_id=?))`)
		args = append(args, claim.CorrelationProvider, claim.CorrelationSessionID)
	}
	if claim.UnthreadedOnly {
		where = append(where, "m.reply_to IS NULL")
	}
	where = append(where, "d.completed_at IS NULL", "(d.delivery_token IS NULL OR d.delivery_lease_until < ?)")
	where = append(where, "NOT EXISTS (SELECT 1 FROM projects p WHERE p.mailbox_id=m.recipient_mailbox_id)")
	args = append(args, now.UnixMilli())
	query := `UPDATE delivery_facts SET delivery_token=?, delivery_lease_until=? WHERE message_id=(SELECT m.id FROM messages m JOIN delivery_facts d ON d.message_id=m.id WHERE ` + strings.Join(where, " AND ") + ` ORDER BY m.created_at,m.id LIMIT 1) RETURNING message_id`
	queryArgs := append([]any{token, now.Add(30 * time.Second).UnixMilli()}, args...)
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return model.Message{}, err
	}
	defer tx.Rollback()
	var id string
	if err := tx.QueryRowContext(ctx, query, queryArgs...).Scan(&id); errors.Is(err, sql.ErrNoRows) {
		return model.Message{}, ErrNotReady
	} else if err != nil {
		return model.Message{}, err
	}
	message, err := getMessageWith(ctx, tx, id)
	if err != nil {
		return model.Message{}, err
	}
	if err := recordMutationTx(ctx, tx, message); err != nil {
		return model.Message{}, err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicMessages})
	if err != nil {
		return model.Message{}, err
	}
	if err := tx.Commit(); err != nil {
		return model.Message{}, err
	}
	s.notifyChange(change)
	return message, nil
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
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `UPDATE delivery_facts SET delivery_token=NULL,delivery_lease_until=NULL WHERE message_id=? AND delivery_token=? AND completed_at IS NULL`, id, token); err != nil {
		return err
	}
	if err := recordMutationTx(ctx, tx, nil); err != nil {
		return err
	}
	change, err := recordChangeTx(ctx, tx, []domain.ChangeTopic{domain.TopicMessages})
	if err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyChange(change)
	return nil
}

func (s *SQLite) Rebuild(ctx context.Context) error {
	for attempt := 0; ; attempt++ {
		err := s.rebuildOnce(ctx)
		if err == nil || !sqliteBusy(err) || attempt == 7 {
			return err
		}
		timer := time.NewTimer(time.Duration(attempt+1) * 10 * time.Millisecond)
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-timer.C:
		}
	}
}

func (s *SQLite) rebuildOnce(ctx context.Context) error {
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
	if _, err := s.reconcileProjectInputsTx(ctx, tx, false); err != nil {
		return err
	}
	return tx.Commit()
}

func sqliteBusy(err error) bool {
	message := strings.ToLower(err.Error())
	return strings.Contains(message, "locked") || strings.Contains(message, "busy")
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
	contents := []event.Content{{Type: event.TypePeerBindingSet, Parents: parents, Scope: event.ScopeInstallationPrivate, Payload: payload}}
	access, err := s.mailboxAccessContents(ctx, model.HumanMailboxID, peer.InstallationID, peer.SignerKeyID, true)
	if err != nil {
		return err
	}
	contents = append(contents, access...)
	return s.appendContents(ctx, contents, nil, nil)
}

func (s *SQLite) DistrustPeer(ctx context.Context, installationID string) error {
	state, err := s.reduceCanonicalResources(ctx, canonicalResource{kind: "installation", id: installationID})
	if err != nil {
		return err
	}
	peer, ok := state.Peers[installationID]
	if !ok {
		return errors.New("peer binding not found")
	}
	var contents []event.Content
	for _, access := range state.MailboxAccess {
		if access.GrantorInstallationID != s.signer.InstallationID || access.GranteeInstallationID != installationID || !access.Active {
			continue
		}
		revokes, revokeErr := s.mailboxAccessContents(ctx, access.MailboxID, installationID, peer.SignerKeyID, false)
		if revokeErr != nil {
			return revokeErr
		}
		contents = append(contents, revokes...)
	}
	payload, _ := event.MarshalPayload(event.PeerPayload{InstallationID: installationID})
	contents = append(contents, event.Content{Type: event.TypePeerBindingBlock, Parents: s.peerParents(ctx, installationID), Scope: event.ScopeInstallationPrivate, Payload: payload})
	return s.appendContents(ctx, contents, nil, nil)
}

func (s *SQLite) peerParents(ctx context.Context, installationID string) []string {
	rows, err := s.db.QueryContext(ctx, `SELECT event_id,raw FROM canonical_events WHERE event_type IN ('peer.binding.set','peer.binding.block')`)
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
	var peerSignerKeyID string
	if err := s.db.QueryRowContext(ctx, `SELECT signer_key_id FROM peers WHERE installation_id=?`, peerID).Scan(&peerSignerKeyID); err != nil || peerSignerKeyID == "" {
		return errors.New("peer binding not found")
	}
	contents, err := s.mailboxAccessContents(ctx, mailboxID, peerID, peerSignerKeyID, active)
	if err != nil {
		return err
	}
	if len(contents) == 0 {
		return nil
	}
	return s.appendContents(ctx, contents, nil, nil)
}

func (s *SQLite) mailboxAccessContents(ctx context.Context, mailboxID, peerID, peerSignerKeyID string, active bool) ([]event.Content, error) {
	state, err := s.reduceCanonicalResources(ctx, canonicalResource{kind: "mailbox-access-route", id: s.signer.InstallationID + ":" + mailboxID + ":" + peerID})
	if err != nil {
		return nil, err
	}
	var matching []event.MailboxAccessProjection
	for _, access := range state.MailboxAccess {
		if access.GrantorInstallationID == s.signer.InstallationID && access.MailboxID == mailboxID && access.GranteeInstallationID == peerID && access.GranteeSignerKeyID == peerSignerKeyID && access.Active {
			matching = append(matching, access)
		}
	}
	if active && len(matching) != 0 || !active && len(matching) == 0 {
		return nil, nil
	}
	payload, err := event.MarshalPayload(event.MailboxAccessPayload{MailboxID: mailboxID, GranteeInstallationID: peerID, GranteeSignerKeyID: peerSignerKeyID})
	if err != nil {
		return nil, err
	}
	route := event.Content{
		InstallationID: s.signer.InstallationID,
		Sender:         s.localAddress(model.HumanMailboxID),
		Recipient:      &event.MailboxAddress{InstallationID: peerID, MailboxID: model.HumanMailboxID},
		Scope:          event.ScopePeerAddressed, Payload: payload,
	}
	if active {
		route.Type = event.TypeMailboxAccessGrant
		return []event.Content{route}, nil
	}
	for _, access := range matching {
		route.Type = event.TypeMailboxAccessRevoke
		route.Authorities = []string{access.GrantEventID}
		route.Parents = append([]string{access.GrantEventID}, access.ObservationEventIDs...)
		route.Parents = uniqueSorted(route.Parents)
		if len(route.Parents) > event.MaxParents {
			return nil, errors.New("mailbox access observation frontier exceeds the causal parent limit")
		}
		return []event.Content{route}, nil
	}
	return nil, nil
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

var _ domain.ProjectRuntimeStore = (*SQLite)(nil)
