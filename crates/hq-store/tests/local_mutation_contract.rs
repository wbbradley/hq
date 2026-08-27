//! Public local fact-backed mutation, reconciliation, and common-engine contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use hq_domain::{CommandDigest, CommandId, Revision};
use hq_protocol::CanonicalEventPlan;
use hq_store::{
    LocalMutationDecision, LocalMutationRequest, MutationResultBytes, MutationResultKind, Store,
    StoreErrorClass,
};

mod support;

use support::{
    TestDirectory, authority_policy, open_store, signer, verified_fact, verified_question,
};

#[test]
fn committed_retry_conflict_and_remote_ingest_share_one_canonical_engine() {
    let local_directory = TestDirectory::new();
    let remote_directory = TestDirectory::new();
    let (local, invalidations) = Store::open_with_invalidations(
        local_directory.database_path(),
        NonZeroUsize::new(4).expect("capacity is nonzero"),
    )
    .expect("local store opens");
    let calls = Arc::new(AtomicUsize::new(0));

    let receipt = local
        .execute_local_mutation(committed_request(
            [0x41; 32],
            [0x51; 32],
            Arc::clone(&calls),
        ))
        .expect("local mutation commits");
    assert_eq!(receipt.result_kind(), MutationResultKind::Committed);
    assert_eq!(receipt.result().as_bytes(), b"created");
    assert_eq!(receipt.revision(), Revision::new(1));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(invalidations.try_revision(), Some(Revision::new(1)));
    assert_eq!(invalidations.try_revision(), None);

    let replay = local
        .execute_local_mutation(committed_request(
            [0x41; 32],
            [0x51; 32],
            Arc::clone(&calls),
        ))
        .expect("exact retry returns retained result");
    assert_eq!(replay, receipt);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(local.current_revision(), Ok(Revision::new(1)));
    assert_eq!(invalidations.try_revision(), None);

    let conflict = local
        .execute_local_mutation(committed_request(
            [0x41; 32],
            [0x52; 32],
            Arc::clone(&calls),
        ))
        .expect_err("changed input under one ID conflicts");
    assert_eq!(conflict.class(), StoreErrorClass::MutationConflict);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    local.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(
        local.load_mutation_receipt(CommandId::from_bytes([0x41; 32])),
        Ok(Some(receipt.clone()))
    );
    assert_eq!(local.current_revision(), Ok(Revision::new(1)));

    let remote = open_store(&remote_directory.database_path());
    remote
        .ingest_verified(verified_fact(), authority_policy())
        .expect("same exact event ingests remotely");
    assert_eq!(exact_events(&local), exact_events(&remote));
    assert_eq!(local.load_reduction_index(), remote.load_reduction_index());
    assert_eq!(
        local.load_authority_snapshot(),
        remote.load_authority_snapshot()
    );
    assert_eq!(
        local.load_conversation_snapshot(),
        remote.load_conversation_snapshot()
    );
    assert_eq!(local.load_agent_snapshot(), remote.load_agent_snapshot());
    assert_eq!(
        local.load_project_snapshot(),
        remote.load_project_snapshot()
    );
    assert_eq!(
        local.load_outbox_intents(16),
        remote.load_outbox_intents(16)
    );
    assert_eq!(local.current_revision(), remote.current_revision());
}

#[test]
fn rejection_is_atomic_replayable_and_signing_failure_leaves_no_trace() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let (store, invalidations) = Store::open_with_invalidations(
        &database,
        NonZeroUsize::new(4).expect("capacity is nonzero"),
    )
    .expect("store opens");
    let rejected = || {
        LocalMutationRequest::new(
            CommandId::from_bytes([0x61; 32]),
            CommandDigest::from_bytes([0x62; 32]),
            authority_policy(),
            Arc::new(signer(1)),
            |_| {
                LocalMutationDecision::reject(
                    MutationResultBytes::new(b"not-authorized".to_vec())
                        .expect("result is bounded"),
                )
            },
        )
    };
    let receipt = store
        .execute_local_mutation(rejected())
        .expect("rejection receipt commits");
    assert_eq!(receipt.result_kind(), MutationResultKind::Rejected);
    assert_eq!(receipt.result().as_bytes(), b"not-authorized");
    assert_eq!(receipt.revision(), Revision::new(1));
    assert!(store.load_corpus().expect("corpus loads").is_empty());
    assert_eq!(invalidations.try_revision(), Some(Revision::new(1)));
    assert_eq!(
        store.execute_local_mutation(rejected()),
        Ok(receipt.clone())
    );
    assert_eq!(invalidations.try_revision(), None);

    let invalid_plan = CanonicalEventPlan::from_fact(verified_fact().fact());
    let error = store
        .execute_local_mutation(LocalMutationRequest::new(
            CommandId::from_bytes([0x71; 32]),
            CommandDigest::from_bytes([0x72; 32]),
            authority_policy(),
            Arc::new(signer(2)),
            |_| {
                LocalMutationDecision::commit(
                    invalid_plan,
                    [7; 32],
                    MutationResultBytes::new(b"impossible".to_vec()).expect("result is bounded"),
                )
            },
        ))
        .expect_err("signer and typed declaration mismatch");
    assert_eq!(error.class(), StoreErrorClass::InvalidOperationalRequest);
    assert_eq!(store.current_revision(), Ok(Revision::new(1)));
    assert_eq!(
        store.load_mutation_receipt(CommandId::from_bytes([0x71; 32])),
        Ok(None)
    );
    assert_eq!(invalidations.try_revision(), None);

    let unresolved_plan = CanonicalEventPlan::from_fact(verified_question([0x99; 32]).fact());
    let error = store
        .execute_local_mutation(LocalMutationRequest::new(
            CommandId::from_bytes([0x73; 32]),
            CommandDigest::from_bytes([0x74; 32]),
            authority_policy(),
            Arc::new(signer(1)),
            |_| {
                LocalMutationDecision::commit(
                    unresolved_plan,
                    [10; 32],
                    MutationResultBytes::new(b"unresolved".to_vec()).expect("result is bounded"),
                )
            },
        ))
        .expect_err("local committed facts must be admitted");
    assert_eq!(error.class(), StoreErrorClass::InvalidOperationalRequest);
    assert_eq!(store.current_revision(), Ok(Revision::new(1)));
    assert_eq!(
        store.load_mutation_receipt(CommandId::from_bytes([0x73; 32])),
        Ok(None)
    );
    assert!(
        store
            .load_corpus()
            .expect("corpus remains empty")
            .is_empty()
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(reopened.execute_local_mutation(rejected()), Ok(receipt));
    assert_eq!(reopened.current_revision(), Ok(Revision::new(1)));
    assert!(reopened.load_corpus().expect("corpus reopens").is_empty());
}

fn committed_request(
    command: [u8; 32],
    digest: [u8; 32],
    calls: Arc<AtomicUsize>,
) -> LocalMutationRequest {
    let plan = CanonicalEventPlan::from_fact(verified_fact().fact());
    LocalMutationRequest::new(
        CommandId::from_bytes(command),
        CommandDigest::from_bytes(digest),
        authority_policy(),
        Arc::new(signer(1)),
        move |snapshot| {
            assert_eq!(snapshot.policy(), authority_policy());
            calls.fetch_add(1, Ordering::SeqCst);
            LocalMutationDecision::commit(
                plan,
                [7; 32],
                MutationResultBytes::new(b"created".to_vec()).expect("result is bounded"),
            )
        },
    )
}

fn exact_events(store: &Store) -> Vec<Vec<u8>> {
    store
        .load_corpus()
        .expect("corpus loads")
        .iter()
        .map(|fact| fact.verified_event().exact_event_bytes().to_vec())
        .collect()
}
