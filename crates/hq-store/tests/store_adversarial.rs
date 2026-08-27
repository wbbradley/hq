//! Filesystem, schema, corruption, and trust-replay failures fail closed.

#![allow(clippy::expect_used)]

use std::{fs, num::NonZeroUsize};

use hq_protocol::Bip340Signer;
use hq_store::{Store, StoreErrorClass};
use rusqlite::Connection;

mod support;

use support::{TestDirectory, open_store, verified_child, verified_fact};

#[test]
fn foreign_sqlite_schema_is_not_opened_or_migrated() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    fs::create_dir_all(database.parent().expect("database parent exists"))
        .expect("state directory creates");
    set_mode(database.parent().expect("database parent exists"), 0o700);
    let connection = Connection::open(&database).expect("foreign database creates");
    connection
        .execute_batch("CREATE TABLE legacy_facts(id TEXT PRIMARY KEY);")
        .expect("foreign schema creates");
    let journal_before: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("foreign journal reads");
    drop(connection);
    set_mode(&database, 0o600);

    let error = Store::open(&database, NonZeroUsize::MIN).expect_err("foreign schema rejects");
    assert_eq!(error.class(), StoreErrorClass::IncompatibleSchema);
    let unchanged = Connection::open(&database).expect("foreign database still opens");
    let journal_after: String = unchanged
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("foreign journal rereads");
    assert_eq!(journal_after, journal_before);
    let legacy_rows: i64 = unchanged
        .query_row("SELECT count(*) FROM legacy_facts", [], |row| row.get(0))
        .expect("foreign table remains");
    assert_eq!(legacy_rows, 0);
}

#[test]
fn wrong_version_or_schema_marker_is_incompatible() {
    for mutation in [
        "PRAGMA user_version = 3",
        "UPDATE storage_metadata SET schema_marker = 'not-hq-store-v2'",
        "CREATE TABLE unexpected_table(value INTEGER)",
    ] {
        let directory = TestDirectory::new();
        let database = directory.database_path();
        open_store(&database).close().expect("store initializes");
        let connection = Connection::open(&database).expect("test mutation connection opens");
        connection
            .execute_batch(mutation)
            .expect("schema metadata mutates");
        drop(connection);

        let error =
            Store::open(&database, NonZeroUsize::MIN).expect_err("incompatible metadata rejects");
        assert_eq!(error.class(), StoreErrorClass::IncompatibleSchema);
    }
}

#[test]
fn relative_database_paths_are_rejected() {
    let error =
        Store::open("relative.sqlite3", NonZeroUsize::MIN).expect_err("relative path rejects");
    assert_eq!(error.class(), StoreErrorClass::InvalidPath);
}

#[test]
fn an_unclaimed_empty_sqlite_file_is_initialized_as_storage_v2() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    fs::create_dir_all(database.parent().expect("database parent exists"))
        .expect("state directory creates");
    set_mode(database.parent().expect("database parent exists"), 0o700);
    drop(Connection::open(&database).expect("empty SQLite database creates"));
    set_mode(&database, 0o600);

    let store = open_store(&database);
    assert!(store.load_corpus().expect("empty corpus loads").is_empty());
}

#[test]
fn corrupt_database_is_classified_without_sqlite_prose() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    fs::create_dir_all(database.parent().expect("database parent exists"))
        .expect("state directory creates");
    set_mode(database.parent().expect("database parent exists"), 0o700);
    fs::write(&database, b"not a sqlite database").expect("corrupt fixture writes");
    set_mode(&database, 0o600);

    let error = Store::open(&database, NonZeroUsize::MIN).expect_err("corruption rejects");
    assert_eq!(error.class(), StoreErrorClass::CorruptDatabase);
    assert_eq!(error.to_string(), "database is corrupt");
}

#[test]
fn tampered_signed_evidence_is_reverified_on_load() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    store
        .append_verified(verified_fact())
        .expect("fixture appends");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("test corruption connection opens");
    connection
        .execute("UPDATE canonical_facts SET event_bytes = x'7b7d'", [])
        .expect("test corrupts evidence");
    drop(connection);

    let reopened = open_store(&database);
    let error = reopened
        .load_corpus()
        .expect_err("tampered evidence rejects");
    assert_eq!(error.class(), StoreErrorClass::InvalidEvidence);
}

#[test]
fn validly_signed_but_unsupported_evidence_cannot_enter_the_corpus() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("store initializes");
    let content = r#"{"p":"hq/canonical","v":2,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"future":null}"#;
    let signer = Bip340Signer::from_secret_bytes({
        let mut secret = [0_u8; 32];
        secret[31] = 1;
        secret
    })
    .expect("fixture secret is valid");
    let event = signer
        .sign(0, content.as_bytes(), [4; 32])
        .expect("unsupported fixture signs");
    let connection = Connection::open(&database).expect("test injection connection opens");
    connection
        .execute(
            "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
             VALUES (?1, ?2, 1, 1)",
            rusqlite::params![event.event_id().as_slice(), event.exact_event_bytes()],
        )
        .expect("unsupported evidence injects");
    drop(connection);

    let reopened = open_store(&database);
    let error = reopened
        .load_corpus()
        .expect_err("unsupported evidence rejects");
    assert_eq!(error.class(), StoreErrorClass::InvalidEvidence);
}

#[test]
fn partial_or_changed_causal_indexes_are_rejected_on_load() {
    for corruption in ["DELETE FROM fact_parents", "DELETE FROM fact_authorities"] {
        let directory = TestDirectory::new();
        let database = directory.database_path();
        let root = verified_fact();
        let child = verified_child(root.verified_event().event_id());
        let store = open_store(&database);
        store.append_verified(root).expect("root appends");
        store.append_verified(child).expect("child appends");
        store.close().expect("store closes");

        let connection = Connection::open(&database).expect("test corruption connection opens");
        connection
            .execute(corruption, [])
            .expect("test corrupts an index");
        drop(connection);

        let reopened = open_store(&database);
        let error = reopened.load_corpus().expect_err("partial index rejects");
        assert_eq!(error.class(), StoreErrorClass::InvalidEvidence);
    }
}

#[test]
fn foreign_key_corruption_is_rejected_during_open() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("store initializes");
    let connection = Connection::open(&database).expect("test corruption connection opens");
    connection
        .pragma_update(None, "foreign_keys", false)
        .expect("test disables foreign keys");
    connection
        .execute(
            "INSERT INTO fact_parents(fact_id, parent_id) VALUES (?1, ?2)",
            rusqlite::params![[1_u8; 32].as_slice(), [2_u8; 32].as_slice()],
        )
        .expect("foreign-key violation injects with test enforcement disabled");
    drop(connection);

    let error =
        Store::open(&database, NonZeroUsize::MIN).expect_err("foreign-key corruption rejects");
    assert_eq!(error.class(), StoreErrorClass::CorruptDatabase);
}

#[cfg(unix)]
#[test]
fn unsafe_directory_permissions_and_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let unsafe_state = directory.path().join("unsafe");
    fs::create_dir(&unsafe_state).expect("unsafe state creates");
    set_mode(&unsafe_state, 0o755);
    let unsafe_error = Store::open(unsafe_state.join("hq.sqlite3"), NonZeroUsize::MIN)
        .expect_err("unsafe directory rejects");
    assert_eq!(unsafe_error.class(), StoreErrorClass::UnsafePermissions);

    let target_state = directory.path().join("target");
    fs::create_dir(&target_state).expect("target state creates");
    set_mode(&target_state, 0o700);
    let linked_state = directory.path().join("linked");
    symlink(&target_state, &linked_state).expect("state symlink creates");
    let symlink_error = Store::open(linked_state.join("hq.sqlite3"), NonZeroUsize::MIN)
        .expect_err("state symlink rejects");
    assert_eq!(symlink_error.class(), StoreErrorClass::SymbolicLink);

    let private_state = directory.path().join("private");
    fs::create_dir(&private_state).expect("private state creates");
    set_mode(&private_state, 0o700);
    let target_database = private_state.join("target.sqlite3");
    fs::write(&target_database, []).expect("target database creates");
    set_mode(&target_database, 0o600);
    let linked_database = private_state.join("hq.sqlite3");
    symlink(&target_database, &linked_database).expect("database symlink creates");
    let database_symlink_error =
        Store::open(&linked_database, NonZeroUsize::MIN).expect_err("database symlink rejects");
    assert_eq!(
        database_symlink_error.class(),
        StoreErrorClass::SymbolicLink
    );

    let unsafe_database = private_state.join("unsafe.sqlite3");
    fs::write(&unsafe_database, []).expect("unsafe database creates");
    set_mode(&unsafe_database, 0o644);
    let unsafe_database_error =
        Store::open(&unsafe_database, NonZeroUsize::MIN).expect_err("unsafe database mode rejects");
    assert_eq!(
        unsafe_database_error.class(),
        StoreErrorClass::UnsafePermissions
    );

    let sidecar_database = private_state.join("sidecar.sqlite3");
    fs::write(&sidecar_database, []).expect("sidecar database creates");
    set_mode(&sidecar_database, 0o600);
    let sidecar_target = private_state.join("sidecar-target");
    fs::write(&sidecar_target, []).expect("sidecar target creates");
    let mut sidecar_link = sidecar_database.as_os_str().to_owned();
    sidecar_link.push("-wal");
    let sidecar_link = std::path::PathBuf::from(sidecar_link);
    symlink(&sidecar_target, &sidecar_link).expect("sidecar symlink creates");
    let sidecar_error =
        Store::open(&sidecar_database, NonZeroUsize::MIN).expect_err("sidecar symlink rejects");
    assert_eq!(sidecar_error.class(), StoreErrorClass::SymbolicLink);
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode sets");
}

#[cfg(not(unix))]
fn set_mode(_: &std::path::Path, _: u32) {}
