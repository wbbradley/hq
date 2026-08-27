//! Explicit relational codecs for rebuildable named-agent projections.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{
    AgentId, BoundedText, FactId, InstallationId, MailboxAddress, MailboxId, ProviderId,
    ProviderSessionId, RepositoryContext, ResourceLocator, ResourceScheme, ShortText,
};
use hq_reducer::{
    AgentAggregateKey, AgentLifecycle, AgentProjection, AgentProjectionKey, AgentView,
    ContextHistoryView, DirectSessionView, NameClaimSubject, NameReservationView, RenameView,
    SelectionCandidate, SelectionView, SessionBindingView, SessionIdentity,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::{AgentProjectionSnapshot, StoreError, StoreErrorClass};

const MAXIMUM_AGENT_ROWS: i64 = 64_000_000;
const ZERO: [u8; 32] = [0; 32];

const TABLES: [&str; 24] = [
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

pub(super) fn clear(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction
        .execute_batch(
            "DELETE FROM agent_state;
             DELETE FROM agent_direct_binding_facts;
             DELETE FROM agent_direct_sessions;
             DELETE FROM agent_rename_frontiers;
             DELETE FROM agent_rename_candidates;
             DELETE FROM agent_renames;
             DELETE FROM agent_selection_frontiers;
             DELETE FROM agent_selection_candidates;
             DELETE FROM agent_selections;
             DELETE FROM agent_context_frontiers;
             DELETE FROM agent_context_history;
             DELETE FROM agent_contexts;
             DELETE FROM agent_session_bindings;
             DELETE FROM agent_sessions;
             DELETE FROM agent_agent_retirements;
             DELETE FROM agent_agent_mailboxes;
             DELETE FROM agent_agent_names;
             DELETE FROM agent_agent_claims;
             DELETE FROM agent_agents;
             DELETE FROM agent_name_claims;
             DELETE FROM agent_names;
             DELETE FROM agent_support;
             DELETE FROM agent_projection_keys;
             DELETE FROM agent_frontiers;
             DELETE FROM agent_aggregate_keys;",
        )
        .map_err(database)
}

pub(super) fn insert(
    transaction: &Transaction<'_>,
    snapshot: &AgentProjectionSnapshot,
) -> Result<(), StoreError> {
    if snapshot.projections.keys().ne(snapshot.support.keys()) {
        return Err(corrupt());
    }
    for (key, facts) in &snapshot.frontiers {
        let digest = insert_key(transaction, KeyTable::Aggregate, &aggregate_parts(key))?;
        insert_facts(transaction, "agent_frontiers", digest, facts)?;
    }
    for (key, projection) in &snapshot.projections {
        let digest = insert_key(transaction, KeyTable::Projection, &projection_parts(key))?;
        insert_projection(transaction, digest, key, projection)?;
        insert_facts(
            transaction,
            "agent_support",
            digest,
            snapshot.support.get(key).ok_or_else(corrupt)?,
        )?;
    }
    let counts = Counts::read(transaction)?;
    validate_counts(snapshot, counts)?;
    let digest = row_digest(transaction)?;
    transaction
        .execute(
            "INSERT INTO agent_state(singleton, aggregate_key_count, frontier_count, \
                 projection_key_count, projection_count, support_count, row_count, row_digest) \
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                counts.aggregate_key_count,
                counts.frontier_count,
                counts.projection_key_count,
                counts.projection_count,
                counts.support_count,
                counts.row_count,
                digest.as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

pub(super) fn load(connection: &Connection) -> Result<AgentProjectionSnapshot, StoreError> {
    let Some(state) = load_state(connection)? else {
        return if Counts::read(connection)?.row_count == 0 {
            Err(StoreError::new(StoreErrorClass::NotRepaired))
        } else {
            Err(corrupt())
        };
    };
    state.counts.validate()?;
    if Counts::read(connection)? != state.counts || row_digest(connection)? != state.digest {
        return Err(corrupt());
    }
    let frontiers = load_frontiers(connection)?;
    let rows = load_keys(connection, KeyTable::Projection)?;
    let mut projections = BTreeMap::new();
    let mut support = BTreeMap::new();
    for (digest, parts) in rows {
        let key = decode_projection_key(parts)?;
        let projection = load_projection(connection, digest, &key)?;
        if projections.insert(key.clone(), projection).is_some()
            || support
                .insert(key, load_facts(connection, "agent_support", digest)?)
                .is_some()
        {
            return Err(corrupt());
        }
    }
    let snapshot = AgentProjectionSnapshot {
        frontiers,
        projections,
        support,
    };
    validate_counts(&snapshot, state.counts)?;
    Ok(snapshot)
}

#[derive(Clone, Copy)]
enum KeyTable {
    Aggregate,
    Projection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct KeyParts {
    kind: i64,
    a: [u8; 32],
    b: [u8; 32],
    name: String,
    provider: String,
    session: String,
}

impl KeyParts {
    fn simple(kind: i64, a: [u8; 32]) -> Self {
        Self {
            kind,
            a,
            b: ZERO,
            name: String::new(),
            provider: String::new(),
            session: String::new(),
        }
    }

    fn name(kind: i64, name: &ShortText) -> Self {
        Self {
            kind,
            a: ZERO,
            b: ZERO,
            name: name.as_str().to_owned(),
            provider: String::new(),
            session: String::new(),
        }
    }

    fn mailbox(kind: i64, mailbox: MailboxAddress) -> Self {
        Self {
            kind,
            a: *mailbox.installation_id().as_bytes(),
            b: *mailbox.mailbox_id().as_bytes(),
            name: String::new(),
            provider: String::new(),
            session: String::new(),
        }
    }

    fn session(kind: i64, identity: &SessionIdentity) -> Self {
        Self {
            kind,
            a: ZERO,
            b: ZERO,
            name: String::new(),
            provider: identity.provider.as_str().to_owned(),
            session: identity.session.as_str().to_owned(),
        }
    }

    fn agent_session(kind: i64, agent: AgentId, identity: &SessionIdentity) -> Self {
        let mut parts = Self::session(kind, identity);
        parts.a = *agent.as_bytes();
        parts
    }

    fn mailbox_session(kind: i64, mailbox: MailboxAddress, identity: &SessionIdentity) -> Self {
        let mut parts = Self::mailbox(kind, mailbox);
        identity.provider.as_str().clone_into(&mut parts.provider);
        identity.session.as_str().clone_into(&mut parts.session);
        parts
    }
}

fn aggregate_parts(key: &AgentAggregateKey) -> KeyParts {
    match key {
        AgentAggregateKey::Name(value) => KeyParts::name(1, value),
        AgentAggregateKey::Agent(value) => KeyParts::simple(2, *value.as_bytes()),
        AgentAggregateKey::Mailbox(value) => KeyParts::mailbox(3, *value),
        AgentAggregateKey::Session(value) => KeyParts::session(4, value),
        AgentAggregateKey::Selection(value) => KeyParts::simple(5, *value.as_bytes()),
        AgentAggregateKey::Rename { agent, session } => KeyParts::agent_session(6, *agent, session),
        AgentAggregateKey::Context(value) => KeyParts::mailbox(7, *value),
    }
}

fn projection_parts(key: &AgentProjectionKey) -> KeyParts {
    match key {
        AgentProjectionKey::Name(value) => KeyParts::name(1, value),
        AgentProjectionKey::Agent(value) => KeyParts::simple(2, *value.as_bytes()),
        AgentProjectionKey::Session(value) => KeyParts::session(3, value),
        AgentProjectionKey::Context(value) => KeyParts::mailbox(4, *value),
        AgentProjectionKey::Selection(value) => KeyParts::simple(5, *value.as_bytes()),
        AgentProjectionKey::Rename { agent, session } => {
            KeyParts::agent_session(6, *agent, session)
        }
        AgentProjectionKey::DirectSession { mailbox, session } => {
            KeyParts::mailbox_session(7, *mailbox, session)
        }
    }
}

fn insert_key(
    transaction: &Transaction<'_>,
    table: KeyTable,
    parts: &KeyParts,
) -> Result<[u8; 32], StoreError> {
    let digest = key_digest(table, parts);
    let sql = match table {
        KeyTable::Aggregate => {
            "INSERT INTO agent_aggregate_keys( \
             key_digest, key_kind, key_a, key_b, name, provider, session \
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        }
        KeyTable::Projection => {
            "INSERT INTO agent_projection_keys( \
             key_digest, key_kind, key_a, key_b, name, provider, session \
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        }
    };
    transaction
        .execute(
            sql,
            params![
                digest.as_slice(),
                parts.kind,
                parts.a.as_slice(),
                parts.b.as_slice(),
                parts.name,
                parts.provider,
                parts.session,
            ],
        )
        .map_err(database)?;
    Ok(digest)
}

fn key_digest(table: KeyTable, parts: &KeyParts) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(match table {
        KeyTable::Aggregate => b"hq-agent-aggregate-key-v1".as_slice(),
        KeyTable::Projection => b"hq-agent-projection-key-v1".as_slice(),
    });
    digest.update(parts.kind.to_be_bytes());
    digest.update(parts.a);
    digest.update(parts.b);
    put_text(&mut digest, &parts.name);
    put_text(&mut digest, &parts.provider);
    put_text(&mut digest, &parts.session);
    digest.finalize().into()
}

fn put_text(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value.as_bytes());
}

fn insert_facts(
    transaction: &Transaction<'_>,
    table: &str,
    digest: [u8; 32],
    facts: &BTreeSet<FactId>,
) -> Result<(), StoreError> {
    let sql = match table {
        "agent_frontiers" => "INSERT INTO agent_frontiers(key_digest, fact_id) VALUES (?1, ?2)",
        "agent_support" => "INSERT INTO agent_support(key_digest, fact_id) VALUES (?1, ?2)",
        _ => return Err(corrupt()),
    };
    for fact in facts {
        transaction
            .execute(sql, params![digest.as_slice(), fact.as_bytes().as_slice()])
            .map_err(database)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn insert_projection(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    key: &AgentProjectionKey,
    projection: &AgentProjection,
) -> Result<(), StoreError> {
    match (key, projection) {
        (AgentProjectionKey::Name(_), AgentProjection::Name(view)) => {
            transaction
                .execute(
                    "INSERT INTO agent_names(key_digest, conflicted, retired) VALUES (?1, ?2, ?3)",
                    params![
                        digest.as_slice(),
                        i64::from(view.conflicted),
                        i64::from(view.retired),
                    ],
                )
                .map_err(database)?;
            for (fact, subject) in &view.claims {
                transaction
                    .execute(
                        "INSERT INTO agent_name_claims( \
                             key_digest, fact_id, agent_id, mailbox_installation, mailbox_id \
                         ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            digest.as_slice(),
                            fact.as_bytes().as_slice(),
                            subject.agent_id.as_bytes().as_slice(),
                            subject.mailbox.installation_id().as_bytes().as_slice(),
                            subject.mailbox.mailbox_id().as_bytes().as_slice(),
                        ],
                    )
                    .map_err(database)?;
            }
        }
        (AgentProjectionKey::Agent(_), AgentProjection::Agent(view)) => {
            let (selected_present, selected_provider, selected_session) =
                encode_session(view.selected_session.as_ref());
            transaction
                .execute(
                    "INSERT INTO agent_agents( \
                         key_digest, lifecycle, runnable, selected_present, selected_provider, \
                         selected_session, name_reserved \
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        digest.as_slice(),
                        encode_lifecycle(view.lifecycle),
                        i64::from(view.runnable),
                        selected_present,
                        selected_provider,
                        selected_session,
                        i64::from(view.name_reserved),
                    ],
                )
                .map_err(database)?;
            insert_child_set(transaction, "agent_agent_claims", digest, &view.claims)?;
            insert_child_texts(transaction, digest, &view.names)?;
            insert_child_mailboxes(transaction, digest, &view.mailboxes)?;
            insert_child_set(
                transaction,
                "agent_agent_retirements",
                digest,
                &view.retirements,
            )?;
        }
        (AgentProjectionKey::Session(_), AgentProjection::Session(view)) => {
            let (present, installation, mailbox) = encode_mailbox(view.mailbox);
            transaction
                .execute(
                    "INSERT INTO agent_sessions( \
                         key_digest, conflicted, mailbox_present, mailbox_installation, mailbox_id \
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        digest.as_slice(),
                        i64::from(view.conflicted),
                        present,
                        installation.as_slice(),
                        mailbox.as_slice(),
                    ],
                )
                .map_err(database)?;
            for (fact, mailbox) in &view.bindings {
                transaction
                    .execute(
                        "INSERT INTO agent_session_bindings( \
                             key_digest, fact_id, mailbox_installation, mailbox_id \
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![
                            digest.as_slice(),
                            fact.as_bytes().as_slice(),
                            mailbox.installation_id().as_bytes().as_slice(),
                            mailbox.mailbox_id().as_bytes().as_slice(),
                        ],
                    )
                    .map_err(database)?;
            }
        }
        (AgentProjectionKey::Context(_), AgentProjection::Context(view)) => {
            transaction
                .execute(
                    "INSERT INTO agent_contexts(key_digest) VALUES (?1)",
                    [digest.as_slice()],
                )
                .map_err(database)?;
            for (fact, context) in &view.history {
                insert_context_history(transaction, digest, *fact, context)?;
            }
            insert_child_set(
                transaction,
                "agent_context_frontiers",
                digest,
                &view.frontier,
            )?;
        }
        (AgentProjectionKey::Selection(_), AgentProjection::Selection(view)) => {
            let active = view.active.as_ref().map_or_else(
                ContextSessionParts::empty,
                ContextSessionParts::from_candidate,
            );
            transaction
                .execute(
                    "INSERT INTO agent_selections( \
                         key_digest, active_present, active_provider, active_session, \
                         active_directory_scheme, active_directory_value, active_repository_present, \
                         active_repository_scheme, active_repository_value, active_worktree_present, \
                         active_worktree_scheme, active_worktree_value, active_branch_present, \
                         active_branch, conflicted \
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        digest.as_slice(),
                        i64::from(view.active.is_some()),
                        active.provider,
                        active.session,
                        active.context.directory_scheme,
                        active.context.directory_value,
                        active.context.repository_present,
                        active.context.repository_scheme,
                        active.context.repository_value,
                        active.context.worktree_present,
                        active.context.worktree_scheme,
                        active.context.worktree_value,
                        active.context.branch_present,
                        active.context.branch,
                        i64::from(view.conflicted),
                    ],
                )
                .map_err(database)?;
            for (fact, candidate) in &view.candidates {
                insert_selection_candidate(transaction, digest, *fact, candidate)?;
            }
            insert_child_set(
                transaction,
                "agent_selection_frontiers",
                digest,
                &view.frontier,
            )?;
        }
        (AgentProjectionKey::Rename { .. }, AgentProjection::Rename(view)) => {
            let (present, display) = encode_text(view.display_name.as_ref());
            transaction
                .execute(
                    "INSERT INTO agent_renames( \
                         key_digest, resolved, display_name_present, display_name \
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        digest.as_slice(),
                        i64::from(view.resolved),
                        present,
                        display,
                    ],
                )
                .map_err(database)?;
            for (fact, candidate) in &view.candidates {
                let (present, display) = encode_text(candidate.as_ref());
                transaction
                    .execute(
                        "INSERT INTO agent_rename_candidates( \
                             key_digest, fact_id, display_name_present, display_name \
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![
                            digest.as_slice(),
                            fact.as_bytes().as_slice(),
                            present,
                            display,
                        ],
                    )
                    .map_err(database)?;
            }
            insert_child_set(
                transaction,
                "agent_rename_frontiers",
                digest,
                &view.frontier,
            )?;
        }
        (
            AgentProjectionKey::DirectSession { mailbox, .. },
            AgentProjection::DirectSession(view),
        ) => {
            if view.mailbox != *mailbox {
                return Err(corrupt());
            }
            let (named_present, named) = encode_id(view.named_agent);
            transaction
                .execute(
                    "INSERT INTO agent_direct_sessions( \
                         key_digest, mailbox_installation, mailbox_id, named_agent_present, \
                         named_agent, conflicted \
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        digest.as_slice(),
                        view.mailbox.installation_id().as_bytes().as_slice(),
                        view.mailbox.mailbox_id().as_bytes().as_slice(),
                        named_present,
                        named.as_slice(),
                        i64::from(view.conflicted),
                    ],
                )
                .map_err(database)?;
            insert_child_set(
                transaction,
                "agent_direct_binding_facts",
                digest,
                &view.binding_facts,
            )?;
        }
        _ => return Err(corrupt()),
    }
    Ok(())
}

fn insert_child_set(
    transaction: &Transaction<'_>,
    table: &str,
    digest: [u8; 32],
    facts: &BTreeSet<FactId>,
) -> Result<(), StoreError> {
    let sql = match table {
        "agent_agent_claims" => {
            "INSERT INTO agent_agent_claims(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "agent_agent_retirements" => {
            "INSERT INTO agent_agent_retirements(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "agent_context_frontiers" => {
            "INSERT INTO agent_context_frontiers(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "agent_selection_frontiers" => {
            "INSERT INTO agent_selection_frontiers(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "agent_rename_frontiers" => {
            "INSERT INTO agent_rename_frontiers(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "agent_direct_binding_facts" => {
            "INSERT INTO agent_direct_binding_facts(key_digest, fact_id) VALUES (?1, ?2)"
        }
        _ => return Err(corrupt()),
    };
    for fact in facts {
        transaction
            .execute(sql, params![digest.as_slice(), fact.as_bytes().as_slice()])
            .map_err(database)?;
    }
    Ok(())
}

fn insert_child_texts(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    names: &BTreeSet<ShortText>,
) -> Result<(), StoreError> {
    for name in names {
        transaction
            .execute(
                "INSERT INTO agent_agent_names(key_digest, name) VALUES (?1, ?2)",
                params![digest.as_slice(), name.as_str()],
            )
            .map_err(database)?;
    }
    Ok(())
}

fn insert_child_mailboxes(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    mailboxes: &BTreeSet<MailboxAddress>,
) -> Result<(), StoreError> {
    for mailbox in mailboxes {
        transaction
            .execute(
                "INSERT INTO agent_agent_mailboxes(key_digest, installation_id, mailbox_id) \
                 VALUES (?1, ?2, ?3)",
                params![
                    digest.as_slice(),
                    mailbox.installation_id().as_bytes().as_slice(),
                    mailbox.mailbox_id().as_bytes().as_slice(),
                ],
            )
            .map_err(database)?;
    }
    Ok(())
}

struct ContextParts {
    directory_scheme: i64,
    directory_value: String,
    repository_present: i64,
    repository_scheme: i64,
    repository_value: String,
    worktree_present: i64,
    worktree_scheme: i64,
    worktree_value: String,
    branch_present: i64,
    branch: String,
}

impl ContextParts {
    fn from_context(context: &RepositoryContext) -> Self {
        let (repository_present, repository_scheme, repository_value) =
            encode_locator(context.repository.as_ref());
        let (worktree_present, worktree_scheme, worktree_value) =
            encode_locator(context.worktree.as_ref());
        let (branch_present, branch) = encode_text(context.branch.as_ref());
        Self {
            directory_scheme: encode_scheme(context.directory.scheme()),
            directory_value: context.directory.value().to_owned(),
            repository_present,
            repository_scheme,
            repository_value: repository_value.to_owned(),
            worktree_present,
            worktree_scheme,
            worktree_value: worktree_value.to_owned(),
            branch_present,
            branch: branch.to_owned(),
        }
    }

    fn empty() -> Self {
        Self {
            directory_scheme: 0,
            directory_value: String::new(),
            repository_present: 0,
            repository_scheme: 0,
            repository_value: String::new(),
            worktree_present: 0,
            worktree_scheme: 0,
            worktree_value: String::new(),
            branch_present: 0,
            branch: String::new(),
        }
    }
}

struct ContextSessionParts {
    provider: String,
    session: String,
    context: ContextParts,
}

impl ContextSessionParts {
    fn from_candidate(candidate: &SelectionCandidate) -> Self {
        Self {
            provider: candidate.session.provider.as_str().to_owned(),
            session: candidate.session.session.as_str().to_owned(),
            context: ContextParts::from_context(&candidate.context),
        }
    }

    fn empty() -> Self {
        Self {
            provider: String::new(),
            session: String::new(),
            context: ContextParts::empty(),
        }
    }
}

fn insert_context_history(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    fact: FactId,
    context: &RepositoryContext,
) -> Result<(), StoreError> {
    let parts = ContextParts::from_context(context);
    transaction
        .execute(
            "INSERT INTO agent_context_history( \
                 key_digest, fact_id, directory_scheme, directory_value, repository_present, \
                 repository_scheme, repository_value, worktree_present, worktree_scheme, \
                 worktree_value, branch_present, branch \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                digest.as_slice(),
                fact.as_bytes().as_slice(),
                parts.directory_scheme,
                parts.directory_value,
                parts.repository_present,
                parts.repository_scheme,
                parts.repository_value,
                parts.worktree_present,
                parts.worktree_scheme,
                parts.worktree_value,
                parts.branch_present,
                parts.branch,
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn insert_selection_candidate(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    fact: FactId,
    candidate: &SelectionCandidate,
) -> Result<(), StoreError> {
    let parts = ContextSessionParts::from_candidate(candidate);
    transaction
        .execute(
            "INSERT INTO agent_selection_candidates( \
                 key_digest, fact_id, provider, session, directory_scheme, directory_value, \
                 repository_present, repository_scheme, repository_value, worktree_present, \
                 worktree_scheme, worktree_value, branch_present, branch \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                digest.as_slice(),
                fact.as_bytes().as_slice(),
                parts.provider,
                parts.session,
                parts.context.directory_scheme,
                parts.context.directory_value,
                parts.context.repository_present,
                parts.context.repository_scheme,
                parts.context.repository_value,
                parts.context.worktree_present,
                parts.context.worktree_scheme,
                parts.context.worktree_value,
                parts.context.branch_present,
                parts.context.branch,
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn encode_session(value: Option<&SessionIdentity>) -> (i64, &str, &str) {
    value.map_or((0, "", ""), |identity| {
        (1, identity.provider.as_str(), identity.session.as_str())
    })
}

fn encode_mailbox(value: Option<MailboxAddress>) -> (i64, [u8; 32], [u8; 32]) {
    value.map_or((0, ZERO, ZERO), |mailbox| {
        (
            1,
            *mailbox.installation_id().as_bytes(),
            *mailbox.mailbox_id().as_bytes(),
        )
    })
}

fn encode_id(value: Option<AgentId>) -> (i64, [u8; 32]) {
    value.map_or((0, ZERO), |value| (1, *value.as_bytes()))
}

fn encode_text(value: Option<&ShortText>) -> (i64, &str) {
    value.map_or((0, ""), |value| (1, value.as_str()))
}

fn encode_locator(value: Option<&ResourceLocator>) -> (i64, i64, &str) {
    value.map_or((0, 0, ""), |value| {
        (1, encode_scheme(value.scheme()), value.value())
    })
}

fn load_frontiers(
    connection: &Connection,
) -> Result<BTreeMap<AgentAggregateKey, BTreeSet<FactId>>, StoreError> {
    let mut result = BTreeMap::new();
    for (digest, parts) in load_keys(connection, KeyTable::Aggregate)? {
        let key = decode_aggregate_key(parts)?;
        if result
            .insert(key, load_facts(connection, "agent_frontiers", digest)?)
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(result)
}

fn load_keys(
    connection: &Connection,
    table: KeyTable,
) -> Result<Vec<([u8; 32], KeyParts)>, StoreError> {
    let (sql, table_name) = match table {
        KeyTable::Aggregate => (
            "SELECT key_digest, key_kind, key_a, key_b, name, provider, session \
             FROM agent_aggregate_keys ORDER BY key_digest",
            "agent_aggregate_keys",
        ),
        KeyTable::Projection => (
            "SELECT key_digest, key_kind, key_a, key_b, name, provider, session \
             FROM agent_projection_keys ORDER BY key_digest",
            "agent_projection_keys",
        ),
    };
    let expected = capacity(count(connection, table_name)?)?;
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                KeyParts {
                    kind: row.get(1)?,
                    a: fixed_sql(row.get(2)?)?,
                    b: fixed_sql(row.get(3)?)?,
                    name: row.get(4)?,
                    provider: row.get(5)?,
                    session: row.get(6)?,
                },
            ))
        })
        .map_err(database)?;
    let mut result = Vec::with_capacity(expected);
    for row in rows {
        let (stored, parts) = row.map_err(database)?;
        let stored = fixed(stored)?;
        if stored != key_digest(table, &parts) {
            return Err(corrupt());
        }
        result.push((stored, parts));
    }
    if result.len() != expected {
        return Err(corrupt());
    }
    Ok(result)
}

fn fixed_sql(value: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    value.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "wrong fixed identity width".into(),
        )
    })
}

fn decode_aggregate_key(parts: KeyParts) -> Result<AgentAggregateKey, StoreError> {
    match parts.kind {
        1 if name_shape(&parts) => Ok(AgentAggregateKey::Name(
            ShortText::new(parts.name).map_err(|_| corrupt())?,
        )),
        2 if simple_shape(&parts) => Ok(AgentAggregateKey::Agent(AgentId::from_bytes(parts.a))),
        3 if mailbox_shape(&parts) => Ok(AgentAggregateKey::Mailbox(mailbox(&parts))),
        4 if session_shape(&parts) => Ok(AgentAggregateKey::Session(session(&parts)?)),
        5 if simple_shape(&parts) => Ok(AgentAggregateKey::Selection(AgentId::from_bytes(parts.a))),
        6 if agent_session_shape(&parts) => Ok(AgentAggregateKey::Rename {
            agent: AgentId::from_bytes(parts.a),
            session: session(&parts)?,
        }),
        7 if mailbox_shape(&parts) => Ok(AgentAggregateKey::Context(mailbox(&parts))),
        _ => Err(corrupt()),
    }
}

fn decode_projection_key(parts: KeyParts) -> Result<AgentProjectionKey, StoreError> {
    match parts.kind {
        1 if name_shape(&parts) => Ok(AgentProjectionKey::Name(
            ShortText::new(parts.name).map_err(|_| corrupt())?,
        )),
        2 if simple_shape(&parts) => Ok(AgentProjectionKey::Agent(AgentId::from_bytes(parts.a))),
        3 if session_shape(&parts) => Ok(AgentProjectionKey::Session(session(&parts)?)),
        4 if mailbox_shape(&parts) => Ok(AgentProjectionKey::Context(mailbox(&parts))),
        5 if simple_shape(&parts) => {
            Ok(AgentProjectionKey::Selection(AgentId::from_bytes(parts.a)))
        }
        6 if agent_session_shape(&parts) => Ok(AgentProjectionKey::Rename {
            agent: AgentId::from_bytes(parts.a),
            session: session(&parts)?,
        }),
        7 if mailbox_session_shape(&parts) => Ok(AgentProjectionKey::DirectSession {
            mailbox: mailbox(&parts),
            session: session(&parts)?,
        }),
        _ => Err(corrupt()),
    }
}

fn simple_shape(parts: &KeyParts) -> bool {
    parts.b == ZERO
        && parts.name.is_empty()
        && parts.provider.is_empty()
        && parts.session.is_empty()
}

fn name_shape(parts: &KeyParts) -> bool {
    parts.a == ZERO
        && parts.b == ZERO
        && !parts.name.is_empty()
        && parts.provider.is_empty()
        && parts.session.is_empty()
}

fn mailbox_shape(parts: &KeyParts) -> bool {
    parts.name.is_empty() && parts.provider.is_empty() && parts.session.is_empty()
}

fn session_shape(parts: &KeyParts) -> bool {
    parts.a == ZERO
        && parts.b == ZERO
        && parts.name.is_empty()
        && !parts.provider.is_empty()
        && !parts.session.is_empty()
}

fn agent_session_shape(parts: &KeyParts) -> bool {
    parts.b == ZERO
        && parts.name.is_empty()
        && !parts.provider.is_empty()
        && !parts.session.is_empty()
}

fn mailbox_session_shape(parts: &KeyParts) -> bool {
    parts.name.is_empty() && !parts.provider.is_empty() && !parts.session.is_empty()
}

fn mailbox(parts: &KeyParts) -> MailboxAddress {
    MailboxAddress::new(
        InstallationId::from_bytes(parts.a),
        MailboxId::from_bytes(parts.b),
    )
}

fn session(parts: &KeyParts) -> Result<SessionIdentity, StoreError> {
    Ok(SessionIdentity {
        provider: ProviderId::new(&parts.provider).map_err(|_| corrupt())?,
        session: ProviderSessionId::new(&parts.session).map_err(|_| corrupt())?,
    })
}

fn load_facts(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
) -> Result<BTreeSet<FactId>, StoreError> {
    let sql = match table {
        "agent_frontiers" => {
            "SELECT fact_id FROM agent_frontiers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "agent_support" => {
            "SELECT fact_id FROM agent_support WHERE key_digest = ?1 ORDER BY fact_id"
        }
        _ => return Err(corrupt()),
    };
    let mut statement = connection.prepare(sql).map_err(database)?;
    statement
        .query_map([digest.as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(database)?
        .map(|row| {
            row.map_err(database)
                .and_then(fixed)
                .map(FactId::from_bytes)
        })
        .collect()
}

fn load_projection(
    connection: &Connection,
    digest: [u8; 32],
    key: &AgentProjectionKey,
) -> Result<AgentProjection, StoreError> {
    match key {
        AgentProjectionKey::Name(_) => load_name(connection, digest),
        AgentProjectionKey::Agent(_) => load_agent(connection, digest),
        AgentProjectionKey::Session(_) => load_session_binding(connection, digest),
        AgentProjectionKey::Context(_) => load_context(connection, digest),
        AgentProjectionKey::Selection(_) => load_selection(connection, digest),
        AgentProjectionKey::Rename { .. } => load_rename(connection, digest),
        AgentProjectionKey::DirectSession { mailbox, .. } => {
            load_direct_session(connection, digest, *mailbox)
        }
    }
}

fn load_name(connection: &Connection, digest: [u8; 32]) -> Result<AgentProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT conflicted, retired FROM agent_names WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let mut statement = connection
        .prepare(
            "SELECT fact_id, agent_id, mailbox_installation, mailbox_id \
             FROM agent_name_claims WHERE key_digest = ?1 ORDER BY fact_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(database)?;
    let mut claims = BTreeMap::new();
    for row in rows {
        let (fact, agent, installation, mailbox_id) = row.map_err(database)?;
        if claims
            .insert(
                FactId::from_bytes(fixed(fact)?),
                NameClaimSubject {
                    agent_id: AgentId::from_bytes(fixed(agent)?),
                    mailbox: MailboxAddress::new(
                        InstallationId::from_bytes(fixed(installation)?),
                        MailboxId::from_bytes(fixed(mailbox_id)?),
                    ),
                },
            )
            .is_some()
        {
            return Err(corrupt());
        }
    }
    let conflicted = decode_bool(row.0)?;
    let subjects = claims.values().cloned().collect::<BTreeSet<_>>();
    if conflicted != (subjects.len() > 1) {
        return Err(corrupt());
    }
    Ok(AgentProjection::Name(Box::new(NameReservationView {
        claims,
        conflicted,
        retired: decode_bool(row.1)?,
    })))
}

fn load_agent(connection: &Connection, digest: [u8; 32]) -> Result<AgentProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT lifecycle, runnable, selected_present, selected_provider, selected_session, \
                 name_reserved FROM agent_agents WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let claims = load_child_set(connection, "agent_agent_claims", digest)?;
    let names = load_child_names(connection, digest)?;
    let mailboxes = load_child_mailboxes(connection, digest)?;
    let retirements = load_child_set(connection, "agent_agent_retirements", digest)?;
    let lifecycle = decode_lifecycle(row.0).ok_or_else(corrupt)?;
    let runnable = decode_bool(row.1)?;
    let selected_session = decode_session_option(row.2, row.3, row.4)?;
    let name_reserved = decode_bool(row.5)?;
    let expected_lifecycle = if !retirements.is_empty() {
        AgentLifecycle::Retired
    } else if names.len() == 1 && mailboxes.len() == 1 {
        AgentLifecycle::Active
    } else {
        AgentLifecycle::Conflicted
    };
    if !name_reserved
        || runnable != selected_session.is_some()
        || (runnable && lifecycle != AgentLifecycle::Active)
        || (lifecycle == AgentLifecycle::Retired && expected_lifecycle != AgentLifecycle::Retired)
        || (lifecycle == AgentLifecycle::Active && expected_lifecycle != AgentLifecycle::Active)
    {
        return Err(corrupt());
    }
    Ok(AgentProjection::Agent(Box::new(AgentView {
        claims,
        names,
        mailboxes,
        retirements,
        lifecycle,
        runnable,
        selected_session,
        name_reserved,
    })))
}

fn load_session_binding(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<AgentProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT conflicted, mailbox_present, mailbox_installation, mailbox_id \
             FROM agent_sessions WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let bindings = load_binding_map(connection, digest)?;
    let mailboxes = bindings.values().copied().collect::<BTreeSet<_>>();
    let conflicted = decode_bool(row.0)?;
    let mailbox = decode_mailbox_option(row.1, fixed(row.2)?, fixed(row.3)?)?;
    if conflicted != (mailboxes.len() > 1)
        || mailbox
            != (mailboxes.len() == 1)
                .then(|| mailboxes.iter().next().copied())
                .flatten()
    {
        return Err(corrupt());
    }
    Ok(AgentProjection::Session(Box::new(SessionBindingView {
        bindings,
        conflicted,
        mailbox,
    })))
}

fn load_context(connection: &Connection, digest: [u8; 32]) -> Result<AgentProjection, StoreError> {
    let present = connection
        .query_row(
            "SELECT 1 FROM agent_contexts WHERE key_digest = ?1",
            [digest.as_slice()],
            |_| Ok(()),
        )
        .optional()
        .map_err(database)?;
    if present.is_none() {
        return Err(corrupt());
    }
    let history = load_context_history(connection, digest)?;
    let frontier = load_child_set(connection, "agent_context_frontiers", digest)?;
    if !frontier.is_subset(&history.keys().copied().collect()) {
        return Err(corrupt());
    }
    Ok(AgentProjection::Context(Box::new(ContextHistoryView {
        history,
        frontier,
    })))
}

fn load_selection(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<AgentProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT active_present, active_provider, active_session, active_directory_scheme, \
                 active_directory_value, active_repository_present, active_repository_scheme, \
                 active_repository_value, active_worktree_present, active_worktree_scheme, \
                 active_worktree_value, active_branch_present, active_branch, conflicted \
             FROM agent_selections WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok(StoredSelection {
                    active_present: row.get(0)?,
                    provider: row.get(1)?,
                    session: row.get(2)?,
                    context: StoredContext {
                        directory_scheme: row.get(3)?,
                        directory_value: row.get(4)?,
                        repository_present: row.get(5)?,
                        repository_scheme: row.get(6)?,
                        repository_value: row.get(7)?,
                        worktree_present: row.get(8)?,
                        worktree_scheme: row.get(9)?,
                        worktree_value: row.get(10)?,
                        branch_present: row.get(11)?,
                        branch: row.get(12)?,
                    },
                    conflicted: row.get(13)?,
                })
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let candidates = load_selection_candidates(connection, digest)?;
    let frontier = load_child_set(connection, "agent_selection_frontiers", digest)?;
    let active = decode_active_candidate(&row)?;
    let values = candidates.values().cloned().collect::<BTreeSet<_>>();
    let conflicted = decode_bool(row.conflicted)?;
    if frontier != candidates.keys().copied().collect()
        || conflicted != (values.len() > 1)
        || active.as_ref().is_some_and(|value| !values.contains(value))
        || (active.is_some() && conflicted)
    {
        return Err(corrupt());
    }
    Ok(AgentProjection::Selection(Box::new(SelectionView {
        candidates,
        frontier,
        active,
        conflicted,
    })))
}

fn load_rename(connection: &Connection, digest: [u8; 32]) -> Result<AgentProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT resolved, display_name_present, display_name FROM agent_renames \
             WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let candidates = load_rename_candidates(connection, digest)?;
    let frontier = load_child_set(connection, "agent_rename_frontiers", digest)?;
    let resolved = decode_bool(row.0)?;
    let display_name = decode_short_option(row.1, row.2)?;
    let values = candidates.values().cloned().collect::<BTreeSet<_>>();
    if frontier != candidates.keys().copied().collect()
        || (resolved && values.len() != 1)
        || (!resolved && display_name.is_some())
        || (resolved && display_name != values.iter().next().cloned().flatten())
    {
        return Err(corrupt());
    }
    Ok(AgentProjection::Rename(Box::new(RenameView {
        candidates,
        frontier,
        resolved,
        display_name,
    })))
}

fn load_direct_session(
    connection: &Connection,
    digest: [u8; 32],
    expected_mailbox: MailboxAddress,
) -> Result<AgentProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT mailbox_installation, mailbox_id, named_agent_present, named_agent, conflicted \
             FROM agent_direct_sessions WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let mailbox = MailboxAddress::new(
        InstallationId::from_bytes(fixed(row.0)?),
        MailboxId::from_bytes(fixed(row.1)?),
    );
    if mailbox != expected_mailbox {
        return Err(corrupt());
    }
    Ok(AgentProjection::DirectSession(Box::new(
        DirectSessionView {
            binding_facts: load_child_set(connection, "agent_direct_binding_facts", digest)?,
            mailbox,
            named_agent: decode_agent_option(row.2, fixed(row.3)?)?,
            conflicted: decode_bool(row.4)?,
        },
    )))
}

fn load_child_set(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
) -> Result<BTreeSet<FactId>, StoreError> {
    let sql = match table {
        "agent_agent_claims" => {
            "SELECT fact_id FROM agent_agent_claims WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "agent_agent_retirements" => {
            "SELECT fact_id FROM agent_agent_retirements WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "agent_context_frontiers" => {
            "SELECT fact_id FROM agent_context_frontiers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "agent_selection_frontiers" => {
            "SELECT fact_id FROM agent_selection_frontiers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "agent_rename_frontiers" => {
            "SELECT fact_id FROM agent_rename_frontiers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "agent_direct_binding_facts" => {
            "SELECT fact_id FROM agent_direct_binding_facts WHERE key_digest = ?1 ORDER BY fact_id"
        }
        _ => return Err(corrupt()),
    };
    let mut statement = connection.prepare(sql).map_err(database)?;
    statement
        .query_map([digest.as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(database)?
        .map(|row| {
            row.map_err(database)
                .and_then(fixed)
                .map(FactId::from_bytes)
        })
        .collect()
}

fn load_child_names(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeSet<ShortText>, StoreError> {
    let mut statement = connection
        .prepare("SELECT name FROM agent_agent_names WHERE key_digest = ?1 ORDER BY name")
        .map_err(database)?;
    statement
        .query_map([digest.as_slice()], |row| row.get::<_, String>(0))
        .map_err(database)?
        .map(|row| ShortText::new(row.map_err(database)?).map_err(|_| corrupt()))
        .collect()
}

fn load_child_mailboxes(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeSet<MailboxAddress>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT installation_id, mailbox_id FROM agent_agent_mailboxes \
             WHERE key_digest = ?1 ORDER BY installation_id, mailbox_id",
        )
        .map_err(database)?;
    statement
        .query_map([digest.as_slice()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(database)?
        .map(|row| {
            let (installation, mailbox_id) = row.map_err(database)?;
            Ok(MailboxAddress::new(
                InstallationId::from_bytes(fixed(installation)?),
                MailboxId::from_bytes(fixed(mailbox_id)?),
            ))
        })
        .collect()
}

fn load_binding_map(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeMap<FactId, MailboxAddress>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT fact_id, mailbox_installation, mailbox_id FROM agent_session_bindings \
             WHERE key_digest = ?1 ORDER BY fact_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(database)?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (fact, installation, mailbox_id) = row.map_err(database)?;
        if result
            .insert(
                FactId::from_bytes(fixed(fact)?),
                MailboxAddress::new(
                    InstallationId::from_bytes(fixed(installation)?),
                    MailboxId::from_bytes(fixed(mailbox_id)?),
                ),
            )
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(result)
}

struct StoredContext {
    directory_scheme: i64,
    directory_value: String,
    repository_present: i64,
    repository_scheme: i64,
    repository_value: String,
    worktree_present: i64,
    worktree_scheme: i64,
    worktree_value: String,
    branch_present: i64,
    branch: String,
}

struct StoredSelection {
    active_present: i64,
    provider: String,
    session: String,
    context: StoredContext,
    conflicted: i64,
}

fn load_context_history(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeMap<FactId, RepositoryContext>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT fact_id, directory_scheme, directory_value, repository_present, \
                 repository_scheme, repository_value, worktree_present, worktree_scheme, \
                 worktree_value, branch_present, branch FROM agent_context_history \
             WHERE key_digest = ?1 ORDER BY fact_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                StoredContext {
                    directory_scheme: row.get(1)?,
                    directory_value: row.get(2)?,
                    repository_present: row.get(3)?,
                    repository_scheme: row.get(4)?,
                    repository_value: row.get(5)?,
                    worktree_present: row.get(6)?,
                    worktree_scheme: row.get(7)?,
                    worktree_value: row.get(8)?,
                    branch_present: row.get(9)?,
                    branch: row.get(10)?,
                },
            ))
        })
        .map_err(database)?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (fact, context) = row.map_err(database)?;
        if result
            .insert(FactId::from_bytes(fixed(fact)?), decode_context(context)?)
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(result)
}

fn load_selection_candidates(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeMap<FactId, SelectionCandidate>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT fact_id, provider, session, directory_scheme, directory_value, \
                 repository_present, repository_scheme, repository_value, worktree_present, \
                 worktree_scheme, worktree_value, branch_present, branch \
             FROM agent_selection_candidates WHERE key_digest = ?1 ORDER BY fact_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                StoredContext {
                    directory_scheme: row.get(3)?,
                    directory_value: row.get(4)?,
                    repository_present: row.get(5)?,
                    repository_scheme: row.get(6)?,
                    repository_value: row.get(7)?,
                    worktree_present: row.get(8)?,
                    worktree_scheme: row.get(9)?,
                    worktree_value: row.get(10)?,
                    branch_present: row.get(11)?,
                    branch: row.get(12)?,
                },
            ))
        })
        .map_err(database)?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (fact, provider, session_value, context) = row.map_err(database)?;
        if result
            .insert(
                FactId::from_bytes(fixed(fact)?),
                SelectionCandidate {
                    session: decode_session(provider, session_value)?,
                    context: decode_context(context)?,
                },
            )
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(result)
}

fn load_rename_candidates(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeMap<FactId, Option<ShortText>>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT fact_id, display_name_present, display_name FROM agent_rename_candidates \
             WHERE key_digest = ?1 ORDER BY fact_id",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database)?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (fact, present, display) = row.map_err(database)?;
        if result
            .insert(
                FactId::from_bytes(fixed(fact)?),
                decode_short_option(present, display)?,
            )
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(result)
}

fn decode_active_candidate(
    row: &StoredSelection,
) -> Result<Option<SelectionCandidate>, StoreError> {
    match row.active_present {
        0 if row.provider.is_empty()
            && row.session.is_empty()
            && stored_context_is_empty(&row.context) =>
        {
            Ok(None)
        }
        1 => Ok(Some(SelectionCandidate {
            session: decode_session(row.provider.clone(), row.session.clone())?,
            context: decode_context(clone_stored_context(&row.context))?,
        })),
        _ => Err(corrupt()),
    }
}

fn clone_stored_context(value: &StoredContext) -> StoredContext {
    StoredContext {
        directory_scheme: value.directory_scheme,
        directory_value: value.directory_value.clone(),
        repository_present: value.repository_present,
        repository_scheme: value.repository_scheme,
        repository_value: value.repository_value.clone(),
        worktree_present: value.worktree_present,
        worktree_scheme: value.worktree_scheme,
        worktree_value: value.worktree_value.clone(),
        branch_present: value.branch_present,
        branch: value.branch.clone(),
    }
}

fn stored_context_is_empty(value: &StoredContext) -> bool {
    value.directory_scheme == 0
        && value.directory_value.is_empty()
        && value.repository_present == 0
        && value.repository_scheme == 0
        && value.repository_value.is_empty()
        && value.worktree_present == 0
        && value.worktree_scheme == 0
        && value.worktree_value.is_empty()
        && value.branch_present == 0
        && value.branch.is_empty()
}

fn decode_context(value: StoredContext) -> Result<RepositoryContext, StoreError> {
    Ok(RepositoryContext {
        directory: decode_required_locator(value.directory_scheme, value.directory_value)?,
        repository: decode_locator(
            value.repository_present,
            value.repository_scheme,
            value.repository_value,
        )?,
        worktree: decode_locator(
            value.worktree_present,
            value.worktree_scheme,
            value.worktree_value,
        )?,
        branch: decode_short_option(value.branch_present, value.branch)?,
    })
}

fn decode_required_locator(scheme: i64, value: String) -> Result<ResourceLocator, StoreError> {
    Ok(ResourceLocator::new(
        decode_scheme(scheme).ok_or_else(corrupt)?,
        BoundedText::new(value).map_err(|_| corrupt())?,
    ))
}

fn decode_locator(
    present: i64,
    scheme: i64,
    value: String,
) -> Result<Option<ResourceLocator>, StoreError> {
    match (present, scheme, value.is_empty()) {
        (0, 0, true) => Ok(None),
        (1, _, false) => Ok(Some(decode_required_locator(scheme, value)?)),
        _ => Err(corrupt()),
    }
}

const fn encode_lifecycle(value: AgentLifecycle) -> i64 {
    match value {
        AgentLifecycle::Active => 1,
        AgentLifecycle::Conflicted => 2,
        AgentLifecycle::Retired => 3,
    }
}

const fn decode_lifecycle(value: i64) -> Option<AgentLifecycle> {
    match value {
        1 => Some(AgentLifecycle::Active),
        2 => Some(AgentLifecycle::Conflicted),
        3 => Some(AgentLifecycle::Retired),
        _ => None,
    }
}

const fn encode_scheme(value: ResourceScheme) -> i64 {
    match value {
        ResourceScheme::GitRepository => 1,
        ResourceScheme::WorkingTree => 2,
        ResourceScheme::Container => 3,
        ResourceScheme::Opaque => 4,
    }
}

const fn decode_scheme(value: i64) -> Option<ResourceScheme> {
    match value {
        1 => Some(ResourceScheme::GitRepository),
        2 => Some(ResourceScheme::WorkingTree),
        3 => Some(ResourceScheme::Container),
        4 => Some(ResourceScheme::Opaque),
        _ => None,
    }
}

fn decode_session_option(
    present: i64,
    provider: String,
    session: String,
) -> Result<Option<SessionIdentity>, StoreError> {
    match (present, provider.is_empty(), session.is_empty()) {
        (0, true, true) => Ok(None),
        (1, false, false) => Ok(Some(decode_session(provider, session)?)),
        _ => Err(corrupt()),
    }
}

fn decode_session(provider: String, session: String) -> Result<SessionIdentity, StoreError> {
    Ok(SessionIdentity {
        provider: ProviderId::new(provider).map_err(|_| corrupt())?,
        session: ProviderSessionId::new(session).map_err(|_| corrupt())?,
    })
}

fn decode_mailbox_option(
    present: i64,
    installation: [u8; 32],
    mailbox: [u8; 32],
) -> Result<Option<MailboxAddress>, StoreError> {
    match (present, installation == ZERO, mailbox == ZERO) {
        (0, true, true) => Ok(None),
        (1, _, _) => Ok(Some(MailboxAddress::new(
            InstallationId::from_bytes(installation),
            MailboxId::from_bytes(mailbox),
        ))),
        _ => Err(corrupt()),
    }
}

fn decode_agent_option(present: i64, agent: [u8; 32]) -> Result<Option<AgentId>, StoreError> {
    match (present, agent == ZERO) {
        (0, true) => Ok(None),
        (1, _) => Ok(Some(AgentId::from_bytes(agent))),
        _ => Err(corrupt()),
    }
}

fn decode_short_option(present: i64, value: String) -> Result<Option<ShortText>, StoreError> {
    match (present, value.is_empty()) {
        (0, true) => Ok(None),
        (1, false) => Ok(Some(ShortText::new(value).map_err(|_| corrupt())?)),
        _ => Err(corrupt()),
    }
}

fn decode_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt()),
    }
}

fn fixed(value: Vec<u8>) -> Result<[u8; 32], StoreError> {
    value.try_into().map_err(|_| corrupt())
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Counts {
    aggregate_key_count: i64,
    frontier_count: i64,
    projection_key_count: i64,
    projection_count: i64,
    support_count: i64,
    row_count: i64,
}

impl Counts {
    fn read(connection: &Connection) -> Result<Self, StoreError> {
        let aggregate_key_count = count(connection, "agent_aggregate_keys")?;
        let frontier_count = count(connection, "agent_frontiers")?;
        let projection_key_count = count(connection, "agent_projection_keys")?;
        let support_count = count(connection, "agent_support")?;
        let projection_count = [
            "agent_names",
            "agent_agents",
            "agent_sessions",
            "agent_contexts",
            "agent_selections",
            "agent_renames",
            "agent_direct_sessions",
        ]
        .into_iter()
        .try_fold(0_i64, |total, table| {
            total
                .checked_add(count(connection, table)?)
                .ok_or_else(corrupt)
        })?;
        let row_count = TABLES.into_iter().try_fold(0_i64, |total, table| {
            total
                .checked_add(count(connection, table)?)
                .ok_or_else(corrupt)
        })?;
        let counts = Self {
            aggregate_key_count,
            frontier_count,
            projection_key_count,
            projection_count,
            support_count,
            row_count,
        };
        counts.validate()?;
        Ok(counts)
    }

    fn validate(self) -> Result<(), StoreError> {
        for value in [
            self.aggregate_key_count,
            self.frontier_count,
            self.projection_key_count,
            self.projection_count,
            self.support_count,
            self.row_count,
        ] {
            if !(0..=MAXIMUM_AGENT_ROWS).contains(&value) {
                return Err(corrupt());
            }
        }
        Ok(())
    }
}

struct State {
    counts: Counts,
    digest: [u8; 32],
}

fn load_state(connection: &Connection) -> Result<Option<State>, StoreError> {
    let row = connection
        .query_row(
            "SELECT aggregate_key_count, frontier_count, projection_key_count, projection_count, \
                 support_count, row_count, row_digest FROM agent_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database)?;
    row.map(|row| {
        Ok(State {
            counts: Counts {
                aggregate_key_count: row.0,
                frontier_count: row.1,
                projection_key_count: row.2,
                projection_count: row.3,
                support_count: row.4,
                row_count: row.5,
            },
            digest: fixed(row.6)?,
        })
    })
    .transpose()
}

fn count(connection: &Connection, table: &str) -> Result<i64, StoreError> {
    if !TABLES.contains(&table) {
        return Err(corrupt());
    }
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(database)
}

fn row_digest(connection: &Connection) -> Result<[u8; 32], StoreError> {
    const QUERIES: [&str; 24] = [
        "SELECT * FROM agent_aggregate_keys ORDER BY key_digest",
        "SELECT * FROM agent_frontiers ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_projection_keys ORDER BY key_digest",
        "SELECT * FROM agent_support ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_names ORDER BY key_digest",
        "SELECT * FROM agent_name_claims ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_agents ORDER BY key_digest",
        "SELECT * FROM agent_agent_claims ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_agent_names ORDER BY key_digest, name",
        "SELECT * FROM agent_agent_mailboxes ORDER BY key_digest, installation_id, mailbox_id",
        "SELECT * FROM agent_agent_retirements ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_sessions ORDER BY key_digest",
        "SELECT * FROM agent_session_bindings ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_contexts ORDER BY key_digest",
        "SELECT * FROM agent_context_history ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_context_frontiers ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_selections ORDER BY key_digest",
        "SELECT * FROM agent_selection_candidates ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_selection_frontiers ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_renames ORDER BY key_digest",
        "SELECT * FROM agent_rename_candidates ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_rename_frontiers ORDER BY key_digest, fact_id",
        "SELECT * FROM agent_direct_sessions ORDER BY key_digest",
        "SELECT * FROM agent_direct_binding_facts ORDER BY key_digest, fact_id",
    ];
    let mut digest = Sha256::new();
    for (table, query) in TABLES.into_iter().zip(QUERIES) {
        put_text(&mut digest, table);
        let mut statement = connection.prepare(query).map_err(database)?;
        let columns = statement.column_count();
        let mut rows = statement.query([]).map_err(database)?;
        while let Some(row) = rows.next().map_err(database)? {
            digest.update(u64::try_from(columns).unwrap_or(u64::MAX).to_be_bytes());
            for index in 0..columns {
                put_value(&mut digest, row.get_ref(index).map_err(database)?);
            }
        }
    }
    Ok(digest.finalize().into())
}

fn put_value(digest: &mut Sha256, value: rusqlite::types::ValueRef<'_>) {
    match value {
        rusqlite::types::ValueRef::Null => digest.update([0]),
        rusqlite::types::ValueRef::Integer(value) => {
            digest.update([1]);
            digest.update(value.to_be_bytes());
        }
        rusqlite::types::ValueRef::Real(value) => {
            digest.update([2]);
            digest.update(value.to_bits().to_be_bytes());
        }
        rusqlite::types::ValueRef::Text(value) => {
            digest.update([3]);
            digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(value);
        }
        rusqlite::types::ValueRef::Blob(value) => {
            digest.update([4]);
            digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(value);
        }
    }
}

fn validate_counts(snapshot: &AgentProjectionSnapshot, counts: Counts) -> Result<(), StoreError> {
    counts.validate()?;
    if snapshot.projections.keys().ne(snapshot.support.keys())
        || counts.aggregate_key_count != length(std::iter::once(snapshot.frontiers.len()))?
        || counts.frontier_count != length(snapshot.frontiers.values().map(BTreeSet::len))?
        || counts.projection_key_count != length(std::iter::once(snapshot.projections.len()))?
        || counts.projection_count != length(std::iter::once(snapshot.projections.len()))?
        || counts.support_count != length(snapshot.support.values().map(BTreeSet::len))?
    {
        return Err(corrupt());
    }
    Ok(())
}

fn capacity(count: i64) -> Result<usize, StoreError> {
    if !(0..=MAXIMUM_AGENT_ROWS).contains(&count) {
        return Err(corrupt());
    }
    usize::try_from(count).map_err(|_| corrupt())
}

fn length(values: impl IntoIterator<Item = usize>) -> Result<i64, StoreError> {
    values.into_iter().try_fold(0_i64, |total, value| {
        total
            .checked_add(i64::try_from(value).map_err(|_| corrupt())?)
            .ok_or_else(corrupt)
    })
}

fn database(_: rusqlite::Error) -> StoreError {
    StoreError::new(StoreErrorClass::DatabaseUnavailable)
}

fn corrupt() -> StoreError {
    StoreError::new(StoreErrorClass::RebuildableStateCorrupt)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn every_agent_projection_variant_and_register_state_round_trips() {
        let expected = exhaustive_snapshot();
        let connection = fixture_connection(&expected);
        assert_eq!(load(&connection).expect("agent rows load"), expected);
    }

    #[test]
    fn agent_scalar_codecs_are_closed() {
        for lifecycle in [
            AgentLifecycle::Active,
            AgentLifecycle::Conflicted,
            AgentLifecycle::Retired,
        ] {
            assert_eq!(
                decode_lifecycle(encode_lifecycle(lifecycle)),
                Some(lifecycle)
            );
        }
        assert_eq!(decode_lifecycle(0), None);
        assert_eq!(decode_lifecycle(4), None);
        for scheme in [
            ResourceScheme::GitRepository,
            ResourceScheme::WorkingTree,
            ResourceScheme::Container,
            ResourceScheme::Opaque,
        ] {
            assert_eq!(decode_scheme(encode_scheme(scheme)), Some(scheme));
        }
        assert_eq!(decode_scheme(0), None);
        assert!(decode_session_option(0, "provider".to_owned(), String::new()).is_err());
        assert!(decode_short_option(1, String::new()).is_err());
        assert!(decode_locator(0, 1, "value".to_owned()).is_err());
    }

    #[test]
    fn every_agent_table_family_fails_closed_on_valid_looking_corruption() {
        let expected = exhaustive_snapshot();
        for mutation in [
            "UPDATE agent_state SET row_count = row_count + 1",
            "UPDATE agent_aggregate_keys SET name = 'changed' WHERE key_kind = 1",
            "UPDATE agent_frontiers SET fact_id = zeroblob(32)",
            "UPDATE agent_projection_keys SET name = 'changed' WHERE key_kind = 1",
            "UPDATE agent_support SET fact_id = zeroblob(32)",
            "UPDATE agent_names SET retired = CASE retired WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE agent_name_claims SET agent_id = zeroblob(32)",
            "UPDATE agent_agents SET name_reserved = 0",
            "UPDATE agent_agent_claims SET fact_id = zeroblob(32) WHERE fact_id = (SELECT fact_id FROM agent_agent_claims LIMIT 1)",
            "UPDATE agent_agent_names SET name = name || '-changed'",
            "UPDATE agent_agent_mailboxes SET installation_id = zeroblob(32)",
            "UPDATE agent_agent_retirements SET fact_id = zeroblob(32)",
            "UPDATE agent_sessions SET conflicted = CASE conflicted WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE agent_session_bindings SET mailbox_id = zeroblob(32)",
            "DELETE FROM agent_context_frontiers; DELETE FROM agent_context_history; DELETE FROM agent_contexts",
            "UPDATE agent_context_history SET directory_value = 'changed'",
            "UPDATE agent_context_frontiers SET fact_id = zeroblob(32)",
            "UPDATE agent_selections SET conflicted = CASE conflicted WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE agent_selection_candidates SET directory_value = 'changed'",
            "UPDATE agent_selection_frontiers SET fact_id = zeroblob(32) WHERE fact_id = (SELECT fact_id FROM agent_selection_frontiers LIMIT 1)",
            "UPDATE agent_renames SET resolved = CASE resolved WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE agent_rename_candidates SET display_name = 'changed', display_name_present = 1",
            "UPDATE agent_rename_frontiers SET fact_id = zeroblob(32) WHERE fact_id = (SELECT fact_id FROM agent_rename_frontiers LIMIT 1)",
            "UPDATE agent_direct_sessions SET conflicted = CASE conflicted WHEN 1 THEN 0 ELSE 1 END",
            "UPDATE agent_direct_binding_facts SET fact_id = zeroblob(32)",
        ] {
            let connection = fixture_connection(&expected);
            connection
                .execute_batch(mutation)
                .expect("constraint-valid mutation applies");
            assert_eq!(
                load(&connection)
                    .expect_err("changed agent rows reject")
                    .class(),
                StoreErrorClass::RebuildableStateCorrupt,
                "mutation unexpectedly loaded: {mutation}",
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    fn exhaustive_snapshot() -> AgentProjectionSnapshot {
        let agent_id = AgentId::from_bytes([0x41; 32]);
        let active_agent = AgentId::from_bytes([0x42; 32]);
        let mailbox_one = MailboxAddress::new(
            InstallationId::from_bytes([0x43; 32]),
            MailboxId::from_bytes([0x44; 32]),
        );
        let mailbox_two = MailboxAddress::new(
            InstallationId::from_bytes([0x45; 32]),
            MailboxId::from_bytes([0x46; 32]),
        );
        let session_one = session("provider-one", "session-one");
        let session_two = session("provider-two", "session-two");
        let context_one = context("/workspace/one", Some("repo:one"), None, Some("main"));
        let context_two = context("/workspace/two", None, Some("/workspace/two"), None);
        let candidate_one = SelectionCandidate {
            session: session_one.clone(),
            context: context_one.clone(),
        };
        let candidate_two = SelectionCandidate {
            session: session_two.clone(),
            context: context_two.clone(),
        };
        let primary_keys = [
            AgentProjectionKey::Name(text("helper")),
            AgentProjectionKey::Agent(agent_id),
            AgentProjectionKey::Session(session_one.clone()),
            AgentProjectionKey::Context(mailbox_one),
            AgentProjectionKey::Selection(agent_id),
            AgentProjectionKey::Rename {
                agent: agent_id,
                session: session_one.clone(),
            },
            AgentProjectionKey::DirectSession {
                mailbox: mailbox_one,
                session: session_one.clone(),
            },
        ];
        let mut projections = BTreeMap::from([
            (
                primary_keys[0].clone(),
                AgentProjection::Name(Box::new(NameReservationView {
                    claims: BTreeMap::from([
                        (
                            id(1),
                            NameClaimSubject {
                                agent_id,
                                mailbox: mailbox_one,
                            },
                        ),
                        (
                            id(2),
                            NameClaimSubject {
                                agent_id: active_agent,
                                mailbox: mailbox_two,
                            },
                        ),
                    ]),
                    conflicted: true,
                    retired: true,
                })),
            ),
            (
                primary_keys[1].clone(),
                AgentProjection::Agent(Box::new(AgentView {
                    claims: BTreeSet::from([id(1), id(2)]),
                    names: BTreeSet::from([text("helper"), text("other")]),
                    mailboxes: BTreeSet::from([mailbox_one, mailbox_two]),
                    retirements: BTreeSet::from([id(3)]),
                    lifecycle: AgentLifecycle::Retired,
                    runnable: false,
                    selected_session: None,
                    name_reserved: true,
                })),
            ),
            (
                primary_keys[2].clone(),
                AgentProjection::Session(Box::new(SessionBindingView {
                    bindings: BTreeMap::from([(id(4), mailbox_one), (id(5), mailbox_two)]),
                    conflicted: true,
                    mailbox: None,
                })),
            ),
            (
                primary_keys[3].clone(),
                AgentProjection::Context(Box::new(ContextHistoryView {
                    history: BTreeMap::from([(id(6), context_one.clone()), (id(7), context_two)]),
                    frontier: BTreeSet::from([id(7)]),
                })),
            ),
            (
                primary_keys[4].clone(),
                AgentProjection::Selection(Box::new(SelectionView {
                    candidates: BTreeMap::from([
                        (id(8), candidate_one.clone()),
                        (id(9), candidate_two),
                    ]),
                    frontier: BTreeSet::from([id(8), id(9)]),
                    active: None,
                    conflicted: true,
                })),
            ),
            (
                primary_keys[5].clone(),
                AgentProjection::Rename(Box::new(RenameView {
                    candidates: BTreeMap::from([(id(10), Some(text("display"))), (id(11), None)]),
                    frontier: BTreeSet::from([id(10), id(11)]),
                    resolved: false,
                    display_name: None,
                })),
            ),
            (
                primary_keys[6].clone(),
                AgentProjection::DirectSession(Box::new(DirectSessionView {
                    binding_facts: BTreeSet::from([id(12)]),
                    mailbox: mailbox_one,
                    named_agent: Some(agent_id),
                    conflicted: true,
                })),
            ),
        ]);
        let active_key = AgentProjectionKey::Agent(active_agent);
        projections.insert(
            active_key.clone(),
            AgentProjection::Agent(Box::new(AgentView {
                claims: BTreeSet::from([id(13)]),
                names: BTreeSet::from([text("active")]),
                mailboxes: BTreeSet::from([mailbox_one]),
                retirements: BTreeSet::new(),
                lifecycle: AgentLifecycle::Active,
                runnable: true,
                selected_session: Some(session_one.clone()),
                name_reserved: true,
            })),
        );
        let selected_key = AgentProjectionKey::Selection(active_agent);
        projections.insert(
            selected_key.clone(),
            AgentProjection::Selection(Box::new(SelectionView {
                candidates: BTreeMap::from([(id(14), candidate_one.clone())]),
                frontier: BTreeSet::from([id(14)]),
                active: Some(candidate_one),
                conflicted: false,
            })),
        );
        let clear_key = AgentProjectionKey::Rename {
            agent: active_agent,
            session: session_two,
        };
        projections.insert(
            clear_key.clone(),
            AgentProjection::Rename(Box::new(RenameView {
                candidates: BTreeMap::from([(id(15), None)]),
                frontier: BTreeSet::from([id(15)]),
                resolved: true,
                display_name: None,
            })),
        );
        let frontiers = BTreeMap::from([
            (
                AgentAggregateKey::Name(text("helper")),
                BTreeSet::from([id(1)]),
            ),
            (AgentAggregateKey::Agent(agent_id), BTreeSet::from([id(3)])),
            (
                AgentAggregateKey::Mailbox(mailbox_one),
                BTreeSet::from([id(4)]),
            ),
            (
                AgentAggregateKey::Session(session_one.clone()),
                BTreeSet::from([id(5)]),
            ),
            (
                AgentAggregateKey::Selection(agent_id),
                BTreeSet::from([id(9)]),
            ),
            (
                AgentAggregateKey::Rename {
                    agent: agent_id,
                    session: session_one,
                },
                BTreeSet::from([id(11)]),
            ),
            (
                AgentAggregateKey::Context(mailbox_one),
                BTreeSet::from([id(7)]),
            ),
        ]);
        let support = projections
            .keys()
            .cloned()
            .enumerate()
            .map(|(index, key)| {
                (
                    key,
                    BTreeSet::from([id(u8::try_from(index + 30).expect("fixture id fits"))]),
                )
            })
            .collect();
        AgentProjectionSnapshot {
            frontiers,
            projections,
            support,
        }
    }

    fn session(provider: &str, value: &str) -> SessionIdentity {
        SessionIdentity {
            provider: ProviderId::new(provider).expect("provider validates"),
            session: ProviderSessionId::new(value).expect("session validates"),
        }
    }

    fn context(
        directory: &str,
        repository: Option<&str>,
        worktree: Option<&str>,
        branch: Option<&str>,
    ) -> RepositoryContext {
        RepositoryContext {
            directory: locator(ResourceScheme::WorkingTree, directory),
            repository: repository.map(|value| locator(ResourceScheme::GitRepository, value)),
            worktree: worktree.map(|value| locator(ResourceScheme::WorkingTree, value)),
            branch: branch.map(text),
        }
    }

    fn locator(scheme: ResourceScheme, value: &str) -> ResourceLocator {
        ResourceLocator::new(
            scheme,
            BoundedText::new(value).expect("resource value validates"),
        )
    }

    fn text(value: &str) -> ShortText {
        ShortText::new(value).expect("short text validates")
    }

    fn fixture_connection(expected: &AgentProjectionSnapshot) -> Connection {
        let mut connection = Connection::open_in_memory().expect("memory database opens");
        connection
            .execute_batch(super::super::SCHEMA)
            .expect("schema creates");
        for value in 0_u8..=60 {
            connection
                .execute(
                    "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                     VALUES (?1, ?2, 1, 1)",
                    params![id(value).as_bytes().as_slice(), vec![value]],
                )
                .expect("canonical support inserts");
        }
        let transaction = connection.transaction().expect("transaction starts");
        insert(&transaction, expected).expect("agent rows insert");
        transaction.commit().expect("agent rows commit");
        connection
    }

    fn id(value: u8) -> FactId {
        FactId::from_bytes([value; 32])
    }
}
