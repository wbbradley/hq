//! Durable managed-runtime lease, delivery, and persistence checkpoints.

use std::num::NonZeroU64;

use hq_domain::{
    AgentId, AssignmentId, CommandDigest, ContentText, DispatchId, MessageId, OperationId,
    ProjectId, ProviderId, ProviderSessionId, ThreadId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    HarnessLeaseOutcome, HarnessSessionOperation, HarnessSessionOperationKind,
    HarnessSessionOperationState, MAX_HARNESS_STATE_QUERY_ITEMS, StoreError, StoreErrorClass,
    StoredHarnessDelivery, StoredHarnessDeliveryState, StoredHarnessEventCheckpoint,
    StoredHarnessLease, StoredHarnessProjectDelivery, StoredHarnessReadySession,
    StoredHarnessStateMutation, StoredHarnessStateSnapshot,
};

pub(super) fn apply(
    connection: &mut Connection,
    mutation: StoredHarnessStateMutation,
) -> Result<HarnessLeaseOutcome, StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database)?;
    let outcome = match mutation {
        StoredHarnessStateMutation::ClaimLease {
            agent_id,
            owner_token,
            now_millis,
            expires_at_millis,
        } => claim_lease(
            &transaction,
            agent_id,
            owner_token,
            now_millis,
            expires_at_millis,
        )?,
        StoredHarnessStateMutation::ReleaseLease {
            agent_id,
            owner_token,
        } => release_lease(&transaction, agent_id, owner_token)?,
        StoredHarnessStateMutation::SetReadySession { owner_token, ready } => {
            ensure_owner(&transaction, ready.agent_id, owner_token)?;
            set_ready_session(&transaction, &ready)?;
            HarnessLeaseOutcome::Acquired
        }
        StoredHarnessStateMutation::QueueSessionOperation(operation) => {
            queue_session_operation(&transaction, &operation)?;
            HarnessLeaseOutcome::Acquired
        }
        StoredHarnessStateMutation::SetSessionOperationState {
            operation_id,
            state,
        } => {
            set_session_operation_state(&transaction, operation_id, &state)?;
            HarnessLeaseOutcome::Acquired
        }
        StoredHarnessStateMutation::QueueDelivery(delivery) => {
            queue_delivery(&transaction, &delivery)?;
            HarnessLeaseOutcome::Acquired
        }
        StoredHarnessStateMutation::SetDeliveryState {
            agent_id,
            submission_id,
            owner_token,
            state,
        } => {
            ensure_owner(&transaction, agent_id, owner_token)?;
            set_delivery_state(&transaction, agent_id, submission_id, state)?;
            HarnessLeaseOutcome::Acquired
        }
        StoredHarnessStateMutation::CheckpointEvent {
            owner_token,
            checkpoint,
        } => {
            ensure_owner(&transaction, checkpoint.agent_id, owner_token)?;
            checkpoint_event(&transaction, &checkpoint)?;
            HarnessLeaseOutcome::Acquired
        }
    };
    transaction.commit().map_err(database)?;
    Ok(outcome)
}

fn ensure_owner(
    connection: &Connection,
    agent_id: AgentId,
    owner_token: [u8; 32],
) -> Result<(), StoreError> {
    let lease = load_lease(connection, agent_id)?.ok_or_else(conflict)?;
    if lease.owner_token == owner_token {
        Ok(())
    } else {
        Err(conflict())
    }
}

pub(super) fn load(
    connection: &Connection,
    limit: usize,
) -> Result<StoredHarnessStateSnapshot, StoreError> {
    let limit = bounded_limit(limit)?;
    Ok(StoredHarnessStateSnapshot {
        leases: load_leases(connection, limit)?,
        ready_sessions: load_ready_sessions(connection, limit)?,
        session_operations: load_session_operations(connection, limit)?,
        deliveries: load_deliveries(connection, limit)?,
        events: load_events(connection, limit)?,
    })
}

fn queue_session_operation(
    transaction: &Transaction<'_>,
    operation: &HarnessSessionOperation,
) -> Result<(), StoreError> {
    if operation.state != HarnessSessionOperationState::Prepared {
        return Err(invalid());
    }
    if let Some(stored) = load_session_operation(transaction, operation.operation_id)? {
        return if same_session_operation_identity(&stored, operation) {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    let (control_kind, requested_session) = encode_session_operation_kind(&operation.kind);
    transaction
        .execute(
            "INSERT INTO harness_session_operations(\
                operation_id, request_digest, agent_id, provider_id, control_kind,\
                requested_session, operation_state, ready_session\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, NULL)",
            params![
                operation.operation_id.as_bytes().as_slice(),
                operation.request_digest.as_bytes().as_slice(),
                operation.agent_id.as_bytes().as_slice(),
                operation.provider_id.as_str(),
                control_kind,
                requested_session,
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn same_session_operation_identity(
    stored: &HarnessSessionOperation,
    proposed: &HarnessSessionOperation,
) -> bool {
    stored.operation_id == proposed.operation_id
        && stored.request_digest == proposed.request_digest
        && stored.agent_id == proposed.agent_id
        && stored.provider_id == proposed.provider_id
        && stored.kind == proposed.kind
}

fn set_session_operation_state(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    proposed: &HarnessSessionOperationState,
) -> Result<(), StoreError> {
    let stored = load_session_operation(transaction, operation_id)?.ok_or_else(conflict)?;
    if stored.state == *proposed {
        return Ok(());
    }
    if session_operation_rank(proposed) <= session_operation_rank(&stored.state)
        || session_operation_terminal(&stored.state)
    {
        return Err(conflict());
    }
    let (state, ready_session) = encode_session_operation_state(proposed);
    transaction
        .execute(
            "UPDATE harness_session_operations SET operation_state = ?2, ready_session = ?3 \
             WHERE operation_id = ?1",
            params![operation_id.as_bytes().as_slice(), state, ready_session],
        )
        .map_err(database)?;
    Ok(())
}

fn set_ready_session(
    transaction: &Transaction<'_>,
    ready: &StoredHarnessReadySession,
) -> Result<(), StoreError> {
    transaction
        .execute(
            "INSERT INTO harness_ready_sessions(agent_id, provider_id, session_id) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(agent_id) DO UPDATE SET \
                provider_id = excluded.provider_id, session_id = excluded.session_id",
            params![
                ready.agent_id.as_bytes().as_slice(),
                ready.provider_id.as_str(),
                ready.session_id.as_str(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn claim_lease(
    transaction: &Transaction<'_>,
    agent_id: AgentId,
    owner_token: [u8; 32],
    now_millis: u64,
    expires_at_millis: u64,
) -> Result<HarnessLeaseOutcome, StoreError> {
    if owner_token == [0; 32] || expires_at_millis <= now_millis {
        return Err(invalid());
    }
    if let Some(stored) = load_lease(transaction, agent_id)? {
        if stored.owner_token != owner_token && stored.expires_at_millis > now_millis {
            return Ok(HarnessLeaseOutcome::Held);
        }
        if stored.owner_token == owner_token && expires_at_millis < stored.expires_at_millis {
            return Err(conflict());
        }
        transaction
            .execute(
                "UPDATE harness_worker_leases SET owner_token = ?2, expires_at_millis = ?3 \
                 WHERE agent_id = ?1",
                params![
                    agent_id.as_bytes().as_slice(),
                    owner_token.as_slice(),
                    expires_at_millis.to_be_bytes().as_slice(),
                ],
            )
            .map_err(database)?;
        return Ok(HarnessLeaseOutcome::Acquired);
    }
    transaction
        .execute(
            "INSERT INTO harness_worker_leases(agent_id, owner_token, expires_at_millis) \
             VALUES (?1, ?2, ?3)",
            params![
                agent_id.as_bytes().as_slice(),
                owner_token.as_slice(),
                expires_at_millis.to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(HarnessLeaseOutcome::Acquired)
}

fn release_lease(
    transaction: &Transaction<'_>,
    agent_id: AgentId,
    owner_token: [u8; 32],
) -> Result<HarnessLeaseOutcome, StoreError> {
    if owner_token == [0; 32] {
        return Err(invalid());
    }
    let removed = transaction
        .execute(
            "DELETE FROM harness_worker_leases WHERE agent_id = ?1 AND owner_token = ?2",
            params![agent_id.as_bytes().as_slice(), owner_token.as_slice()],
        )
        .map_err(database)?;
    Ok(if removed == 1 {
        HarnessLeaseOutcome::Released
    } else {
        HarnessLeaseOutcome::Held
    })
}

fn queue_delivery(
    transaction: &Transaction<'_>,
    delivery: &StoredHarnessDelivery,
) -> Result<(), StoreError> {
    if delivery.state != StoredHarnessDeliveryState::Pending {
        return Err(invalid());
    }
    if let Some(stored) = load_delivery(transaction, delivery.agent_id, delivery.submission_id)? {
        return if same_delivery_identity(&stored, delivery) {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    let input_sequence = delivery
        .project
        .map(|project| project.sequence.get().to_be_bytes());
    transaction
        .execute(
            "INSERT INTO harness_deliveries(\
                agent_id, submission_id, provider_id, session_id, digest, operation_id, \
                project_id, dispatch_id, assignment_id, project_thread_id, input_sequence, body, \
                queued_at_millis, delivery_state\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                delivery.agent_id.as_bytes().as_slice(),
                delivery.submission_id.as_bytes().as_slice(),
                delivery.provider_id.as_str(),
                delivery.session_id.as_str(),
                delivery.digest.as_bytes().as_slice(),
                delivery.operation_id.as_bytes().as_slice(),
                delivery
                    .project
                    .as_ref()
                    .map(|value| value.project_id.as_bytes().as_slice()),
                delivery
                    .project
                    .as_ref()
                    .map(|value| value.dispatch_id.as_bytes().as_slice()),
                delivery
                    .project
                    .as_ref()
                    .map(|value| value.assignment_id.as_bytes().as_slice()),
                delivery
                    .project
                    .as_ref()
                    .map(|value| value.thread_id.as_bytes().as_slice()),
                input_sequence.as_ref().map(<[u8; 8]>::as_slice),
                delivery.body.as_str(),
                delivery.queued_at_millis.to_be_bytes().as_slice(),
                encode_delivery_state(delivery.state),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn same_delivery_identity(
    stored: &StoredHarnessDelivery,
    proposed: &StoredHarnessDelivery,
) -> bool {
    stored.agent_id == proposed.agent_id
        && stored.provider_id == proposed.provider_id
        && stored.session_id == proposed.session_id
        && stored.submission_id == proposed.submission_id
        && stored.digest == proposed.digest
        && stored.operation_id == proposed.operation_id
        && stored.project == proposed.project
        && stored.body == proposed.body
}

fn set_delivery_state(
    transaction: &Transaction<'_>,
    agent_id: AgentId,
    submission_id: MessageId,
    state: StoredHarnessDeliveryState,
) -> Result<(), StoreError> {
    let stored = load_delivery(transaction, agent_id, submission_id)?.ok_or_else(conflict)?;
    if matches!(
        stored.state,
        StoredHarnessDeliveryState::Accepted | StoredHarnessDeliveryState::Rejected
    ) && state != stored.state
    {
        return Err(conflict());
    }
    if delivery_rank(state) < delivery_rank(stored.state) {
        return Err(conflict());
    }
    transaction
        .execute(
            "UPDATE harness_deliveries SET delivery_state = ?3 \
             WHERE agent_id = ?1 AND submission_id = ?2",
            params![
                agent_id.as_bytes().as_slice(),
                submission_id.as_bytes().as_slice(),
                encode_delivery_state(state),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn checkpoint_event(
    transaction: &Transaction<'_>,
    checkpoint: &StoredHarnessEventCheckpoint,
) -> Result<(), StoreError> {
    if let Some(stored) = load_event(transaction, checkpoint.agent_id, checkpoint.event_id)? {
        if stored.digest != checkpoint.digest
            || (stored.output_committed && !checkpoint.output_committed)
            || (stored.activity_committed && !checkpoint.activity_committed)
        {
            return Err(conflict());
        }
        transaction
            .execute(
                "UPDATE harness_event_checkpoints \
                 SET output_committed = ?3, activity_committed = ?4 \
                 WHERE agent_id = ?1 AND event_id = ?2",
                params![
                    checkpoint.agent_id.as_bytes().as_slice(),
                    checkpoint.event_id.as_bytes().as_slice(),
                    encode_bool(checkpoint.output_committed),
                    encode_bool(checkpoint.activity_committed),
                ],
            )
            .map_err(database)?;
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO harness_event_checkpoints(\
                agent_id, event_id, digest, output_committed, activity_committed\
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                checkpoint.agent_id.as_bytes().as_slice(),
                checkpoint.event_id.as_bytes().as_slice(),
                checkpoint.digest.as_bytes().as_slice(),
                encode_bool(checkpoint.output_committed),
                encode_bool(checkpoint.activity_committed),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn load_leases(connection: &Connection, limit: i64) -> Result<Vec<StoredHarnessLease>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT agent_id, owner_token, expires_at_millis FROM harness_worker_leases \
             ORDER BY agent_id LIMIT ?1",
        )
        .map_err(database)?;
    statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(database)?
        .map(|row| {
            let (agent, token, expires) = row.map_err(database)?;
            Ok(StoredHarnessLease {
                agent_id: AgentId::from_bytes(fixed(agent)?),
                owner_token: fixed(token)?,
                expires_at_millis: decode_u64(expires)?,
            })
        })
        .collect()
}

fn load_deliveries(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredHarnessDelivery>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT agent_id, provider_id, session_id, submission_id, digest, operation_id, \
                    project_id, dispatch_id, assignment_id, project_thread_id, input_sequence, \
                    body, queued_at_millis, delivery_state \
             FROM harness_deliveries \
             ORDER BY queued_at_millis, agent_id, submission_id LIMIT ?1",
        )
        .map_err(database)?;
    statement
        .query_map([limit], delivery_row)
        .map_err(database)?
        .map(|row| decode_delivery(row.map_err(database)?))
        .collect()
}

pub(super) fn load_runnable_deliveries(
    connection: &Connection,
    agent_id: AgentId,
    limit: usize,
) -> Result<Vec<StoredHarnessDelivery>, StoreError> {
    let limit = bounded_limit(limit)?;
    let mut statement = connection
        .prepare(
            "SELECT agent_id, provider_id, session_id, submission_id, digest, operation_id, \
                    project_id, dispatch_id, assignment_id, project_thread_id, input_sequence, \
                    body, queued_at_millis, delivery_state \
             FROM harness_deliveries \
             WHERE agent_id = ?1 AND delivery_state IN (1, 2) \
             ORDER BY queued_at_millis, submission_id LIMIT ?2",
        )
        .map_err(database)?;
    statement
        .query_map(params![agent_id.as_bytes().as_slice(), limit], delivery_row)
        .map_err(database)?
        .map(|row| decode_delivery(row.map_err(database)?))
        .collect()
}

fn load_ready_sessions(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredHarnessReadySession>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT agent_id, provider_id, session_id FROM harness_ready_sessions \
             ORDER BY agent_id LIMIT ?1",
        )
        .map_err(database)?;
    statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database)?
        .map(|row| {
            let (agent, provider, session) = row.map_err(database)?;
            Ok(StoredHarnessReadySession {
                agent_id: AgentId::from_bytes(fixed(agent)?),
                provider_id: ProviderId::new(provider).map_err(|_| corrupt())?,
                session_id: ProviderSessionId::new(session).map_err(|_| corrupt())?,
            })
        })
        .collect()
}

fn load_session_operations(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<HarnessSessionOperation>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT operation_id, request_digest, agent_id, provider_id, control_kind, \
                    requested_session, operation_state, ready_session \
             FROM harness_session_operations ORDER BY operation_id LIMIT ?1",
        )
        .map_err(database)?;
    statement
        .query_map([limit], session_operation_row)
        .map_err(database)?
        .map(|row| decode_session_operation(row.map_err(database)?))
        .collect()
}

/// Loads one exact managed-session operation.
pub(super) fn load_session_operation(
    connection: &Connection,
    operation_id: OperationId,
) -> Result<Option<HarnessSessionOperation>, StoreError> {
    connection
        .query_row(
            "SELECT operation_id, request_digest, agent_id, provider_id, control_kind, \
                    requested_session, operation_state, ready_session \
             FROM harness_session_operations WHERE operation_id = ?1",
            [operation_id.as_bytes().as_slice()],
            session_operation_row,
        )
        .optional()
        .map_err(database)?
        .map(decode_session_operation)
        .transpose()
}

type SessionOperationRow = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    String,
    i64,
    Option<String>,
    i64,
    Option<String>,
);

fn session_operation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionOperationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn decode_session_operation(
    row: SessionOperationRow,
) -> Result<HarnessSessionOperation, StoreError> {
    Ok(HarnessSessionOperation {
        operation_id: OperationId::from_bytes(fixed(row.0)?),
        request_digest: CommandDigest::from_bytes(fixed(row.1)?),
        agent_id: AgentId::from_bytes(fixed(row.2)?),
        provider_id: ProviderId::new(row.3).map_err(|_| corrupt())?,
        kind: decode_session_operation_kind(row.4, row.5)?,
        state: decode_session_operation_state(row.6, row.7)?,
    })
}

fn load_events(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredHarnessEventCheckpoint>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT agent_id, event_id, digest, output_committed, activity_committed \
             FROM harness_event_checkpoints ORDER BY agent_id, event_id LIMIT ?1",
        )
        .map_err(database)?;
    statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(database)?
        .map(|row| {
            let (agent, event, digest, output, activity) = row.map_err(database)?;
            Ok(StoredHarnessEventCheckpoint {
                agent_id: AgentId::from_bytes(fixed(agent)?),
                event_id: MessageId::from_bytes(fixed(event)?),
                digest: CommandDigest::from_bytes(fixed(digest)?),
                output_committed: decode_bool(output)?,
                activity_committed: decode_bool(activity)?,
            })
        })
        .collect()
}

fn load_lease(
    connection: &Connection,
    agent_id: AgentId,
) -> Result<Option<StoredHarnessLease>, StoreError> {
    connection
        .query_row(
            "SELECT owner_token, expires_at_millis FROM harness_worker_leases WHERE agent_id = ?1",
            [agent_id.as_bytes().as_slice()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(database)?
        .map(|(token, expires)| {
            Ok(StoredHarnessLease {
                agent_id,
                owner_token: fixed(token)?,
                expires_at_millis: decode_u64(expires)?,
            })
        })
        .transpose()
}

type DeliveryRow = (
    Vec<u8>,
    String,
    String,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    String,
    Vec<u8>,
    i64,
);

fn delivery_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryRow> {
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
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
    ))
}

fn decode_delivery(row: DeliveryRow) -> Result<StoredHarnessDelivery, StoreError> {
    Ok(StoredHarnessDelivery {
        agent_id: AgentId::from_bytes(fixed(row.0)?),
        provider_id: ProviderId::new(row.1).map_err(|_| corrupt())?,
        session_id: ProviderSessionId::new(row.2).map_err(|_| corrupt())?,
        submission_id: MessageId::from_bytes(fixed(row.3)?),
        digest: CommandDigest::from_bytes(fixed(row.4)?),
        operation_id: OperationId::from_bytes(fixed(row.5)?),
        project: decode_project_delivery(row.6, row.7, row.8, row.9, row.10)?,
        body: ContentText::new(row.11).map_err(|_| corrupt())?,
        queued_at_millis: decode_u64(row.12)?,
        state: decode_delivery_state(row.13)?,
    })
}

fn decode_project_delivery(
    project_id: Option<Vec<u8>>,
    dispatch_id: Option<Vec<u8>>,
    assignment_id: Option<Vec<u8>>,
    thread_id: Option<Vec<u8>>,
    sequence: Option<Vec<u8>>,
) -> Result<Option<StoredHarnessProjectDelivery>, StoreError> {
    match (project_id, dispatch_id, assignment_id, thread_id, sequence) {
        (None, None, None, None, None) => Ok(None),
        (
            Some(project_id),
            Some(dispatch_id),
            Some(assignment_id),
            Some(thread_id),
            Some(sequence),
        ) => Ok(Some(StoredHarnessProjectDelivery {
            project_id: ProjectId::from_bytes(fixed(project_id)?),
            dispatch_id: DispatchId::from_bytes(fixed(dispatch_id)?),
            assignment_id: AssignmentId::from_bytes(fixed(assignment_id)?),
            thread_id: ThreadId::from_bytes(fixed(thread_id)?),
            sequence: NonZeroU64::new(decode_u64(sequence)?).ok_or_else(corrupt)?,
        })),
        _ => Err(corrupt()),
    }
}

pub(super) fn load_delivery(
    connection: &Connection,
    agent_id: AgentId,
    submission_id: MessageId,
) -> Result<Option<StoredHarnessDelivery>, StoreError> {
    connection
        .query_row(
            "SELECT agent_id, provider_id, session_id, submission_id, digest, operation_id, \
                    project_id, dispatch_id, assignment_id, project_thread_id, input_sequence, \
                    body, queued_at_millis, delivery_state \
             FROM harness_deliveries WHERE agent_id = ?1 AND submission_id = ?2",
            params![
                agent_id.as_bytes().as_slice(),
                submission_id.as_bytes().as_slice()
            ],
            delivery_row,
        )
        .optional()
        .map_err(database)?
        .map(decode_delivery)
        .transpose()
}

fn load_event(
    connection: &Connection,
    agent_id: AgentId,
    event_id: MessageId,
) -> Result<Option<StoredHarnessEventCheckpoint>, StoreError> {
    connection
        .query_row(
            "SELECT digest, output_committed, activity_committed \
             FROM harness_event_checkpoints WHERE agent_id = ?1 AND event_id = ?2",
            params![
                agent_id.as_bytes().as_slice(),
                event_id.as_bytes().as_slice()
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .map(|(digest, output, activity)| {
            Ok(StoredHarnessEventCheckpoint {
                agent_id,
                event_id,
                digest: CommandDigest::from_bytes(fixed(digest)?),
                output_committed: decode_bool(output)?,
                activity_committed: decode_bool(activity)?,
            })
        })
        .transpose()
}

const fn delivery_rank(state: StoredHarnessDeliveryState) -> u8 {
    match state {
        StoredHarnessDeliveryState::Pending => 1,
        StoredHarnessDeliveryState::Uncertain => 2,
        StoredHarnessDeliveryState::Accepted | StoredHarnessDeliveryState::Rejected => 3,
    }
}

const fn encode_delivery_state(state: StoredHarnessDeliveryState) -> i64 {
    match state {
        StoredHarnessDeliveryState::Pending => 1,
        StoredHarnessDeliveryState::Uncertain => 2,
        StoredHarnessDeliveryState::Accepted => 3,
        StoredHarnessDeliveryState::Rejected => 4,
    }
}

fn decode_delivery_state(value: i64) -> Result<StoredHarnessDeliveryState, StoreError> {
    match value {
        1 => Ok(StoredHarnessDeliveryState::Pending),
        2 => Ok(StoredHarnessDeliveryState::Uncertain),
        3 => Ok(StoredHarnessDeliveryState::Accepted),
        4 => Ok(StoredHarnessDeliveryState::Rejected),
        _ => Err(corrupt()),
    }
}

fn encode_session_operation_kind(kind: &HarnessSessionOperationKind) -> (i64, Option<&str>) {
    match kind {
        HarnessSessionOperationKind::Start => (1, None),
        HarnessSessionOperationKind::Resume(session) => (2, Some(session.as_str())),
        HarnessSessionOperationKind::Stop => (3, None),
    }
}

fn decode_session_operation_kind(
    kind: i64,
    requested_session: Option<String>,
) -> Result<HarnessSessionOperationKind, StoreError> {
    match (kind, requested_session) {
        (1, None) => Ok(HarnessSessionOperationKind::Start),
        (2, Some(session)) => ProviderSessionId::new(session)
            .map(HarnessSessionOperationKind::Resume)
            .map_err(|_| corrupt()),
        (3, None) => Ok(HarnessSessionOperationKind::Stop),
        _ => Err(corrupt()),
    }
}

fn encode_session_operation_state(state: &HarnessSessionOperationState) -> (i64, Option<&str>) {
    match state {
        HarnessSessionOperationState::Prepared => (1, None),
        HarnessSessionOperationState::Uncertain => (2, None),
        HarnessSessionOperationState::Ready(session) => (3, Some(session.as_str())),
        HarnessSessionOperationState::Stopped => (4, None),
        HarnessSessionOperationState::Rejected => (5, None),
    }
}

fn decode_session_operation_state(
    state: i64,
    ready_session: Option<String>,
) -> Result<HarnessSessionOperationState, StoreError> {
    match (state, ready_session) {
        (1, None) => Ok(HarnessSessionOperationState::Prepared),
        (2, None) => Ok(HarnessSessionOperationState::Uncertain),
        (3, Some(session)) => ProviderSessionId::new(session)
            .map(HarnessSessionOperationState::Ready)
            .map_err(|_| corrupt()),
        (4, None) => Ok(HarnessSessionOperationState::Stopped),
        (5, None) => Ok(HarnessSessionOperationState::Rejected),
        _ => Err(corrupt()),
    }
}

const fn session_operation_rank(state: &HarnessSessionOperationState) -> u8 {
    match state {
        HarnessSessionOperationState::Prepared => 1,
        HarnessSessionOperationState::Uncertain => 2,
        HarnessSessionOperationState::Ready(_)
        | HarnessSessionOperationState::Stopped
        | HarnessSessionOperationState::Rejected => 3,
    }
}

const fn session_operation_terminal(state: &HarnessSessionOperationState) -> bool {
    matches!(
        state,
        HarnessSessionOperationState::Ready(_)
            | HarnessSessionOperationState::Stopped
            | HarnessSessionOperationState::Rejected
    )
}

const fn encode_bool(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn decode_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt()),
    }
}

fn bounded_limit(limit: usize) -> Result<i64, StoreError> {
    if limit == 0 || limit > MAX_HARNESS_STATE_QUERY_ITEMS {
        return Err(invalid());
    }
    i64::try_from(limit).map_err(|_| invalid())
}

fn decode_u64(bytes: Vec<u8>) -> Result<u64, StoreError> {
    Ok(u64::from_be_bytes(fixed(bytes)?))
}

fn fixed<const SIZE: usize>(bytes: Vec<u8>) -> Result<[u8; SIZE], StoreError> {
    bytes.try_into().map_err(|_| corrupt())
}

fn database(_: rusqlite::Error) -> StoreError {
    StoreError::new(StoreErrorClass::DatabaseUnavailable)
}

const fn invalid() -> StoreError {
    StoreError::new(StoreErrorClass::InvalidOperationalRequest)
}

const fn conflict() -> StoreError {
    StoreError::new(StoreErrorClass::HarnessStateConflict)
}

const fn corrupt() -> StoreError {
    StoreError::new(StoreErrorClass::OperationalStateCorrupt)
}
