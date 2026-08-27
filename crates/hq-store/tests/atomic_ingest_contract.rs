//! Public common-ingest transaction, replay, fanout, and invalidation contracts.

#![allow(clippy::expect_used)]

use std::num::NonZeroUsize;

use hq_domain::{InstallationId, Revision};
use hq_store::{IngestOutcome, Store};

mod support;

use support::{
    TestDirectory, authority_policy, open_store, verified_account, verified_device_acceptance,
    verified_device_grant, verified_fact, verified_project, verified_question,
};

#[test]
fn ingest_materializes_every_package_and_duplicate_replay_is_an_exact_noop() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let (store, invalidations) = Store::open_with_invalidations(
        &database,
        NonZeroUsize::new(4).expect("capacity is nonzero"),
    )
    .expect("store opens");
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    assert_eq!(
        store.ingest_verified(root, authority_policy()),
        Ok(IngestOutcome::Inserted(Revision::new(1)))
    );
    assert_eq!(
        store.ingest_verified(verified_question(root_id), authority_policy()),
        Ok(IngestOutcome::Inserted(Revision::new(2)))
    );

    assert_eq!(invalidations.try_revision(), Some(Revision::new(2)));
    assert_eq!(invalidations.try_revision(), None);
    let complete = store
        .complete_snapshot(authority_policy())
        .expect("complete oracle succeeds");
    assert_eq!(
        store
            .load_reduction_index()
            .expect("atomic index is visible"),
        complete.normalized_index()
    );
    assert_eq!(
        store
            .load_authority_snapshot()
            .expect("atomic authority is visible"),
        complete.authority_projection_snapshot()
    );
    assert_eq!(
        store
            .load_conversation_snapshot()
            .expect("atomic conversation is visible"),
        complete.conversation_projection_snapshot()
    );
    assert_eq!(
        store
            .load_agent_snapshot()
            .expect("atomic agent is visible"),
        complete.agent_projection_snapshot()
    );
    assert_eq!(
        store
            .load_project_snapshot()
            .expect("atomic project is visible"),
        complete.project_projection_snapshot()
    );

    assert_eq!(
        store.ingest_verified(verified_question(root_id), authority_policy()),
        Ok(IngestOutcome::AlreadyPresent(Revision::new(2)))
    );
    assert_eq!(store.current_revision(), Ok(Revision::new(2)));
    assert_eq!(invalidations.try_revision(), None);
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened.ingest_verified(verified_question(root_id), authority_policy()),
        Ok(IngestOutcome::AlreadyPresent(Revision::new(2)))
    );
    assert_eq!(reopened.current_revision(), Ok(Revision::new(2)));
}

#[test]
fn admitted_account_fact_creates_exact_per_recipient_intent_and_repair_preserves_it() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store
        .ingest_verified(root, authority_policy())
        .expect("root ingests");
    let account = verified_account(root_id);
    let account_fact_id = account.verified_event().event_id();
    store
        .ingest_verified(account, authority_policy())
        .expect("account ingests");
    let grant = verified_device_grant(account_fact_id);
    let grant_id = grant.verified_event().event_id();
    store
        .ingest_verified(grant, authority_policy())
        .expect("device grant ingests");
    store
        .ingest_verified(verified_device_acceptance(grant_id), authority_policy())
        .expect("device acceptance ingests");
    let project = verified_project(root_id, account_fact_id);
    let project_id = project.fact().id();
    let exact = project.verified_event().exact_event_bytes().to_vec();
    assert_eq!(
        store.ingest_verified(project, authority_policy()),
        Ok(IngestOutcome::Inserted(Revision::new(5)))
    );

    let intents = store.load_outbox_intents(16).expect("outbox loads");
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].fact_id(), project_id);
    assert_eq!(
        intents[0].recipient(),
        InstallationId::from_bytes([0x22; 32])
    );
    assert_eq!(intents[0].exact_canonical_bytes(), exact);
    assert_eq!(intents[0].revision(), Revision::new(5));
    store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(
        store
            .load_outbox_intents(16)
            .expect("repair preserves outbox"),
        intents
    );
    assert_eq!(store.current_revision(), Ok(Revision::new(5)));
}
