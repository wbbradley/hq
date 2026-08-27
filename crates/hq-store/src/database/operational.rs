//! Durable non-rebuildable receipt, revision, and outbox codecs.

use hq_domain::{CommandDigest, CommandId, FactId, InstallationId, Revision};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{
    MAX_OUTBOX_QUERY_ITEMS, MutationReceipt, MutationResultBytes, MutationResultKind, OutboxIntent,
    StoreError, StoreErrorClass,
};

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PutOutcome {
    Inserted,
    AlreadyPresent,
}

pub(crate) fn current_revision(connection: &Connection) -> Result<Revision, StoreError> {
    let bytes = connection
        .query_row(
            "SELECT revision FROM change_revision WHERE singleton = 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(database)?;
    decode_revision(bytes)
}

pub(crate) fn canonical_commit_revision(
    connection: &Connection,
    fact_id: FactId,
) -> Result<Option<Revision>, StoreError> {
    connection
        .query_row(
            "SELECT revision FROM canonical_commits WHERE fact_id = ?1",
            [fact_id.as_bytes().as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(database)?
        .map(decode_revision)
        .transpose()
}

pub(crate) fn put_canonical_commit(
    transaction: &Transaction<'_>,
    fact_id: FactId,
    revision: Revision,
) -> Result<PutOutcome, StoreError> {
    if let Some(stored) = canonical_commit_revision(transaction, fact_id)? {
        return if stored == revision {
            Ok(PutOutcome::AlreadyPresent)
        } else {
            Err(StoreError::new(StoreErrorClass::OperationalStateCorrupt))
        };
    }
    transaction
        .execute(
            "INSERT INTO canonical_commits(fact_id, revision) VALUES (?1, ?2)",
            params![
                fact_id.as_bytes().as_slice(),
                revision.value().to_be_bytes().as_slice()
            ],
        )
        .map_err(database)?;
    Ok(PutOutcome::Inserted)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn allocate_revision(transaction: &Transaction<'_>) -> Result<Revision, StoreError> {
    let current = current_revision(transaction)?.value();
    let next = current
        .checked_add(1)
        .ok_or_else(|| StoreError::new(StoreErrorClass::RevisionExhausted))?;
    let changed = transaction
        .execute(
            "UPDATE change_revision SET revision = ?1 WHERE singleton = 1",
            [next.to_be_bytes().as_slice()],
        )
        .map_err(database)?;
    if changed != 1 {
        return Err(corrupt());
    }
    Ok(Revision::new(next))
}

pub(crate) fn load_receipt(
    connection: &Connection,
    command_id: CommandId,
) -> Result<Option<MutationReceipt>, StoreError> {
    let row = connection
        .query_row(
            "SELECT request_digest, result_kind, result_bytes, revision \
             FROM mutation_receipts WHERE command_id = ?1",
            [command_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(database)?;
    row.map(|(digest, kind, result, revision)| {
        Ok(MutationReceipt::new(
            command_id,
            CommandDigest::from_bytes(fixed::<32>(digest)?),
            decode_result_kind(kind)?,
            MutationResultBytes::new(result).map_err(|_| corrupt())?,
            decode_revision(revision)?,
        ))
    })
    .transpose()
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn put_receipt(
    transaction: &Transaction<'_>,
    receipt: &MutationReceipt,
) -> Result<PutOutcome, StoreError> {
    if let Some(stored) = load_receipt(transaction, receipt.command_id())? {
        return if stored == *receipt {
            Ok(PutOutcome::AlreadyPresent)
        } else {
            Err(StoreError::new(StoreErrorClass::MutationConflict))
        };
    }
    transaction
        .execute(
            "INSERT INTO mutation_receipts(\
                command_id, request_digest, result_kind, result_bytes, revision\
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                receipt.command_id().as_bytes().as_slice(),
                receipt.request_digest().as_bytes().as_slice(),
                encode_result_kind(receipt.result_kind()),
                receipt.result().as_bytes(),
                receipt.revision().value().to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(PutOutcome::Inserted)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn put_outbox_intent(
    transaction: &Transaction<'_>,
    intent: &OutboxIntent,
) -> Result<PutOutcome, StoreError> {
    let stored = transaction
        .query_row(
            "SELECT exact_canonical_bytes, revision FROM outbox_intents \
             WHERE fact_id = ?1 AND recipient_installation = ?2",
            params![
                intent.fact_id().as_bytes().as_slice(),
                intent.recipient().as_bytes().as_slice()
            ],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(database)?;
    if let Some((bytes, revision)) = stored {
        return if bytes == intent.exact_canonical_bytes()
            && decode_revision(revision)? == intent.revision()
        {
            Ok(PutOutcome::AlreadyPresent)
        } else {
            Err(StoreError::new(StoreErrorClass::IdentityCollision))
        };
    }
    transaction
        .execute(
            "INSERT INTO outbox_intents(\
                fact_id, recipient_installation, exact_canonical_bytes, revision\
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                intent.fact_id().as_bytes().as_slice(),
                intent.recipient().as_bytes().as_slice(),
                intent.exact_canonical_bytes(),
                intent.revision().value().to_be_bytes().as_slice(),
            ],
        )
        .map_err(database)?;
    Ok(PutOutcome::Inserted)
}

pub(crate) fn load_outbox_intents(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<OutboxIntent>, StoreError> {
    if limit == 0 || limit > MAX_OUTBOX_QUERY_ITEMS {
        return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
    }
    let sql_limit = i64::try_from(limit)
        .map_err(|_| StoreError::new(StoreErrorClass::InvalidOperationalRequest))?;
    let mut statement = connection
        .prepare(
            "SELECT fact_id, recipient_installation, exact_canonical_bytes, revision \
             FROM outbox_intents ORDER BY revision, fact_id, recipient_installation LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([sql_limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(database)?;
    let mut intents = Vec::new();
    for row in rows {
        let (fact, recipient, bytes, revision) = row.map_err(database)?;
        intents.push(
            OutboxIntent::new(
                FactId::from_bytes(fixed::<32>(fact)?),
                InstallationId::from_bytes(fixed::<32>(recipient)?),
                bytes,
                decode_revision(revision)?,
            )
            .map_err(|_| corrupt())?,
        );
    }
    Ok(intents)
}

#[cfg_attr(not(test), allow(dead_code))]
const fn encode_result_kind(kind: MutationResultKind) -> i64 {
    match kind {
        MutationResultKind::Committed => 1,
        MutationResultKind::Rejected => 2,
    }
}

fn decode_result_kind(value: i64) -> Result<MutationResultKind, StoreError> {
    match value {
        1 => Ok(MutationResultKind::Committed),
        2 => Ok(MutationResultKind::Rejected),
        _ => Err(corrupt()),
    }
}

fn decode_revision(bytes: Vec<u8>) -> Result<Revision, StoreError> {
    Ok(Revision::new(u64::from_be_bytes(fixed::<8>(bytes)?)))
}

fn fixed<const SIZE: usize>(bytes: Vec<u8>) -> Result<[u8; SIZE], StoreError> {
    bytes.try_into().map_err(|_| corrupt())
}

fn database(_: rusqlite::Error) -> StoreError {
    StoreError::new(StoreErrorClass::DatabaseUnavailable)
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

    fn receipt(revision: u64) -> MutationReceipt {
        MutationReceipt::new(
            CommandId::from_bytes([1; 32]),
            CommandDigest::from_bytes([2; 32]),
            MutationResultKind::Committed,
            MutationResultBytes::new([3, 4]).expect("result is bounded"),
            Revision::new(revision),
        )
    }

    #[test]
    fn revisions_round_trip_the_full_u64_domain_and_exhaust_cleanly() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        connection.execute_batch(SCHEMA).expect("schema creates");
        connection
            .execute(
                "UPDATE change_revision SET revision = ?1 WHERE singleton = 1",
                [(u64::MAX - 1).to_be_bytes().as_slice()],
            )
            .expect("near-maximum revision stores");
        let transaction = connection.transaction().expect("transaction starts");
        assert_eq!(
            allocate_revision(&transaction).expect("maximum allocates"),
            Revision::new(u64::MAX)
        );
        transaction.commit().expect("transaction commits");
        assert_eq!(
            current_revision(&connection).expect("maximum reloads"),
            Revision::new(u64::MAX)
        );
        let transaction = connection.transaction().expect("transaction starts");
        assert_eq!(
            allocate_revision(&transaction)
                .expect_err("overflow rejects")
                .class(),
            StoreErrorClass::RevisionExhausted
        );
    }

    #[test]
    fn receipts_round_trip_and_conflict_on_any_unequal_reuse() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        connection.execute_batch(SCHEMA).expect("schema creates");
        let expected = receipt(u64::MAX);
        let transaction = connection.transaction().expect("transaction starts");
        assert_eq!(
            put_receipt(&transaction, &expected).expect("receipt stores"),
            PutOutcome::Inserted
        );
        transaction.commit().expect("transaction commits");
        assert_eq!(
            load_receipt(&connection, expected.command_id()).expect("receipt loads"),
            Some(expected.clone())
        );
        let transaction = connection.transaction().expect("transaction starts");
        assert_eq!(
            put_receipt(&transaction, &expected).expect("same receipt deduplicates"),
            PutOutcome::AlreadyPresent
        );
        let changed = receipt(u64::MAX - 1);
        assert_eq!(
            put_receipt(&transaction, &changed)
                .expect_err("changed receipt conflicts")
                .class(),
            StoreErrorClass::MutationConflict
        );
    }

    #[test]
    fn outbox_intents_retain_exact_bytes_and_reject_unequal_identity_reuse() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        connection.execute_batch(SCHEMA).expect("schema creates");
        let fact = crate::database::tests::fixture();
        crate::database::append_with_failpoint(
            &mut connection,
            &fact,
            crate::database::Failpoint::Never,
        )
        .expect("canonical fact stores");
        let expected = OutboxIntent::new(
            fact.fact().id(),
            InstallationId::from_bytes([9; 32]),
            fact.verified_event().exact_event_bytes().to_vec(),
            Revision::new(u64::MAX),
        )
        .expect("intent validates");
        let transaction = connection.transaction().expect("transaction starts");
        assert_eq!(
            put_outbox_intent(&transaction, &expected).expect("intent stores"),
            PutOutcome::Inserted
        );
        transaction.commit().expect("transaction commits");
        assert_eq!(
            load_outbox_intents(&connection, 1).expect("intent reloads"),
            vec![expected.clone()]
        );
        let changed = OutboxIntent::new(
            expected.fact_id(),
            expected.recipient(),
            vec![1],
            expected.revision(),
        )
        .expect("changed intent validates");
        let transaction = connection.transaction().expect("transaction starts");
        assert_eq!(
            put_outbox_intent(&transaction, &changed)
                .expect_err("changed identity conflicts")
                .class(),
            StoreErrorClass::IdentityCollision
        );
    }

    #[test]
    fn outbox_queries_are_explicitly_bounded() {
        let connection = Connection::open_in_memory().expect("database opens");
        connection.execute_batch(SCHEMA).expect("schema creates");
        for invalid in [0, MAX_OUTBOX_QUERY_ITEMS + 1] {
            assert_eq!(
                load_outbox_intents(&connection, invalid)
                    .expect_err("invalid limit rejects")
                    .class(),
                StoreErrorClass::InvalidOperationalRequest
            );
        }
    }

    #[test]
    fn result_kind_codec_is_closed() {
        assert_eq!(decode_result_kind(1), Ok(MutationResultKind::Committed));
        assert_eq!(decode_result_kind(2), Ok(MutationResultKind::Rejected));
        for invalid in [i64::MIN, 0, 3, i64::MAX] {
            assert_eq!(
                decode_result_kind(invalid)
                    .expect_err("unknown kind rejects")
                    .class(),
                StoreErrorClass::OperationalStateCorrupt
            );
        }
    }
}
