//! Explicit relational codecs for rebuildable conversation and activity projections.

use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
};

use hq_domain::{
    AccountId, ActivityKind, ActivityStatus, BoundedVec, CompletedFileChange,
    CompletedItemPresentation, ContentText, ErrorCode, FactId, InstallationId, MailboxAddress,
    MailboxId, MessageContent, MessageId, MessagePurpose, OperationCorrelation, OperationId,
    PresentationKind, ProjectId, ProviderId, ProviderSessionId, ShortText, ThreadId, Timestamp,
};
use hq_reducer::{
    ActionGroupView, ActivityKey, ActivityRetentionView, ActivitySessionKey, ActivityView,
    CausalRelation, ConversationAggregateKey, ConversationProjection, ConversationProjectionKey,
    MessageView, ThreadView,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ConversationEntry, ConversationMessageEntry, ConversationProjectionSnapshot, StoreError,
    StoreErrorClass,
};

const MAXIMUM_CONVERSATION_ROWS: i64 = 64_000_000;
const ZERO: [u8; 32] = [0; 32];

const TABLES: [&str; 17] = [
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
];

pub(super) fn clear(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction
        .execute_batch(
            "DELETE FROM conversation_state;
             DELETE FROM conversation_retained_progress;
             DELETE FROM conversation_activity_retentions;
             DELETE FROM conversation_activities;
             DELETE FROM conversation_action_entries;
             DELETE FROM conversation_action_groups;
             DELETE FROM conversation_message_receipts;
             DELETE FROM conversation_message_frontiers;
             DELETE FROM conversation_messages;
             DELETE FROM conversation_thread_ready_answers;
             DELETE FROM conversation_thread_relations;
             DELETE FROM conversation_thread_cancellations;
             DELETE FROM conversation_thread_answers;
             DELETE FROM conversation_threads;
             DELETE FROM conversation_support;
             DELETE FROM conversation_projection_keys;
             DELETE FROM conversation_frontiers;
             DELETE FROM conversation_aggregate_keys;",
        )
        .map_err(database)
}

pub(super) fn insert(
    transaction: &Transaction<'_>,
    snapshot: &ConversationProjectionSnapshot,
) -> Result<(), StoreError> {
    if snapshot.projections().keys().ne(snapshot.support().keys()) {
        return Err(corrupt());
    }
    for (key, facts) in snapshot.frontiers() {
        let parts = aggregate_parts(key);
        let digest = insert_key(transaction, KeyTable::Aggregate, &parts)?;
        insert_facts(transaction, "conversation_frontiers", digest, facts)?;
    }
    for (key, projection) in snapshot.projections() {
        let parts = projection_parts(key);
        let digest = insert_key(transaction, KeyTable::Projection, &parts)?;
        insert_projection(transaction, digest, key, projection)?;
        let support = snapshot.support().get(key).ok_or_else(corrupt)?;
        insert_facts(transaction, "conversation_support", digest, support)?;
    }
    let counts = Counts::read(transaction)?;
    counts.validate()?;
    if counts.aggregate_key_count != length(std::iter::once(snapshot.frontiers().len()))?
        || counts.frontier_count != length(snapshot.frontiers().values().map(BTreeSet::len))?
        || counts.projection_key_count != length(std::iter::once(snapshot.projections().len()))?
        || counts.projection_count != length(std::iter::once(snapshot.projections().len()))?
        || counts.support_count != length(snapshot.support().values().map(BTreeSet::len))?
    {
        return Err(corrupt());
    }
    let digest = row_digest(transaction)?;
    transaction
        .execute(
            "INSERT INTO conversation_state(singleton, aggregate_key_count, frontier_count, \
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

pub(super) fn load(connection: &Connection) -> Result<ConversationProjectionSnapshot, StoreError> {
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
    let (projection_digests, projection_keys) = load_projection_keys(connection)?;
    let mut projections = BTreeMap::new();
    let mut support = BTreeMap::new();
    for (digest, key) in projection_digests.into_iter().zip(projection_keys) {
        let value = load_projection(connection, digest, &key)?;
        if projections.insert(key.clone(), value).is_some() {
            return Err(corrupt());
        }
        let facts = load_facts(connection, "conversation_support", digest)?;
        support.insert(key, facts);
    }
    let snapshot = ConversationProjectionSnapshot::new(frontiers, projections, support);
    validate_snapshot(&snapshot, state.counts)?;
    Ok(snapshot)
}

pub(super) fn load_entry(
    connection: &Connection,
    fact_id: FactId,
    entry_kind: i64,
) -> Result<ConversationEntry, StoreError> {
    let table = match entry_kind {
        1 => "conversation_messages",
        2 => "conversation_activities",
        _ => return Err(corrupt()),
    };
    let sql = match table {
        "conversation_messages" => {
            "SELECT key_digest FROM conversation_messages WHERE fact_id = ?1"
        }
        "conversation_activities" => {
            "SELECT key_digest FROM conversation_activities WHERE fact_id = ?1"
        }
        _ => return Err(corrupt()),
    };
    let mut statement = connection.prepare(sql).map_err(database)?;
    let digests = statement
        .query_map([fact_id.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .map_err(database)?
        .map(|row| row.map_err(database).and_then(fixed))
        .collect::<Result<Vec<_>, _>>()?;
    let [digest] = digests.as_slice() else {
        return Err(corrupt());
    };
    let key = load_projection_key(connection, *digest)?;
    let projection = load_projection(connection, *digest, &key)?;
    match (entry_kind, projection) {
        (1, ConversationProjection::Message(message)) if message.fact_id == fact_id => {
            let thread_key = ConversationProjectionKey::Thread(message.thread_id);
            let thread_digest = key_digest(KeyTable::Projection, &projection_parts(&thread_key));
            let thread = match load_projection(connection, thread_digest, &thread_key) {
                Ok(ConversationProjection::Thread(thread)) => Some(*thread),
                Err(_) if message.content.purpose != MessagePurpose::Question => None,
                Ok(_) | Err(_) => return Err(corrupt()),
            };
            Ok(ConversationEntry::Message(Box::new(
                ConversationMessageEntry {
                    message: *message,
                    thread,
                },
            )))
        }
        (2, ConversationProjection::Activity(activity)) if activity.fact_id == fact_id => {
            Ok(ConversationEntry::Activity(activity))
        }
        _ => Err(corrupt()),
    }
}

fn load_projection_key(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<ConversationProjectionKey, StoreError> {
    type Row = (
        i64,
        Vec<u8>,
        Vec<u8>,
        String,
        String,
        Vec<u8>,
        i64,
        String,
        i64,
        String,
        String,
    );
    let row: Row = connection
        .query_row(
            "SELECT key_kind, key_a, key_b, provider, session, operation_id, item_present, item, \
                    activity_kind, logical_key, runtime \
             FROM conversation_projection_keys WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let parts = KeyParts {
        kind: row.0,
        a: fixed(row.1)?,
        b: fixed(row.2)?,
        provider: row.3,
        session: row.4,
        operation: fixed(row.5)?,
        item: decode_item_sql(row.6, row.7).map_err(database)?,
        activity_kind: row.8,
        logical_key: row.9,
        runtime: row.10,
    };
    if key_digest(KeyTable::Projection, &parts) != digest {
        return Err(corrupt());
    }
    decode_projection_key(parts)
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
    provider: String,
    session: String,
    operation: [u8; 32],
    item: Option<String>,
    activity_kind: i64,
    logical_key: String,
    runtime: String,
}

impl KeyParts {
    fn simple(kind: i64, a: [u8; 32]) -> Self {
        Self {
            kind,
            a,
            b: ZERO,
            provider: String::new(),
            session: String::new(),
            operation: ZERO,
            item: None,
            activity_kind: 0,
            logical_key: String::new(),
            runtime: String::new(),
        }
    }

    fn operation(kind: i64, correlation: &OperationCorrelation) -> Self {
        Self {
            kind,
            a: ZERO,
            b: ZERO,
            provider: correlation.provider().as_str().to_owned(),
            session: correlation.session().as_str().to_owned(),
            operation: *correlation.operation().as_bytes(),
            item: None,
            activity_kind: 0,
            logical_key: String::new(),
            runtime: String::new(),
        }
    }

    fn activity(kind: i64, key: &ActivityKey) -> Self {
        Self {
            kind,
            a: *key.source.installation_id().as_bytes(),
            b: *key.source.mailbox_id().as_bytes(),
            provider: key.correlation.provider().as_str().to_owned(),
            session: key.correlation.session().as_str().to_owned(),
            operation: *key.correlation.operation().as_bytes(),
            item: key.item.as_ref().map(|value| value.as_str().to_owned()),
            activity_kind: encode_activity_kind(key.kind),
            logical_key: key.logical_key.as_str().to_owned(),
            runtime: key.runtime.as_str().to_owned(),
        }
    }

    fn retention(kind: i64, key: &ActivitySessionKey) -> Self {
        Self {
            kind,
            a: *key.source.installation_id().as_bytes(),
            b: *key.source.mailbox_id().as_bytes(),
            provider: key.provider.as_str().to_owned(),
            session: key.session.as_str().to_owned(),
            operation: ZERO,
            item: None,
            activity_kind: 0,
            logical_key: String::new(),
            runtime: String::new(),
        }
    }
}

fn aggregate_parts(key: &ConversationAggregateKey) -> KeyParts {
    match key {
        ConversationAggregateKey::MessageIdentity(value) => KeyParts::simple(1, *value.as_bytes()),
        ConversationAggregateKey::Thread(value) => KeyParts::simple(2, *value.as_bytes()),
        ConversationAggregateKey::MessageState(value) => KeyParts::simple(3, *value.as_bytes()),
        ConversationAggregateKey::Activity(value) => KeyParts::activity(4, value),
    }
}

fn projection_parts(key: &ConversationProjectionKey) -> KeyParts {
    match key {
        ConversationProjectionKey::Thread(value) => KeyParts::simple(1, *value.as_bytes()),
        ConversationProjectionKey::Message(value) => KeyParts::simple(2, *value.as_bytes()),
        ConversationProjectionKey::ActionGroup(value) => KeyParts::operation(3, value),
        ConversationProjectionKey::Activity(value) => KeyParts::activity(4, value),
        ConversationProjectionKey::ActivityRecord(value) => KeyParts::simple(5, *value.as_bytes()),
        ConversationProjectionKey::ActivityRetention(value) => KeyParts::retention(6, value),
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
            "INSERT INTO conversation_aggregate_keys(key_digest, key_kind, key_a, key_b, provider, \
                 session, operation_id, item_present, item, activity_kind, logical_key, runtime) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        }
        KeyTable::Projection => {
            "INSERT INTO conversation_projection_keys(key_digest, key_kind, key_a, key_b, provider, \
                 session, operation_id, item_present, item, activity_kind, logical_key, runtime) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
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
                parts.provider,
                parts.session,
                parts.operation.as_slice(),
                i64::from(parts.item.is_some()),
                parts.item.as_deref().unwrap_or(""),
                parts.activity_kind,
                parts.logical_key,
                parts.runtime,
            ],
        )
        .map_err(database)?;
    Ok(digest)
}

fn key_digest(table: KeyTable, parts: &KeyParts) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(match table {
        KeyTable::Aggregate => b"hq-conversation-aggregate-key-v1".as_slice(),
        KeyTable::Projection => b"hq-conversation-projection-key-v1".as_slice(),
    });
    digest.update(parts.kind.to_be_bytes());
    digest.update(parts.a);
    digest.update(parts.b);
    put_text(&mut digest, &parts.provider);
    put_text(&mut digest, &parts.session);
    digest.update(parts.operation);
    digest.update([u8::from(parts.item.is_some())]);
    put_text(&mut digest, parts.item.as_deref().unwrap_or(""));
    digest.update(parts.activity_kind.to_be_bytes());
    put_text(&mut digest, &parts.logical_key);
    put_text(&mut digest, &parts.runtime);
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
        "conversation_frontiers" => {
            "INSERT INTO conversation_frontiers(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "conversation_support" => {
            "INSERT INTO conversation_support(key_digest, fact_id) VALUES (?1, ?2)"
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

#[allow(clippy::too_many_lines)]
fn insert_projection(
    transaction: &Transaction<'_>,
    digest: [u8; 32],
    key: &ConversationProjectionKey,
    projection: &ConversationProjection,
) -> Result<(), StoreError> {
    match (key, projection) {
        (ConversationProjectionKey::Thread(_), ConversationProjection::Thread(view)) => {
            transaction
                .execute(
                    "INSERT INTO conversation_threads(key_digest, root_fact, root_message, cancelled) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        digest.as_slice(),
                        view.root_fact.as_bytes().as_slice(),
                        view.root_message.as_bytes().as_slice(),
                        i64::from(view.cancelled),
                    ],
                )
                .map_err(database)?;
            insert_child_set(
                transaction,
                "conversation_thread_answers",
                digest,
                &view.answers,
            )?;
            insert_child_set(
                transaction,
                "conversation_thread_cancellations",
                digest,
                &view.cancellations,
            )?;
            for ((answer, cancellation), relation) in &view.relations {
                transaction
                    .execute(
                        "INSERT INTO conversation_thread_relations( \
                             key_digest, answer_fact, cancellation_fact, relation \
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![
                            digest.as_slice(),
                            answer.as_bytes().as_slice(),
                            cancellation.as_bytes().as_slice(),
                            encode_relation(*relation),
                        ],
                    )
                    .map_err(database)?;
            }
            insert_ordered(
                transaction,
                "conversation_thread_ready_answers",
                digest,
                &view.ready_answers,
            )?;
        }
        (ConversationProjectionKey::Message(message_id), ConversationProjection::Message(view)) => {
            if view.content.message_id != *message_id {
                return Err(corrupt());
            }
            let (recipient_present, recipient_installation, recipient_mailbox) =
                encode_address(view.content.recipient);
            let (correlation_present, provider, session, operation) =
                encode_correlation(view.content.correlation.as_ref());
            let (project_present, project) = encode_id_option(view.content.project_id);
            let (account_present, account) = encode_id_option(view.account_id);
            transaction
                .execute(
                    "INSERT INTO conversation_messages( \
                         key_digest, fact_id, authored_at, account_present, account_id, thread_id, sender_installation, sender_mailbox, \
                         recipient_present, recipient_installation, recipient_mailbox, body, purpose, \
                         presentation, correlation_present, provider, session, operation_id, \
                         project_present, project_id, open, rejected \
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
                         ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
                    params![
                        digest.as_slice(),
                        view.fact_id.as_bytes().as_slice(),
                        view.authored_at.as_unix_millis(),
                        account_present,
                        account.as_slice(),
                        view.thread_id.as_bytes().as_slice(),
                        view.content.sender.installation_id().as_bytes().as_slice(),
                        view.content.sender.mailbox_id().as_bytes().as_slice(),
                        recipient_present,
                        recipient_installation.as_slice(),
                        recipient_mailbox.as_slice(),
                        view.content.body.as_str(),
                        encode_purpose(view.content.purpose),
                        encode_presentation(view.content.presentation),
                        correlation_present,
                        provider,
                        session,
                        operation.as_slice(),
                        project_present,
                        project.as_slice(),
                        i64::from(view.open),
                        i64::from(view.rejected),
                    ],
                )
                .map_err(database)?;
            insert_child_set(
                transaction,
                "conversation_message_frontiers",
                digest,
                &view.state_frontier,
            )?;
            insert_child_set(
                transaction,
                "conversation_message_receipts",
                digest,
                &view.peer_received_by,
            )?;
        }
        (ConversationProjectionKey::ActionGroup(_), ConversationProjection::ActionGroup(view)) => {
            let (present, final_answer) = encode_id_option(view.final_answer);
            transaction
                .execute(
                    "INSERT INTO conversation_action_groups( \
                         key_digest, final_answer_present, final_answer \
                     ) VALUES (?1, ?2, ?3)",
                    params![digest.as_slice(), present, final_answer.as_slice()],
                )
                .map_err(database)?;
            insert_ordered(
                transaction,
                "conversation_action_entries",
                digest,
                &view.entries,
            )?;
        }
        (
            ConversationProjectionKey::Activity(_) | ConversationProjectionKey::ActivityRecord(_),
            ConversationProjection::Activity(view),
        ) => {
            if matches!(key, ConversationProjectionKey::ActivityRecord(fact) if *fact != view.fact_id)
            {
                return Err(corrupt());
            }
            let (status, failure_reason) = encode_status(&view.status);
            let (item_present, item) = encode_text_option(view.item.as_ref());
            let completed = encode_completed(view.completed.as_ref())?;
            transaction
                .execute(
                    "INSERT INTO conversation_activities( \
                         key_digest, fact_id, kind, sequence, status, failure_reason, content, truncated, \
                         source_installation, source_mailbox, provider, session, operation, item_present, \
                         item, logical_key, runtime, occurred_at, completed \
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                         ?15, ?16, ?17, ?18, ?19)",
                    params![
                        digest.as_slice(),
                        view.fact_id.as_bytes().as_slice(),
                        encode_activity_kind(view.kind),
                        view.sequence.get().to_be_bytes().as_slice(),
                        status,
                        failure_reason,
                        view.content.as_str(),
                        i64::from(view.truncated),
                        view.source.installation_id().as_bytes().as_slice(),
                        view.source.mailbox_id().as_bytes().as_slice(),
                        view.correlation.provider().as_str(),
                        view.correlation.session().as_str(),
                        view.correlation.operation().as_bytes().as_slice(),
                        item_present,
                        item,
                        view.logical_key.as_str(),
                        view.runtime.as_str(),
                        view.occurred_at.as_unix_millis(),
                        completed,
                    ],
                )
                .map_err(database)?;
        }
        (
            ConversationProjectionKey::ActivityRetention(_),
            ConversationProjection::ActivityRetention(view),
        ) => {
            transaction
                .execute(
                    "INSERT INTO conversation_activity_retentions(key_digest, total_progress) \
                     VALUES (?1, ?2)",
                    params![
                        digest.as_slice(),
                        i64::try_from(view.total_progress).map_err(|_| corrupt())?,
                    ],
                )
                .map_err(database)?;
            insert_ordered(
                transaction,
                "conversation_retained_progress",
                digest,
                &view.retained_progress,
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
        "conversation_thread_answers" => {
            "INSERT INTO conversation_thread_answers(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "conversation_thread_cancellations" => {
            "INSERT INTO conversation_thread_cancellations(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "conversation_message_frontiers" => {
            "INSERT INTO conversation_message_frontiers(key_digest, fact_id) VALUES (?1, ?2)"
        }
        "conversation_message_receipts" => {
            "INSERT INTO conversation_message_receipts(key_digest, fact_id) VALUES (?1, ?2)"
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

fn insert_ordered(
    transaction: &Transaction<'_>,
    table: &str,
    digest: [u8; 32],
    facts: &[FactId],
) -> Result<(), StoreError> {
    let sql = match table {
        "conversation_thread_ready_answers" => {
            "INSERT INTO conversation_thread_ready_answers(key_digest, position, fact_id) \
             VALUES (?1, ?2, ?3)"
        }
        "conversation_action_entries" => {
            "INSERT INTO conversation_action_entries(key_digest, position, fact_id) \
             VALUES (?1, ?2, ?3)"
        }
        "conversation_retained_progress" => {
            "INSERT INTO conversation_retained_progress(key_digest, position, fact_id) \
             VALUES (?1, ?2, ?3)"
        }
        _ => return Err(corrupt()),
    };
    for (position, fact) in facts.iter().enumerate() {
        transaction
            .execute(
                sql,
                params![
                    digest.as_slice(),
                    i64::try_from(position).map_err(|_| corrupt())?,
                    fact.as_bytes().as_slice(),
                ],
            )
            .map_err(database)?;
    }
    Ok(())
}

fn encode_address(value: Option<MailboxAddress>) -> (i64, [u8; 32], [u8; 32]) {
    value.map_or((0, ZERO, ZERO), |address| {
        (
            1,
            *address.installation_id().as_bytes(),
            *address.mailbox_id().as_bytes(),
        )
    })
}

fn encode_correlation(value: Option<&OperationCorrelation>) -> (i64, &str, &str, [u8; 32]) {
    value.map_or((0, "", "", ZERO), |correlation| {
        (
            1,
            correlation.provider().as_str(),
            correlation.session().as_str(),
            *correlation.operation().as_bytes(),
        )
    })
}

fn encode_text_option(value: Option<&ShortText>) -> (i64, &str) {
    value.map_or((0, ""), |value| (1, value.as_str()))
}

fn decode_text(present: i64, value: String) -> Result<Option<ShortText>, StoreError> {
    match (present, value.is_empty()) {
        (0, true) => Ok(None),
        (1, false) => ShortText::new(value).map(Some).map_err(|_| corrupt()),
        _ => Err(corrupt()),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredCompletedItem {
    Command {
        command: String,
        output: Option<String>,
        exit_code: Option<i64>,
        command_truncated: bool,
        output_truncated: bool,
    },
    FileChange {
        changes: Vec<StoredFileChange>,
        changes_truncated: bool,
    },
    Tool {
        name: String,
        name_truncated: bool,
    },
    WebSearch {
        query: String,
        query_truncated: bool,
    },
    Unknown,
}

#[derive(Serialize, Deserialize)]
struct StoredFileChange {
    path: String,
    diff: Option<String>,
    path_truncated: bool,
    diff_truncated: bool,
}

fn encode_completed(value: Option<&CompletedItemPresentation>) -> Result<Vec<u8>, StoreError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let stored = match value {
        CompletedItemPresentation::Command {
            command,
            output,
            exit_code,
            command_truncated,
            output_truncated,
        } => StoredCompletedItem::Command {
            command: command.as_str().to_owned(),
            output: output.as_ref().map(|value| value.as_str().to_owned()),
            exit_code: *exit_code,
            command_truncated: *command_truncated,
            output_truncated: *output_truncated,
        },
        CompletedItemPresentation::FileChange {
            changes,
            changes_truncated,
        } => StoredCompletedItem::FileChange {
            changes: changes
                .as_slice()
                .iter()
                .map(|change| StoredFileChange {
                    path: change.path.as_str().to_owned(),
                    diff: change.diff.as_ref().map(|value| value.as_str().to_owned()),
                    path_truncated: change.path_truncated,
                    diff_truncated: change.diff_truncated,
                })
                .collect(),
            changes_truncated: *changes_truncated,
        },
        CompletedItemPresentation::Tool {
            name,
            name_truncated,
        } => StoredCompletedItem::Tool {
            name: name.as_str().to_owned(),
            name_truncated: *name_truncated,
        },
        CompletedItemPresentation::WebSearch {
            query,
            query_truncated,
        } => StoredCompletedItem::WebSearch {
            query: query.as_str().to_owned(),
            query_truncated: *query_truncated,
        },
        CompletedItemPresentation::Unknown => StoredCompletedItem::Unknown,
    };
    serde_json::to_vec(&stored).map_err(|_| corrupt())
}

fn decode_completed(value: &[u8]) -> Result<Option<CompletedItemPresentation>, StoreError> {
    if value.is_empty() {
        return Ok(None);
    }
    let stored: StoredCompletedItem = serde_json::from_slice(value).map_err(|_| corrupt())?;
    Ok(Some(match stored {
        StoredCompletedItem::Command {
            command,
            output,
            exit_code,
            command_truncated,
            output_truncated,
        } => CompletedItemPresentation::Command {
            command: ContentText::new(command).map_err(|_| corrupt())?,
            output: output
                .map(ContentText::new)
                .transpose()
                .map_err(|_| corrupt())?,
            exit_code,
            command_truncated,
            output_truncated,
        },
        StoredCompletedItem::FileChange {
            changes,
            changes_truncated,
        } => CompletedItemPresentation::FileChange {
            changes: BoundedVec::new(
                changes
                    .into_iter()
                    .map(|change| {
                        Ok(CompletedFileChange {
                            path: ContentText::new(change.path).map_err(|_| corrupt())?,
                            diff: change
                                .diff
                                .map(ContentText::new)
                                .transpose()
                                .map_err(|_| corrupt())?,
                            path_truncated: change.path_truncated,
                            diff_truncated: change.diff_truncated,
                        })
                    })
                    .collect::<Result<Vec<_>, StoreError>>()?,
            )
            .map_err(|_| corrupt())?,
            changes_truncated,
        },
        StoredCompletedItem::Tool {
            name,
            name_truncated,
        } => CompletedItemPresentation::Tool {
            name: ShortText::new(name).map_err(|_| corrupt())?,
            name_truncated,
        },
        StoredCompletedItem::WebSearch {
            query,
            query_truncated,
        } => CompletedItemPresentation::WebSearch {
            query: ContentText::new(query).map_err(|_| corrupt())?,
            query_truncated,
        },
        StoredCompletedItem::Unknown => CompletedItemPresentation::Unknown,
    }))
}

fn encode_id_option<T>(value: Option<T>) -> (i64, [u8; 32])
where
    T: Copy + IntoIdBytes,
{
    value.map_or((0, ZERO), |value| (1, value.id_bytes()))
}

trait IntoIdBytes {
    fn id_bytes(self) -> [u8; 32];
}

impl IntoIdBytes for FactId {
    fn id_bytes(self) -> [u8; 32] {
        *self.as_bytes()
    }
}

impl IntoIdBytes for ProjectId {
    fn id_bytes(self) -> [u8; 32] {
        *self.as_bytes()
    }
}

impl IntoIdBytes for AccountId {
    fn id_bytes(self) -> [u8; 32] {
        *self.as_bytes()
    }
}

fn load_frontiers(
    connection: &Connection,
) -> Result<BTreeMap<ConversationAggregateKey, BTreeSet<FactId>>, StoreError> {
    let rows = load_keys(connection, KeyTable::Aggregate)?;
    let mut result = BTreeMap::new();
    for (digest, parts) in rows {
        let key = decode_aggregate_key(parts)?;
        let facts = load_facts(connection, "conversation_frontiers", digest)?;
        if result.insert(key, facts).is_some() {
            return Err(corrupt());
        }
    }
    Ok(result)
}

fn load_projection_keys(
    connection: &Connection,
) -> Result<(Vec<[u8; 32]>, Vec<ConversationProjectionKey>), StoreError> {
    let rows = load_keys(connection, KeyTable::Projection)?;
    let mut digests = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(rows.len());
    let mut unique = BTreeSet::new();
    for (digest, parts) in rows {
        let key = decode_projection_key(parts)?;
        if !unique.insert(key.clone()) {
            return Err(corrupt());
        }
        digests.push(digest);
        keys.push(key);
    }
    Ok((digests, keys))
}

fn load_keys(
    connection: &Connection,
    table: KeyTable,
) -> Result<Vec<([u8; 32], KeyParts)>, StoreError> {
    let sql = match table {
        KeyTable::Aggregate => {
            "SELECT key_digest, key_kind, key_a, key_b, provider, session, operation_id, \
                 item_present, item, activity_kind, logical_key, runtime \
             FROM conversation_aggregate_keys ORDER BY key_digest"
        }
        KeyTable::Projection => {
            "SELECT key_digest, key_kind, key_a, key_b, provider, session, operation_id, \
                 item_present, item, activity_kind, logical_key, runtime \
             FROM conversation_projection_keys ORDER BY key_digest"
        }
    };
    let count = match table {
        KeyTable::Aggregate => count(connection, "conversation_aggregate_keys")?,
        KeyTable::Projection => count(connection, "conversation_projection_keys")?,
    };
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                KeyParts {
                    kind: row.get(1)?,
                    a: fixed_sql(row.get(2)?)?,
                    b: fixed_sql(row.get(3)?)?,
                    provider: row.get(4)?,
                    session: row.get(5)?,
                    operation: fixed_sql(row.get(6)?)?,
                    item: decode_item_sql(row.get(7)?, row.get(8)?)?,
                    activity_kind: row.get(9)?,
                    logical_key: row.get(10)?,
                    runtime: row.get(11)?,
                },
            ))
        })
        .map_err(database)?;
    let mut result = Vec::with_capacity(capacity(count)?);
    for row in rows {
        let (stored_digest, parts) = row.map_err(database)?;
        let stored_digest = fixed(stored_digest)?;
        if stored_digest != key_digest(table, &parts) {
            return Err(corrupt());
        }
        result.push((stored_digest, parts));
    }
    if result.len() != capacity(count)? {
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

fn decode_item_sql(present: i64, value: String) -> rusqlite::Result<Option<String>> {
    match (present, value.is_empty()) {
        (0, true) => Ok(None),
        (1, false) => Ok(Some(value)),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            "invalid optional item shape".into(),
        )),
    }
}

fn decode_aggregate_key(parts: KeyParts) -> Result<ConversationAggregateKey, StoreError> {
    match parts.kind {
        1 if simple_shape(&parts) => Ok(ConversationAggregateKey::MessageIdentity(
            MessageId::from_bytes(parts.a),
        )),
        2 if simple_shape(&parts) => Ok(ConversationAggregateKey::Thread(ThreadId::from_bytes(
            parts.a,
        ))),
        3 if simple_shape(&parts) => Ok(ConversationAggregateKey::MessageState(
            MessageId::from_bytes(parts.a),
        )),
        4 => Ok(ConversationAggregateKey::Activity(decode_activity_key(
            parts,
        )?)),
        _ => Err(corrupt()),
    }
}

fn decode_projection_key(parts: KeyParts) -> Result<ConversationProjectionKey, StoreError> {
    match parts.kind {
        1 if simple_shape(&parts) => Ok(ConversationProjectionKey::Thread(ThreadId::from_bytes(
            parts.a,
        ))),
        2 if simple_shape(&parts) => Ok(ConversationProjectionKey::Message(MessageId::from_bytes(
            parts.a,
        ))),
        3 if operation_shape(&parts) => Ok(ConversationProjectionKey::ActionGroup(
            decode_operation(&parts.provider, &parts.session, parts.operation)?,
        )),
        4 => Ok(ConversationProjectionKey::Activity(decode_activity_key(
            parts,
        )?)),
        5 if simple_shape(&parts) => Ok(ConversationProjectionKey::ActivityRecord(
            FactId::from_bytes(parts.a),
        )),
        6 if retention_shape(&parts) => Ok(ConversationProjectionKey::ActivityRetention(
            ActivitySessionKey {
                source: MailboxAddress::new(
                    InstallationId::from_bytes(parts.a),
                    MailboxId::from_bytes(parts.b),
                ),
                provider: ProviderId::new(parts.provider).map_err(|_| corrupt())?,
                session: ProviderSessionId::new(parts.session).map_err(|_| corrupt())?,
            },
        )),
        _ => Err(corrupt()),
    }
}

fn simple_shape(parts: &KeyParts) -> bool {
    parts.b == ZERO
        && parts.provider.is_empty()
        && parts.session.is_empty()
        && parts.operation == ZERO
        && parts.item.is_none()
        && parts.activity_kind == 0
        && parts.logical_key.is_empty()
        && parts.runtime.is_empty()
}

fn operation_shape(parts: &KeyParts) -> bool {
    parts.a == ZERO
        && parts.b == ZERO
        && !parts.provider.is_empty()
        && !parts.session.is_empty()
        && parts.item.is_none()
        && parts.activity_kind == 0
        && parts.logical_key.is_empty()
        && parts.runtime.is_empty()
}

fn retention_shape(parts: &KeyParts) -> bool {
    !parts.provider.is_empty()
        && !parts.session.is_empty()
        && parts.operation == ZERO
        && parts.item.is_none()
        && parts.activity_kind == 0
        && parts.logical_key.is_empty()
        && parts.runtime.is_empty()
}

fn decode_activity_key(parts: KeyParts) -> Result<ActivityKey, StoreError> {
    if parts.provider.is_empty()
        || parts.session.is_empty()
        || parts.activity_kind == 0
        || parts.logical_key.is_empty()
        || parts.runtime.is_empty()
    {
        return Err(corrupt());
    }
    Ok(ActivityKey {
        source: MailboxAddress::new(
            InstallationId::from_bytes(parts.a),
            MailboxId::from_bytes(parts.b),
        ),
        correlation: decode_operation(&parts.provider, &parts.session, parts.operation)?,
        item: parts
            .item
            .map(ShortText::new)
            .transpose()
            .map_err(|_| corrupt())?,
        kind: decode_activity_kind(parts.activity_kind).ok_or_else(corrupt)?,
        logical_key: ShortText::new(parts.logical_key).map_err(|_| corrupt())?,
        runtime: ShortText::new(parts.runtime).map_err(|_| corrupt())?,
    })
}

fn decode_operation(
    provider: &str,
    session: &str,
    operation: [u8; 32],
) -> Result<OperationCorrelation, StoreError> {
    Ok(OperationCorrelation::new(
        ProviderId::new(provider).map_err(|_| corrupt())?,
        ProviderSessionId::new(session).map_err(|_| corrupt())?,
        OperationId::from_bytes(operation),
    ))
}

fn load_facts(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
) -> Result<BTreeSet<FactId>, StoreError> {
    let sql = match table {
        "conversation_frontiers" => {
            "SELECT fact_id FROM conversation_frontiers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "conversation_support" => {
            "SELECT fact_id FROM conversation_support WHERE key_digest = ?1 ORDER BY fact_id"
        }
        _ => return Err(corrupt()),
    };
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| row.get::<_, Vec<u8>>(0))
        .map_err(database)?;
    rows.map(|row| {
        row.map_err(database)
            .and_then(fixed)
            .map(FactId::from_bytes)
    })
    .collect()
}

fn load_projection(
    connection: &Connection,
    digest: [u8; 32],
    key: &ConversationProjectionKey,
) -> Result<ConversationProjection, StoreError> {
    match key {
        ConversationProjectionKey::Thread(_) => load_thread(connection, digest),
        ConversationProjectionKey::Message(message) => load_message(connection, digest, *message),
        ConversationProjectionKey::ActionGroup(_) => load_action_group(connection, digest),
        ConversationProjectionKey::Activity(_) => load_activity(connection, digest),
        ConversationProjectionKey::ActivityRecord(expected) => {
            let projection = load_activity(connection, digest)?;
            if !matches!(&projection, ConversationProjection::Activity(view) if view.fact_id == *expected)
            {
                return Err(corrupt());
            }
            Ok(projection)
        }
        ConversationProjectionKey::ActivityRetention(_) => load_retention(connection, digest),
    }
}

fn load_thread(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<ConversationProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT root_fact, root_message, cancelled FROM conversation_threads \
             WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let answers = load_child_set(connection, "conversation_thread_answers", digest)?;
    let cancellations = load_child_set(connection, "conversation_thread_cancellations", digest)?;
    let relations = load_relations(connection, digest)?;
    let ready_answers = load_ordered(connection, "conversation_thread_ready_answers", digest)?;
    let cancelled = decode_bool(row.2)?;
    let ready_set = ready_answers.iter().copied().collect::<BTreeSet<_>>();
    let expected_relations = answers
        .len()
        .checked_mul(cancellations.len())
        .ok_or_else(corrupt)?;
    if cancelled == cancellations.is_empty()
        || ready_set != answers
        || relations.len() != expected_relations
        || relations.keys().any(|(answer, cancellation)| {
            !answers.contains(answer) || !cancellations.contains(cancellation)
        })
    {
        return Err(corrupt());
    }
    Ok(ConversationProjection::Thread(Box::new(ThreadView {
        root_fact: FactId::from_bytes(fixed(row.0)?),
        root_message: MessageId::from_bytes(fixed(row.1)?),
        answers,
        cancellations,
        relations,
        ready_answers,
        cancelled,
    })))
}

#[allow(clippy::type_complexity)]
fn load_relations(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<BTreeMap<(FactId, FactId), CausalRelation>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT answer_fact, cancellation_fact, relation \
             FROM conversation_thread_relations WHERE key_digest = ?1 \
             ORDER BY answer_fact, cancellation_fact",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(database)?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (answer, cancellation, relation) = row.map_err(database)?;
        let key = (
            FactId::from_bytes(fixed(answer)?),
            FactId::from_bytes(fixed(cancellation)?),
        );
        if result
            .insert(key, decode_relation(relation).ok_or_else(corrupt)?)
            .is_some()
        {
            return Err(corrupt());
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_lines)]
fn load_message(
    connection: &Connection,
    digest: [u8; 32],
    message_id: MessageId,
) -> Result<ConversationProjection, StoreError> {
    type Row = (
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        i64,
        Vec<u8>,
        Vec<u8>,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        Vec<u8>,
        i64,
        Vec<u8>,
        i64,
        i64,
        i64,
        Vec<u8>,
        i64,
    );
    let row: Row = connection
        .query_row(
            "SELECT fact_id, thread_id, sender_installation, sender_mailbox, recipient_present, \
                 recipient_installation, recipient_mailbox, body, purpose, presentation, \
                 correlation_present, provider, session, operation_id, project_present, project_id, \
                 open, rejected, account_present, account_id, authored_at FROM conversation_messages WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                    row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?,
                    row.get(15)?, row.get(16)?, row.get(17)?, row.get(18)?, row.get(19)?,
                    row.get(20)?))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let recipient = decode_address(row.4, fixed(row.5)?, fixed(row.6)?)?;
    let correlation = decode_correlation(row.10, &row.11, &row.12, fixed(row.13)?)?;
    let project_id = decode_project(row.14, fixed(row.15)?)?;
    let open = decode_bool(row.16)?;
    let rejected = decode_bool(row.17)?;
    let account_id = decode_account(row.18, fixed(row.19)?)?;
    if open && rejected {
        return Err(corrupt());
    }
    let state_frontier = load_child_set(connection, "conversation_message_frontiers", digest)?;
    let peer_received_by = load_child_set(connection, "conversation_message_receipts", digest)?;
    Ok(ConversationProjection::Message(Box::new(MessageView {
        fact_id: FactId::from_bytes(fixed(row.0)?),
        authored_at: Timestamp::from_unix_millis(row.20),
        account_id,
        thread_id: ThreadId::from_bytes(fixed(row.1)?),
        content: MessageContent {
            message_id,
            sender: MailboxAddress::new(
                InstallationId::from_bytes(fixed(row.2)?),
                MailboxId::from_bytes(fixed(row.3)?),
            ),
            recipient,
            body: ContentText::new(row.7).map_err(|_| corrupt())?,
            purpose: decode_purpose(row.8).ok_or_else(corrupt)?,
            presentation: decode_presentation(row.9).ok_or_else(corrupt)?,
            correlation,
            project_id,
        },
        open,
        rejected,
        state_frontier,
        peer_received_by,
    })))
}

fn load_action_group(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<ConversationProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT final_answer_present, final_answer FROM conversation_action_groups \
             WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let final_answer = decode_fact(row.0, fixed(row.1)?)?;
    let entries = load_ordered(connection, "conversation_action_entries", digest)?;
    if final_answer.is_some_and(|value| !entries.contains(&value)) {
        return Err(corrupt());
    }
    Ok(ConversationProjection::ActionGroup(ActionGroupView {
        entries,
        final_answer,
    }))
}

fn load_activity(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<ConversationProjection, StoreError> {
    let row = connection
        .query_row(
            "SELECT fact_id, kind, sequence, status, failure_reason, content, truncated, \
                    source_installation, source_mailbox, provider, session, operation, item_present, \
                    item, logical_key, runtime, occurred_at, completed \
             FROM conversation_activities WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
                    row.get::<_, Vec<u8>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Vec<u8>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, Vec<u8>>(17)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let sequence = u64::from_be_bytes(row.2.try_into().map_err(|_| corrupt())?);
    Ok(ConversationProjection::Activity(Box::new(ActivityView {
        fact_id: FactId::from_bytes(fixed(row.0)?),
        source: MailboxAddress::new(
            InstallationId::from_bytes(fixed(row.7)?),
            MailboxId::from_bytes(fixed(row.8)?),
        ),
        correlation: OperationCorrelation::new(
            ProviderId::new(row.9).map_err(|_| corrupt())?,
            ProviderSessionId::new(row.10).map_err(|_| corrupt())?,
            OperationId::from_bytes(fixed(row.11)?),
        ),
        item: decode_text(row.12, row.13)?,
        kind: decode_activity_kind(row.1).ok_or_else(corrupt)?,
        sequence: NonZeroU64::new(sequence).ok_or_else(corrupt)?,
        logical_key: ShortText::new(row.14).map_err(|_| corrupt())?,
        runtime: ShortText::new(row.15).map_err(|_| corrupt())?,
        occurred_at: Timestamp::from_unix_millis(row.16),
        status: decode_status(row.3, row.4)?,
        content: ContentText::new(row.5).map_err(|_| corrupt())?,
        truncated: decode_bool(row.6)?,
        completed: decode_completed(&row.17)?,
    })))
}

fn load_retention(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<ConversationProjection, StoreError> {
    let total: i64 = connection
        .query_row(
            "SELECT total_progress FROM conversation_activity_retentions WHERE key_digest = ?1",
            [digest.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(corrupt)?;
    let total_progress = usize::try_from(total).map_err(|_| corrupt())?;
    let retained_progress = load_ordered(connection, "conversation_retained_progress", digest)?;
    if retained_progress.len() > 200 || retained_progress.len() > total_progress {
        return Err(corrupt());
    }
    Ok(ConversationProjection::ActivityRetention(
        ActivityRetentionView {
            retained_progress,
            total_progress,
        },
    ))
}

fn load_child_set(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
) -> Result<BTreeSet<FactId>, StoreError> {
    let sql = match table {
        "conversation_thread_answers" => {
            "SELECT fact_id FROM conversation_thread_answers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "conversation_thread_cancellations" => {
            "SELECT fact_id FROM conversation_thread_cancellations WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "conversation_message_frontiers" => {
            "SELECT fact_id FROM conversation_message_frontiers WHERE key_digest = ?1 ORDER BY fact_id"
        }
        "conversation_message_receipts" => {
            "SELECT fact_id FROM conversation_message_receipts WHERE key_digest = ?1 ORDER BY fact_id"
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

fn load_ordered(
    connection: &Connection,
    table: &str,
    digest: [u8; 32],
) -> Result<Vec<FactId>, StoreError> {
    let sql = match table {
        "conversation_thread_ready_answers" => {
            "SELECT position, fact_id FROM conversation_thread_ready_answers WHERE key_digest = ?1 ORDER BY position"
        }
        "conversation_action_entries" => {
            "SELECT position, fact_id FROM conversation_action_entries WHERE key_digest = ?1 ORDER BY position"
        }
        "conversation_retained_progress" => {
            "SELECT position, fact_id FROM conversation_retained_progress WHERE key_digest = ?1 ORDER BY position"
        }
        _ => return Err(corrupt()),
    };
    let mut statement = connection.prepare(sql).map_err(database)?;
    let rows = statement
        .query_map([digest.as_slice()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(database)?;
    let mut result = Vec::new();
    for (expected, row) in rows.enumerate() {
        let (position, fact) = row.map_err(database)?;
        if position != i64::try_from(expected).map_err(|_| corrupt())? {
            return Err(corrupt());
        }
        result.push(FactId::from_bytes(fixed(fact)?));
    }
    Ok(result)
}

const fn encode_activity_kind(value: ActivityKind) -> i64 {
    match value {
        ActivityKind::Status => 1,
        ActivityKind::AgentTurn => 6,
        ActivityKind::Progress => 2,
        ActivityKind::Plan => 3,
        ActivityKind::Diff => 4,
        ActivityKind::CompletedItem => 5,
    }
}

const fn decode_activity_kind(value: i64) -> Option<ActivityKind> {
    match value {
        1 => Some(ActivityKind::Status),
        6 => Some(ActivityKind::AgentTurn),
        2 => Some(ActivityKind::Progress),
        3 => Some(ActivityKind::Plan),
        4 => Some(ActivityKind::Diff),
        5 => Some(ActivityKind::CompletedItem),
        _ => None,
    }
}

const fn encode_purpose(value: MessagePurpose) -> i64 {
    match value {
        MessagePurpose::Question => 1,
        MessagePurpose::Asynchronous => 2,
        MessagePurpose::ProjectOutput => 3,
    }
}

const fn decode_purpose(value: i64) -> Option<MessagePurpose> {
    match value {
        1 => Some(MessagePurpose::Question),
        2 => Some(MessagePurpose::Asynchronous),
        3 => Some(MessagePurpose::ProjectOutput),
        _ => None,
    }
}

const fn encode_presentation(value: PresentationKind) -> i64 {
    match value {
        PresentationKind::Message => 1,
        PresentationKind::FinalAnswer => 2,
        PresentationKind::Status => 3,
    }
}

const fn decode_presentation(value: i64) -> Option<PresentationKind> {
    match value {
        1 => Some(PresentationKind::Message),
        2 => Some(PresentationKind::FinalAnswer),
        3 => Some(PresentationKind::Status),
        _ => None,
    }
}

const fn encode_relation(value: CausalRelation) -> i64 {
    match value {
        CausalRelation::Before => 1,
        CausalRelation::After => 2,
        CausalRelation::Concurrent => 3,
    }
}

const fn decode_relation(value: i64) -> Option<CausalRelation> {
    match value {
        1 => Some(CausalRelation::Before),
        2 => Some(CausalRelation::After),
        3 => Some(CausalRelation::Concurrent),
        _ => None,
    }
}

fn encode_status(value: &ActivityStatus) -> (i64, &str) {
    match value {
        ActivityStatus::Snapshot => (1, ""),
        ActivityStatus::Running => (2, ""),
        ActivityStatus::Succeeded => (3, ""),
        ActivityStatus::Failed(reason) => (4, reason.as_str()),
        ActivityStatus::Interrupted => (5, ""),
    }
}

fn decode_status(code: i64, reason: String) -> Result<ActivityStatus, StoreError> {
    match (code, reason.is_empty()) {
        (1, true) => Ok(ActivityStatus::Snapshot),
        (2, true) => Ok(ActivityStatus::Running),
        (3, true) => Ok(ActivityStatus::Succeeded),
        (4, false) => Ok(ActivityStatus::Failed(
            ErrorCode::new(reason).map_err(|_| corrupt())?,
        )),
        (5, true) => Ok(ActivityStatus::Interrupted),
        _ => Err(corrupt()),
    }
}

fn decode_address(
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

fn decode_correlation(
    present: i64,
    provider: &str,
    session: &str,
    operation: [u8; 32],
) -> Result<Option<OperationCorrelation>, StoreError> {
    match (
        present,
        provider.is_empty(),
        session.is_empty(),
        operation == ZERO,
    ) {
        (0, true, true, true) => Ok(None),
        (1, false, false, _) => Ok(Some(decode_operation(provider, session, operation)?)),
        _ => Err(corrupt()),
    }
}

fn decode_project(present: i64, project: [u8; 32]) -> Result<Option<ProjectId>, StoreError> {
    match (present, project == ZERO) {
        (0, true) => Ok(None),
        (1, _) => Ok(Some(ProjectId::from_bytes(project))),
        _ => Err(corrupt()),
    }
}

fn decode_account(present: i64, account: [u8; 32]) -> Result<Option<AccountId>, StoreError> {
    match (present, account == ZERO) {
        (0, true) => Ok(None),
        (1, _) => Ok(Some(AccountId::from_bytes(account))),
        _ => Err(corrupt()),
    }
}

fn decode_fact(present: i64, fact: [u8; 32]) -> Result<Option<FactId>, StoreError> {
    match (present, fact == ZERO) {
        (0, true) => Ok(None),
        (1, _) => Ok(Some(FactId::from_bytes(fact))),
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
        let aggregate_key_count = count(connection, "conversation_aggregate_keys")?;
        let frontier_count = count(connection, "conversation_frontiers")?;
        let projection_key_count = count(connection, "conversation_projection_keys")?;
        let support_count = count(connection, "conversation_support")?;
        let projection_count = [
            "conversation_threads",
            "conversation_messages",
            "conversation_action_groups",
            "conversation_activities",
            "conversation_activity_retentions",
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
        Ok(Self {
            aggregate_key_count,
            frontier_count,
            projection_key_count,
            projection_count,
            support_count,
            row_count,
        })
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
            if !(0..=MAXIMUM_CONVERSATION_ROWS).contains(&value) {
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
                 support_count, row_count, row_digest FROM conversation_state WHERE singleton = 1",
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
    let sql = match table {
        "conversation_aggregate_keys" => "SELECT count(*) FROM conversation_aggregate_keys",
        "conversation_frontiers" => "SELECT count(*) FROM conversation_frontiers",
        "conversation_projection_keys" => "SELECT count(*) FROM conversation_projection_keys",
        "conversation_support" => "SELECT count(*) FROM conversation_support",
        "conversation_threads" => "SELECT count(*) FROM conversation_threads",
        "conversation_thread_answers" => "SELECT count(*) FROM conversation_thread_answers",
        "conversation_thread_cancellations" => {
            "SELECT count(*) FROM conversation_thread_cancellations"
        }
        "conversation_thread_relations" => "SELECT count(*) FROM conversation_thread_relations",
        "conversation_thread_ready_answers" => {
            "SELECT count(*) FROM conversation_thread_ready_answers"
        }
        "conversation_messages" => "SELECT count(*) FROM conversation_messages",
        "conversation_message_frontiers" => "SELECT count(*) FROM conversation_message_frontiers",
        "conversation_message_receipts" => "SELECT count(*) FROM conversation_message_receipts",
        "conversation_action_groups" => "SELECT count(*) FROM conversation_action_groups",
        "conversation_action_entries" => "SELECT count(*) FROM conversation_action_entries",
        "conversation_activities" => "SELECT count(*) FROM conversation_activities",
        "conversation_activity_retentions" => {
            "SELECT count(*) FROM conversation_activity_retentions"
        }
        "conversation_retained_progress" => "SELECT count(*) FROM conversation_retained_progress",
        _ => return Err(corrupt()),
    };
    connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(database)
}

fn capacity(count: i64) -> Result<usize, StoreError> {
    if !(0..=MAXIMUM_CONVERSATION_ROWS).contains(&count) {
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

fn row_digest(connection: &Connection) -> Result<[u8; 32], StoreError> {
    const QUERIES: [&str; 17] = [
        "SELECT quote(key_digest)||char(31)||quote(key_kind)||char(31)||quote(key_a)||char(31)||quote(key_b)||char(31)||quote(provider)||char(31)||quote(session)||char(31)||quote(operation_id)||char(31)||quote(item_present)||char(31)||quote(item)||char(31)||quote(activity_kind)||char(31)||quote(logical_key)||char(31)||quote(runtime) FROM conversation_aggregate_keys ORDER BY key_digest",
        "SELECT quote(key_digest)||char(31)||quote(fact_id) FROM conversation_frontiers ORDER BY key_digest, fact_id",
        "SELECT quote(key_digest)||char(31)||quote(key_kind)||char(31)||quote(key_a)||char(31)||quote(key_b)||char(31)||quote(provider)||char(31)||quote(session)||char(31)||quote(operation_id)||char(31)||quote(item_present)||char(31)||quote(item)||char(31)||quote(activity_kind)||char(31)||quote(logical_key)||char(31)||quote(runtime) FROM conversation_projection_keys ORDER BY key_digest",
        "SELECT quote(key_digest)||char(31)||quote(fact_id) FROM conversation_support ORDER BY key_digest, fact_id",
        "SELECT quote(key_digest)||char(31)||quote(root_fact)||char(31)||quote(root_message)||char(31)||quote(cancelled) FROM conversation_threads ORDER BY key_digest",
        "SELECT quote(key_digest)||char(31)||quote(fact_id) FROM conversation_thread_answers ORDER BY key_digest, fact_id",
        "SELECT quote(key_digest)||char(31)||quote(fact_id) FROM conversation_thread_cancellations ORDER BY key_digest, fact_id",
        "SELECT quote(key_digest)||char(31)||quote(answer_fact)||char(31)||quote(cancellation_fact)||char(31)||quote(relation) FROM conversation_thread_relations ORDER BY key_digest, answer_fact, cancellation_fact",
        "SELECT quote(key_digest)||char(31)||quote(position)||char(31)||quote(fact_id) FROM conversation_thread_ready_answers ORDER BY key_digest, position",
        "SELECT quote(key_digest)||char(31)||quote(fact_id)||char(31)||quote(thread_id)||char(31)||quote(sender_installation)||char(31)||quote(sender_mailbox)||char(31)||quote(recipient_present)||char(31)||quote(recipient_installation)||char(31)||quote(recipient_mailbox)||char(31)||quote(body)||char(31)||quote(purpose)||char(31)||quote(presentation)||char(31)||quote(correlation_present)||char(31)||quote(provider)||char(31)||quote(session)||char(31)||quote(operation_id)||char(31)||quote(project_present)||char(31)||quote(project_id)||char(31)||quote(open)||char(31)||quote(rejected) FROM conversation_messages ORDER BY key_digest",
        "SELECT quote(key_digest)||char(31)||quote(fact_id) FROM conversation_message_frontiers ORDER BY key_digest, fact_id",
        "SELECT quote(key_digest)||char(31)||quote(fact_id) FROM conversation_message_receipts ORDER BY key_digest, fact_id",
        "SELECT quote(key_digest)||char(31)||quote(final_answer_present)||char(31)||quote(final_answer) FROM conversation_action_groups ORDER BY key_digest",
        "SELECT quote(key_digest)||char(31)||quote(position)||char(31)||quote(fact_id) FROM conversation_action_entries ORDER BY key_digest, position",
        "SELECT quote(key_digest)||char(31)||quote(fact_id)||char(31)||quote(kind)||char(31)||quote(sequence)||char(31)||quote(status)||char(31)||quote(failure_reason)||char(31)||quote(content)||char(31)||quote(truncated) FROM conversation_activities ORDER BY key_digest",
        "SELECT quote(key_digest)||char(31)||quote(total_progress) FROM conversation_activity_retentions ORDER BY key_digest",
        "SELECT quote(key_digest)||char(31)||quote(position)||char(31)||quote(fact_id) FROM conversation_retained_progress ORDER BY key_digest, position",
    ];
    let mut digest = Sha256::new();
    for (table, query) in TABLES.into_iter().zip(QUERIES) {
        put_text(&mut digest, table);
        let mut statement = connection.prepare(query).map_err(database)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(database)?;
        for row in rows {
            put_text(&mut digest, &row.map_err(database)?);
        }
    }
    Ok(digest.finalize().into())
}

fn validate_snapshot(
    snapshot: &ConversationProjectionSnapshot,
    counts: Counts,
) -> Result<(), StoreError> {
    if snapshot.projections().keys().ne(snapshot.support().keys())
        || counts.aggregate_key_count != length(std::iter::once(snapshot.frontiers().len()))?
        || counts.frontier_count != length(snapshot.frontiers().values().map(BTreeSet::len))?
        || counts.projection_key_count != length(std::iter::once(snapshot.projections().len()))?
        || counts.projection_count != length(std::iter::once(snapshot.projections().len()))?
        || counts.support_count != length(snapshot.support().values().map(BTreeSet::len))?
    {
        return Err(corrupt());
    }
    Ok(())
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
    fn every_conversation_projection_variant_round_trips_relationally() {
        let expected = exhaustive_snapshot();
        let connection = fixture_connection(&expected);
        assert_eq!(load(&connection).expect("conversation rows load"), expected);
    }

    #[test]
    fn conversation_scalar_codecs_are_closed_and_full_width() {
        for value in [
            ActivityKind::Status,
            ActivityKind::AgentTurn,
            ActivityKind::Progress,
            ActivityKind::Plan,
            ActivityKind::Diff,
            ActivityKind::CompletedItem,
        ] {
            assert_eq!(
                decode_activity_kind(encode_activity_kind(value)),
                Some(value)
            );
        }
        assert_eq!(decode_activity_kind(0), None);
        assert_eq!(decode_activity_kind(7), None);
        for value in [
            MessagePurpose::Question,
            MessagePurpose::Asynchronous,
            MessagePurpose::ProjectOutput,
        ] {
            assert_eq!(decode_purpose(encode_purpose(value)), Some(value));
        }
        assert_eq!(decode_purpose(0), None);
        for value in [
            PresentationKind::Message,
            PresentationKind::FinalAnswer,
            PresentationKind::Status,
        ] {
            assert_eq!(decode_presentation(encode_presentation(value)), Some(value));
        }
        assert_eq!(decode_presentation(4), None);
        for value in [
            CausalRelation::Before,
            CausalRelation::After,
            CausalRelation::Concurrent,
        ] {
            assert_eq!(decode_relation(encode_relation(value)), Some(value));
        }
        assert_eq!(decode_relation(4), None);
        assert_eq!(
            decode_status(1, String::new()),
            Ok(ActivityStatus::Snapshot)
        );
        assert!(decode_status(4, String::new()).is_err());
        assert!(decode_status(1, "reason".to_owned()).is_err());
        assert_eq!(
            NonZeroU64::new(u64::from_be_bytes(u64::MAX.to_be_bytes())),
            NonZeroU64::new(u64::MAX)
        );
    }

    #[test]
    fn every_conversation_table_family_fails_closed_on_valid_looking_corruption() {
        let expected = exhaustive_snapshot();
        for mutation in [
            "UPDATE conversation_state SET row_count = row_count + 1",
            "UPDATE conversation_aggregate_keys SET runtime = 'changed' WHERE key_kind = 4",
            "UPDATE conversation_frontiers SET fact_id = zeroblob(32)",
            "UPDATE conversation_projection_keys SET logical_key = 'changed' WHERE key_kind = 4",
            "UPDATE conversation_support SET fact_id = zeroblob(32)",
            "UPDATE conversation_threads SET root_message = zeroblob(32)",
            "UPDATE conversation_thread_answers SET fact_id = zeroblob(32) WHERE fact_id = (SELECT fact_id FROM conversation_thread_answers LIMIT 1)",
            "UPDATE conversation_thread_cancellations SET fact_id = zeroblob(32)",
            "UPDATE conversation_thread_relations SET relation = CASE relation WHEN 1 THEN 2 ELSE 1 END",
            "UPDATE conversation_thread_ready_answers SET position = position + 10",
            "UPDATE conversation_messages SET body = 'changed'",
            "UPDATE conversation_message_frontiers SET fact_id = zeroblob(32)",
            "UPDATE conversation_message_receipts SET fact_id = zeroblob(32)",
            "UPDATE conversation_action_groups SET final_answer_present = 0",
            "UPDATE conversation_action_entries SET position = position + 10",
            "UPDATE conversation_activities SET content = 'changed'",
            "UPDATE conversation_activities SET kind = CASE kind WHEN 1 THEN 2 ELSE 1 END",
            "UPDATE conversation_activity_retentions SET total_progress = total_progress + 1",
            "UPDATE conversation_retained_progress SET position = position + 10",
        ] {
            let connection = fixture_connection(&expected);
            if mutation.contains("zeroblob(32)") {
                connection
                    .execute(
                        "INSERT OR IGNORE INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                         VALUES (zeroblob(32), X'00', 1, 1)",
                        [],
                    )
                    .expect("replacement support inserts");
            }
            connection
                .execute(mutation, [])
                .expect("constraint-valid mutation applies");
            assert_eq!(
                load(&connection)
                    .expect_err("changed conversation rows reject")
                    .class(),
                StoreErrorClass::RebuildableStateCorrupt,
                "mutation unexpectedly loaded: {mutation}",
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    fn exhaustive_snapshot() -> ConversationProjectionSnapshot {
        let source = MailboxAddress::new(
            InstallationId::from_bytes([0x41; 32]),
            MailboxId::from_bytes([0x42; 32]),
        );
        let recipient = MailboxAddress::new(
            InstallationId::from_bytes([0x43; 32]),
            MailboxId::from_bytes([0x44; 32]),
        );
        let correlation = operation("provider", "session", 0x45);
        let activity_key = ActivityKey {
            source,
            correlation: correlation.clone(),
            item: Some(ShortText::new("item").expect("item validates")),
            kind: ActivityKind::Progress,
            logical_key: ShortText::new("logical").expect("logical key validates"),
            runtime: ShortText::new("runtime").expect("runtime validates"),
        };
        let retention_key = ActivitySessionKey {
            source,
            provider: ProviderId::new("provider").expect("provider validates"),
            session: ProviderSessionId::new("session").expect("session validates"),
        };
        let thread = ThreadId::from_bytes([0x46; 32]);
        let message = MessageId::from_bytes([0x47; 32]);
        let keys = [
            ConversationProjectionKey::Thread(thread),
            ConversationProjectionKey::Message(message),
            ConversationProjectionKey::ActionGroup(correlation.clone()),
            ConversationProjectionKey::Activity(activity_key.clone()),
            ConversationProjectionKey::ActivityRecord(id(13)),
            ConversationProjectionKey::ActivityRetention(retention_key.clone()),
        ];
        let projections = BTreeMap::from([
            (
                keys[0].clone(),
                ConversationProjection::Thread(Box::new(ThreadView {
                    root_fact: id(1),
                    root_message: MessageId::from_bytes([0x48; 32]),
                    answers: BTreeSet::from([id(2), id(3)]),
                    cancellations: BTreeSet::from([id(4)]),
                    relations: BTreeMap::from([
                        ((id(2), id(4)), CausalRelation::Before),
                        ((id(3), id(4)), CausalRelation::Concurrent),
                    ]),
                    ready_answers: vec![id(3), id(2)],
                    cancelled: true,
                })),
            ),
            (
                keys[1].clone(),
                ConversationProjection::Message(Box::new(MessageView {
                    fact_id: id(5),
                    authored_at: Timestamp::from_unix_millis(123),
                    account_id: Some(AccountId::from_bytes([0x4a; 32])),
                    thread_id: thread,
                    content: MessageContent {
                        message_id: message,
                        sender: source,
                        recipient: Some(recipient),
                        body: ContentText::new("message body").expect("body validates"),
                        purpose: MessagePurpose::Question,
                        presentation: PresentationKind::FinalAnswer,
                        correlation: Some(correlation.clone()),
                        project_id: Some(ProjectId::from_bytes([0x49; 32])),
                    },
                    open: false,
                    rejected: true,
                    state_frontier: BTreeSet::from([id(6)]),
                    peer_received_by: BTreeSet::from([id(7)]),
                })),
            ),
            (
                keys[2].clone(),
                ConversationProjection::ActionGroup(ActionGroupView {
                    entries: vec![id(8), id(9)],
                    final_answer: Some(id(9)),
                }),
            ),
            (
                keys[3].clone(),
                ConversationProjection::Activity(Box::new(ActivityView {
                    fact_id: id(10),
                    source,
                    correlation: correlation.clone(),
                    item: Some(ShortText::new("progress-item").expect("item validates")),
                    kind: ActivityKind::Progress,
                    sequence: NonZeroU64::new(u64::MAX).expect("sequence is nonzero"),
                    logical_key: ShortText::new("progress").expect("key validates"),
                    runtime: ShortText::new("runtime").expect("runtime validates"),
                    occurred_at: Timestamp::from_unix_millis(124),
                    status: ActivityStatus::Failed(
                        ErrorCode::new("failed").expect("reason validates"),
                    ),
                    content: ContentText::new("progress").expect("content validates"),
                    truncated: true,
                    completed: None,
                })),
            ),
            (
                keys[4].clone(),
                ConversationProjection::Activity(Box::new(ActivityView {
                    fact_id: id(13),
                    source,
                    correlation: correlation.clone(),
                    item: Some(ShortText::new("completed-item").expect("item validates")),
                    kind: ActivityKind::CompletedItem,
                    sequence: NonZeroU64::MIN,
                    logical_key: ShortText::new("completed").expect("key validates"),
                    runtime: ShortText::new("runtime").expect("runtime validates"),
                    occurred_at: Timestamp::from_unix_millis(125),
                    status: ActivityStatus::Succeeded,
                    content: ContentText::new("complete").expect("content validates"),
                    truncated: false,
                    completed: Some(CompletedItemPresentation::Command {
                        command: ContentText::new("printf one\nprintf two")
                            .expect("command validates"),
                        output: Some(ContentText::new("one\ntwo").expect("output validates")),
                        exit_code: Some(0),
                        command_truncated: false,
                        output_truncated: true,
                    }),
                })),
            ),
            (
                keys[5].clone(),
                ConversationProjection::ActivityRetention(ActivityRetentionView {
                    retained_progress: vec![id(11), id(12)],
                    total_progress: 202,
                }),
            ),
        ]);
        let frontiers = BTreeMap::from([
            (
                ConversationAggregateKey::MessageIdentity(message),
                BTreeSet::from([id(5)]),
            ),
            (
                ConversationAggregateKey::Thread(thread),
                BTreeSet::from([id(4)]),
            ),
            (
                ConversationAggregateKey::MessageState(message),
                BTreeSet::from([id(6)]),
            ),
            (
                ConversationAggregateKey::Activity(activity_key),
                BTreeSet::from([id(10)]),
            ),
        ]);
        let support = keys
            .into_iter()
            .enumerate()
            .map(|(index, key)| {
                (
                    key,
                    BTreeSet::from([id(u8::try_from(index + 20).expect("fixture id fits"))]),
                )
            })
            .collect();
        ConversationProjectionSnapshot::new(frontiers, projections, support)
    }

    fn operation(provider: &str, session: &str, id_byte: u8) -> OperationCorrelation {
        OperationCorrelation::new(
            ProviderId::new(provider).expect("provider validates"),
            ProviderSessionId::new(session).expect("session validates"),
            OperationId::from_bytes([id_byte; 32]),
        )
    }

    fn fixture_connection(expected: &ConversationProjectionSnapshot) -> Connection {
        let mut connection = Connection::open_in_memory().expect("memory database opens");
        connection
            .execute_batch(super::super::SCHEMA)
            .expect("schema creates");
        for value in 0_u8..=30 {
            connection
                .execute(
                    "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                     VALUES (?1, ?2, 1, 1)",
                    params![id(value).as_bytes().as_slice(), vec![value]],
                )
                .expect("canonical support inserts");
        }
        let transaction = connection.transaction().expect("transaction starts");
        insert(&transaction, expected).expect("conversation rows insert");
        transaction.commit().expect("conversation rows commit");
        connection
    }

    fn id(value: u8) -> FactId {
        FactId::from_bytes([value; 32])
    }
}
