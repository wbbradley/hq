//! Private SQLite schema, row codecs, and transactions.

mod agent;
mod authority;
mod conversation;
mod repair;

use std::{path::Path, time::Duration};

use hq_domain::{AuthorityRole, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS};
use hq_protocol::{
    DispatchOutcome, MAX_EVENT_BYTES, ProtocolNamespace, RawEventBytes, VerifiedSemanticFact,
};
use hq_reducer::AuthorityPolicy;
use rusqlite::{
    Connection, Error as SqlError, ErrorCode, OpenFlags, OptionalExtension, Transaction,
    TransactionBehavior, config::DbConfig, params,
};

use crate::{
    AgentProjectionSnapshot, AppendOutcome, AuthorityProjectionSnapshot, CompleteSnapshot,
    ConversationProjectionSnapshot, ReductionIndexSnapshot, RepairOutcome, StoreError,
    StoreErrorClass,
    paths::{prepare_database_path, validate_database_path},
    snapshot::build_complete_snapshot,
};

const APPLICATION_ID: i64 = 0x4851_5253;
const SCHEMA_VERSION: i64 = 5;
const SCHEMA_MARKER: &str = "hq-store-v5-agent-projections-2026-08-27";
const SCHEMA_TABLES: [&str; 77] = [
    "storage_metadata",
    "canonical_facts",
    "fact_parents",
    "fact_authorities",
    "reduction_state",
    "reduction_vertices",
    "reduction_reverse_dependencies",
    "reduction_decisions",
    "reduction_missing_dependencies",
    "reduction_unusable_dependencies",
    "reduction_failed_authorities",
    "reduction_decision_participants",
    "reduction_dependency_order",
    "reduction_presentation_order",
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
    decision_count INTEGER NOT NULL CHECK(decision_count >= 0),
    missing_count INTEGER NOT NULL CHECK(missing_count >= 0),
    unusable_count INTEGER NOT NULL CHECK(unusable_count >= 0),
    failed_authority_count INTEGER NOT NULL CHECK(failed_authority_count >= 0),
    decision_participant_count INTEGER NOT NULL CHECK(decision_participant_count >= 0),
    dependency_order_count INTEGER NOT NULL CHECK(dependency_order_count >= 0),
    presentation_order_count INTEGER NOT NULL CHECK(presentation_order_count >= 0),
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

    pub(super) fn append(
        &mut self,
        fact: &VerifiedSemanticFact,
    ) -> Result<AppendOutcome, StoreError> {
        append_with_failpoint(&mut self.connection, fact, Failpoint::Never)
    }

    pub(super) fn load(&mut self) -> Result<Vec<VerifiedSemanticFact>, StoreError> {
        let transaction = self.connection.transaction().map_err(sql_error)?;
        let count: i64 = transaction
            .query_row("SELECT count(*) FROM canonical_facts", [], |row| row.get(0))
            .map_err(sql_error)?;
        if !(0..=MAXIMUM_CORPUS_FACTS).contains(&count) {
            return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
        }
        let capacity = usize::try_from(count)
            .map_err(|_| StoreError::new(StoreErrorClass::InvalidEvidence))?;
        let mut statement = transaction
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
            let event_bytes = transaction
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
            let stored_parents = load_parents(&transaction, &index.fact_id)?;
            let stored_authorities = load_authorities(&transaction, &index.fact_id)?;
            if stored_parents != index.parents || stored_authorities != index.authorities {
                return Err(StoreError::new(StoreErrorClass::InvalidEvidence));
            }
            facts.push(verified);
        }
        transaction.commit().map_err(sql_error)?;
        Ok(facts)
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
        let (persisted, authority, conversation, agent) = repair::replace(
            &mut self.connection,
            &expected,
            &expected_authority,
            &expected_conversation,
            &expected_agent,
        )?;
        Ok(RepairOutcome::new(
            complete,
            persisted,
            authority,
            conversation,
            agent,
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

    pub(super) fn load_agent_snapshot(&self) -> Result<AgentProjectionSnapshot, StoreError> {
        repair::load(&self.connection)?;
        authority::load(&self.connection)?;
        conversation::load(&self.connection)?;
        agent::load(&self.connection)
    }
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
    if table_count != 77 {
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
    BeforeCommit,
}

fn append_with_failpoint(
    connection: &mut Connection,
    fact: &VerifiedSemanticFact,
    failpoint: Failpoint,
) -> Result<AppendOutcome, StoreError> {
    #[cfg(not(test))]
    let _ = failpoint;
    let index = index_for(fact);
    let event_bytes = fact.verified_event().exact_event_bytes();
    let transaction = connection.transaction().map_err(sql_error)?;
    if immutable_row_exists(&transaction, &index.fact_id)? {
        let equal = immutable_row_equal(&transaction, &index, event_bytes)?;
        return if equal {
            transaction.commit().map_err(sql_error)?;
            Ok(AppendOutcome::AlreadyPresent)
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
    if matches!(failpoint, Failpoint::BeforeCommit) {
        return Err(StoreError::new(StoreErrorClass::DatabaseUnavailable));
    }
    transaction.commit().map_err(sql_error)?;
    Ok(AppendOutcome::Inserted)
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
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;
    use hq_protocol::Bip340Signer;

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
            repair::replace(
                &mut database.connection,
                &prior,
                &prior_authority,
                &prior_conversation,
                &prior_agent,
            )
            .expect("prior index persists");
            let replacement_complete = database
                .complete_snapshot(replacement_policy)
                .expect("replacement snapshot reduces");
            let replacement = replacement_complete.normalized_index();
            let replacement_authority = replacement_complete.authority_projection_snapshot();
            let replacement_conversation = replacement_complete.conversation_projection_snapshot();
            let replacement_agent = replacement_complete.agent_projection_snapshot();

            let error = replace_with_failpoint(
                &mut database.connection,
                &replacement,
                &replacement_authority,
                &replacement_conversation,
                &replacement_agent,
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
                repair::replace(
                    &mut database.connection,
                    &replacement,
                    &replacement_authority,
                    &replacement_conversation,
                    &replacement_agent,
                )
                .expect("retry succeeds"),
                (
                    replacement,
                    replacement_authority,
                    replacement_conversation,
                    replacement_agent,
                )
            );
        }
    }

    fn fixture() -> VerifiedSemanticFact {
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
}
