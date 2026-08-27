//! Durable managed-runtime lease, delivery, and persistence checkpoints.

use hq_domain::{
    AgentId, CommandDigest, ContentText, MessageId, OperationId, ProviderId, ProviderSessionId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    HarnessLeaseOutcome, MAX_HARNESS_STATE_QUERY_ITEMS, StoreError, StoreErrorClass,
    StoredHarnessDelivery, StoredHarnessDeliveryState, StoredHarnessEventCheckpoint,
    StoredHarnessLease, StoredHarnessReadySession, StoredHarnessStateMutation,
    StoredHarnessStateSnapshot,
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
        deliveries: load_deliveries(connection, limit)?,
        events: load_events(connection, limit)?,
    })
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
    transaction
        .execute(
            "INSERT INTO harness_deliveries(\
                agent_id, submission_id, provider_id, session_id, digest, operation_id, body, \
                queued_at_millis, delivery_state\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                delivery.agent_id.as_bytes().as_slice(),
                delivery.submission_id.as_bytes().as_slice(),
                delivery.provider_id.as_str(),
                delivery.session_id.as_str(),
                delivery.digest.as_bytes().as_slice(),
                delivery.operation_id.as_bytes().as_slice(),
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
            "SELECT agent_id, provider_id, session_id, submission_id, digest, operation_id, body, \
                    queued_at_millis, delivery_state \
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
            "SELECT agent_id, provider_id, session_id, submission_id, digest, operation_id, body, \
                    queued_at_millis, delivery_state \
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
        body: ContentText::new(row.6).map_err(|_| corrupt())?,
        queued_at_millis: decode_u64(row.7)?,
        state: decode_delivery_state(row.8)?,
    })
}

pub(super) fn load_delivery(
    connection: &Connection,
    agent_id: AgentId,
    submission_id: MessageId,
) -> Result<Option<StoredHarnessDelivery>, StoreError> {
    connection
        .query_row(
            "SELECT agent_id, provider_id, session_id, submission_id, digest, operation_id, body, \
                    queued_at_millis, delivery_state \
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
