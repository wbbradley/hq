//! Durable operational-state contracts across repair and process restart.

#![allow(clippy::expect_used)]

use hq_domain::{CommandId, InstallationId, Revision};
use hq_store::{MutationResultKind, StoreErrorClass};
use rusqlite::{Connection, params};

mod support;

use support::{TestDirectory, TestStoreExt, authority_policy, open_store, verified_fact};

#[test]
fn receipts_revisions_and_exact_outbox_bytes_survive_repair_and_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let fact = verified_fact();
    let fact_id = fact.fact().id();
    let event_bytes = fact.verified_event().exact_event_bytes().to_vec();
    let store = open_store(&database);
    store.append_verified(fact).expect("fact appends");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("fixture database opens");
    connection
        .execute(
            "UPDATE change_revision SET revision = ?1 WHERE singleton = 1",
            [u64::MAX.to_be_bytes().as_slice()],
        )
        .expect("maximum revision stores");
    connection
        .execute(
            "INSERT INTO mutation_receipts(\
                command_id, request_digest, result_kind, result_bytes, revision\
             ) VALUES (?1, ?2, 1, ?3, ?4)",
            params![
                [0x31_u8; 32].as_slice(),
                [0x32_u8; 32].as_slice(),
                b"exact-result".as_slice(),
                u64::MAX.to_be_bytes().as_slice(),
            ],
        )
        .expect("receipt fixture stores");
    connection
        .execute(
            "INSERT INTO outbox_intents(\
                fact_id, recipient_installation, exact_canonical_bytes, revision\
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                fact_id.as_bytes().as_slice(),
                [0x41_u8; 32].as_slice(),
                event_bytes.as_slice(),
                u64::MAX.to_be_bytes().as_slice(),
            ],
        )
        .expect("outbox fixture stores");
    drop(connection);

    let store = open_store(&database);
    assert_eq!(
        store.current_revision().expect("revision loads"),
        Revision::new(u64::MAX)
    );
    let receipt = store
        .load_mutation_receipt(CommandId::from_bytes([0x31; 32]))
        .expect("receipt query succeeds")
        .expect("receipt exists");
    assert_eq!(receipt.result_kind(), MutationResultKind::Committed);
    assert_eq!(receipt.result().as_bytes(), b"exact-result");
    assert_eq!(receipt.revision(), Revision::new(u64::MAX));
    let intents = store.load_outbox_intents(1).expect("outbox query succeeds");
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].fact_id(), fact_id);
    assert_eq!(
        intents[0].recipient(),
        InstallationId::from_bytes([0x41; 32])
    );
    assert_eq!(intents[0].exact_canonical_bytes(), event_bytes);

    store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(
        store.current_revision().expect("revision survives repair"),
        Revision::new(u64::MAX)
    );
    assert_eq!(
        store
            .load_mutation_receipt(CommandId::from_bytes([0x31; 32]))
            .expect("receipt survives repair"),
        Some(receipt)
    );
    assert_eq!(
        store
            .load_outbox_intents(1)
            .expect("outbox survives repair"),
        intents
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened.current_revision().expect("revision reopens"),
        Revision::new(u64::MAX)
    );
    assert_eq!(
        reopened.load_outbox_intents(1).expect("outbox reopens"),
        intents
    );
}

#[test]
fn public_outbox_query_rejects_unbounded_work() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    for invalid in [0, hq_store::MAX_OUTBOX_QUERY_ITEMS + 1] {
        assert_eq!(
            store
                .load_outbox_intents(invalid)
                .expect_err("invalid limit rejects")
                .class(),
            StoreErrorClass::InvalidOperationalRequest
        );
    }
}
