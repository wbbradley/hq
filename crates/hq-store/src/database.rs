//! Private SQLite schema, row codecs, and transactions.

use std::{path::Path, time::Duration};

use hq_domain::{AuthorityRole, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS};
use hq_protocol::{
    DispatchOutcome, MAX_EVENT_BYTES, ProtocolNamespace, RawEventBytes, VerifiedSemanticFact,
};
use rusqlite::{
    Connection, Error as SqlError, ErrorCode, OpenFlags, OptionalExtension, Transaction,
    TransactionBehavior, config::DbConfig, params,
};

use crate::{
    AppendOutcome, StoreError, StoreErrorClass,
    paths::{prepare_database_path, validate_database_path},
};

const APPLICATION_ID: i64 = 0x4851_5253;
const SCHEMA_VERSION: i64 = 1;
const SCHEMA_MARKER: &str = "hq-store-v1-corpus-2026-08-27";
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
    if table_count != 4 {
        return Err(StoreError::new(StoreErrorClass::IncompatibleSchema));
    }
    for table in [
        "storage_metadata",
        "canonical_facts",
        "fact_parents",
        "fact_authorities",
    ] {
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
