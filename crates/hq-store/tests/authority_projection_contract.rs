//! Persisted authority projection contracts.

#![allow(clippy::expect_used)]

use hq_domain::{InstallationId, MailboxAddress, MailboxId};
use hq_reducer::{AuthorityProjection, AuthorityProjectionKey};
use hq_store::StoreErrorClass;
use rusqlite::Connection;

mod support;

use support::{
    TestDirectory, TestStoreExt, authority_policy, open_store, verified_child, verified_fact,
    verified_fact_with_label,
};

#[test]
fn repair_persists_the_exact_typed_authority_report_and_reopens() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("root appends");
    store
        .append_verified(verified_child(root_id))
        .expect("mailbox appends");

    let ingested = store
        .load_authority_snapshot()
        .expect("ingest materializes authority rows");
    assert_eq!(
        ingested,
        store
            .complete_snapshot(authority_policy())
            .expect("complete oracle succeeds")
            .authority_projection_snapshot()
    );
    let repaired = store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(
        repaired.complete().authority_projection_snapshot(),
        *repaired.authority()
    );
    assert_eq!(
        repaired
            .authority()
            .projection(AuthorityProjectionKey::Installation(
                InstallationId::from_bytes([0x11; 32])
            )),
        repaired
            .complete()
            .authority()
            .projections()
            .get(&AuthorityProjectionKey::Installation(
                InstallationId::from_bytes([0x11; 32])
            ))
    );
    assert!(matches!(
        repaired
            .authority()
            .projection(AuthorityProjectionKey::Mailbox(MailboxAddress::new(
                InstallationId::from_bytes([0x11; 32]),
                MailboxId::from_bytes([0x33; 32]),
            ))),
        Some(AuthorityProjection::Mailbox(_))
    ));
    let expected = repaired.authority().clone();
    assert_eq!(
        store
            .repair(authority_policy())
            .expect("repeated repair succeeds")
            .authority(),
        &expected
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_authority_snapshot()
            .expect("authority snapshot reopens"),
        expected
    );
}

#[test]
fn authority_snapshot_changes_atomically_on_ingest_and_repair_is_equal() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    store
        .append_verified(verified_fact_with_label("alpha", [7; 32]))
        .expect("first root appends");
    let before = store
        .load_authority_snapshot()
        .expect("initial authority loads");
    store
        .append_verified(verified_fact_with_label("beta", [8; 32]))
        .expect("conflicting root appends");

    let after = store
        .load_authority_snapshot()
        .expect("ingest updates authority snapshot");
    assert_ne!(after, before);
    assert!(after.projections().is_empty());
    assert_eq!(
        store
            .repair(authority_policy())
            .expect("repair succeeds")
            .authority(),
        &after
    );
}

#[test]
fn repair_replaces_authority_rows_under_the_new_explicit_policy() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    store
        .append_verified(verified_fact())
        .expect("root appends");
    store
        .repair(authority_policy())
        .expect("first repair succeeds");
    let replacement_policy = hq_reducer::AuthorityPolicy::new(
        InstallationId::from_bytes([0x22; 32]),
        MailboxId::from_bytes([0x44; 32]),
    );

    let replacement = store
        .repair(replacement_policy)
        .expect("replacement repair succeeds");
    assert_eq!(replacement.complete().policy(), replacement_policy);
    assert_eq!(
        replacement.complete().authority_projection_snapshot(),
        *replacement.authority()
    );
    assert_eq!(
        store
            .load_authority_snapshot()
            .expect("replacement authority loads"),
        *replacement.authority()
    );
}

#[test]
fn authority_corruption_fails_closed_until_explicit_repair() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    store
        .append_verified(verified_fact())
        .expect("root appends");
    store.repair(authority_policy()).expect("repair succeeds");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("corruption connection opens");
    connection
        .execute("UPDATE authority_installations SET label = 'changed'", [])
        .expect("valid-looking corruption writes");
    drop(connection);

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_authority_snapshot()
            .expect_err("changed projection rejects")
            .class(),
        StoreErrorClass::RebuildableStateCorrupt
    );
    let repaired = reopened
        .repair(authority_policy())
        .expect("repair recovers authority rows");
    assert_eq!(
        reopened
            .load_authority_snapshot()
            .expect("authority snapshot reloads"),
        *repaired.authority()
    );
}
