//! Durable relay-state codecs and atomic transitions.

use hq_application::{RelayAccess, RelayAuthentication};
use hq_domain::{CommandDigest, FactId, InstallationId};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::{
    MAX_RELAY_QUARANTINE_BYTES, MAX_RELAY_QUARANTINE_ITEMS, MAX_RELAY_QUARANTINE_SAMPLE_BYTES,
    MAX_RELAY_STAGING_BYTES, MAX_RELAY_STAGING_ITEMS, MAX_RELAY_STATE_QUERY_ITEMS,
    MAX_RELAY_WRAPPER_BYTES, StoreError, StoreErrorClass, StoredAttemptDisposition,
    StoredCatchupCursor, StoredDesiredRelayPolicy, StoredInboundClaim, StoredPreparedOutbound,
    StoredQuarantineEvidence, StoredRelayAttempt, StoredRelayPolicy, StoredRelayPolicyChange,
    StoredRelayStateMutation, StoredRelayStateSnapshot, StoredStagedInput,
};

const MAX_RELAY_URL_BYTES: usize = 2_048;

pub(super) fn apply(
    connection: &mut Connection,
    mutation: StoredRelayStateMutation,
) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database)?;
    match mutation {
        StoredRelayStateMutation::Configure(change) => configure(&transaction, &change)?,
        StoredRelayStateMutation::Prepare(prepared) => prepare(&transaction, &prepared)?,
        StoredRelayStateMutation::Attempt(attempt) => put_attempt(&transaction, &attempt)?,
        StoredRelayStateMutation::Cursor(cursor) => put_cursor(&transaction, &cursor)?,
        StoredRelayStateMutation::ClaimInbound {
            claim,
            remove_staged,
        } => {
            claim_inbound(&transaction, &claim)?;
            remove_staging(&transaction, remove_staged)?;
        }
        StoredRelayStateMutation::Stage(input) => put_staged(&transaction, &input)?,
        StoredRelayStateMutation::Quarantine {
            evidence,
            remove_staged,
        } => {
            if remove_staged.is_some_and(|digest| digest != evidence.wrapper_sha256) {
                return Err(invalid());
            }
            put_quarantine(&transaction, &evidence)?;
            remove_staging(&transaction, remove_staged)?;
        }
    }
    transaction.commit().map_err(database)
}

fn remove_staging(
    transaction: &Transaction<'_>,
    digest: Option<[u8; 32]>,
) -> Result<(), StoreError> {
    if let Some(digest) = digest {
        transaction
            .execute(
                "DELETE FROM relay_staging WHERE wrapper_sha256 = ?1",
                [digest.as_slice()],
            )
            .map_err(database)?;
    }
    Ok(())
}

pub(super) fn load(
    connection: &Connection,
    limit: usize,
) -> Result<StoredRelayStateSnapshot, StoreError> {
    let sql_limit = bounded_limit(limit)?;
    Ok(StoredRelayStateSnapshot {
        policies: load_policies(connection, sql_limit)?,
        outbound: super::operational::load_outbox_intents(connection, limit)?,
        prepared: load_prepared(connection, sql_limit)?,
        attempts: load_attempts(connection, sql_limit)?,
        cursors: load_cursors(connection, sql_limit)?,
        staged: load_staged(connection, sql_limit)?,
        quarantine: load_quarantine(connection, sql_limit)?,
    })
}

fn configure(
    transaction: &Transaction<'_>,
    change: &StoredRelayPolicyChange,
) -> Result<(), StoreError> {
    validate_desired(&change.desired)?;
    let prior_operation = transaction
        .query_row(
            "SELECT request_digest, url, access, authentication, enabled, generation \
             FROM relay_policy_operations WHERE operation_id = ?1",
            [change.operation_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database)?;
    if let Some((digest, url, access, authentication, enabled, generation)) = prior_operation {
        let equal = CommandDigest::from_bytes(fixed(digest)?) == change.request_digest
            && url == change.desired.url
            && decode_access(access)? == change.desired.access
            && decode_authentication(authentication)? == change.desired.authentication
            && decode_bool(enabled)? == change.desired.enabled
            && decode_positive_u64(generation).is_ok();
        return if equal { Ok(()) } else { Err(conflict()) };
    }

    let prior_policy = load_policy(transaction, &change.desired.url)?;
    let generation = match prior_policy {
        Some(policy)
            if policy.access == change.desired.access
                && policy.authentication == change.desired.authentication
                && policy.enabled == change.desired.enabled =>
        {
            policy.generation
        }
        Some(policy) => policy.generation.checked_add(1).ok_or_else(corrupt)?,
        None => 1,
    };
    transaction
        .execute(
            "INSERT INTO relay_policy_operations(\
                operation_id, request_digest, url, access, authentication, enabled, generation\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                change.operation_id.as_bytes().as_slice(),
                change.request_digest.as_bytes().as_slice(),
                change.desired.url,
                encode_access(change.desired.access),
                encode_authentication(change.desired.authentication),
                encode_bool(change.desired.enabled),
                generation.to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    transaction
        .execute(
            "INSERT INTO relay_policies(url, access, authentication, enabled, generation) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(url) DO UPDATE SET access = excluded.access, \
                authentication = excluded.authentication, enabled = excluded.enabled, \
                generation = excluded.generation",
            params![
                change.desired.url,
                encode_access(change.desired.access),
                encode_authentication(change.desired.authentication),
                encode_bool(change.desired.enabled),
                generation.to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn prepare(
    transaction: &Transaction<'_>,
    prepared: &StoredPreparedOutbound,
) -> Result<(), StoreError> {
    validate_prepared(transaction, prepared)?;
    if let Some(stored) = load_prepared_lineage(transaction, prepared.fact_id, prepared.recipient)?
    {
        return if stored == *prepared {
            Ok(())
        } else {
            Err(identity_collision())
        };
    }
    let collision: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM prepared_relay_outbox \
             WHERE wrapper_id = ?1 OR one_use_public_key = ?2 LIMIT 1",
            params![
                prepared.wrapper_id.as_slice(),
                prepared.one_use_public_key.as_slice()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    if collision.is_some() {
        return Err(identity_collision());
    }
    transaction
        .execute(
            "INSERT INTO prepared_relay_outbox(\
                fact_id, recipient_installation, wrapper_id, one_use_public_key, \
                recipient_public_key, canonical_event_id, canonical_sha256, wrapper_sha256, \
                seal_created_at, gift_wrap_created_at, exact_wire\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                prepared.fact_id.as_bytes().as_slice(),
                prepared.recipient.as_bytes().as_slice(),
                prepared.wrapper_id.as_slice(),
                prepared.one_use_public_key.as_slice(),
                prepared.recipient_public_key.as_slice(),
                prepared.canonical_event_id.as_slice(),
                prepared.canonical_sha256.as_slice(),
                prepared.wrapper_sha256.as_slice(),
                prepared.seal_created_at.to_be_bytes().as_slice(),
                prepared.gift_wrap_created_at.to_be_bytes().as_slice(),
                prepared.exact_wire,
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn validate_prepared(
    transaction: &Transaction<'_>,
    prepared: &StoredPreparedOutbound,
) -> Result<(), StoreError> {
    if prepared.exact_wire.is_empty()
        || prepared.exact_wire.len() > MAX_RELAY_WRAPPER_BYTES
        || prepared.canonical_event_id != *prepared.fact_id.as_bytes()
        || <[u8; 32]>::from(Sha256::digest(&prepared.exact_wire)) != prepared.wrapper_sha256
    {
        return Err(invalid());
    }
    let canonical = transaction
        .query_row(
            "SELECT exact_canonical_bytes FROM outbox_intents \
             WHERE fact_id = ?1 AND recipient_installation = ?2",
            params![
                prepared.fact_id.as_bytes().as_slice(),
                prepared.recipient.as_bytes().as_slice()
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(invalid)?;
    if <[u8; 32]>::from(Sha256::digest(canonical)) != prepared.canonical_sha256 {
        return Err(invalid());
    }
    Ok(())
}

fn put_attempt(
    transaction: &Transaction<'_>,
    attempt: &StoredRelayAttempt,
) -> Result<(), StoreError> {
    validate_url(&attempt.url)?;
    if attempt.attempts == 0
        || (attempt.disposition == StoredAttemptDisposition::Accepted
            && attempt.retry_at_millis.is_some())
    {
        return Err(invalid());
    }
    let stored = load_attempt(transaction, &attempt.url, attempt.wrapper_id)?;
    if let Some(stored) = stored {
        if stored == *attempt {
            return Ok(());
        }
        if attempt.attempts < stored.attempts
            || attempt.last_attempt_millis < stored.last_attempt_millis
            || stored.disposition == StoredAttemptDisposition::Accepted
            || (attempt.attempts == stored.attempts
                && stored.disposition != StoredAttemptDisposition::Uncertain)
            || (attempt.attempts == stored.attempts
                && attempt.last_attempt_millis != stored.last_attempt_millis)
        {
            return Err(conflict());
        }
        transaction
            .execute(
                "UPDATE relay_attempts SET attempts = ?3, disposition = ?4, \
                    last_attempt_millis = ?5, retry_at_millis = ?6 \
                 WHERE url = ?1 AND wrapper_id = ?2",
                params![
                    attempt.url,
                    attempt.wrapper_id.as_slice(),
                    i64::from(attempt.attempts),
                    encode_disposition(attempt.disposition),
                    attempt.last_attempt_millis.to_be_bytes().as_slice(),
                    attempt.retry_at_millis.map(u64::to_be_bytes),
                ],
            )
            .map_err(database)?;
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO relay_attempts(\
                url, wrapper_id, attempts, disposition, last_attempt_millis, retry_at_millis\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                attempt.url,
                attempt.wrapper_id.as_slice(),
                i64::from(attempt.attempts),
                encode_disposition(attempt.disposition),
                attempt.last_attempt_millis.to_be_bytes().as_slice(),
                attempt.retry_at_millis.map(u64::to_be_bytes),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn put_cursor(
    transaction: &Transaction<'_>,
    cursor: &StoredCatchupCursor,
) -> Result<(), StoreError> {
    validate_url(&cursor.url)?;
    if cursor.generation == 0
        || cursor.oldest_created_at.is_some() != cursor.oldest_wrapper_id.is_some()
    {
        return Err(invalid());
    }
    let policy = load_policy(transaction, &cursor.url)?.ok_or_else(invalid)?;
    if cursor.generation != policy.generation {
        return Err(conflict());
    }
    if let Some(stored) = load_cursor(transaction, &cursor.url)? {
        if stored == *cursor {
            return Ok(());
        }
        if cursor.generation < stored.generation
            || (cursor.generation == stored.generation
                && (stored.exhausted && !cursor.exhausted || !boundary_advances(&stored, cursor)))
        {
            return Err(conflict());
        }
    }
    transaction
        .execute(
            "INSERT INTO relay_cursors(\
                url, generation, oldest_created_at, oldest_wrapper_id, exhausted\
             ) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(url) DO UPDATE SET generation = excluded.generation, \
                oldest_created_at = excluded.oldest_created_at, \
                oldest_wrapper_id = excluded.oldest_wrapper_id, exhausted = excluded.exhausted",
            params![
                cursor.url,
                cursor.generation.to_be_bytes().as_slice(),
                cursor.oldest_created_at.map(u64::to_be_bytes),
                cursor.oldest_wrapper_id.map(|value| value.to_vec()),
                encode_bool(cursor.exhausted),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn boundary_advances(stored: &StoredCatchupCursor, candidate: &StoredCatchupCursor) -> bool {
    match (
        stored.oldest_created_at.zip(stored.oldest_wrapper_id),
        candidate.oldest_created_at.zip(candidate.oldest_wrapper_id),
    ) {
        (None, Some(_)) => true,
        (None, None) => stored.exhausted != candidate.exhausted,
        (Some(_), None) => false,
        (Some(old), Some(new)) => {
            new < old || (new == old && stored.exhausted != candidate.exhausted)
        }
    }
}

fn claim_inbound(
    transaction: &Transaction<'_>,
    claim: &StoredInboundClaim,
) -> Result<(), StoreError> {
    let mut statement = transaction
        .prepare(
            "SELECT wrapper_id, origin_installation_id, canonical_event_id, canonical_sha256, \
                    received_at_millis \
             FROM inbound_relay_claims WHERE wrapper_id = ?1 OR \
                (origin_installation_id = ?2 AND canonical_event_id = ?3)",
        )
        .map_err(database)?;
    let rows = statement
        .query_map(
            params![
                claim.wrapper_id.as_slice(),
                claim.origin_installation_id.as_slice(),
                claim.canonical_event_id.as_slice()
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .map_err(database)?;
    let mut found = false;
    for row in rows {
        found = true;
        if decode_claim(row.map_err(database)?)? != *claim {
            return Err(identity_collision());
        }
    }
    drop(statement);
    if found {
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO inbound_relay_claims(\
                wrapper_id, origin_installation_id, canonical_event_id, canonical_sha256, \
                received_at_millis\
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                claim.wrapper_id.as_slice(),
                claim.origin_installation_id.as_slice(),
                claim.canonical_event_id.as_slice(),
                claim.canonical_sha256.as_slice(),
                claim.received_at_millis.to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn put_staged(transaction: &Transaction<'_>, input: &StoredStagedInput) -> Result<(), StoreError> {
    if input.exact_outer.is_empty()
        || input.exact_outer.len() > MAX_RELAY_WRAPPER_BYTES
        || <[u8; 32]>::from(Sha256::digest(&input.exact_outer)) != input.wrapper_sha256
    {
        return Err(invalid());
    }
    if let Some(stored) = load_staged_one(transaction, input.wrapper_sha256)? {
        if stored == *input {
            return Ok(());
        }
        if stored.exact_outer != input.exact_outer
            || stored.first_received_millis != input.first_received_millis
            || input.attempts < stored.attempts
            || input.attempts == stored.attempts
        {
            return Err(conflict());
        }
        transaction
            .execute(
                "UPDATE relay_staging SET attempts = ?2, retry_at_millis = ?3 \
                 WHERE wrapper_sha256 = ?1",
                params![
                    input.wrapper_sha256.as_slice(),
                    i64::from(input.attempts),
                    input.retry_at_millis.to_be_bytes().as_slice(),
                ],
            )
            .map_err(database)?;
        return Ok(());
    }
    let (count, bytes): (i64, i64) = transaction
        .query_row(
            "SELECT count(*), coalesce(sum(length(exact_outer)), 0) FROM relay_staging",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database)?;
    let incoming = i64::try_from(input.exact_outer.len()).map_err(|_| invalid())?;
    if count >= i64::try_from(MAX_RELAY_STAGING_ITEMS).map_err(|_| invalid())?
        || bytes
            .checked_add(incoming)
            .is_none_or(|total| total > i64::try_from(MAX_RELAY_STAGING_BYTES).unwrap_or(i64::MAX))
    {
        return Err(StoreError::new(StoreErrorClass::RelayStagingFull));
    }
    transaction
        .execute(
            "INSERT INTO relay_staging(\
                wrapper_sha256, exact_outer, first_received_millis, attempts, retry_at_millis\
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                input.wrapper_sha256.as_slice(),
                input.exact_outer,
                input.first_received_millis.to_be_bytes().as_slice(),
                i64::from(input.attempts),
                input.retry_at_millis.to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn put_quarantine(
    transaction: &Transaction<'_>,
    evidence: &StoredQuarantineEvidence,
) -> Result<(), StoreError> {
    if evidence.failure_code == 0
        || evidence.byte_len == 0
        || evidence.raw_sample.len() > MAX_RELAY_QUARANTINE_SAMPLE_BYTES
        || evidence.raw_sample.len() > evidence.byte_len
        || i64::try_from(evidence.byte_len).is_err()
    {
        return Err(invalid());
    }
    if let Some(stored) = load_quarantine_one(transaction, evidence.wrapper_sha256)? {
        return if stored == *evidence {
            Ok(())
        } else {
            Err(identity_collision())
        };
    }
    transaction
        .execute(
            "INSERT INTO relay_quarantine(\
                wrapper_sha256, wrapper_id, failure_code, received_at_millis, byte_len, raw_sample\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                evidence.wrapper_sha256.as_slice(),
                evidence.wrapper_id.map(|value| value.to_vec()),
                i64::from(evidence.failure_code),
                evidence.received_at_millis.to_be_bytes().as_slice(),
                i64::try_from(evidence.byte_len).map_err(|_| invalid())?,
                evidence.raw_sample,
            ],
        )
        .map_err(database)?;
    loop {
        let (count, bytes): (i64, i64) = transaction
            .query_row(
                "SELECT count(*), coalesce(sum(length(raw_sample)), 0) FROM relay_quarantine",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(database)?;
        if count <= i64::try_from(MAX_RELAY_QUARANTINE_ITEMS).unwrap_or(i64::MAX)
            && bytes <= i64::try_from(MAX_RELAY_QUARANTINE_BYTES).unwrap_or(i64::MAX)
        {
            break;
        }
        let oldest: Vec<u8> = transaction
            .query_row(
                "SELECT wrapper_sha256 FROM relay_quarantine \
                 ORDER BY received_at_millis, wrapper_sha256 LIMIT 1",
                [],
                |row| row.get(0),
            )
            .map_err(database)?;
        transaction
            .execute(
                "DELETE FROM relay_quarantine WHERE wrapper_sha256 = ?1",
                [oldest],
            )
            .map_err(database)?;
    }
    Ok(())
}

fn load_policies(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredRelayPolicy>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT url, access, authentication, enabled, generation FROM relay_policies \
             ORDER BY url LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(database)?;
    rows.map(|row| decode_policy(row.map_err(database)?))
        .collect()
}

fn load_policy(
    connection: &Connection,
    url: &str,
) -> Result<Option<StoredRelayPolicy>, StoreError> {
    connection
        .query_row(
            "SELECT url, access, authentication, enabled, generation \
             FROM relay_policies WHERE url = ?1",
            [url],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .map(decode_policy)
        .transpose()
}

type PreparedRow = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
);
type AttemptRow = (String, Vec<u8>, i64, i64, Vec<u8>, Option<Vec<u8>>);
type CursorRow = (String, Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>, i64);
type QuarantineRow = (Vec<u8>, Option<Vec<u8>>, i64, Vec<u8>, i64, Vec<u8>);
type ClaimRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

const PREPARED_COLUMNS: &str = "fact_id, recipient_installation, wrapper_id, \
    one_use_public_key, recipient_public_key, canonical_event_id, canonical_sha256, \
    wrapper_sha256, seal_created_at, gift_wrap_created_at, exact_wire";

fn prepared_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PreparedRow> {
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
}

fn load_prepared_lineage(
    connection: &Connection,
    fact_id: FactId,
    recipient: InstallationId,
) -> Result<Option<StoredPreparedOutbound>, StoreError> {
    connection
        .query_row(
            &format!(
                "SELECT {PREPARED_COLUMNS} FROM prepared_relay_outbox \
                 WHERE fact_id = ?1 AND recipient_installation = ?2"
            ),
            params![
                fact_id.as_bytes().as_slice(),
                recipient.as_bytes().as_slice()
            ],
            prepared_row,
        )
        .optional()
        .map_err(database)?
        .map(decode_prepared)
        .transpose()
}

fn load_prepared(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredPreparedOutbound>, StoreError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {PREPARED_COLUMNS} FROM prepared_relay_outbox \
             ORDER BY fact_id, recipient_installation LIMIT ?1"
        ))
        .map_err(database)?;
    let rows = statement
        .query_map([limit], prepared_row)
        .map_err(database)?;
    rows.map(|row| decode_prepared(row.map_err(database)?))
        .collect()
}

fn decode_prepared(row: PreparedRow) -> Result<StoredPreparedOutbound, StoreError> {
    let prepared = StoredPreparedOutbound {
        fact_id: FactId::from_bytes(fixed(row.0)?),
        recipient: InstallationId::from_bytes(fixed(row.1)?),
        wrapper_id: fixed(row.2)?,
        one_use_public_key: fixed(row.3)?,
        recipient_public_key: fixed(row.4)?,
        canonical_event_id: fixed(row.5)?,
        canonical_sha256: fixed(row.6)?,
        wrapper_sha256: fixed(row.7)?,
        seal_created_at: decode_u64(row.8)?,
        gift_wrap_created_at: decode_u64(row.9)?,
        exact_wire: row.10,
    };
    if prepared.exact_wire.is_empty()
        || prepared.exact_wire.len() > MAX_RELAY_WRAPPER_BYTES
        || prepared.canonical_event_id != *prepared.fact_id.as_bytes()
        || <[u8; 32]>::from(Sha256::digest(&prepared.exact_wire)) != prepared.wrapper_sha256
    {
        return Err(corrupt());
    }
    Ok(prepared)
}

fn load_attempt(
    connection: &Connection,
    url: &str,
    wrapper_id: [u8; 32],
) -> Result<Option<StoredRelayAttempt>, StoreError> {
    connection
        .query_row(
            "SELECT url, wrapper_id, attempts, disposition, last_attempt_millis, retry_at_millis \
             FROM relay_attempts WHERE url = ?1 AND wrapper_id = ?2",
            params![url, wrapper_id.as_slice()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Option<Vec<u8>>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .map(decode_attempt)
        .transpose()
}

fn load_attempts(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredRelayAttempt>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT url, wrapper_id, attempts, disposition, last_attempt_millis, retry_at_millis \
             FROM relay_attempts ORDER BY url, wrapper_id LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
            ))
        })
        .map_err(database)?;
    rows.map(|row| decode_attempt(row.map_err(database)?))
        .collect()
}

fn decode_attempt(row: AttemptRow) -> Result<StoredRelayAttempt, StoreError> {
    validate_url(&row.0).map_err(|_| corrupt())?;
    let attempts = u32::try_from(row.2).map_err(|_| corrupt())?;
    if attempts == 0 {
        return Err(corrupt());
    }
    let disposition = decode_disposition(row.3)?;
    let retry_at_millis = row.5.map(decode_u64).transpose()?;
    if disposition == StoredAttemptDisposition::Accepted && retry_at_millis.is_some() {
        return Err(corrupt());
    }
    Ok(StoredRelayAttempt {
        url: row.0,
        wrapper_id: fixed(row.1)?,
        attempts,
        disposition,
        last_attempt_millis: decode_u64(row.4)?,
        retry_at_millis,
    })
}

fn load_cursor(
    connection: &Connection,
    url: &str,
) -> Result<Option<StoredCatchupCursor>, StoreError> {
    connection
        .query_row(
            "SELECT url, generation, oldest_created_at, oldest_wrapper_id, exhausted \
             FROM relay_cursors WHERE url = ?1",
            [url],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .map(decode_cursor)
        .transpose()
}

fn load_cursors(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredCatchupCursor>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT url, generation, oldest_created_at, oldest_wrapper_id, exhausted \
             FROM relay_cursors ORDER BY url LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(database)?;
    rows.map(|row| decode_cursor(row.map_err(database)?))
        .collect()
}

fn decode_cursor(row: CursorRow) -> Result<StoredCatchupCursor, StoreError> {
    validate_url(&row.0).map_err(|_| corrupt())?;
    let oldest_created_at = row.2.map(decode_u64).transpose()?;
    let oldest_wrapper_id = row.3.map(fixed).transpose()?;
    if oldest_created_at.is_some() != oldest_wrapper_id.is_some() {
        return Err(corrupt());
    }
    Ok(StoredCatchupCursor {
        url: row.0,
        generation: decode_positive_u64(row.1)?,
        oldest_created_at,
        oldest_wrapper_id,
        exhausted: decode_bool(row.4)?,
    })
}

fn load_staged_one(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<Option<StoredStagedInput>, StoreError> {
    connection
        .query_row(
            "SELECT wrapper_sha256, exact_outer, first_received_millis, attempts, retry_at_millis \
             FROM relay_staging WHERE wrapper_sha256 = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .map(decode_staged)
        .transpose()
}

fn load_staged(connection: &Connection, limit: i64) -> Result<Vec<StoredStagedInput>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT wrapper_sha256, exact_outer, first_received_millis, attempts, retry_at_millis \
             FROM relay_staging ORDER BY first_received_millis, wrapper_sha256 LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(database)?;
    rows.map(|row| decode_staged(row.map_err(database)?))
        .collect()
}

fn decode_staged(
    row: (Vec<u8>, Vec<u8>, Vec<u8>, i64, Vec<u8>),
) -> Result<StoredStagedInput, StoreError> {
    let digest = fixed(row.0)?;
    if row.1.is_empty()
        || row.1.len() > MAX_RELAY_WRAPPER_BYTES
        || <[u8; 32]>::from(Sha256::digest(&row.1)) != digest
    {
        return Err(corrupt());
    }
    Ok(StoredStagedInput {
        wrapper_sha256: digest,
        exact_outer: row.1,
        first_received_millis: decode_u64(row.2)?,
        attempts: u32::try_from(row.3).map_err(|_| corrupt())?,
        retry_at_millis: decode_u64(row.4)?,
    })
}

fn load_quarantine_one(
    connection: &Connection,
    digest: [u8; 32],
) -> Result<Option<StoredQuarantineEvidence>, StoreError> {
    connection
        .query_row(
            "SELECT wrapper_sha256, wrapper_id, failure_code, received_at_millis, \
                    byte_len, raw_sample \
             FROM relay_quarantine WHERE wrapper_sha256 = ?1",
            [digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database)?
        .map(decode_quarantine)
        .transpose()
}

fn load_quarantine(
    connection: &Connection,
    limit: i64,
) -> Result<Vec<StoredQuarantineEvidence>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT wrapper_sha256, wrapper_id, failure_code, received_at_millis, \
                    byte_len, raw_sample \
             FROM relay_quarantine ORDER BY received_at_millis, wrapper_sha256 LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Vec<u8>>(5)?,
            ))
        })
        .map_err(database)?;
    rows.map(|row| decode_quarantine(row.map_err(database)?))
        .collect()
}

fn decode_quarantine(row: QuarantineRow) -> Result<StoredQuarantineEvidence, StoreError> {
    let failure_code = u16::try_from(row.2).map_err(|_| corrupt())?;
    let byte_len = usize::try_from(row.4).map_err(|_| corrupt())?;
    if failure_code == 0
        || byte_len == 0
        || row.5.len() > MAX_RELAY_QUARANTINE_SAMPLE_BYTES
        || row.5.len() > byte_len
    {
        return Err(corrupt());
    }
    Ok(StoredQuarantineEvidence {
        wrapper_sha256: fixed(row.0)?,
        wrapper_id: row.1.map(fixed).transpose()?,
        failure_code,
        received_at_millis: decode_u64(row.3)?,
        byte_len,
        raw_sample: row.5,
    })
}

fn decode_claim(row: ClaimRow) -> Result<StoredInboundClaim, StoreError> {
    Ok(StoredInboundClaim {
        wrapper_id: fixed(row.0)?,
        origin_installation_id: fixed(row.1)?,
        canonical_event_id: fixed(row.2)?,
        canonical_sha256: fixed(row.3)?,
        received_at_millis: decode_u64(row.4)?,
    })
}

fn decode_policy(row: (String, i64, i64, i64, Vec<u8>)) -> Result<StoredRelayPolicy, StoreError> {
    validate_url(&row.0).map_err(|_| corrupt())?;
    Ok(StoredRelayPolicy {
        url: row.0,
        access: decode_access(row.1)?,
        authentication: decode_authentication(row.2)?,
        enabled: decode_bool(row.3)?,
        generation: decode_positive_u64(row.4)?,
    })
}

fn validate_desired(policy: &StoredDesiredRelayPolicy) -> Result<(), StoreError> {
    validate_url(&policy.url)
}

fn validate_url(value: &str) -> Result<(), StoreError> {
    let suffix = value
        .strip_prefix("ws://")
        .or_else(|| value.strip_prefix("wss://"));
    if suffix.is_none_or(|suffix| !valid_authority(suffix))
        || value.len() > MAX_RELAY_URL_BYTES
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(invalid());
    }
    Ok(())
}

fn valid_authority(suffix: &str) -> bool {
    if suffix.contains('#') {
        return false;
    }
    let authority = suffix
        .split_once(['/', '?'])
        .map_or(suffix, |(authority, _)| authority);
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, remainder)) = bracketed.split_once(']') else {
            return false;
        };
        return !host.is_empty()
            && (remainder.is_empty() || remainder.strip_prefix(':').is_some_and(valid_port));
    }
    if authority.contains(['[', ']']) {
        return false;
    }
    authority.rsplit_once(':').map_or_else(
        || !authority.is_empty(),
        |(host, port)| !host.is_empty() && !host.contains(':') && valid_port(port),
    )
}

fn valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok()
}

const fn encode_access(value: RelayAccess) -> i64 {
    match value {
        RelayAccess::Read => 1,
        RelayAccess::Write => 2,
        RelayAccess::ReadWrite => 3,
    }
}

fn decode_access(value: i64) -> Result<RelayAccess, StoreError> {
    match value {
        1 => Ok(RelayAccess::Read),
        2 => Ok(RelayAccess::Write),
        3 => Ok(RelayAccess::ReadWrite),
        _ => Err(corrupt()),
    }
}

const fn encode_authentication(value: RelayAuthentication) -> i64 {
    match value {
        RelayAuthentication::Disabled => 1,
        RelayAuthentication::OnChallenge => 2,
        RelayAuthentication::Required => 3,
    }
}

fn decode_authentication(value: i64) -> Result<RelayAuthentication, StoreError> {
    match value {
        1 => Ok(RelayAuthentication::Disabled),
        2 => Ok(RelayAuthentication::OnChallenge),
        3 => Ok(RelayAuthentication::Required),
        _ => Err(corrupt()),
    }
}

const fn encode_disposition(value: StoredAttemptDisposition) -> i64 {
    match value {
        StoredAttemptDisposition::Uncertain => 1,
        StoredAttemptDisposition::Rejected => 2,
        StoredAttemptDisposition::Accepted => 3,
    }
}

fn decode_disposition(value: i64) -> Result<StoredAttemptDisposition, StoreError> {
    match value {
        1 => Ok(StoredAttemptDisposition::Uncertain),
        2 => Ok(StoredAttemptDisposition::Rejected),
        3 => Ok(StoredAttemptDisposition::Accepted),
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
    if limit == 0 || limit > MAX_RELAY_STATE_QUERY_ITEMS {
        return Err(invalid());
    }
    i64::try_from(limit).map_err(|_| invalid())
}

fn decode_positive_u64(bytes: Vec<u8>) -> Result<u64, StoreError> {
    let value = decode_u64(bytes)?;
    if value == 0 {
        Err(corrupt())
    } else {
        Ok(value)
    }
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
    StoreError::new(StoreErrorClass::RelayStateConflict)
}

const fn identity_collision() -> StoreError {
    StoreError::new(StoreErrorClass::IdentityCollision)
}

const fn corrupt() -> StoreError {
    StoreError::new(StoreErrorClass::OperationalStateCorrupt)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::database::SCHEMA;
    use rusqlite::Connection;

    #[test]
    fn preparation_claims_wrapper_and_one_use_key_in_the_same_transaction() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        connection.execute_batch(SCHEMA).expect("schema creates");
        for identity in [1_u8, 2] {
            connection
                .execute(
                    "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                     VALUES (?1, ?2, 1, 1)",
                    params![[identity; 32].as_slice(), [identity].as_slice()],
                )
                .expect("canonical fixture stores");
            connection
                .execute(
                    "INSERT INTO outbox_intents(\
                        fact_id, recipient_installation, exact_canonical_bytes, revision\
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        [identity; 32].as_slice(),
                        [9_u8; 32].as_slice(),
                        [identity].as_slice(),
                        [0_u8; 8].as_slice(),
                    ],
                )
                .expect("outbox fixture stores");
        }
        let first = prepared_fixture(1, 3, 4);
        apply(
            &mut connection,
            StoredRelayStateMutation::Prepare(first.clone()),
        )
        .expect("first lineage and claims commit");

        let reused_key = prepared_fixture(2, 5, 4);
        assert_eq!(
            apply(
                &mut connection,
                StoredRelayStateMutation::Prepare(reused_key)
            )
            .expect_err("one-use key cannot cross lineages")
            .class(),
            StoreErrorClass::IdentityCollision
        );
        let reused_wrapper = prepared_fixture(2, 3, 6);
        assert_eq!(
            apply(
                &mut connection,
                StoredRelayStateMutation::Prepare(reused_wrapper)
            )
            .expect_err("wrapper identity cannot cross lineages")
            .class(),
            StoreErrorClass::IdentityCollision
        );
        let rows: i64 = connection
            .query_row("SELECT count(*) FROM prepared_relay_outbox", [], |row| {
                row.get(0)
            })
            .expect("prepared count loads");
        assert_eq!(rows, 1);
        assert_eq!(load_prepared(&connection, 8), Ok(vec![first]));
    }

    fn prepared_fixture(fact: u8, wrapper: u8, one_use: u8) -> StoredPreparedOutbound {
        let exact_wire = vec![wrapper];
        StoredPreparedOutbound {
            fact_id: FactId::from_bytes([fact; 32]),
            recipient: InstallationId::from_bytes([9; 32]),
            wrapper_id: [wrapper; 32],
            one_use_public_key: [one_use; 32],
            recipient_public_key: [7; 32],
            canonical_event_id: [fact; 32],
            canonical_sha256: Sha256::digest([fact]).into(),
            wrapper_sha256: Sha256::digest(&exact_wire).into(),
            seal_created_at: 10,
            gift_wrap_created_at: 11,
            exact_wire,
        }
    }
}
