//! Private SQLite schema, row codecs, and transactions.

mod agent;
mod authority;
mod conversation;
mod harness;
mod operational;
mod project;
mod relay;
mod repair;

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    time::Duration,
};

use hq_application::ConversationSummary;
use hq_domain::{
    AuthorityRole, FactId, FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, Page,
    PageCursor,
};
use hq_protocol::{
    DispatchOutcome, MAX_EVENT_BYTES, ProtocolNamespace, RawEventBytes, VerifiedSemanticFact,
};
use hq_reducer::{
    AuthorityPolicy, AuthorityProjection, AuthorityProjectionKey, ConversationKey,
    ConversationProjection, DecisionStatus, MembershipState,
};
use rusqlite::{
    Connection, Error as SqlError, ErrorCode, OpenFlags, OptionalExtension, Transaction,
    TransactionBehavior, config::DbConfig, params,
};

use crate::{
    AgentProjectionSnapshot, AuthoritativeSnapshot, AuthorityProjectionSnapshot, CompleteSnapshot,
    ConversationEntry, ConversationProjectionSnapshot, DomainSnapshot, IngestOutcome,
    LocalMutationRequest, MutationReceipt, MutationResultKind, OutboxIntent,
    ProjectProjectionSnapshot, ReductionIndexSnapshot, RepairOutcome, StoreError, StoreErrorClass,
    operational::LocalMutationDecisionParts,
    paths::{prepare_database_path, validate_database_path},
    snapshot::build_complete_snapshot,
};

const APPLICATION_ID: i64 = 0x4851_5253;
const SCHEMA_VERSION: i64 = 13;
const SCHEMA_MARKER: &str = "hq-store-v13-harness-supervision-2026-08-27";
const SCHEMA_TABLES: [&str; 114] = [
    "storage_metadata",
    "canonical_facts",
    "fact_parents",
    "fact_authorities",
    "reduction_state",
    "reduction_vertices",
    "reduction_reverse_dependencies",
    "reduction_affected_dependencies",
    "reduction_decisions",
    "reduction_missing_dependencies",
    "reduction_unusable_dependencies",
    "reduction_failed_authorities",
    "reduction_decision_participants",
    "reduction_dependency_order",
    "reduction_presentation_order",
    "reduction_conversation_keys",
    "reduction_conversation_order",
    "reduction_conflicts",
    "reduction_conflict_participants",
    "authority_state",
    "authority_frontiers",
    "authority_support",
    "authority_installations",
    "authority_mailboxes",
    "authority_peer_routes",
    "authority_peer_route_facts",
    "authority_peer_route_candidates",
    "authority_peer_route_relays",
    "authority_capabilities",
    "authority_capability_facts",
    "authority_accounts",
    "authority_memberships",
    "authority_membership_facts",
    "authority_membership_grants",
    "authority_membership_grant_relays",
    "authority_account_selections",
    "authority_account_selection_candidates",
    "conversation_state",
    "conversation_aggregate_keys",
    "conversation_frontiers",
    "conversation_projection_keys",
    "conversation_support",
    "conversation_threads",
    "conversation_thread_answers",
    "conversation_thread_cancellations",
    "conversation_thread_relations",
    "conversation_thread_ready_answers",
    "conversation_messages",
    "conversation_message_frontiers",
    "conversation_message_receipts",
    "conversation_action_groups",
    "conversation_action_entries",
    "conversation_activities",
    "conversation_activity_retentions",
    "conversation_retained_progress",
    "agent_state",
    "agent_aggregate_keys",
    "agent_frontiers",
    "agent_projection_keys",
    "agent_support",
    "agent_names",
    "agent_name_claims",
    "agent_agents",
    "agent_agent_claims",
    "agent_agent_names",
    "agent_agent_mailboxes",
    "agent_agent_retirements",
    "agent_sessions",
    "agent_session_bindings",
    "agent_contexts",
    "agent_context_history",
    "agent_context_frontiers",
    "agent_selections",
    "agent_selection_candidates",
    "agent_selection_frontiers",
    "agent_renames",
    "agent_rename_candidates",
    "agent_rename_frontiers",
    "agent_direct_sessions",
    "agent_direct_binding_facts",
    "project_state",
    "project_aggregate_keys",
    "project_frontiers",
    "project_projection_keys",
    "project_support",
    "project_projects",
    "project_fork_participants",
    "project_resources",
    "project_active_claims",
    "project_claim_conflicts",
    "project_assignments",
    "project_assignment_support",
    "project_inputs",
    "project_dispatches",
    "project_outputs",
    "project_output_facts",
    "project_commands",
    "project_command_support",
    "mutation_receipts",
    "change_revision",
    "outbox_intents",
    "canonical_commits",
    "relay_policy_operations",
    "relay_policies",
    "prepared_relay_outbox",
    "relay_attempts",
    "relay_cursors",
    "inbound_relay_claims",
    "relay_staging",
    "relay_quarantine",
    "harness_worker_leases",
    "harness_ready_sessions",
    "harness_deliveries",
    "harness_event_checkpoints",
];
const OPERATIONAL_TABLE_COUNT: usize = 16;
const SCHEMA_INDEXES: [&str; 2] = [
    "conversation_messages_by_fact_id",
    "conversation_activities_by_fact_id",
];
const MAXIMUM_CORPUS_FACTS: i64 = 1_000_000;

const SCHEMA: &str = r"
CREATE TABLE storage_metadata (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    schema_marker TEXT NOT NULL CHECK(typeof(schema_marker) = 'text')
) STRICT;

CREATE TABLE canonical_facts (
    fact_id BLOB PRIMARY KEY NOT NULL CHECK(typeof(fact_id) = 'blob' AND length(fact_id) = 32),
    event_bytes BLOB NOT NULL CHECK(typeof(event_bytes) = 'blob'),
    namespace INTEGER NOT NULL CHECK(namespace IN (1, 2)),
    family INTEGER NOT NULL CHECK(family BETWEEN 1 AND 48)
) STRICT, WITHOUT ROWID;

CREATE TABLE fact_parents (
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    parent_id BLOB NOT NULL CHECK(typeof(parent_id) = 'blob' AND length(parent_id) = 32),
    PRIMARY KEY (fact_id, parent_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE fact_authorities (
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    authority_role INTEGER NOT NULL CHECK(authority_role BETWEEN 1 AND 13),
    authority_fact_id BLOB NOT NULL
        CHECK(typeof(authority_fact_id) = 'blob' AND length(authority_fact_id) = 32),
    PRIMARY KEY (fact_id, authority_role)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    policy_installation BLOB NOT NULL
        CHECK(typeof(policy_installation) = 'blob' AND length(policy_installation) = 32),
    policy_human_mailbox BLOB NOT NULL
        CHECK(typeof(policy_human_mailbox) = 'blob' AND length(policy_human_mailbox) = 32),
    fact_count INTEGER NOT NULL CHECK(fact_count >= 0),
    vertex_count INTEGER NOT NULL CHECK(vertex_count >= 0),
    reverse_count INTEGER NOT NULL CHECK(reverse_count >= 0),
    affected_count INTEGER NOT NULL CHECK(affected_count >= 0),
    decision_count INTEGER NOT NULL CHECK(decision_count >= 0),
    missing_count INTEGER NOT NULL CHECK(missing_count >= 0),
    unusable_count INTEGER NOT NULL CHECK(unusable_count >= 0),
    failed_authority_count INTEGER NOT NULL CHECK(failed_authority_count >= 0),
    decision_participant_count INTEGER NOT NULL CHECK(decision_participant_count >= 0),
    dependency_order_count INTEGER NOT NULL CHECK(dependency_order_count >= 0),
    presentation_order_count INTEGER NOT NULL CHECK(presentation_order_count >= 0),
    conversation_key_count INTEGER NOT NULL CHECK(conversation_key_count >= 0),
    conversation_order_count INTEGER NOT NULL CHECK(conversation_order_count >= 0),
    conflict_count INTEGER NOT NULL CHECK(conflict_count >= 0),
    conflict_participant_count INTEGER NOT NULL CHECK(conflict_participant_count >= 0),
    index_digest BLOB NOT NULL CHECK(typeof(index_digest) = 'blob' AND length(index_digest) = 32)
) STRICT;

CREATE TABLE reduction_vertices (
    fact_id BLOB PRIMARY KEY NOT NULL CHECK(typeof(fact_id) = 'blob' AND length(fact_id) = 32)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_reverse_dependencies (
    parent_id BLOB NOT NULL REFERENCES reduction_vertices(fact_id) ON DELETE RESTRICT,
    child_id BLOB NOT NULL REFERENCES reduction_vertices(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (parent_id, child_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_affected_dependencies (
    source_id BLOB NOT NULL REFERENCES reduction_vertices(fact_id) ON DELETE RESTRICT,
    affected_id BLOB NOT NULL REFERENCES reduction_vertices(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (source_id, affected_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_decisions (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    status INTEGER NOT NULL CHECK(status BETWEEN 1 AND 6),
    reason_code INTEGER NOT NULL CHECK(reason_code >= 0),
    reason_parameter INTEGER NOT NULL CHECK(reason_parameter >= 0),
    PRIMARY KEY (domain, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_missing_dependencies (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    dependency_id BLOB NOT NULL
        CHECK(typeof(dependency_id) = 'blob' AND length(dependency_id) = 32),
    PRIMARY KEY (domain, fact_id, dependency_id),
    FOREIGN KEY (domain, fact_id) REFERENCES reduction_decisions(domain, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_unusable_dependencies (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    dependency_id BLOB NOT NULL
        CHECK(typeof(dependency_id) = 'blob' AND length(dependency_id) = 32),
    dependency_status INTEGER NOT NULL CHECK(dependency_status BETWEEN 1 AND 6),
    PRIMARY KEY (domain, fact_id, dependency_id),
    FOREIGN KEY (domain, fact_id) REFERENCES reduction_decisions(domain, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_failed_authorities (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    authority_role INTEGER NOT NULL CHECK(authority_role BETWEEN 1 AND 13),
    PRIMARY KEY (domain, fact_id, authority_role),
    FOREIGN KEY (domain, fact_id) REFERENCES reduction_decisions(domain, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_decision_participants (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    participant_id BLOB NOT NULL
        CHECK(typeof(participant_id) = 'blob' AND length(participant_id) = 32),
    PRIMARY KEY (domain, fact_id, participant_id),
    FOREIGN KEY (domain, fact_id) REFERENCES reduction_decisions(domain, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_dependency_order (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    position INTEGER NOT NULL CHECK(position >= 0),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (domain, position),
    UNIQUE (domain, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_presentation_order (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    position INTEGER NOT NULL CHECK(position >= 0),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (domain, position),
    UNIQUE (domain, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_conversation_keys (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK(typeof(key_digest) = 'blob' AND length(key_digest) = 32),
    key_kind INTEGER NOT NULL CHECK(key_kind IN (1, 2)),
    counterparty_installation BLOB NOT NULL
        CHECK(typeof(counterparty_installation) = 'blob' AND length(counterparty_installation) = 32),
    counterparty_mailbox BLOB NOT NULL
        CHECK(typeof(counterparty_mailbox) = 'blob' AND length(counterparty_mailbox) = 32),
    thread_id BLOB NOT NULL CHECK(typeof(thread_id) = 'blob' AND length(thread_id) = 32),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) <= 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) <= 256)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_conversation_order (
    key_digest BLOB NOT NULL REFERENCES reduction_conversation_keys(key_digest) ON DELETE RESTRICT,
    position INTEGER NOT NULL CHECK(position >= 0),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    entry_kind INTEGER NOT NULL CHECK(entry_kind IN (1, 2)),
    PRIMARY KEY (key_digest, position),
    UNIQUE (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_conflicts (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    reason_code INTEGER NOT NULL CHECK(reason_code > 0),
    reason_parameter INTEGER NOT NULL CHECK(reason_parameter >= 0),
    PRIMARY KEY (domain, ordinal)
) STRICT, WITHOUT ROWID;

CREATE TABLE reduction_conflict_participants (
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 4),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    participant_id BLOB NOT NULL
        CHECK(typeof(participant_id) = 'blob' AND length(participant_id) = 32),
    PRIMARY KEY (domain, ordinal, participant_id),
    FOREIGN KEY (domain, ordinal) REFERENCES reduction_conflicts(domain, ordinal)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    frontier_count INTEGER NOT NULL CHECK(frontier_count >= 0),
    projection_count INTEGER NOT NULL CHECK(projection_count >= 0),
    support_count INTEGER NOT NULL CHECK(support_count >= 0),
    row_count INTEGER NOT NULL CHECK(row_count >= 0),
    row_digest BLOB NOT NULL CHECK(typeof(row_digest) = 'blob' AND length(row_digest) = 32)
) STRICT;

CREATE TABLE authority_frontiers (
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 7),
    key_a BLOB NOT NULL CHECK(typeof(key_a) = 'blob' AND length(key_a) = 32),
    key_b BLOB NOT NULL CHECK(typeof(key_b) = 'blob' AND length(key_b) = 32),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_kind, key_a, key_b, fact_id)
) STRICT;

CREATE TABLE authority_support (
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 7),
    key_a BLOB NOT NULL CHECK(typeof(key_a) = 'blob' AND length(key_a) = 32),
    key_b BLOB NOT NULL CHECK(typeof(key_b) = 'blob' AND length(key_b) = 32),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_kind, key_a, key_b, fact_id)
) STRICT;

CREATE TABLE authority_installations (
    installation_id BLOB PRIMARY KEY NOT NULL CHECK(typeof(installation_id) = 'blob' AND length(installation_id) = 32),
    root_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    signing_key BLOB NOT NULL CHECK(typeof(signing_key) = 'blob' AND length(signing_key) = 32),
    encryption_key BLOB NOT NULL CHECK(typeof(encryption_key) = 'blob' AND length(encryption_key) = 32),
    label TEXT CHECK(label IS NULL OR (typeof(label) = 'text' AND length(CAST(label AS BLOB)) BETWEEN 1 AND 128))
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_mailboxes (
    owner_id BLOB NOT NULL CHECK(typeof(owner_id) = 'blob' AND length(owner_id) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32),
    create_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    mailbox_kind INTEGER NOT NULL CHECK(mailbox_kind IN (1, 2)),
    label TEXT CHECK(label IS NULL OR (typeof(label) = 'text' AND length(CAST(label AS BLOB)) BETWEEN 1 AND 128)),
    PRIMARY KEY (owner_id, mailbox_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_peer_routes (
    owner_id BLOB NOT NULL CHECK(typeof(owner_id) = 'blob' AND length(owner_id) = 32),
    peer_id BLOB NOT NULL CHECK(typeof(peer_id) = 'blob' AND length(peer_id) = 32),
    route_state INTEGER NOT NULL CHECK(route_state BETWEEN 1 AND 3),
    PRIMARY KEY (owner_id, peer_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_peer_route_facts (
    owner_id BLOB NOT NULL,
    peer_id BLOB NOT NULL,
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    relation INTEGER NOT NULL CHECK(relation IN (1, 2)),
    reason TEXT CHECK((relation = 1 AND reason IS NULL) OR (relation = 2 AND typeof(reason) = 'text' AND length(CAST(reason AS BLOB)) BETWEEN 1 AND 96)),
    PRIMARY KEY (owner_id, peer_id, fact_id, relation),
    FOREIGN KEY (owner_id, peer_id) REFERENCES authority_peer_routes(owner_id, peer_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_peer_route_candidates (
    owner_id BLOB NOT NULL,
    peer_id BLOB NOT NULL,
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    candidate_installation BLOB NOT NULL CHECK(typeof(candidate_installation) = 'blob' AND length(candidate_installation) = 32),
    candidate_signing_key BLOB NOT NULL CHECK(typeof(candidate_signing_key) = 'blob' AND length(candidate_signing_key) = 32),
    encryption_key BLOB NOT NULL CHECK(typeof(encryption_key) = 'blob' AND length(encryption_key) = 32),
    label TEXT CHECK(label IS NULL OR (typeof(label) = 'text' AND length(CAST(label AS BLOB)) BETWEEN 1 AND 128)),
    PRIMARY KEY (owner_id, peer_id, fact_id),
    FOREIGN KEY (owner_id, peer_id) REFERENCES authority_peer_routes(owner_id, peer_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_peer_route_relays (
    owner_id BLOB NOT NULL,
    peer_id BLOB NOT NULL,
    fact_id BLOB NOT NULL,
    position INTEGER NOT NULL CHECK(position >= 0),
    scheme INTEGER NOT NULL CHECK(scheme BETWEEN 1 AND 4),
    value TEXT NOT NULL CHECK(typeof(value) = 'text' AND length(CAST(value AS BLOB)) BETWEEN 1 AND 4096),
    PRIMARY KEY (owner_id, peer_id, fact_id, position),
    FOREIGN KEY (owner_id, peer_id, fact_id) REFERENCES authority_peer_route_candidates(owner_id, peer_id, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_capabilities (
    grant_id BLOB PRIMARY KEY NOT NULL CHECK(typeof(grant_id) = 'blob' AND length(grant_id) = 32),
    mailbox_owner BLOB NOT NULL CHECK(typeof(mailbox_owner) = 'blob' AND length(mailbox_owner) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32),
    grantee_installation BLOB NOT NULL CHECK(typeof(grantee_installation) = 'blob' AND length(grantee_installation) = 32),
    grantee_signing_key BLOB NOT NULL CHECK(typeof(grantee_signing_key) = 'blob' AND length(grantee_signing_key) = 32),
    active INTEGER NOT NULL CHECK(active IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_capability_facts (
    grant_id BLOB NOT NULL REFERENCES authority_capabilities(grant_id),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    relation INTEGER NOT NULL CHECK(relation IN (1, 2)),
    PRIMARY KEY (grant_id, fact_id, relation)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_accounts (
    account_id BLOB PRIMARY KEY NOT NULL CHECK(typeof(account_id) = 'blob' AND length(account_id) = 32),
    root_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    creator_installation BLOB NOT NULL CHECK(typeof(creator_installation) = 'blob' AND length(creator_installation) = 32),
    creator_signing_key BLOB NOT NULL CHECK(typeof(creator_signing_key) = 'blob' AND length(creator_signing_key) = 32),
    label TEXT CHECK(label IS NULL OR (typeof(label) = 'text' AND length(CAST(label AS BLOB)) BETWEEN 1 AND 128))
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_memberships (
    account_id BLOB NOT NULL CHECK(typeof(account_id) = 'blob' AND length(account_id) = 32),
    device_id BLOB NOT NULL CHECK(typeof(device_id) = 'blob' AND length(device_id) = 32),
    membership_state INTEGER NOT NULL CHECK(membership_state BETWEEN 1 AND 3),
    PRIMARY KEY (account_id, device_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_membership_facts (
    account_id BLOB NOT NULL,
    device_id BLOB NOT NULL,
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    relation INTEGER NOT NULL CHECK(relation BETWEEN 1 AND 4),
    PRIMARY KEY (account_id, device_id, fact_id, relation),
    FOREIGN KEY (account_id, device_id) REFERENCES authority_memberships(account_id, device_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_membership_grants (
    account_id BLOB NOT NULL,
    device_id BLOB NOT NULL,
    grant_id BLOB NOT NULL CHECK(typeof(grant_id) = 'blob' AND length(grant_id) = 32),
    grant_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    granted_installation BLOB NOT NULL CHECK(typeof(granted_installation) = 'blob' AND length(granted_installation) = 32),
    granted_signing_key BLOB NOT NULL CHECK(typeof(granted_signing_key) = 'blob' AND length(granted_signing_key) = 32),
    label TEXT CHECK(label IS NULL OR (typeof(label) = 'text' AND length(CAST(label AS BLOB)) BETWEEN 1 AND 128)),
    PRIMARY KEY (account_id, device_id, grant_id),
    FOREIGN KEY (account_id, device_id) REFERENCES authority_memberships(account_id, device_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_membership_grant_relays (
    account_id BLOB NOT NULL,
    device_id BLOB NOT NULL,
    grant_id BLOB NOT NULL,
    position INTEGER NOT NULL CHECK(position >= 0),
    scheme INTEGER NOT NULL CHECK(scheme BETWEEN 1 AND 4),
    value TEXT NOT NULL CHECK(typeof(value) = 'text' AND length(CAST(value AS BLOB)) BETWEEN 1 AND 4096),
    PRIMARY KEY (account_id, device_id, grant_id, position),
    FOREIGN KEY (account_id, device_id, grant_id) REFERENCES authority_membership_grants(account_id, device_id, grant_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_account_selections (
    installation_id BLOB PRIMARY KEY NOT NULL CHECK(typeof(installation_id) = 'blob' AND length(installation_id) = 32),
    active_account BLOB CHECK(active_account IS NULL OR (typeof(active_account) = 'blob' AND length(active_account) = 32))
) STRICT, WITHOUT ROWID;

CREATE TABLE authority_account_selection_candidates (
    installation_id BLOB NOT NULL REFERENCES authority_account_selections(installation_id),
    account_id BLOB NOT NULL CHECK(typeof(account_id) = 'blob' AND length(account_id) = 32),
    PRIMARY KEY (installation_id, account_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    aggregate_key_count INTEGER NOT NULL CHECK(aggregate_key_count >= 0),
    frontier_count INTEGER NOT NULL CHECK(frontier_count >= 0),
    projection_key_count INTEGER NOT NULL CHECK(projection_key_count >= 0),
    projection_count INTEGER NOT NULL CHECK(projection_count >= 0),
    support_count INTEGER NOT NULL CHECK(support_count >= 0),
    row_count INTEGER NOT NULL CHECK(row_count >= 0),
    row_digest BLOB NOT NULL CHECK(typeof(row_digest) = 'blob' AND length(row_digest) = 32)
) STRICT;

CREATE TABLE conversation_aggregate_keys (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK(typeof(key_digest) = 'blob' AND length(key_digest) = 32),
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 4),
    key_a BLOB NOT NULL CHECK(typeof(key_a) = 'blob' AND length(key_a) = 32),
    key_b BLOB NOT NULL CHECK(typeof(key_b) = 'blob' AND length(key_b) = 32),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) <= 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) <= 256),
    operation_id BLOB NOT NULL CHECK(typeof(operation_id) = 'blob' AND length(operation_id) = 32),
    item_present INTEGER NOT NULL CHECK(item_present IN (0, 1)),
    item TEXT NOT NULL CHECK(typeof(item) = 'text' AND length(CAST(item AS BLOB)) <= 128),
    activity_kind INTEGER NOT NULL CHECK(activity_kind BETWEEN 0 AND 5),
    logical_key TEXT NOT NULL CHECK(typeof(logical_key) = 'text' AND length(CAST(logical_key AS BLOB)) <= 128),
    runtime TEXT NOT NULL CHECK(typeof(runtime) = 'text' AND length(CAST(runtime AS BLOB)) <= 128)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_frontiers (
    key_digest BLOB NOT NULL REFERENCES conversation_aggregate_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_projection_keys (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK(typeof(key_digest) = 'blob' AND length(key_digest) = 32),
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 6),
    key_a BLOB NOT NULL CHECK(typeof(key_a) = 'blob' AND length(key_a) = 32),
    key_b BLOB NOT NULL CHECK(typeof(key_b) = 'blob' AND length(key_b) = 32),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) <= 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) <= 256),
    operation_id BLOB NOT NULL CHECK(typeof(operation_id) = 'blob' AND length(operation_id) = 32),
    item_present INTEGER NOT NULL CHECK(item_present IN (0, 1)),
    item TEXT NOT NULL CHECK(typeof(item) = 'text' AND length(CAST(item AS BLOB)) <= 128),
    activity_kind INTEGER NOT NULL CHECK(activity_kind BETWEEN 0 AND 5),
    logical_key TEXT NOT NULL CHECK(typeof(logical_key) = 'text' AND length(CAST(logical_key AS BLOB)) <= 128),
    runtime TEXT NOT NULL CHECK(typeof(runtime) = 'text' AND length(CAST(runtime AS BLOB)) <= 128)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_support (
    key_digest BLOB NOT NULL REFERENCES conversation_projection_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_threads (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES conversation_projection_keys(key_digest),
    root_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    root_message BLOB NOT NULL CHECK(typeof(root_message) = 'blob' AND length(root_message) = 32),
    cancelled INTEGER NOT NULL CHECK(cancelled IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_thread_answers (
    key_digest BLOB NOT NULL REFERENCES conversation_threads(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_thread_cancellations (
    key_digest BLOB NOT NULL REFERENCES conversation_threads(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_thread_relations (
    key_digest BLOB NOT NULL REFERENCES conversation_threads(key_digest),
    answer_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    cancellation_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    relation INTEGER NOT NULL CHECK(relation BETWEEN 1 AND 3),
    PRIMARY KEY (key_digest, answer_fact, cancellation_fact)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_thread_ready_answers (
    key_digest BLOB NOT NULL REFERENCES conversation_threads(key_digest),
    position INTEGER NOT NULL CHECK(position >= 0),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, position),
    UNIQUE (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_messages (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES conversation_projection_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    thread_id BLOB NOT NULL CHECK(typeof(thread_id) = 'blob' AND length(thread_id) = 32),
    sender_installation BLOB NOT NULL CHECK(typeof(sender_installation) = 'blob' AND length(sender_installation) = 32),
    sender_mailbox BLOB NOT NULL CHECK(typeof(sender_mailbox) = 'blob' AND length(sender_mailbox) = 32),
    recipient_present INTEGER NOT NULL CHECK(recipient_present IN (0, 1)),
    recipient_installation BLOB NOT NULL CHECK(typeof(recipient_installation) = 'blob' AND length(recipient_installation) = 32),
    recipient_mailbox BLOB NOT NULL CHECK(typeof(recipient_mailbox) = 'blob' AND length(recipient_mailbox) = 32),
    body TEXT NOT NULL CHECK(typeof(body) = 'text' AND length(CAST(body AS BLOB)) BETWEEN 1 AND 16384),
    purpose INTEGER NOT NULL CHECK(purpose BETWEEN 1 AND 3),
    presentation INTEGER NOT NULL CHECK(presentation BETWEEN 1 AND 3),
    correlation_present INTEGER NOT NULL CHECK(correlation_present IN (0, 1)),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) <= 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) <= 256),
    operation_id BLOB NOT NULL CHECK(typeof(operation_id) = 'blob' AND length(operation_id) = 32),
    project_present INTEGER NOT NULL CHECK(project_present IN (0, 1)),
    project_id BLOB NOT NULL CHECK(typeof(project_id) = 'blob' AND length(project_id) = 32),
    open INTEGER NOT NULL CHECK(open IN (0, 1)),
    rejected INTEGER NOT NULL CHECK(rejected IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX conversation_messages_by_fact_id ON conversation_messages(fact_id);

CREATE TABLE conversation_message_frontiers (
    key_digest BLOB NOT NULL REFERENCES conversation_messages(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_message_receipts (
    key_digest BLOB NOT NULL REFERENCES conversation_messages(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_action_groups (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES conversation_projection_keys(key_digest),
    final_answer_present INTEGER NOT NULL CHECK(final_answer_present IN (0, 1)),
    final_answer BLOB NOT NULL CHECK(typeof(final_answer) = 'blob' AND length(final_answer) = 32)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_action_entries (
    key_digest BLOB NOT NULL REFERENCES conversation_action_groups(key_digest),
    position INTEGER NOT NULL CHECK(position >= 0),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, position),
    UNIQUE (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_activities (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES conversation_projection_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    sequence BLOB NOT NULL CHECK(typeof(sequence) = 'blob' AND length(sequence) = 8),
    status INTEGER NOT NULL CHECK(status BETWEEN 1 AND 5),
    failure_reason TEXT NOT NULL CHECK(typeof(failure_reason) = 'text' AND length(CAST(failure_reason AS BLOB)) <= 96),
    content TEXT NOT NULL CHECK(typeof(content) = 'text' AND length(CAST(content AS BLOB)) BETWEEN 1 AND 16384),
    truncated INTEGER NOT NULL CHECK(truncated IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX conversation_activities_by_fact_id ON conversation_activities(fact_id);

CREATE TABLE conversation_activity_retentions (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES conversation_projection_keys(key_digest),
    total_progress INTEGER NOT NULL CHECK(total_progress >= 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE conversation_retained_progress (
    key_digest BLOB NOT NULL REFERENCES conversation_activity_retentions(key_digest),
    position INTEGER NOT NULL CHECK(position >= 0),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, position),
    UNIQUE (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    aggregate_key_count INTEGER NOT NULL CHECK(aggregate_key_count >= 0),
    frontier_count INTEGER NOT NULL CHECK(frontier_count >= 0),
    projection_key_count INTEGER NOT NULL CHECK(projection_key_count >= 0),
    projection_count INTEGER NOT NULL CHECK(projection_count >= 0),
    support_count INTEGER NOT NULL CHECK(support_count >= 0),
    row_count INTEGER NOT NULL CHECK(row_count >= 0),
    row_digest BLOB NOT NULL CHECK(typeof(row_digest) = 'blob' AND length(row_digest) = 32)
) STRICT;

CREATE TABLE agent_aggregate_keys (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK(typeof(key_digest) = 'blob' AND length(key_digest) = 32),
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 7),
    key_a BLOB NOT NULL CHECK(typeof(key_a) = 'blob' AND length(key_a) = 32),
    key_b BLOB NOT NULL CHECK(typeof(key_b) = 'blob' AND length(key_b) = 32),
    name TEXT NOT NULL CHECK(typeof(name) = 'text' AND length(CAST(name AS BLOB)) <= 128),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) <= 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) <= 256)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_frontiers (
    key_digest BLOB NOT NULL REFERENCES agent_aggregate_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_projection_keys (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK(typeof(key_digest) = 'blob' AND length(key_digest) = 32),
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 7),
    key_a BLOB NOT NULL CHECK(typeof(key_a) = 'blob' AND length(key_a) = 32),
    key_b BLOB NOT NULL CHECK(typeof(key_b) = 'blob' AND length(key_b) = 32),
    name TEXT NOT NULL CHECK(typeof(name) = 'text' AND length(CAST(name AS BLOB)) <= 128),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) <= 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) <= 256)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_support (
    key_digest BLOB NOT NULL REFERENCES agent_projection_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_names (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES agent_projection_keys(key_digest),
    conflicted INTEGER NOT NULL CHECK(conflicted IN (0, 1)),
    retired INTEGER NOT NULL CHECK(retired IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_name_claims (
    key_digest BLOB NOT NULL REFERENCES agent_names(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    agent_id BLOB NOT NULL CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    mailbox_installation BLOB NOT NULL CHECK(typeof(mailbox_installation) = 'blob' AND length(mailbox_installation) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32),
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_agents (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES agent_projection_keys(key_digest),
    lifecycle INTEGER NOT NULL CHECK(lifecycle BETWEEN 1 AND 3),
    runnable INTEGER NOT NULL CHECK(runnable IN (0, 1)),
    selected_present INTEGER NOT NULL CHECK(selected_present IN (0, 1)),
    selected_provider TEXT NOT NULL CHECK(typeof(selected_provider) = 'text' AND length(CAST(selected_provider AS BLOB)) <= 64),
    selected_session TEXT NOT NULL CHECK(typeof(selected_session) = 'text' AND length(CAST(selected_session AS BLOB)) <= 256),
    name_reserved INTEGER NOT NULL CHECK(name_reserved IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_agent_claims (
    key_digest BLOB NOT NULL REFERENCES agent_agents(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_agent_names (
    key_digest BLOB NOT NULL REFERENCES agent_agents(key_digest),
    name TEXT NOT NULL CHECK(typeof(name) = 'text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 128),
    PRIMARY KEY (key_digest, name)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_agent_mailboxes (
    key_digest BLOB NOT NULL REFERENCES agent_agents(key_digest),
    installation_id BLOB NOT NULL CHECK(typeof(installation_id) = 'blob' AND length(installation_id) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32),
    PRIMARY KEY (key_digest, installation_id, mailbox_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_agent_retirements (
    key_digest BLOB NOT NULL REFERENCES agent_agents(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_sessions (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES agent_projection_keys(key_digest),
    conflicted INTEGER NOT NULL CHECK(conflicted IN (0, 1)),
    mailbox_present INTEGER NOT NULL CHECK(mailbox_present IN (0, 1)),
    mailbox_installation BLOB NOT NULL CHECK(typeof(mailbox_installation) = 'blob' AND length(mailbox_installation) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_session_bindings (
    key_digest BLOB NOT NULL REFERENCES agent_sessions(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    mailbox_installation BLOB NOT NULL CHECK(typeof(mailbox_installation) = 'blob' AND length(mailbox_installation) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32),
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_contexts (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES agent_projection_keys(key_digest)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_context_history (
    key_digest BLOB NOT NULL REFERENCES agent_contexts(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    directory_scheme INTEGER NOT NULL CHECK(directory_scheme BETWEEN 1 AND 4),
    directory_value TEXT NOT NULL CHECK(typeof(directory_value) = 'text' AND length(CAST(directory_value AS BLOB)) BETWEEN 1 AND 4096),
    repository_present INTEGER NOT NULL CHECK(repository_present IN (0, 1)),
    repository_scheme INTEGER NOT NULL CHECK(repository_scheme BETWEEN 0 AND 4),
    repository_value TEXT NOT NULL CHECK(typeof(repository_value) = 'text' AND length(CAST(repository_value AS BLOB)) <= 4096),
    worktree_present INTEGER NOT NULL CHECK(worktree_present IN (0, 1)),
    worktree_scheme INTEGER NOT NULL CHECK(worktree_scheme BETWEEN 0 AND 4),
    worktree_value TEXT NOT NULL CHECK(typeof(worktree_value) = 'text' AND length(CAST(worktree_value AS BLOB)) <= 4096),
    branch_present INTEGER NOT NULL CHECK(branch_present IN (0, 1)),
    branch TEXT NOT NULL CHECK(typeof(branch) = 'text' AND length(CAST(branch AS BLOB)) <= 128),
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_context_frontiers (
    key_digest BLOB NOT NULL REFERENCES agent_contexts(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_selections (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES agent_projection_keys(key_digest),
    active_present INTEGER NOT NULL CHECK(active_present IN (0, 1)),
    active_provider TEXT NOT NULL CHECK(typeof(active_provider) = 'text' AND length(CAST(active_provider AS BLOB)) <= 64),
    active_session TEXT NOT NULL CHECK(typeof(active_session) = 'text' AND length(CAST(active_session AS BLOB)) <= 256),
    active_directory_scheme INTEGER NOT NULL CHECK(active_directory_scheme BETWEEN 0 AND 4),
    active_directory_value TEXT NOT NULL CHECK(typeof(active_directory_value) = 'text' AND length(CAST(active_directory_value AS BLOB)) <= 4096),
    active_repository_present INTEGER NOT NULL CHECK(active_repository_present IN (0, 1)),
    active_repository_scheme INTEGER NOT NULL CHECK(active_repository_scheme BETWEEN 0 AND 4),
    active_repository_value TEXT NOT NULL CHECK(typeof(active_repository_value) = 'text' AND length(CAST(active_repository_value AS BLOB)) <= 4096),
    active_worktree_present INTEGER NOT NULL CHECK(active_worktree_present IN (0, 1)),
    active_worktree_scheme INTEGER NOT NULL CHECK(active_worktree_scheme BETWEEN 0 AND 4),
    active_worktree_value TEXT NOT NULL CHECK(typeof(active_worktree_value) = 'text' AND length(CAST(active_worktree_value AS BLOB)) <= 4096),
    active_branch_present INTEGER NOT NULL CHECK(active_branch_present IN (0, 1)),
    active_branch TEXT NOT NULL CHECK(typeof(active_branch) = 'text' AND length(CAST(active_branch AS BLOB)) <= 128),
    conflicted INTEGER NOT NULL CHECK(conflicted IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_selection_candidates (
    key_digest BLOB NOT NULL REFERENCES agent_selections(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) BETWEEN 1 AND 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) BETWEEN 1 AND 256),
    directory_scheme INTEGER NOT NULL CHECK(directory_scheme BETWEEN 1 AND 4),
    directory_value TEXT NOT NULL CHECK(typeof(directory_value) = 'text' AND length(CAST(directory_value AS BLOB)) BETWEEN 1 AND 4096),
    repository_present INTEGER NOT NULL CHECK(repository_present IN (0, 1)),
    repository_scheme INTEGER NOT NULL CHECK(repository_scheme BETWEEN 0 AND 4),
    repository_value TEXT NOT NULL CHECK(typeof(repository_value) = 'text' AND length(CAST(repository_value AS BLOB)) <= 4096),
    worktree_present INTEGER NOT NULL CHECK(worktree_present IN (0, 1)),
    worktree_scheme INTEGER NOT NULL CHECK(worktree_scheme BETWEEN 0 AND 4),
    worktree_value TEXT NOT NULL CHECK(typeof(worktree_value) = 'text' AND length(CAST(worktree_value AS BLOB)) <= 4096),
    branch_present INTEGER NOT NULL CHECK(branch_present IN (0, 1)),
    branch TEXT NOT NULL CHECK(typeof(branch) = 'text' AND length(CAST(branch AS BLOB)) <= 128),
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_selection_frontiers (
    key_digest BLOB NOT NULL REFERENCES agent_selections(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_renames (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES agent_projection_keys(key_digest),
    resolved INTEGER NOT NULL CHECK(resolved IN (0, 1)),
    display_name_present INTEGER NOT NULL CHECK(display_name_present IN (0, 1)),
    display_name TEXT NOT NULL CHECK(typeof(display_name) = 'text' AND length(CAST(display_name AS BLOB)) <= 128)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_rename_candidates (
    key_digest BLOB NOT NULL REFERENCES agent_renames(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    display_name_present INTEGER NOT NULL CHECK(display_name_present IN (0, 1)),
    display_name TEXT NOT NULL CHECK(typeof(display_name) = 'text' AND length(CAST(display_name AS BLOB)) <= 128),
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_rename_frontiers (
    key_digest BLOB NOT NULL REFERENCES agent_renames(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_direct_sessions (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES agent_projection_keys(key_digest),
    mailbox_installation BLOB NOT NULL CHECK(typeof(mailbox_installation) = 'blob' AND length(mailbox_installation) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32),
    named_agent_present INTEGER NOT NULL CHECK(named_agent_present IN (0, 1)),
    named_agent BLOB NOT NULL CHECK(typeof(named_agent) = 'blob' AND length(named_agent) = 32),
    conflicted INTEGER NOT NULL CHECK(conflicted IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_direct_binding_facts (
    key_digest BLOB NOT NULL REFERENCES agent_direct_sessions(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    aggregate_key_count INTEGER NOT NULL CHECK(aggregate_key_count >= 0),
    frontier_count INTEGER NOT NULL CHECK(frontier_count >= 0),
    projection_key_count INTEGER NOT NULL CHECK(projection_key_count >= 0),
    projection_count INTEGER NOT NULL CHECK(projection_count >= 0),
    support_count INTEGER NOT NULL CHECK(support_count >= 0),
    row_count INTEGER NOT NULL CHECK(row_count >= 0),
    row_digest BLOB NOT NULL CHECK(typeof(row_digest) = 'blob' AND length(row_digest) = 32)
) STRICT;

CREATE TABLE project_aggregate_keys (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK(typeof(key_digest) = 'blob' AND length(key_digest) = 32),
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 7),
    key_a BLOB NOT NULL CHECK(typeof(key_a) = 'blob' AND length(key_a) = 32),
    key_b BLOB NOT NULL CHECK(typeof(key_b) = 'blob' AND length(key_b) = 32),
    locator_scheme INTEGER NOT NULL CHECK(locator_scheme BETWEEN 0 AND 4),
    locator_value TEXT NOT NULL CHECK(typeof(locator_value) = 'text' AND length(CAST(locator_value AS BLOB)) <= 4096)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_frontiers (
    key_digest BLOB NOT NULL REFERENCES project_aggregate_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_projection_keys (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK(typeof(key_digest) = 'blob' AND length(key_digest) = 32),
    key_kind INTEGER NOT NULL CHECK(key_kind BETWEEN 1 AND 5),
    key_id BLOB NOT NULL CHECK(typeof(key_id) = 'blob' AND length(key_id) = 32)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_support (
    key_digest BLOB NOT NULL REFERENCES project_projection_keys(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_projects (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES project_projection_keys(key_digest),
    root_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT CHECK(typeof(root_id) = 'blob' AND length(root_id) = 32),
    head_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT CHECK(typeof(head_id) = 'blob' AND length(head_id) = 32),
    home_id BLOB NOT NULL CHECK(typeof(home_id) = 'blob' AND length(home_id) = 32),
    mailbox_installation BLOB NOT NULL CHECK(typeof(mailbox_installation) = 'blob' AND length(mailbox_installation) = 32),
    mailbox_id BLOB NOT NULL CHECK(typeof(mailbox_id) = 'blob' AND length(mailbox_id) = 32),
    predecessor_present INTEGER NOT NULL CHECK(predecessor_present IN (0, 1)),
    predecessor_id BLOB NOT NULL CHECK(typeof(predecessor_id) = 'blob' AND length(predecessor_id) = 32),
    name TEXT NOT NULL CHECK(typeof(name) = 'text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 128),
    brief_present INTEGER NOT NULL CHECK(brief_present IN (0, 1)),
    brief TEXT NOT NULL CHECK(typeof(brief) = 'text' AND length(CAST(brief AS BLOB)) <= 16384),
    primary_present INTEGER NOT NULL CHECK(primary_present IN (0, 1)),
    primary_id BLOB NOT NULL CHECK(typeof(primary_id) = 'blob' AND length(primary_id) = 32),
    lifecycle INTEGER NOT NULL CHECK(lifecycle BETWEEN 1 AND 3),
    archived INTEGER NOT NULL CHECK(archived IN (0, 1)),
    claimable INTEGER NOT NULL CHECK(claimable IN (0, 1)),
    assignment_present INTEGER NOT NULL CHECK(assignment_present IN (0, 1)),
    input_sequence BLOB NOT NULL CHECK(typeof(input_sequence) = 'blob' AND length(input_sequence) = 8)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_fork_participants (
    key_digest BLOB NOT NULL REFERENCES project_projects(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_resources (
    key_digest BLOB NOT NULL REFERENCES project_projects(key_digest),
    resource_id BLOB NOT NULL CHECK(typeof(resource_id) = 'blob' AND length(resource_id) = 32),
    locator_scheme INTEGER NOT NULL CHECK(locator_scheme BETWEEN 1 AND 4),
    locator_value TEXT NOT NULL CHECK(typeof(locator_value) = 'text' AND length(CAST(locator_value AS BLOB)) BETWEEN 1 AND 4096),
    health INTEGER NOT NULL CHECK(health BETWEEN 1 AND 4),
    PRIMARY KEY (key_digest, resource_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_active_claims (
    key_digest BLOB NOT NULL REFERENCES project_projects(key_digest),
    resource_id BLOB NOT NULL CHECK(typeof(resource_id) = 'blob' AND length(resource_id) = 32),
    PRIMARY KEY (key_digest, resource_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_claim_conflicts (
    key_digest BLOB NOT NULL REFERENCES project_projects(key_digest),
    resource_id BLOB NOT NULL CHECK(typeof(resource_id) = 'blob' AND length(resource_id) = 32),
    project_id BLOB NOT NULL CHECK(typeof(project_id) = 'blob' AND length(project_id) = 32),
    PRIMARY KEY (key_digest, resource_id, project_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_assignments (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES project_projects(key_digest),
    assignment_id BLOB NOT NULL CHECK(typeof(assignment_id) = 'blob' AND length(assignment_id) = 32),
    agent_id BLOB NOT NULL CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) BETWEEN 1 AND 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) BETWEEN 1 AND 256),
    phase INTEGER NOT NULL CHECK(phase BETWEEN 1 AND 3),
    thread_id BLOB NOT NULL CHECK(typeof(thread_id) = 'blob' AND length(thread_id) = 32),
    launch_scheme INTEGER NOT NULL CHECK(launch_scheme BETWEEN 0 AND 4),
    launch_value TEXT NOT NULL CHECK(typeof(launch_value) = 'text' AND length(CAST(launch_value AS BLOB)) <= 4096),
    error_code TEXT NOT NULL CHECK(typeof(error_code) = 'text' AND length(CAST(error_code AS BLOB)) <= 128),
    cardinality_conflicted INTEGER NOT NULL CHECK(cardinality_conflicted IN (0, 1)),
    runnable INTEGER NOT NULL CHECK(runnable IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE project_assignment_support (
    key_digest BLOB NOT NULL REFERENCES project_assignments(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_inputs (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES project_projection_keys(key_digest),
    project_id BLOB NOT NULL CHECK(typeof(project_id) = 'blob' AND length(project_id) = 32),
    message_id BLOB NOT NULL CHECK(typeof(message_id) = 'blob' AND length(message_id) = 32),
    input_fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT CHECK(typeof(input_fact_id) = 'blob' AND length(input_fact_id) = 32),
    sequence BLOB NOT NULL CHECK(typeof(sequence) = 'blob' AND length(sequence) = 8),
    accepted_fact BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT CHECK(typeof(accepted_fact) = 'blob' AND length(accepted_fact) = 32)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_dispatches (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES project_projection_keys(key_digest),
    dispatch_id BLOB NOT NULL CHECK(typeof(dispatch_id) = 'blob' AND length(dispatch_id) = 32),
    message_id BLOB NOT NULL CHECK(typeof(message_id) = 'blob' AND length(message_id) = 32),
    sequence BLOB NOT NULL CHECK(typeof(sequence) = 'blob' AND length(sequence) = 8),
    assignment_id BLOB NOT NULL CHECK(typeof(assignment_id) = 'blob' AND length(assignment_id) = 32),
    agent_id BLOB NOT NULL CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) BETWEEN 1 AND 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) BETWEEN 1 AND 256),
    thread_id BLOB NOT NULL CHECK(typeof(thread_id) = 'blob' AND length(thread_id) = 32),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT CHECK(typeof(fact_id) = 'blob' AND length(fact_id) = 32),
    conflicted INTEGER NOT NULL CHECK(conflicted IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE project_outputs (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES project_projection_keys(key_digest),
    output_id BLOB NOT NULL CHECK(typeof(output_id) = 'blob' AND length(output_id) = 32),
    dispatch_id BLOB NOT NULL CHECK(typeof(dispatch_id) = 'blob' AND length(dispatch_id) = 32),
    assignment_id BLOB NOT NULL CHECK(typeof(assignment_id) = 'blob' AND length(assignment_id) = 32),
    agent_id BLOB NOT NULL CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    provider TEXT NOT NULL CHECK(typeof(provider) = 'text' AND length(CAST(provider AS BLOB)) BETWEEN 1 AND 64),
    session TEXT NOT NULL CHECK(typeof(session) = 'text' AND length(CAST(session AS BLOB)) BETWEEN 1 AND 256),
    thread_id BLOB NOT NULL CHECK(typeof(thread_id) = 'blob' AND length(thread_id) = 32),
    message_id BLOB NOT NULL CHECK(typeof(message_id) = 'blob' AND length(message_id) = 32),
    sender_installation BLOB NOT NULL CHECK(typeof(sender_installation) = 'blob' AND length(sender_installation) = 32),
    sender_mailbox BLOB NOT NULL CHECK(typeof(sender_mailbox) = 'blob' AND length(sender_mailbox) = 32),
    recipient_present INTEGER NOT NULL CHECK(recipient_present IN (0, 1)),
    recipient_installation BLOB NOT NULL CHECK(typeof(recipient_installation) = 'blob' AND length(recipient_installation) = 32),
    recipient_mailbox BLOB NOT NULL CHECK(typeof(recipient_mailbox) = 'blob' AND length(recipient_mailbox) = 32),
    body TEXT NOT NULL CHECK(typeof(body) = 'text' AND length(CAST(body AS BLOB)) BETWEEN 1 AND 16384),
    purpose INTEGER NOT NULL CHECK(purpose BETWEEN 1 AND 3),
    presentation INTEGER NOT NULL CHECK(presentation BETWEEN 1 AND 3),
    correlation_present INTEGER NOT NULL CHECK(correlation_present IN (0, 1)),
    correlation_provider TEXT NOT NULL CHECK(typeof(correlation_provider) = 'text' AND length(CAST(correlation_provider AS BLOB)) <= 64),
    correlation_session TEXT NOT NULL CHECK(typeof(correlation_session) = 'text' AND length(CAST(correlation_session AS BLOB)) <= 256),
    correlation_id BLOB NOT NULL CHECK(typeof(correlation_id) = 'blob' AND length(correlation_id) = 32),
    project_present INTEGER NOT NULL CHECK(project_present IN (0, 1)),
    project_id BLOB NOT NULL CHECK(typeof(project_id) = 'blob' AND length(project_id) = 32),
    status INTEGER NOT NULL CHECK(status BETWEEN 1 AND 3)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_output_facts (
    key_digest BLOB NOT NULL REFERENCES project_outputs(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_commands (
    key_digest BLOB PRIMARY KEY NOT NULL REFERENCES project_projection_keys(key_digest),
    digest BLOB NOT NULL CHECK(typeof(digest) = 'blob' AND length(digest) = 32),
    project_id BLOB NOT NULL CHECK(typeof(project_id) = 'blob' AND length(project_id) = 32),
    expected_head BLOB NOT NULL CHECK(typeof(expected_head) = 'blob' AND length(expected_head) = 32),
    stage INTEGER NOT NULL CHECK(stage BETWEEN 1 AND 4),
    received_head BLOB NOT NULL CHECK(typeof(received_head) = 'blob' AND length(received_head) = 32),
    result_kind INTEGER NOT NULL CHECK(result_kind BETWEEN 0 AND 2),
    result_head BLOB NOT NULL CHECK(typeof(result_head) = 'blob' AND length(result_head) = 32),
    result_error TEXT NOT NULL CHECK(typeof(result_error) = 'text' AND length(CAST(result_error AS BLOB)) <= 128),
    runtime_kind INTEGER NOT NULL CHECK(runtime_kind BETWEEN 0 AND 3),
    runtime_error TEXT NOT NULL CHECK(typeof(runtime_error) = 'text' AND length(CAST(runtime_error AS BLOB)) <= 128)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_command_support (
    key_digest BLOB NOT NULL REFERENCES project_commands(key_digest),
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    PRIMARY KEY (key_digest, fact_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE mutation_receipts (
    command_id BLOB PRIMARY KEY NOT NULL
        CHECK(typeof(command_id) = 'blob' AND length(command_id) = 32),
    request_digest BLOB NOT NULL
        CHECK(typeof(request_digest) = 'blob' AND length(request_digest) = 32),
    result_kind INTEGER NOT NULL CHECK(result_kind IN (1, 2)),
    result_bytes BLOB NOT NULL
        CHECK(typeof(result_bytes) = 'blob' AND length(result_bytes) <= 65536),
    revision BLOB NOT NULL CHECK(typeof(revision) = 'blob' AND length(revision) = 8)
) STRICT, WITHOUT ROWID;

CREATE TABLE change_revision (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    revision BLOB NOT NULL CHECK(typeof(revision) = 'blob' AND length(revision) = 8)
) STRICT;

INSERT INTO change_revision(singleton, revision) VALUES (1, X'0000000000000000');

CREATE TABLE outbox_intents (
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    recipient_installation BLOB NOT NULL
        CHECK(typeof(recipient_installation) = 'blob' AND length(recipient_installation) = 32),
    exact_canonical_bytes BLOB NOT NULL
        CHECK(typeof(exact_canonical_bytes) = 'blob' AND length(exact_canonical_bytes) BETWEEN 1 AND 65536),
    revision BLOB NOT NULL CHECK(typeof(revision) = 'blob' AND length(revision) = 8),
    PRIMARY KEY (fact_id, recipient_installation)
) STRICT, WITHOUT ROWID;

CREATE TABLE canonical_commits (
    fact_id BLOB PRIMARY KEY NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    revision BLOB NOT NULL CHECK(typeof(revision) = 'blob' AND length(revision) = 8)
) STRICT, WITHOUT ROWID;

CREATE TABLE relay_policy_operations (
    operation_id BLOB PRIMARY KEY NOT NULL
        CHECK(typeof(operation_id) = 'blob' AND length(operation_id) = 32),
    request_digest BLOB NOT NULL
        CHECK(typeof(request_digest) = 'blob' AND length(request_digest) = 32),
    url TEXT NOT NULL CHECK(typeof(url) = 'text' AND length(url) BETWEEN 1 AND 2048),
    access INTEGER NOT NULL CHECK(access BETWEEN 1 AND 3),
    authentication INTEGER NOT NULL CHECK(authentication BETWEEN 1 AND 3),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    generation BLOB NOT NULL CHECK(typeof(generation) = 'blob' AND length(generation) = 8)
) STRICT, WITHOUT ROWID;

CREATE TABLE relay_policies (
    url TEXT PRIMARY KEY NOT NULL CHECK(typeof(url) = 'text' AND length(url) BETWEEN 1 AND 2048),
    access INTEGER NOT NULL CHECK(access BETWEEN 1 AND 3),
    authentication INTEGER NOT NULL CHECK(authentication BETWEEN 1 AND 3),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    generation BLOB NOT NULL CHECK(typeof(generation) = 'blob' AND length(generation) = 8)
) STRICT, WITHOUT ROWID;

CREATE TABLE prepared_relay_outbox (
    fact_id BLOB NOT NULL REFERENCES canonical_facts(fact_id) ON DELETE RESTRICT,
    recipient_installation BLOB NOT NULL
        CHECK(typeof(recipient_installation) = 'blob' AND length(recipient_installation) = 32),
    wrapper_id BLOB NOT NULL UNIQUE
        CHECK(typeof(wrapper_id) = 'blob' AND length(wrapper_id) = 32),
    one_use_public_key BLOB NOT NULL UNIQUE
        CHECK(typeof(one_use_public_key) = 'blob' AND length(one_use_public_key) = 32),
    recipient_public_key BLOB NOT NULL
        CHECK(typeof(recipient_public_key) = 'blob' AND length(recipient_public_key) = 32),
    canonical_event_id BLOB NOT NULL
        CHECK(typeof(canonical_event_id) = 'blob' AND length(canonical_event_id) = 32),
    canonical_sha256 BLOB NOT NULL
        CHECK(typeof(canonical_sha256) = 'blob' AND length(canonical_sha256) = 32),
    wrapper_sha256 BLOB NOT NULL
        CHECK(typeof(wrapper_sha256) = 'blob' AND length(wrapper_sha256) = 32),
    seal_created_at BLOB NOT NULL
        CHECK(typeof(seal_created_at) = 'blob' AND length(seal_created_at) = 8),
    gift_wrap_created_at BLOB NOT NULL
        CHECK(typeof(gift_wrap_created_at) = 'blob' AND length(gift_wrap_created_at) = 8),
    exact_wire BLOB NOT NULL
        CHECK(typeof(exact_wire) = 'blob' AND length(exact_wire) BETWEEN 1 AND 262144),
    PRIMARY KEY (fact_id, recipient_installation)
) STRICT, WITHOUT ROWID;

CREATE TABLE relay_attempts (
    url TEXT NOT NULL REFERENCES relay_policies(url) ON DELETE RESTRICT,
    wrapper_id BLOB NOT NULL REFERENCES prepared_relay_outbox(wrapper_id) ON DELETE RESTRICT,
    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 1 AND 4294967295),
    disposition INTEGER NOT NULL CHECK(disposition BETWEEN 1 AND 3),
    failure_code INTEGER CHECK(failure_code IS NULL OR failure_code BETWEEN 1 AND 3),
    last_attempt_millis BLOB NOT NULL
        CHECK(typeof(last_attempt_millis) = 'blob' AND length(last_attempt_millis) = 8),
    retry_at_millis BLOB
        CHECK(retry_at_millis IS NULL OR
              (typeof(retry_at_millis) = 'blob' AND length(retry_at_millis) = 8)),
    CHECK((disposition = 2) = (failure_code IS NOT NULL)),
    CHECK(disposition != 3 OR retry_at_millis IS NULL),
    PRIMARY KEY (url, wrapper_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE relay_cursors (
    url TEXT PRIMARY KEY NOT NULL REFERENCES relay_policies(url) ON DELETE RESTRICT,
    generation BLOB NOT NULL CHECK(typeof(generation) = 'blob' AND length(generation) = 8),
    scan_started_at_millis BLOB NOT NULL
        CHECK(typeof(scan_started_at_millis) = 'blob' AND length(scan_started_at_millis) = 8),
    covered_through_millis BLOB
        CHECK(covered_through_millis IS NULL OR
              (typeof(covered_through_millis) = 'blob' AND length(covered_through_millis) = 8)),
    oldest_created_at BLOB
        CHECK(oldest_created_at IS NULL OR
              (typeof(oldest_created_at) = 'blob' AND length(oldest_created_at) = 8)),
    oldest_wrapper_id BLOB
        CHECK(oldest_wrapper_id IS NULL OR
              (typeof(oldest_wrapper_id) = 'blob' AND length(oldest_wrapper_id) = 32)),
    exhausted INTEGER NOT NULL CHECK(exhausted IN (0, 1)),
    CHECK((oldest_created_at IS NULL) = (oldest_wrapper_id IS NULL)),
    CHECK(covered_through_millis IS NULL OR covered_through_millis <= scan_started_at_millis)
) STRICT, WITHOUT ROWID;

CREATE TABLE inbound_relay_claims (
    wrapper_id BLOB PRIMARY KEY NOT NULL
        CHECK(typeof(wrapper_id) = 'blob' AND length(wrapper_id) = 32),
    origin_installation_id BLOB NOT NULL
        CHECK(typeof(origin_installation_id) = 'blob' AND length(origin_installation_id) = 32),
    canonical_event_id BLOB NOT NULL
        CHECK(typeof(canonical_event_id) = 'blob' AND length(canonical_event_id) = 32),
    canonical_sha256 BLOB NOT NULL
        CHECK(typeof(canonical_sha256) = 'blob' AND length(canonical_sha256) = 32),
    received_at_millis BLOB NOT NULL
        CHECK(typeof(received_at_millis) = 'blob' AND length(received_at_millis) = 8),
    UNIQUE (origin_installation_id, canonical_event_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE relay_staging (
    wrapper_sha256 BLOB PRIMARY KEY NOT NULL
        CHECK(typeof(wrapper_sha256) = 'blob' AND length(wrapper_sha256) = 32),
    exact_outer BLOB NOT NULL
        CHECK(typeof(exact_outer) = 'blob' AND length(exact_outer) BETWEEN 1 AND 262144),
    first_received_millis BLOB NOT NULL
        CHECK(typeof(first_received_millis) = 'blob' AND length(first_received_millis) = 8),
    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 4294967295),
    retry_at_millis BLOB NOT NULL
        CHECK(typeof(retry_at_millis) = 'blob' AND length(retry_at_millis) = 8)
) STRICT, WITHOUT ROWID;

CREATE TABLE relay_quarantine (
    wrapper_sha256 BLOB PRIMARY KEY NOT NULL
        CHECK(typeof(wrapper_sha256) = 'blob' AND length(wrapper_sha256) = 32),
    wrapper_id BLOB
        CHECK(wrapper_id IS NULL OR (typeof(wrapper_id) = 'blob' AND length(wrapper_id) = 32)),
    failure_code INTEGER NOT NULL CHECK(failure_code BETWEEN 1 AND 65535),
    received_at_millis BLOB NOT NULL
        CHECK(typeof(received_at_millis) = 'blob' AND length(received_at_millis) = 8),
    byte_len INTEGER NOT NULL CHECK(byte_len >= 1),
    raw_sample BLOB NOT NULL
        CHECK(typeof(raw_sample) = 'blob' AND length(raw_sample) <= 4096 AND length(raw_sample) <= byte_len)
) STRICT, WITHOUT ROWID;

CREATE TABLE harness_worker_leases (
    agent_id BLOB PRIMARY KEY NOT NULL
        CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    owner_token BLOB NOT NULL
        CHECK(typeof(owner_token) = 'blob' AND length(owner_token) = 32),
    expires_at_millis BLOB NOT NULL
        CHECK(typeof(expires_at_millis) = 'blob' AND length(expires_at_millis) = 8)
) STRICT, WITHOUT ROWID;

CREATE TABLE harness_ready_sessions (
    agent_id BLOB PRIMARY KEY NOT NULL
        CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    provider_id TEXT NOT NULL
        CHECK(typeof(provider_id) = 'text' AND length(CAST(provider_id AS BLOB)) BETWEEN 1 AND 64),
    session_id TEXT NOT NULL
        CHECK(typeof(session_id) = 'text' AND length(CAST(session_id AS BLOB)) BETWEEN 1 AND 256)
) STRICT, WITHOUT ROWID;

CREATE TABLE harness_deliveries (
    agent_id BLOB NOT NULL CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    submission_id BLOB NOT NULL
        CHECK(typeof(submission_id) = 'blob' AND length(submission_id) = 32),
    provider_id TEXT NOT NULL
        CHECK(typeof(provider_id) = 'text' AND length(CAST(provider_id AS BLOB)) BETWEEN 1 AND 64),
    session_id TEXT NOT NULL
        CHECK(typeof(session_id) = 'text' AND length(CAST(session_id AS BLOB)) BETWEEN 1 AND 256),
    digest BLOB NOT NULL CHECK(typeof(digest) = 'blob' AND length(digest) = 32),
    operation_id BLOB NOT NULL
        CHECK(typeof(operation_id) = 'blob' AND length(operation_id) = 32),
    body TEXT NOT NULL
        CHECK(typeof(body) = 'text' AND length(CAST(body AS BLOB)) BETWEEN 1 AND 16384),
    queued_at_millis BLOB NOT NULL
        CHECK(typeof(queued_at_millis) = 'blob' AND length(queued_at_millis) = 8),
    delivery_state INTEGER NOT NULL CHECK(delivery_state BETWEEN 1 AND 4),
    PRIMARY KEY (agent_id, submission_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE harness_event_checkpoints (
    agent_id BLOB NOT NULL CHECK(typeof(agent_id) = 'blob' AND length(agent_id) = 32),
    event_id BLOB NOT NULL CHECK(typeof(event_id) = 'blob' AND length(event_id) = 32),
    digest BLOB NOT NULL CHECK(typeof(digest) = 'blob' AND length(digest) = 32),
    output_committed INTEGER NOT NULL CHECK(output_committed IN (0, 1)),
    activity_committed INTEGER NOT NULL CHECK(activity_committed IN (0, 1)),
    PRIMARY KEY (agent_id, event_id)
) STRICT, WITHOUT ROWID;
";

pub(super) struct Database {
    connection: Connection,
}

impl Database {
    pub(super) fn open(path: &Path) -> Result<Self, StoreError> {
        let new_or_empty_file = prepare_database_path(path)?;
        let initialize = if new_or_empty_file {
            true
        } else {
            let inspection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql_error)?;
            let unclaimed = inspect_existing(&inspection)?;
            drop(inspection);
            unclaimed
        };
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql_error)?;
        configure(&connection)?;
        validate_database_path(path)?;
        if initialize {
            initialize_schema(&mut connection)?;
        } else {
            verify_schema(&connection)?;
        }
        verify_integrity(&connection)?;
        Ok(Self { connection })
    }

    pub(super) fn load(&mut self) -> Result<Vec<VerifiedSemanticFact>, StoreError> {
        let transaction = self.connection.transaction().map_err(sql_error)?;
        let facts = load_facts(&transaction)?;
        transaction.commit().map_err(sql_error)?;
        Ok(facts)
    }

    pub(super) fn ingest(
        &mut self,
        fact: &VerifiedSemanticFact,
        policy: AuthorityPolicy,
    ) -> Result<IngestOutcome, StoreError> {
        ingest_with_failpoint(&mut self.connection, fact, policy, IngestFailpoint::Never)
    }

    pub(super) fn execute_local_mutation(
        &mut self,
        request: LocalMutationRequest,
    ) -> Result<(MutationReceipt, bool), StoreError> {
        execute_local_mutation_with_failpoint(
            &mut self.connection,
            request,
            LocalMutationFailpoint::Never,
        )
    }

    pub(super) fn complete_snapshot(
        &mut self,
        policy: AuthorityPolicy,
    ) -> Result<CompleteSnapshot, StoreError> {
        let facts = self.load()?;
        build_complete_snapshot(&facts, policy)
    }

    pub(super) fn repair(&mut self, policy: AuthorityPolicy) -> Result<RepairOutcome, StoreError> {
        let complete = self.complete_snapshot(policy)?;
        let expected = complete.normalized_index();
        let expected_authority = complete.authority_projection_snapshot();
        let expected_conversation = complete.conversation_projection_snapshot();
        let expected_agent = complete.agent_projection_snapshot();
        let expected_project = complete.project_projection_snapshot();
        let (persisted, authority, conversation, agent, project) = repair::replace(
            &mut self.connection,
            &expected,
            &expected_authority,
            &expected_conversation,
            &expected_agent,
            &expected_project,
        )?;
        Ok(RepairOutcome::new(
            complete,
            persisted,
            authority,
            conversation,
            agent,
            project,
        ))
    }

    pub(super) fn load_reduction_index(&self) -> Result<ReductionIndexSnapshot, StoreError> {
        repair::load(&self.connection)
    }

    pub(super) fn load_authority_snapshot(
        &self,
    ) -> Result<AuthorityProjectionSnapshot, StoreError> {
        repair::load(&self.connection)?;
        authority::load(&self.connection)
    }

    pub(super) fn load_conversation_snapshot(
        &self,
    ) -> Result<ConversationProjectionSnapshot, StoreError> {
        repair::load(&self.connection)?;
        authority::load(&self.connection)?;
        conversation::load(&self.connection)
    }

    pub(super) fn load_conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<ConversationEntry>, StoreError> {
        if !(1..=200).contains(&limit) {
            return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
        }
        let repaired: i64 = self
            .connection
            .query_row("SELECT count(*) FROM reduction_state", [], |row| row.get(0))
            .map_err(sql_error)?;
        if repaired != 1 {
            return Err(StoreError::new(StoreErrorClass::NotRepaired));
        }
        let key_digest = repair::conversation_key_digest(key);
        let persisted = repair::conversation_key_is_persisted(&self.connection, key)?;
        let after = match cursor {
            None => -1,
            Some(cursor) => {
                let (cursor_key, fact_id) = decode_conversation_cursor(cursor)?;
                if cursor_key != key_digest || !persisted {
                    return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
                }
                self.connection
                    .query_row(
                        "SELECT position FROM reduction_conversation_order \
                         WHERE key_digest = ?1 AND fact_id = ?2",
                        params![key_digest.as_slice(), fact_id.as_bytes().as_slice()],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(sql_error)?
                    .ok_or_else(|| StoreError::new(StoreErrorClass::InvalidOperationalRequest))?
            }
        };
        if !persisted {
            return Ok(Page::new(Vec::new(), None));
        }
        let row_limit = i64::try_from(limit + 1)
            .map_err(|_| StoreError::new(StoreErrorClass::InvalidOperationalRequest))?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT position, fact_id, entry_kind FROM reduction_conversation_order \
                 WHERE key_digest = ?1 AND position > ?2 ORDER BY position LIMIT ?3",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![key_digest.as_slice(), after, row_limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(sql_error)?;
        let mut selected = Vec::with_capacity(limit + 1);
        for (offset, row) in rows.enumerate() {
            let (position, fact_id, entry_kind) = row.map_err(sql_error)?;
            let expected = after
                .checked_add(1)
                .and_then(|value| value.checked_add(i64::try_from(offset).ok()?))
                .ok_or_else(|| StoreError::new(StoreErrorClass::RebuildableStateCorrupt))?;
            if position != expected {
                return Err(StoreError::new(StoreErrorClass::RebuildableStateCorrupt));
            }
            selected.push((FactId::from_bytes(fixed_bytes(fact_id)?), entry_kind));
        }
        let has_more = selected.len() > limit;
        selected.truncate(limit);
        let next_cursor = if has_more {
            let fact_id = selected
                .last()
                .map(|(fact_id, _)| *fact_id)
                .ok_or_else(|| StoreError::new(StoreErrorClass::RebuildableStateCorrupt))?;
            Some(encode_conversation_cursor(key_digest, fact_id)?)
        } else {
            None
        };
        let items = selected
            .into_iter()
            .map(|(fact_id, kind)| conversation::load_entry(&self.connection, fact_id, kind))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page::new(items, next_cursor))
    }

    pub(super) fn load_agent_snapshot(&self) -> Result<AgentProjectionSnapshot, StoreError> {
        repair::load(&self.connection)?;
        authority::load(&self.connection)?;
        conversation::load(&self.connection)?;
        agent::load(&self.connection)
    }

    pub(super) fn load_project_snapshot(&self) -> Result<ProjectProjectionSnapshot, StoreError> {
        repair::load(&self.connection)?;
        authority::load(&self.connection)?;
        conversation::load(&self.connection)?;
        agent::load(&self.connection)?;
        project::load(&self.connection)
    }

    pub(super) fn load_authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, StoreError> {
        let index = repair::load(&self.connection)?;
        let authority = authority::load(&self.connection)?;
        let conversation = conversation::load(&self.connection)?;
        let agent = agent::load(&self.connection)?;
        let project = project::load(&self.connection)?;
        let revision = operational::current_revision(&self.connection)?;
        let conversations = conversation_summaries(&index, &conversation)?;
        Ok(AuthoritativeSnapshot::with_conversations(
            revision,
            DomainSnapshot::new(authority, conversation, agent, project),
            conversations,
        ))
    }

    pub(super) fn current_revision(&self) -> Result<hq_domain::Revision, StoreError> {
        operational::current_revision(&self.connection)
    }

    pub(super) fn load_mutation_receipt(
        &self,
        command_id: hq_domain::CommandId,
    ) -> Result<Option<crate::MutationReceipt>, StoreError> {
        operational::load_receipt(&self.connection, command_id)
    }

    pub(super) fn load_outbox_intents(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::OutboxIntent>, StoreError> {
        operational::load_outbox_intents(&self.connection, limit)
    }

    pub(super) fn apply_relay_state(
        &mut self,
        mutation: crate::StoredRelayStateMutation,
    ) -> Result<(), StoreError> {
        relay::apply(&mut self.connection, mutation)
    }

    pub(super) fn load_relay_state(
        &self,
        query: &crate::StoredRelayStateQuery,
    ) -> Result<crate::StoredRelayStatePage, StoreError> {
        relay::load(&self.connection, query)
    }

    pub(super) fn load_prepared_relay_lineage(
        &self,
        fact_id: hq_domain::FactId,
        recipient: hq_domain::InstallationId,
    ) -> Result<Option<crate::StoredPreparedOutbound>, StoreError> {
        relay::load_prepared_lineage(&self.connection, fact_id, recipient)
    }

    pub(super) fn load_relay_attempt(
        &self,
        url: &str,
        wrapper_id: [u8; 32],
    ) -> Result<Option<crate::StoredRelayAttempt>, StoreError> {
        relay::load_attempt(&self.connection, url, wrapper_id)
    }

    pub(super) fn load_relay_cursor(
        &self,
        url: &str,
    ) -> Result<Option<crate::StoredCatchupCursor>, StoreError> {
        relay::load_cursor(&self.connection, url)
    }

    pub(super) fn apply_harness_state(
        &mut self,
        mutation: crate::StoredHarnessStateMutation,
    ) -> Result<crate::HarnessLeaseOutcome, StoreError> {
        harness::apply(&mut self.connection, mutation)
    }

    pub(super) fn load_harness_state(
        &self,
        limit: usize,
    ) -> Result<crate::StoredHarnessStateSnapshot, StoreError> {
        harness::load(&self.connection, limit)
    }

    pub(super) fn load_harness_delivery(
        &self,
        agent_id: hq_domain::AgentId,
        submission_id: hq_domain::MessageId,
    ) -> Result<Option<crate::StoredHarnessDelivery>, StoreError> {
        harness::load_delivery(&self.connection, agent_id, submission_id)
    }

    pub(super) fn load_runnable_harness_deliveries(
        &self,
        agent_id: hq_domain::AgentId,
        limit: usize,
    ) -> Result<Vec<crate::StoredHarnessDelivery>, StoreError> {
        harness::load_runnable_deliveries(&self.connection, agent_id, limit)
    }
}

fn conversation_summaries(
    index: &ReductionIndexSnapshot,
    snapshot: &ConversationProjectionSnapshot,
) -> Result<Vec<ConversationSummary>, StoreError> {
    let open_messages = snapshot
        .projections()
        .values()
        .filter_map(|projection| match projection {
            ConversationProjection::Message(message) => Some((message.fact_id, message.open)),
            ConversationProjection::Thread(_)
            | ConversationProjection::ActionGroup(_)
            | ConversationProjection::Activity(_)
            | ConversationProjection::ActivityRetention(_) => None,
        })
        .collect::<BTreeMap<_, _>>();

    index
        .conversation_orders()
        .iter()
        .map(|(key, order)| {
            let open_messages = order
                .iter()
                .filter(|fact_id| open_messages.get(fact_id).is_some_and(|open| *open))
                .count();
            Ok(ConversationSummary {
                key: key.clone(),
                latest_fact: order.last().copied(),
                open_messages: u32::try_from(open_messages)
                    .map_err(|_| StoreError::new(StoreErrorClass::RebuildableStateCorrupt))?,
            })
        })
        .collect()
}

fn encode_conversation_cursor(
    key_digest: [u8; 32],
    fact_id: FactId,
) -> Result<PageCursor, StoreError> {
    PageCursor::new(format!(
        "v1:{}:{}",
        lower_hex(&key_digest),
        lower_hex(fact_id.as_bytes())
    ))
    .map_err(|_| StoreError::new(StoreErrorClass::OperationalStateCorrupt))
}

fn fixed_bytes(bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
    bytes
        .try_into()
        .map_err(|_| StoreError::new(StoreErrorClass::RebuildableStateCorrupt))
}

fn decode_conversation_cursor(cursor: &PageCursor) -> Result<([u8; 32], FactId), StoreError> {
    let value = cursor.as_str();
    let mut parts = value.split(':');
    let parsed = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("v1"), Some(key), Some(fact), None) => {
            Some((decode_lower_hex(key), decode_lower_hex(fact)))
        }
        _ => None,
    };
    let Some((Some(key), Some(fact))) = parsed else {
        return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
    };
    Ok((key, FactId::from_bytes(fact)))
}

fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    result
}

fn decode_lower_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, chunk) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        output[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Some(output)
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy)]
enum IngestFailpoint {
    Never,
    Canonical(Failpoint),
    AfterReduction,
    Repair(repair::RepairFailpoint),
    AfterProjectionReplacement,
    AfterRevision,
    AfterOutbox,
    AfterCanonicalCommit,
    BeforeCommit,
    AfterCommit,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy)]
enum LocalMutationFailpoint {
    Never,
    AfterReceiptLookup,
    AfterSnapshot,
    AfterDecision,
    AfterSigning,
    Ingest(IngestFailpoint),
    AfterRejectedRevision,
    AfterReceipt,
    BeforeCommit,
    AfterCommit,
}

fn ingest_with_failpoint(
    connection: &mut Connection,
    fact: &VerifiedSemanticFact,
    policy: AuthorityPolicy,
    failpoint: IngestFailpoint,
) -> Result<IngestOutcome, StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    let outcome = ingest_in_transaction(&transaction, fact, policy, failpoint)?;
    fail_ingest_at(failpoint, IngestFailpoint::BeforeCommit)?;
    transaction.commit().map_err(sql_error)?;
    fail_ingest_at(failpoint, IngestFailpoint::AfterCommit)?;
    Ok(outcome)
}

fn ingest_in_transaction(
    transaction: &Transaction<'_>,
    fact: &VerifiedSemanticFact,
    policy: AuthorityPolicy,
    failpoint: IngestFailpoint,
) -> Result<IngestOutcome, StoreError> {
    let fact_id = fact.fact().id();
    if let Some(revision) = operational::canonical_commit_revision(transaction, fact_id)? {
        if append_in_transaction(transaction, fact, Failpoint::Never)?
            != CanonicalAppendOutcome::AlreadyPresent
        {
            return Err(StoreError::new(StoreErrorClass::OperationalStateCorrupt));
        }
        return Ok(IngestOutcome::AlreadyPresent(revision));
    }

    let previous = load_previous_rebuildable(transaction)?;

    let canonical_failpoint = match failpoint {
        IngestFailpoint::Canonical(value) => value,
        _ => Failpoint::Never,
    };
    if append_in_transaction(transaction, fact, canonical_failpoint)?
        != CanonicalAppendOutcome::Inserted
    {
        return Err(StoreError::new(StoreErrorClass::OperationalStateCorrupt));
    }
    let facts = load_facts(transaction)?;
    let complete = build_complete_snapshot(&facts, policy)?;
    fail_ingest_at(failpoint, IngestFailpoint::AfterReduction)?;
    let index = complete.normalized_index();
    let authority = complete.authority_projection_snapshot();
    let conversation = complete.conversation_projection_snapshot();
    let agent = complete.agent_projection_snapshot();
    let project = complete.project_projection_snapshot();
    validate_incremental_change(
        previous.as_ref(),
        &index,
        &authority,
        &conversation,
        &agent,
        &project,
        fact_id,
    )?;
    #[cfg(test)]
    if let IngestFailpoint::Repair(repair_failpoint) = failpoint {
        repair::replace_in_transaction_with_failpoint(
            transaction,
            &index,
            &authority,
            &conversation,
            &agent,
            &project,
            repair_failpoint,
        )?;
    } else {
        repair::patch_in_transaction(
            transaction,
            &index,
            &authority,
            &conversation,
            &agent,
            &project,
        )?;
    }
    #[cfg(not(test))]
    repair::patch_in_transaction(
        transaction,
        &index,
        &authority,
        &conversation,
        &agent,
        &project,
    )?;
    fail_ingest_at(failpoint, IngestFailpoint::AfterProjectionReplacement)?;
    let revision = operational::allocate_revision(transaction)?;
    fail_ingest_at(failpoint, IngestFailpoint::AfterRevision)?;
    if fact_is_admitted(&index, fact_id) {
        for recipient in outbox_recipients(fact, policy, &authority) {
            let intent = OutboxIntent::new(
                fact_id,
                recipient,
                fact.verified_event().exact_event_bytes().to_vec(),
                revision,
            )
            .map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))?;
            operational::put_outbox_intent(transaction, &intent)?;
        }
    }
    fail_ingest_at(failpoint, IngestFailpoint::AfterOutbox)?;
    operational::put_canonical_commit(transaction, fact_id, revision)?;
    fail_ingest_at(failpoint, IngestFailpoint::AfterCanonicalCommit)?;
    Ok(IngestOutcome::Inserted(revision))
}

struct PreviousRebuildable {
    index: ReductionIndexSnapshot,
    authority: AuthorityProjectionSnapshot,
    conversation: ConversationProjectionSnapshot,
    agent: AgentProjectionSnapshot,
    project: ProjectProjectionSnapshot,
}

fn load_previous_rebuildable(
    transaction: &Transaction<'_>,
) -> Result<Option<PreviousRebuildable>, StoreError> {
    let index = match repair::load(transaction) {
        Ok(index) => index,
        Err(error) if error.class() == StoreErrorClass::NotRepaired => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(PreviousRebuildable {
        index,
        authority: authority::load(transaction)?,
        conversation: conversation::load(transaction)?,
        agent: agent::load(transaction)?,
        project: project::load(transaction)?,
    }))
}

#[allow(clippy::too_many_arguments)]
fn validate_incremental_change(
    previous: Option<&PreviousRebuildable>,
    current: &ReductionIndexSnapshot,
    authority: &AuthorityProjectionSnapshot,
    conversation: &ConversationProjectionSnapshot,
    agent: &AgentProjectionSnapshot,
    project: &ProjectProjectionSnapshot,
    delta: FactId,
) -> Result<(), StoreError> {
    let Some(previous) = previous else {
        return Ok(());
    };
    let affected = if previous.index.policy == current.policy {
        affected_union(&previous.index, current, [delta])
    } else {
        previous
            .index
            .affected_dependencies
            .keys()
            .chain(current.affected_dependencies.keys())
            .copied()
            .chain(std::iter::once(delta))
            .collect()
    };
    let decisions_are_covered = previous
        .index
        .decisions
        .keys()
        .chain(current.decisions.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| previous.index.decisions.get(key) != current.decisions.get(key))
        .all(|(_, fact_id)| affected.contains(&fact_id));
    if !decisions_are_covered
        || !projection_changes_are_covered(
            previous.authority.frontiers(),
            authority.frontiers(),
            previous.authority.projections(),
            authority.projections(),
            previous.authority.support(),
            authority.support(),
            &affected,
        )
        || !projection_changes_are_covered(
            previous.conversation.frontiers(),
            conversation.frontiers(),
            previous.conversation.projections(),
            conversation.projections(),
            previous.conversation.support(),
            conversation.support(),
            &affected,
        )
        || !projection_changes_are_covered(
            previous.agent.frontiers(),
            agent.frontiers(),
            previous.agent.projections(),
            agent.projections(),
            previous.agent.support(),
            agent.support(),
            &affected,
        )
        || !projection_changes_are_covered(
            previous.project.frontiers(),
            project.frontiers(),
            previous.project.projections(),
            project.projections(),
            previous.project.support(),
            project.support(),
            &affected,
        )
    {
        return Err(StoreError::new(StoreErrorClass::ReductionFailed));
    }
    Ok(())
}

fn affected_union(
    previous: &ReductionIndexSnapshot,
    current: &ReductionIndexSnapshot,
    roots: impl IntoIterator<Item = FactId>,
) -> BTreeSet<FactId> {
    let mut affected = BTreeSet::new();
    let mut pending = roots.into_iter().collect::<VecDeque<_>>();
    while let Some(fact_id) = pending.pop_front() {
        if affected.insert(fact_id) {
            for index in [previous, current] {
                pending.extend(
                    index
                        .affected_dependencies
                        .get(&fact_id)
                        .into_iter()
                        .flatten()
                        .copied(),
                );
            }
        }
    }
    affected
}

fn projection_changes_are_covered<A, K, V>(
    previous_frontiers: &BTreeMap<A, BTreeSet<FactId>>,
    current_frontiers: &BTreeMap<A, BTreeSet<FactId>>,
    previous_projections: &BTreeMap<K, V>,
    current_projections: &BTreeMap<K, V>,
    previous_support: &BTreeMap<K, BTreeSet<FactId>>,
    current_support: &BTreeMap<K, BTreeSet<FactId>>,
    affected: &BTreeSet<FactId>,
) -> bool
where
    A: Ord,
    K: Ord,
    V: PartialEq,
{
    let frontiers_covered = previous_frontiers
        .keys()
        .chain(current_frontiers.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| previous_frontiers.get(*key) != current_frontiers.get(*key))
        .all(|key| {
            previous_frontiers
                .get(key)
                .into_iter()
                .chain(current_frontiers.get(key))
                .flatten()
                .any(|fact_id| affected.contains(fact_id))
        });
    frontiers_covered
        && previous_projections
            .keys()
            .chain(current_projections.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|key| {
                previous_projections.get(*key) != current_projections.get(*key)
                    || previous_support.get(*key) != current_support.get(*key)
            })
            .all(|key| {
                previous_support
                    .get(key)
                    .into_iter()
                    .chain(current_support.get(key))
                    .flatten()
                    .any(|fact_id| affected.contains(fact_id))
            })
}

fn execute_local_mutation_with_failpoint(
    connection: &mut Connection,
    request: LocalMutationRequest,
    failpoint: LocalMutationFailpoint,
) -> Result<(MutationReceipt, bool), StoreError> {
    let (command_id, request_digest, policy, signer, decide) = request.into_parts();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    if let Some(receipt) = operational::load_receipt(&transaction, command_id)? {
        if receipt.request_digest() != request_digest {
            return Err(StoreError::new(StoreErrorClass::MutationConflict));
        }
        transaction.commit().map_err(sql_error)?;
        return Ok((receipt, false));
    }
    fail_local_at(failpoint, LocalMutationFailpoint::AfterReceiptLookup)?;

    let facts = load_facts(&transaction)?;
    let snapshot = build_complete_snapshot(&facts, policy)?;
    fail_local_at(failpoint, LocalMutationFailpoint::AfterSnapshot)?;
    let (kind, result, revision) = match decide(&snapshot).into_parts() {
        LocalMutationDecisionParts::Commit(commit) => {
            fail_local_at(failpoint, LocalMutationFailpoint::AfterDecision)?;
            let fact = commit
                .plan
                .sign(&signer, commit.auxiliary_randomness)
                .map_err(|_| StoreError::new(StoreErrorClass::InvalidOperationalRequest))?;
            let fact_id = fact.fact().id();
            fail_local_at(failpoint, LocalMutationFailpoint::AfterSigning)?;
            let ingest_failpoint = match failpoint {
                LocalMutationFailpoint::Ingest(value) => value,
                _ => IngestFailpoint::Never,
            };
            let IngestOutcome::Inserted(revision) =
                ingest_in_transaction(&transaction, &fact, policy, ingest_failpoint)?
            else {
                return Err(StoreError::new(StoreErrorClass::OperationalStateCorrupt));
            };
            if !fact_is_admitted(&repair::load(&transaction)?, fact_id) {
                return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
            }
            (MutationResultKind::Committed, commit.result, revision)
        }
        LocalMutationDecisionParts::Reject(result) => {
            fail_local_at(failpoint, LocalMutationFailpoint::AfterDecision)?;
            let revision = operational::allocate_revision(&transaction)?;
            fail_local_at(failpoint, LocalMutationFailpoint::AfterRejectedRevision)?;
            (MutationResultKind::Rejected, result, revision)
        }
    };
    let receipt = MutationReceipt::new(command_id, request_digest, kind, result, revision);
    if operational::put_receipt(&transaction, &receipt)? != operational::PutOutcome::Inserted {
        return Err(StoreError::new(StoreErrorClass::OperationalStateCorrupt));
    }
    fail_local_at(failpoint, LocalMutationFailpoint::AfterReceipt)?;
    fail_local_at(failpoint, LocalMutationFailpoint::BeforeCommit)?;
    transaction.commit().map_err(sql_error)?;
    fail_local_at(failpoint, LocalMutationFailpoint::AfterCommit)?;
    Ok((receipt, true))
}

fn fact_is_admitted(index: &ReductionIndexSnapshot, fact_id: FactId) -> bool {
    crate::ReductionDomain::ALL.into_iter().any(|domain| {
        index
            .decision(domain, fact_id)
            .is_some_and(|decision| decision.status() == DecisionStatus::Projected)
    })
}

fn outbox_recipients(
    fact: &VerifiedSemanticFact,
    policy: AuthorityPolicy,
    authority: &AuthorityProjectionSnapshot,
) -> BTreeSet<InstallationId> {
    let mut recipients = BTreeSet::new();
    match fact.fact().scope() {
        FactScope::InstallationPrivate(_) => {}
        FactScope::PeerAddressed(mailbox) => {
            recipients.insert(mailbox.installation_id());
        }
        FactScope::AccountAddressed(account) => {
            add_account_recipients(&mut recipients, *account, authority);
        }
        FactScope::RemoteControl {
            account_id,
            target_home,
        } => {
            add_account_recipients(&mut recipients, *account_id, authority);
            recipients.insert(*target_home);
        }
    }
    match fact.fact().payload() {
        hq_domain::SemanticPayload::HumanDeviceGranted { device, .. } => {
            recipients.insert(device.installation_id());
        }
        hq_domain::SemanticPayload::HumanDeviceRevoked { device_id, .. } => {
            recipients.insert(*device_id);
        }
        _ => {}
    }
    recipients.remove(&fact.fact().author().installation_id());
    recipients.remove(&policy.local_installation());
    recipients
}

fn add_account_recipients(
    recipients: &mut BTreeSet<InstallationId>,
    account: hq_domain::AccountId,
    authority: &AuthorityProjectionSnapshot,
) {
    if let Some(AuthorityProjection::Account { creator, .. }) =
        authority.projection(AuthorityProjectionKey::Account(account))
    {
        recipients.insert(creator.installation_id());
    }
    for (key, projection) in authority.projections() {
        if let (
            AuthorityProjectionKey::Membership {
                account: candidate,
                device,
            },
            AuthorityProjection::Membership(view),
        ) = (key, projection)
            && *candidate == account
            && view.state() == MembershipState::Active
        {
            recipients.insert(*device);
        }
    }
}

#[allow(clippy::unnecessary_wraps)]
fn fail_ingest_at(actual: IngestFailpoint, expected: IngestFailpoint) -> Result<(), StoreError> {
    #[cfg(test)]
    if std::mem::discriminant(&actual) == std::mem::discriminant(&expected) {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    #[cfg(not(test))]
    let _ = (actual, expected);
    Ok(())
}

#[allow(clippy::unnecessary_wraps)]
fn fail_local_at(
    actual: LocalMutationFailpoint,
    expected: LocalMutationFailpoint,
) -> Result<(), StoreError> {
    #[cfg(test)]
    if std::mem::discriminant(&actual) == std::mem::discriminant(&expected) {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    #[cfg(not(test))]
    let _ = (actual, expected);
    Ok(())
}

fn load_facts(connection: &Connection) -> Result<Vec<VerifiedSemanticFact>, StoreError> {
    let count: i64 = connection
        .query_row("SELECT count(*) FROM canonical_facts", [], |row| row.get(0))
        .map_err(sql_error)?;
    if !(0..=MAXIMUM_CORPUS_FACTS).contains(&count) {
        return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
    }
    let capacity =
        usize::try_from(count).map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))?;
    let mut statement = connection
        .prepare(
            "SELECT fact_id, length(event_bytes), namespace, family \
             FROM canonical_facts ORDER BY fact_id",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(StoredFact {
                fact_id: row.get(0)?,
                event_length: row.get(1)?,
                namespace: row.get(2)?,
                family: row.get(3)?,
            })
        })
        .map_err(sql_error)?;
    let stored = rows
        .map(|row| row.map_err(sql_error))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut facts = Vec::with_capacity(capacity);
    for stored in stored {
        let fact_id = validate_stored_fact_shape(&stored)?;
        let event_bytes = connection
            .query_row(
                "SELECT event_bytes FROM canonical_facts WHERE fact_id = ?1",
                [fact_id.as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .map_err(sql_error)?;
        if event_bytes.len()
            != usize::try_from(stored.event_length)
                .map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))?
        {
            return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
        }
        let verified = verify_event(event_bytes)?;
        let index = index_for(&verified);
        if fact_id != index.fact_id
            || stored.namespace != index.namespace
            || stored.family != index.family
        {
            return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
        }
        let stored_parents = load_parents(connection, &index.fact_id)?;
        let stored_authorities = load_authorities(connection, &index.fact_id)?;
        if stored_parents != index.parents || stored_authorities != index.authorities {
            return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
        }
        facts.push(verified);
    }
    Ok(facts)
}

fn inspect_existing(connection: &Connection) -> Result<bool, StoreError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(sql_error)?;
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql_error)?;
    if application_id == 0 && user_version == 0 {
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema \
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if tables == 0 {
            verify_integrity(connection)?;
            return Ok(true);
        }
    }
    verify_schema(connection)?;
    verify_integrity(connection)?;
    Ok(false)
}

fn configure(connection: &Connection) -> Result<(), StoreError> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(sql_error)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(sql_error)?;
    connection
        .pragma_update(None, "trusted_schema", false)
        .map_err(sql_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(sql_error)?;
    let journal: String = connection
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .map_err(sql_error)?;
    if !journal.eq_ignore_ascii_case("wal")
        || !connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
            .map_err(sql_error)?
        || !connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
            .map(|value| !value)
            .map_err(sql_error)?
    {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    let foreign_keys: i64 = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .map_err(sql_error)?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(sql_error)?;
    if foreign_keys != 1 || synchronous != 2 {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    Ok(())
}

fn initialize_schema(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(sql_error)?;
    transaction.execute_batch(SCHEMA).map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO storage_metadata(singleton, schema_marker) VALUES (1, ?1)",
            [SCHEMA_MARKER],
        )
        .map_err(sql_error)?;
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(sql_error)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

fn verify_schema(connection: &Connection) -> Result<(), StoreError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(sql_error)?;
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql_error)?;
    if application_id != APPLICATION_ID || user_version != SCHEMA_VERSION {
        return Err(StoreError::new(StoreErrorClass::IncompatibleSchema));
    }
    let table_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if table_count != i64::try_from(SCHEMA_TABLES.len()).unwrap_or(i64::MAX) {
        return Err(StoreError::new(StoreErrorClass::IncompatibleSchema));
    }
    for table in SCHEMA_TABLES {
        let present: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if present != 1 {
            return Err(StoreError::new(StoreErrorClass::IncompatibleSchema));
        }
    }
    let index_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'index' AND sql IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if index_count != i64::try_from(SCHEMA_INDEXES.len()).unwrap_or(i64::MAX) {
        return Err(StoreError::new(StoreErrorClass::IncompatibleSchema));
    }
    for index in SCHEMA_INDEXES {
        let present: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                [index],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if present != 1 {
            return Err(StoreError::new(StoreErrorClass::IncompatibleSchema));
        }
    }
    let marker: String = connection
        .query_row(
            "SELECT schema_marker FROM storage_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StoreError::new(StoreErrorClass::IncompatibleSchema))?;
    if marker != SCHEMA_MARKER {
        return Err(StoreError::new(StoreErrorClass::IncompatibleSchema));
    }
    Ok(())
}

fn verify_integrity(connection: &Connection) -> Result<(), StoreError> {
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(sql_error)?;
    if integrity != "ok" {
        return Err(StoreError::new(StoreErrorClass::CorruptDatabase));
    }
    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()
        .map_err(sql_error)?;
    if foreign_key_violation.is_some() {
        return Err(StoreError::new(StoreErrorClass::CorruptDatabase));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Failpoint {
    Never,
    #[cfg(test)]
    AfterFact,
    #[cfg(test)]
    AfterParents,
    #[cfg(test)]
    AfterAuthorities,
    #[cfg(test)]
    BeforeCommit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CanonicalAppendOutcome {
    Inserted,
    AlreadyPresent,
}

#[cfg(test)]
fn append_with_failpoint(
    connection: &mut Connection,
    fact: &VerifiedSemanticFact,
    failpoint: Failpoint,
) -> Result<CanonicalAppendOutcome, StoreError> {
    let transaction = connection.transaction().map_err(sql_error)?;
    let outcome = append_in_transaction(&transaction, fact, failpoint)?;
    #[cfg(test)]
    if matches!(failpoint, Failpoint::BeforeCommit) {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    transaction.commit().map_err(sql_error)?;
    Ok(outcome)
}

fn append_in_transaction(
    transaction: &Transaction<'_>,
    fact: &VerifiedSemanticFact,
    failpoint: Failpoint,
) -> Result<CanonicalAppendOutcome, StoreError> {
    #[cfg(not(test))]
    let _ = failpoint;
    let index = index_for(fact);
    let event_bytes = fact.verified_event().exact_event_bytes();
    if immutable_row_exists(transaction, &index.fact_id)? {
        return if immutable_row_equal(transaction, &index, event_bytes)? {
            Ok(CanonicalAppendOutcome::AlreadyPresent)
        } else {
            Err(StoreError::new(StoreErrorClass::IdentityCollision))
        };
    }
    transaction
        .execute(
            "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                index.fact_id.as_slice(),
                event_bytes,
                index.namespace,
                index.family
            ],
        )
        .map_err(sql_error)?;
    #[cfg(test)]
    if matches!(failpoint, Failpoint::AfterFact) {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    for parent in &index.parents {
        transaction
            .execute(
                "INSERT INTO fact_parents(fact_id, parent_id) VALUES (?1, ?2)",
                params![index.fact_id.as_slice(), parent.as_slice()],
            )
            .map_err(sql_error)?;
    }
    #[cfg(test)]
    if matches!(failpoint, Failpoint::AfterParents) {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    for (role, authority) in &index.authorities {
        transaction
            .execute(
                "INSERT INTO fact_authorities(fact_id, authority_role, authority_fact_id) \
                 VALUES (?1, ?2, ?3)",
                params![index.fact_id.as_slice(), role, authority.as_slice()],
            )
            .map_err(sql_error)?;
    }
    #[cfg(test)]
    if matches!(failpoint, Failpoint::AfterAuthorities) {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    Ok(CanonicalAppendOutcome::Inserted)
}

fn immutable_row_exists(
    transaction: &Transaction<'_>,
    fact_id: &[u8; 32],
) -> Result<bool, StoreError> {
    transaction
        .query_row(
            "SELECT 1 FROM canonical_facts WHERE fact_id = ?1",
            [fact_id.as_slice()],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(sql_error)
}

fn immutable_row_equal(
    transaction: &Transaction<'_>,
    expected: &FactIndex,
    event_bytes: &[u8],
) -> Result<bool, StoreError> {
    let row = transaction
        .query_row(
            "SELECT event_bytes, namespace, family FROM canonical_facts WHERE fact_id = ?1",
            [expected.fact_id.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .map_err(sql_error)?;
    Ok(row.0 == event_bytes
        && row.1 == expected.namespace
        && row.2 == expected.family
        && load_parents(transaction, &expected.fact_id)? == expected.parents
        && load_authorities(transaction, &expected.fact_id)? == expected.authorities)
}

struct StoredFact {
    fact_id: Vec<u8>,
    event_length: i64,
    namespace: i64,
    family: i64,
}

fn validate_stored_fact_shape(stored: &StoredFact) -> Result<[u8; 32], StoreError> {
    let maximum = i64::try_from(MAX_EVENT_BYTES)
        .map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))?;
    if !(0..=maximum).contains(&stored.event_length) {
        return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
    }
    fixed_id(stored.fact_id.clone())
}

fn verify_event(event_bytes: Vec<u8>) -> Result<VerifiedSemanticFact, StoreError> {
    let event = RawEventBytes::new(event_bytes)
        .and_then(RawEventBytes::parse)
        .and_then(hq_protocol::ParsedOuterEvent::verify)
        .and_then(hq_protocol::CryptographicallyVerifiedEvent::dispatch)
        .map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))?;
    let DispatchOutcome::Supported(supported) = event else {
        return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
    };
    supported
        .decode_v1()
        .and_then(hq_protocol::VerifiedSupportedRecord::into_semantic_fact)
        .map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))
}

struct FactIndex {
    fact_id: [u8; 32],
    namespace: i64,
    family: i64,
    parents: Vec<[u8; 32]>,
    authorities: Vec<(i64, [u8; 32])>,
}

fn index_for(fact: &VerifiedSemanticFact) -> FactIndex {
    let semantic = fact.fact();
    let parents = semantic
        .causal()
        .parents()
        .iter()
        .map(|parent| *parent.as_bytes())
        .collect();
    let authorities = AuthorityRole::ALL
        .into_iter()
        .filter_map(|role| {
            semantic
                .causal()
                .authority(role)
                .map(|authority| (encode_role(role), *authority.as_bytes()))
        })
        .collect();
    FactIndex {
        fact_id: *semantic.id().as_bytes(),
        namespace: match fact.namespace() {
            ProtocolNamespace::Canonical => 1,
            ProtocolNamespace::Control => 2,
        },
        family: i64::try_from(fact.family()).unwrap_or(i64::MAX),
        parents,
        authorities,
    }
}

fn load_parents(connection: &Connection, fact_id: &[u8; 32]) -> Result<Vec<[u8; 32]>, StoreError> {
    let count = related_count(connection, "fact_parents", fact_id, MAX_FACT_PARENTS)?;
    let mut statement = connection
        .prepare("SELECT parent_id FROM fact_parents WHERE fact_id = ?1 ORDER BY parent_id")
        .map_err(sql_error)?;
    let rows = statement
        .query_map([fact_id.as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(sql_error)?;
    let mut parents = Vec::with_capacity(count);
    for row in rows {
        parents.push(fixed_id(row.map_err(sql_error)?)?);
    }
    Ok(parents)
}

fn load_authorities(
    connection: &Connection,
    fact_id: &[u8; 32],
) -> Result<Vec<(i64, [u8; 32])>, StoreError> {
    let count = related_count(
        connection,
        "fact_authorities",
        fact_id,
        MAX_FACT_AUTHORITIES,
    )?;
    let mut statement = connection
        .prepare(
            "SELECT authority_role, authority_fact_id FROM fact_authorities \
             WHERE fact_id = ?1 ORDER BY authority_role",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([fact_id.as_slice()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(sql_error)?;
    let mut authorities = Vec::with_capacity(count);
    for row in rows {
        let (role, id) = row.map_err(sql_error)?;
        if !(1..=13).contains(&role) {
            return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
        }
        authorities.push((role, fixed_id(id)?));
    }
    Ok(authorities)
}

fn related_count(
    connection: &Connection,
    table: &str,
    fact_id: &[u8; 32],
    maximum: usize,
) -> Result<usize, StoreError> {
    let sql = match table {
        "fact_parents" => "SELECT count(*) FROM fact_parents WHERE fact_id = ?1",
        "fact_authorities" => "SELECT count(*) FROM fact_authorities WHERE fact_id = ?1",
        _ => return Err(StoreError::new(StoreErrorClass::InvalidEvidence)),
    };
    let count: i64 = connection
        .query_row(sql, [fact_id.as_slice()], |row| row.get(0))
        .map_err(sql_error)?;
    let count =
        usize::try_from(count).map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))?;
    if count > maximum {
        return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
    }
    Ok(count)
}

fn fixed_id(bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
    bytes
        .try_into()
        .map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))
}

const fn encode_role(role: AuthorityRole) -> i64 {
    match role {
        AuthorityRole::LocalInstallation => 1,
        AuthorityRole::MailboxOwner => 2,
        AuthorityRole::MailboxGrant => 3,
        AuthorityRole::AccountCreator => 4,
        AuthorityRole::DeviceGrant => 5,
        AuthorityRole::AccountMembership => 6,
        AuthorityRole::PreviousState => 7,
        AuthorityRole::ProjectHome => 8,
        AuthorityRole::ActiveHuman => 9,
        AuthorityRole::Assignment => 10,
        AuthorityRole::Dispatch => 11,
        AuthorityRole::Request => 12,
        AuthorityRole::OutputBinding => 13,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn sql_error(error: SqlError) -> StoreError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => {
            StoreError::new(StoreErrorClass::CorruptDatabase)
        }
        _ => StoreError::new(StoreErrorClass::DatabaseUnavailable),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    };

    use hq_protocol::{Bip340Signer, CanonicalEventPlan};

    const CONTENT: &str = r#"{"p":"hq/canonical","v":1,"f":2,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":1000,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[["c","3333333333333333333333333333333333333333333333333333333333333333"]],"auth":[["local-installation","c","3333333333333333333333333333333333333333333333333333333333333333"]],"body":{"mailbox":"4444444444444444444444444444444444444444444444444444444444444444","kind":"agent","label":"helper"}}"#;

    #[test]
    fn uncommitted_transactions_roll_back_on_drop() {
        let mut connection = Connection::open_in_memory().expect("memory database opens");
        connection
            .execute_batch("CREATE TABLE values_for_test(value INTEGER NOT NULL);")
            .expect("table creates");
        {
            let transaction = connection.transaction().expect("transaction starts");
            transaction
                .execute("INSERT INTO values_for_test VALUES (1)", [])
                .expect("row inserts");
        }
        let count: i64 = connection
            .query_row("SELECT count(*) FROM values_for_test", [], |row| row.get(0))
            .expect("count reads");
        assert_eq!(count, 0);
    }

    #[test]
    fn stable_role_codes_cover_every_closed_role() {
        assert_eq!(
            AuthorityRole::ALL.map(encode_role),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]
        );
    }

    #[test]
    fn append_failpoints_roll_back_every_write_group() {
        let fact = fixture();
        for failpoint in [
            Failpoint::AfterFact,
            Failpoint::AfterParents,
            Failpoint::AfterAuthorities,
            Failpoint::BeforeCommit,
        ] {
            let mut connection = Connection::open_in_memory().expect("memory database opens");
            connection.execute_batch(SCHEMA).expect("schema creates");
            let error = append_with_failpoint(&mut connection, &fact, failpoint)
                .expect_err("failpoint interrupts append");
            assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
            let count: i64 = connection
                .query_row("SELECT count(*) FROM canonical_facts", [], |row| row.get(0))
                .expect("count reads");
            assert_eq!(count, 0);
            let parent_count: i64 = connection
                .query_row("SELECT count(*) FROM fact_parents", [], |row| row.get(0))
                .expect("parent count reads");
            let authority_count: i64 = connection
                .query_row("SELECT count(*) FROM fact_authorities", [], |row| {
                    row.get(0)
                })
                .expect("authority count reads");
            assert_eq!((parent_count, authority_count), (0, 0));
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn atomic_ingest_failpoints_reopen_to_the_complete_old_or_new_state() {
        use super::repair::RepairFailpoint;
        use std::{
            fs,
            sync::atomic::{AtomicU64, Ordering},
        };

        static NEXT: AtomicU64 = AtomicU64::new(1);
        let repair_failpoints = [
            RepairFailpoint::AfterClear,
            RepairFailpoint::AfterVertices,
            RepairFailpoint::AfterReverseDependencies,
            RepairFailpoint::AfterDecisions,
            RepairFailpoint::AfterDependencyOrder,
            RepairFailpoint::AfterPresentationOrder,
            RepairFailpoint::AfterConflicts,
            RepairFailpoint::AfterState,
            RepairFailpoint::AfterAuthorityInsert,
            RepairFailpoint::AfterAuthorityVerification,
            RepairFailpoint::AfterConversationInsert,
            RepairFailpoint::AfterConversationVerification,
            RepairFailpoint::AfterAgentInsert,
            RepairFailpoint::AfterAgentVerification,
            RepairFailpoint::AfterProjectInsert,
            RepairFailpoint::AfterProjectVerification,
            RepairFailpoint::AfterVerification,
        ];
        let failpoints = [
            IngestFailpoint::Canonical(Failpoint::AfterFact),
            IngestFailpoint::Canonical(Failpoint::AfterParents),
            IngestFailpoint::Canonical(Failpoint::AfterAuthorities),
            IngestFailpoint::AfterReduction,
            IngestFailpoint::AfterProjectionReplacement,
            IngestFailpoint::AfterRevision,
            IngestFailpoint::AfterOutbox,
            IngestFailpoint::AfterCanonicalCommit,
            IngestFailpoint::BeforeCommit,
        ]
        .into_iter()
        .chain(repair_failpoints.into_iter().map(IngestFailpoint::Repair));
        let policy = AuthorityPolicy::new(
            hq_domain::InstallationId::from_bytes([0x11; 32]),
            hq_domain::MailboxId::from_bytes([0x44; 32]),
        );

        for failpoint in failpoints {
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "hq-rust-atomic-ingest-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&root).expect("test root creates");
            let path = root.join("state").join("hq.sqlite3");
            let mut database = Database::open(&path).expect("database opens");
            assert_eq!(
                database
                    .ingest(&root_fixture(), policy)
                    .expect("root ingests"),
                IngestOutcome::Inserted(hq_domain::Revision::new(1))
            );
            let old_index = database.load_reduction_index().expect("old index loads");
            let old_authority = database
                .load_authority_snapshot()
                .expect("old authority loads");
            let old_conversation = database
                .load_conversation_snapshot()
                .expect("old conversation loads");
            let old_agent = database.load_agent_snapshot().expect("old agent loads");
            let old_project = database.load_project_snapshot().expect("old project loads");
            let error =
                ingest_with_failpoint(&mut database.connection, &fixture(), policy, failpoint)
                    .expect_err("failpoint interrupts ingest");
            assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
            drop(database);

            let mut reopened = Database::open(&path).expect("database reopens");
            assert_eq!(reopened.load().expect("old corpus loads").len(), 1);
            assert_eq!(
                reopened.current_revision().expect("old revision loads"),
                hq_domain::Revision::new(1)
            );
            assert_eq!(
                reopened.load_reduction_index().expect("old index remains"),
                old_index
            );
            assert_eq!(
                reopened
                    .load_authority_snapshot()
                    .expect("old authority remains"),
                old_authority
            );
            assert_eq!(
                reopened
                    .load_conversation_snapshot()
                    .expect("old conversation remains"),
                old_conversation
            );
            assert_eq!(
                reopened.load_agent_snapshot().expect("old agent remains"),
                old_agent
            );
            assert_eq!(
                reopened
                    .load_project_snapshot()
                    .expect("old project remains"),
                old_project
            );
            assert_eq!(
                reopened.ingest(&fixture(), policy).expect("retry succeeds"),
                IngestOutcome::Inserted(hq_domain::Revision::new(2))
            );
            drop(reopened);
            fs::remove_dir_all(root).expect("test state cleans up");
        }
    }

    #[test]
    fn response_loss_after_commit_replays_the_original_revision_without_changes() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        connection.execute_batch(SCHEMA).expect("schema creates");
        let policy = AuthorityPolicy::new(
            hq_domain::InstallationId::from_bytes([0x11; 32]),
            hq_domain::MailboxId::from_bytes([0x44; 32]),
        );
        let fact = root_fixture();
        let error =
            ingest_with_failpoint(&mut connection, &fact, policy, IngestFailpoint::AfterCommit)
                .expect_err("response loss is simulated after commit");
        assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
        assert_eq!(
            ingest_with_failpoint(&mut connection, &fact, policy, IngestFailpoint::Never)
                .expect("retry finds canonical commit"),
            IngestOutcome::AlreadyPresent(hq_domain::Revision::new(1))
        );
        assert_eq!(
            operational::current_revision(&connection).expect("revision stays stable"),
            hq_domain::Revision::new(1)
        );
        assert_eq!(load_facts(&connection).expect("corpus loads").len(), 1);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn local_mutation_failpoints_reopen_to_the_complete_old_or_new_state() {
        use super::repair::RepairFailpoint;
        use std::{
            fs,
            sync::atomic::{AtomicU64, Ordering},
        };

        static NEXT: AtomicU64 = AtomicU64::new(1);
        let repair_failpoints = [
            RepairFailpoint::AfterClear,
            RepairFailpoint::AfterVertices,
            RepairFailpoint::AfterReverseDependencies,
            RepairFailpoint::AfterDecisions,
            RepairFailpoint::AfterDependencyOrder,
            RepairFailpoint::AfterPresentationOrder,
            RepairFailpoint::AfterConflicts,
            RepairFailpoint::AfterState,
            RepairFailpoint::AfterAuthorityInsert,
            RepairFailpoint::AfterAuthorityVerification,
            RepairFailpoint::AfterConversationInsert,
            RepairFailpoint::AfterConversationVerification,
            RepairFailpoint::AfterAgentInsert,
            RepairFailpoint::AfterAgentVerification,
            RepairFailpoint::AfterProjectInsert,
            RepairFailpoint::AfterProjectVerification,
            RepairFailpoint::AfterVerification,
        ];
        let failpoints = [
            LocalMutationFailpoint::AfterReceiptLookup,
            LocalMutationFailpoint::AfterSnapshot,
            LocalMutationFailpoint::AfterDecision,
            LocalMutationFailpoint::AfterSigning,
            LocalMutationFailpoint::Ingest(IngestFailpoint::Canonical(Failpoint::AfterFact)),
            LocalMutationFailpoint::Ingest(IngestFailpoint::Canonical(Failpoint::AfterParents)),
            LocalMutationFailpoint::Ingest(IngestFailpoint::Canonical(Failpoint::AfterAuthorities)),
            LocalMutationFailpoint::Ingest(IngestFailpoint::AfterReduction),
            LocalMutationFailpoint::Ingest(IngestFailpoint::AfterProjectionReplacement),
            LocalMutationFailpoint::Ingest(IngestFailpoint::AfterRevision),
            LocalMutationFailpoint::Ingest(IngestFailpoint::AfterOutbox),
            LocalMutationFailpoint::Ingest(IngestFailpoint::AfterCanonicalCommit),
            LocalMutationFailpoint::AfterReceipt,
            LocalMutationFailpoint::BeforeCommit,
        ]
        .into_iter()
        .chain(
            repair_failpoints
                .into_iter()
                .map(|repair| LocalMutationFailpoint::Ingest(IngestFailpoint::Repair(repair))),
        );
        let policy = local_policy();
        let command_id = hq_domain::CommandId::from_bytes([0x81; 32]);

        for failpoint in failpoints {
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "hq-rust-local-mutation-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&root).expect("test root creates");
            let path = root.join("state").join("hq.sqlite3");
            let mut database = Database::open(&path).expect("database opens");
            database
                .ingest(&fixture(), policy)
                .expect("old unresolved fact ingests");
            let old_index = database.load_reduction_index().expect("old index loads");
            let old_authority = database
                .load_authority_snapshot()
                .expect("old authority loads");
            let old_conversation = database
                .load_conversation_snapshot()
                .expect("old conversation loads");
            let old_agent = database.load_agent_snapshot().expect("old agent loads");
            let old_project = database.load_project_snapshot().expect("old project loads");

            let error = execute_local_mutation_with_failpoint(
                &mut database.connection,
                committed_local_request(command_id, [0x82; 32], None),
                failpoint,
            )
            .expect_err("failpoint interrupts local mutation");
            assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
            drop(database);

            let mut reopened = Database::open(&path).expect("database reopens");
            assert_eq!(reopened.load().expect("old corpus loads").len(), 1);
            assert_eq!(
                reopened.current_revision().expect("old revision loads"),
                hq_domain::Revision::new(1)
            );
            assert_eq!(
                reopened
                    .load_mutation_receipt(command_id)
                    .expect("receipt query succeeds"),
                None
            );
            assert_eq!(
                reopened.load_reduction_index().expect("old index remains"),
                old_index
            );
            assert_eq!(
                reopened
                    .load_authority_snapshot()
                    .expect("old authority remains"),
                old_authority
            );
            assert_eq!(
                reopened
                    .load_conversation_snapshot()
                    .expect("old conversation remains"),
                old_conversation
            );
            assert_eq!(
                reopened.load_agent_snapshot().expect("old agent remains"),
                old_agent
            );
            assert_eq!(
                reopened
                    .load_project_snapshot()
                    .expect("old project remains"),
                old_project
            );
            let (receipt, inserted) = reopened
                .execute_local_mutation(committed_local_request(command_id, [0x82; 32], None))
                .expect("retry commits complete new state");
            assert!(inserted);
            assert_eq!(receipt.revision(), hq_domain::Revision::new(2));
            assert_eq!(reopened.load().expect("new corpus loads").len(), 2);
            drop(reopened);
            fs::remove_dir_all(root).expect("test state cleans up");
        }
    }

    #[test]
    fn rejected_local_failpoints_roll_back_revision_and_receipt() {
        let command_id = hq_domain::CommandId::from_bytes([0x91; 32]);
        for failpoint in [
            LocalMutationFailpoint::AfterReceiptLookup,
            LocalMutationFailpoint::AfterSnapshot,
            LocalMutationFailpoint::AfterDecision,
            LocalMutationFailpoint::AfterRejectedRevision,
            LocalMutationFailpoint::AfterReceipt,
            LocalMutationFailpoint::BeforeCommit,
        ] {
            let mut connection = Connection::open_in_memory().expect("database opens");
            connection.execute_batch(SCHEMA).expect("schema creates");
            let error = execute_local_mutation_with_failpoint(
                &mut connection,
                rejected_local_request(command_id, [0x92; 32]),
                failpoint,
            )
            .expect_err("failpoint interrupts rejection");
            assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
            assert_eq!(
                operational::current_revision(&connection).expect("revision loads"),
                hq_domain::Revision::new(0)
            );
            assert_eq!(
                operational::load_receipt(&connection, command_id).expect("receipt query succeeds"),
                None
            );
            assert!(load_facts(&connection).expect("corpus loads").is_empty());
        }
    }

    #[test]
    fn lost_local_response_replays_receipt_without_deciding_or_signing_again() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        connection.execute_batch(SCHEMA).expect("schema creates");
        let command_id = hq_domain::CommandId::from_bytes([0xa1; 32]);
        let calls = Arc::new(AtomicUsize::new(0));
        let error = execute_local_mutation_with_failpoint(
            &mut connection,
            committed_local_request(command_id, [0xa2; 32], Some(Arc::clone(&calls))),
            LocalMutationFailpoint::AfterCommit,
        )
        .expect_err("response loss is simulated after commit");
        assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

        let (receipt, inserted) = execute_local_mutation_with_failpoint(
            &mut connection,
            committed_local_request(command_id, [0xa2; 32], Some(Arc::clone(&calls))),
            LocalMutationFailpoint::Never,
        )
        .expect("retry returns retained receipt");
        assert!(!inserted);
        assert_eq!(receipt.revision(), hq_domain::Revision::new(1));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(load_facts(&connection).expect("corpus loads").len(), 1);
        assert_eq!(
            operational::current_revision(&connection).expect("revision loads"),
            hq_domain::Revision::new(1)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn repair_failpoints_preserve_the_previous_complete_index_and_allow_retry() {
        use super::repair::{RepairFailpoint, replace_with_failpoint};

        for failpoint in [
            RepairFailpoint::AfterClear,
            RepairFailpoint::AfterVertices,
            RepairFailpoint::AfterReverseDependencies,
            RepairFailpoint::AfterDecisions,
            RepairFailpoint::AfterDependencyOrder,
            RepairFailpoint::AfterPresentationOrder,
            RepairFailpoint::AfterConflicts,
            RepairFailpoint::AfterState,
            RepairFailpoint::AfterAuthorityInsert,
            RepairFailpoint::AfterAuthorityVerification,
            RepairFailpoint::AfterConversationInsert,
            RepairFailpoint::AfterConversationVerification,
            RepairFailpoint::AfterAgentInsert,
            RepairFailpoint::AfterAgentVerification,
            RepairFailpoint::AfterProjectInsert,
            RepairFailpoint::AfterProjectVerification,
            RepairFailpoint::AfterVerification,
        ] {
            let mut connection = Connection::open_in_memory().expect("memory database opens");
            connection.execute_batch(SCHEMA).expect("schema creates");
            append_with_failpoint(&mut connection, &fixture(), Failpoint::Never)
                .expect("fixture appends");
            let mut database = Database { connection };
            let first_policy = AuthorityPolicy::new(
                hq_domain::InstallationId::from_bytes([0x11; 32]),
                hq_domain::MailboxId::from_bytes([0x44; 32]),
            );
            let replacement_policy = AuthorityPolicy::new(
                hq_domain::InstallationId::from_bytes([0x22; 32]),
                hq_domain::MailboxId::from_bytes([0x55; 32]),
            );
            let prior_complete = database
                .complete_snapshot(first_policy)
                .expect("prior snapshot reduces");
            let prior = prior_complete.normalized_index();
            let prior_authority = prior_complete.authority_projection_snapshot();
            let prior_conversation = prior_complete.conversation_projection_snapshot();
            let prior_agent = prior_complete.agent_projection_snapshot();
            let prior_project = prior_complete.project_projection_snapshot();
            repair::replace(
                &mut database.connection,
                &prior,
                &prior_authority,
                &prior_conversation,
                &prior_agent,
                &prior_project,
            )
            .expect("prior index persists");
            let replacement_complete = database
                .complete_snapshot(replacement_policy)
                .expect("replacement snapshot reduces");
            let replacement = replacement_complete.normalized_index();
            let replacement_authority = replacement_complete.authority_projection_snapshot();
            let replacement_conversation = replacement_complete.conversation_projection_snapshot();
            let replacement_agent = replacement_complete.agent_projection_snapshot();
            let replacement_project = replacement_complete.project_projection_snapshot();

            let error = replace_with_failpoint(
                &mut database.connection,
                &replacement,
                &replacement_authority,
                &replacement_conversation,
                &replacement_agent,
                &replacement_project,
                failpoint,
            )
            .expect_err("repair failpoint interrupts replacement");
            assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
            assert_eq!(
                repair::load(&database.connection).expect("prior index remains loadable"),
                prior
            );
            assert_eq!(
                authority::load(&database.connection).expect("prior authority remains loadable"),
                prior_authority
            );
            assert_eq!(
                conversation::load(&database.connection)
                    .expect("prior conversation remains loadable"),
                prior_conversation
            );
            assert_eq!(
                agent::load(&database.connection).expect("prior agent remains loadable"),
                prior_agent
            );
            assert_eq!(
                project::load(&database.connection).expect("prior project remains loadable"),
                prior_project
            );
            assert_eq!(
                repair::replace(
                    &mut database.connection,
                    &replacement,
                    &replacement_authority,
                    &replacement_conversation,
                    &replacement_agent,
                    &replacement_project,
                )
                .expect("retry succeeds"),
                (
                    replacement,
                    replacement_authority,
                    replacement_conversation,
                    replacement_agent,
                    replacement_project,
                )
            );
        }
    }

    fn local_policy() -> AuthorityPolicy {
        AuthorityPolicy::new(
            hq_domain::InstallationId::from_bytes([0x11; 32]),
            hq_domain::MailboxId::from_bytes([0x44; 32]),
        )
    }

    fn fixture_signer() -> Bip340Signer {
        Bip340Signer::from_secret_bytes({
            let mut secret = [0_u8; 32];
            secret[31] = 1;
            secret
        })
        .expect("fixture secret is valid")
    }

    fn committed_local_request(
        command_id: hq_domain::CommandId,
        digest: [u8; 32],
        calls: Option<Arc<AtomicUsize>>,
    ) -> crate::LocalMutationRequest {
        let plan = CanonicalEventPlan::from_fact(root_fixture().fact());
        crate::LocalMutationRequest::new(
            command_id,
            hq_domain::CommandDigest::from_bytes(digest),
            local_policy(),
            Arc::new(fixture_signer()),
            move |_| {
                if let Some(calls) = calls {
                    calls.fetch_add(1, AtomicOrdering::SeqCst);
                }
                crate::LocalMutationDecision::commit(
                    plan,
                    [6; 32],
                    crate::MutationResultBytes::new(b"committed".to_vec())
                        .expect("result is bounded"),
                )
            },
        )
    }

    fn rejected_local_request(
        command_id: hq_domain::CommandId,
        digest: [u8; 32],
    ) -> crate::LocalMutationRequest {
        crate::LocalMutationRequest::new(
            command_id,
            hq_domain::CommandDigest::from_bytes(digest),
            local_policy(),
            Arc::new(fixture_signer()),
            |_| {
                crate::LocalMutationDecision::reject(
                    crate::MutationResultBytes::new(b"rejected".to_vec())
                        .expect("result is bounded"),
                )
            },
        )
    }

    pub(crate) fn fixture() -> VerifiedSemanticFact {
        let signer = Bip340Signer::from_secret_bytes({
            let mut secret = [0_u8; 32];
            secret[31] = 1;
            secret
        })
        .expect("fixture secret is valid");
        let event = signer
            .sign(1, CONTENT.as_bytes(), [7; 32])
            .expect("fixture signs");
        let DispatchOutcome::Supported(supported) = event.dispatch().expect("fixture dispatches")
        else {
            panic!("fixture is supported");
        };
        supported
            .decode_v1()
            .expect("fixture DTO verifies")
            .into_semantic_fact()
            .expect("fixture converts")
    }

    pub(crate) fn root_fixture() -> VerifiedSemanticFact {
        const ROOT: &str = r#"{"p":"hq/canonical","v":1,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[],"auth":[],"body":{"installation":"1111111111111111111111111111111111111111111111111111111111111111","signing":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","encryption":"2222222222222222222222222222222222222222222222222222222222222222","label":"alpha"}}"#;
        let signer = Bip340Signer::from_secret_bytes({
            let mut secret = [0_u8; 32];
            secret[31] = 1;
            secret
        })
        .expect("fixture secret is valid");
        let event = signer
            .sign(0, ROOT.as_bytes(), [6; 32])
            .expect("root fixture signs");
        let DispatchOutcome::Supported(supported) = event.dispatch().expect("root dispatches")
        else {
            panic!("root fixture is supported");
        };
        supported
            .decode_v1()
            .expect("root DTO verifies")
            .into_semantic_fact()
            .expect("root converts")
    }
}
