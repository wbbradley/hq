//! Public immutable-corpus and actor-lifecycle contracts.

#![allow(clippy::expect_used, clippy::panic)]

use hq_domain::{FactKind, Revision};
use hq_store::{IngestOutcome, StoreErrorClass};
use rusqlite::Connection;

mod support;

use support::{
    TestDirectory, TestStoreExt, mode, open_store, verified_child, verified_fact,
    verified_fact_with_auxiliary,
};

#[test]
fn store_owner_is_safe_to_share_by_reference() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<hq_store::Store>();
}

#[test]
fn verified_facts_survive_close_and_reopen_with_exact_evidence() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let expected = verified_fact();
    let expected_id = expected.verified_event().event_id();
    let expected_event = expected.verified_event().exact_event_bytes().to_vec();

    let store = open_store(&database);
    assert_eq!(
        store.append_verified(expected),
        Ok(IngestOutcome::Inserted(Revision::new(1)))
    );
    assert_eq!(store.close(), Ok(()));

    let reopened = open_store(&database);
    let corpus = reopened.load_corpus().expect("corpus reloads");
    assert_eq!(corpus.len(), 1);
    let loaded = corpus.iter().next().expect("one fact is present");
    assert_eq!(loaded.fact().kind(), FactKind::InstallationDeclared);
    assert_eq!(loaded.verified_event().event_id(), expected_id);
    assert_eq!(loaded.verified_event().exact_event_bytes(), expected_event);
    assert_eq!(loaded.fact().id().as_bytes(), &expected_id);
    reopened.close().expect("store closes");
}

#[test]
fn equal_duplicate_ingest_is_idempotent() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());

    assert_eq!(
        store.append_verified(verified_fact()),
        Ok(IngestOutcome::Inserted(Revision::new(1)))
    );
    assert_eq!(
        store.append_verified(verified_fact()),
        Ok(IngestOutcome::AlreadyPresent(Revision::new(1)))
    );
    assert_eq!(store.load_corpus().expect("corpus loads").len(), 1);
}

#[test]
fn same_event_identity_with_a_different_valid_signature_fails_closed() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let first = verified_fact_with_auxiliary([7; 32]);
    let unequal = verified_fact_with_auxiliary([8; 32]);
    assert_eq!(
        first.verified_event().event_id(),
        unequal.verified_event().event_id()
    );
    assert_ne!(
        first.verified_event().exact_event_bytes(),
        unequal.verified_event().exact_event_bytes()
    );

    assert_eq!(
        store.append_verified(first),
        Ok(IngestOutcome::Inserted(Revision::new(1)))
    );
    let error = store
        .append_verified(unequal)
        .expect_err("unequal immutable evidence rejects");
    assert_eq!(error.class(), StoreErrorClass::IdentityCollision);
    assert_eq!(store.load_corpus().expect("corpus remains valid").len(), 1);
}

#[test]
fn normalized_parent_and_authority_indexes_survive_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let child = verified_child(root_id);
    let child_id = child.verified_event().event_id();
    let store = open_store(&database);
    store
        .append_verified(child)
        .expect("child may arrive first");
    store.append_verified(root).expect("late parent appends");
    store.close().expect("store closes");

    let reopened = open_store(&database);
    let corpus = reopened.load_corpus().expect("indexed corpus reloads");
    assert_eq!(corpus.len(), 2);
    let loaded_child = corpus
        .iter()
        .find(|fact| fact.verified_event().event_id() == child_id)
        .expect("child reloads");
    assert!(
        loaded_child
            .fact()
            .causal()
            .parents()
            .contains(&hq_domain::FactId::from_bytes(root_id))
    );
    assert_eq!(
        loaded_child
            .fact()
            .causal()
            .authority(hq_domain::AuthorityRole::LocalInstallation),
        Some(hq_domain::FactId::from_bytes(root_id))
    );
}

#[test]
fn dropping_the_owner_stops_the_worker_and_allows_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    {
        let store = open_store(&database);
        store
            .append_verified(verified_fact())
            .expect("append succeeds");
    }

    let reopened = open_store(&database);
    assert_eq!(reopened.load_corpus().expect("corpus reloads").len(), 1);
}

#[cfg(unix)]
#[test]
fn new_state_and_database_are_private_at_creation() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);

    assert_eq!(mode(database.parent().expect("database has parent")), 0o700);
    assert_eq!(mode(&database), 0o600);
    store.close().expect("store closes");
}

#[test]
fn fresh_schema_has_the_exact_storage_identity_and_wal_mode() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("store initializes");

    let connection = Connection::open(&database).expect("test inspection opens");
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .expect("application ID reads");
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user version reads");
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("journal mode reads");
    assert_eq!(application_id, 0x4851_5253);
    assert_eq!(user_version, 10);
    assert_eq!(journal_mode, "wal");
}
