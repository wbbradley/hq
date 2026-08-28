//! Durable managed-runtime ownership and recovery ledgers.

#![allow(clippy::expect_used)]

use hq_domain::{
    AgentId, CommandDigest, ContentText, MessageId, OperationId, ProviderId, ProviderSessionId,
};
use hq_store::{
    HarnessLeaseOutcome, StoreErrorClass, StoredHarnessDelivery, StoredHarnessDeliveryState,
    StoredHarnessEventCheckpoint, StoredHarnessSessionOperation, StoredHarnessSessionOperationKind,
    StoredHarnessSessionOperationState, StoredHarnessStateMutation,
};

mod support;

use support::{TestDirectory, authority_policy, open_store};

#[test]
fn exact_lease_owner_renews_releases_and_expires_without_stale_interference() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let agent = AgentId::from_bytes([1; 32]);
    let owner_a = [2; 32];
    let owner_b = [3; 32];

    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::ClaimLease {
                agent_id: agent,
                owner_token: owner_a,
                now_millis: 10,
                expires_at_millis: 20,
            })
            .expect("first lease commits"),
        HarnessLeaseOutcome::Acquired
    );
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::ClaimLease {
                agent_id: agent,
                owner_token: owner_b,
                now_millis: 19,
                expires_at_millis: 30,
            })
            .expect("live competing lease is observed"),
        HarnessLeaseOutcome::Held
    );
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::ClaimLease {
                agent_id: agent,
                owner_token: owner_b,
                now_millis: 20,
                expires_at_millis: 30,
            })
            .expect("expired lease is replaced"),
        HarnessLeaseOutcome::Acquired
    );
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::ReleaseLease {
                agent_id: agent,
                owner_token: owner_a,
            })
            .expect("stale release is harmless"),
        HarnessLeaseOutcome::Held
    );
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::ReleaseLease {
                agent_id: agent,
                owner_token: owner_b,
            })
            .expect("exact release commits"),
        HarnessLeaseOutcome::Released
    );
}

#[test]
fn delivery_and_event_checkpoints_are_exact_monotonic_and_restart_durable() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let delivery = delivery();
    let owner_token = [12; 32];
    store
        .apply_harness_state(StoredHarnessStateMutation::ClaimLease {
            agent_id: delivery.agent_id,
            owner_token,
            now_millis: 1,
            expires_at_millis: 100,
        })
        .expect("worker lease commits");
    store
        .apply_harness_state(StoredHarnessStateMutation::QueueDelivery(delivery.clone()))
        .expect("delivery queues");
    store
        .apply_harness_state(StoredHarnessStateMutation::QueueDelivery(delivery.clone()))
        .expect("exact delivery replay is idempotent");
    let mut changed = delivery.clone();
    changed.digest = CommandDigest::from_bytes([99; 32]);
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::QueueDelivery(changed))
            .expect_err("changed stable delivery collides")
            .class(),
        StoreErrorClass::HarnessStateConflict
    );
    store
        .apply_harness_state(StoredHarnessStateMutation::SetDeliveryState {
            agent_id: delivery.agent_id,
            submission_id: delivery.submission_id,
            owner_token,
            state: StoredHarnessDeliveryState::Uncertain,
        })
        .expect("uncertainty checkpoints before I/O");
    store
        .apply_harness_state(StoredHarnessStateMutation::SetDeliveryState {
            agent_id: delivery.agent_id,
            submission_id: delivery.submission_id,
            owner_token,
            state: StoredHarnessDeliveryState::Accepted,
        })
        .expect("acceptance is recorded");
    assert_terminal_delivery_replay(&store, &delivery, owner_token);
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::SetDeliveryState {
                agent_id: delivery.agent_id,
                submission_id: delivery.submission_id,
                owner_token,
                state: StoredHarnessDeliveryState::Pending,
            })
            .expect_err("accepted delivery cannot regress")
            .class(),
        StoreErrorClass::HarnessStateConflict
    );

    let checkpoint = StoredHarnessEventCheckpoint {
        agent_id: delivery.agent_id,
        event_id: MessageId::from_bytes([8; 32]),
        digest: CommandDigest::from_bytes([9; 32]),
        output_committed: true,
        activity_committed: false,
    };
    store
        .apply_harness_state(StoredHarnessStateMutation::CheckpointEvent {
            owner_token,
            checkpoint: checkpoint.clone(),
        })
        .expect("output-first partial checkpoint commits");
    let before_repair = store
        .load_harness_state(16)
        .expect("harness state loads before repair");
    store
        .repair(authority_policy())
        .expect("projection repair succeeds");
    assert_eq!(
        store
            .load_harness_state(16)
            .expect("harness state survives repair"),
        before_repair
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    let state = reopened.load_harness_state(16).expect("state reloads");
    assert_eq!(
        state.deliveries,
        vec![StoredHarnessDelivery {
            state: StoredHarnessDeliveryState::Accepted,
            ..delivery
        }]
    );
    assert_eq!(state.events, vec![checkpoint]);
    assert_eq!(state.leases.len(), 1);
}

#[test]
fn session_operations_reject_changed_identity_and_survive_reopen_without_launch_secrets() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let operation = StoredHarnessSessionOperation {
        operation_id: OperationId::from_bytes([21; 32]),
        request_digest: CommandDigest::from_bytes([22; 32]),
        agent_id: AgentId::from_bytes([23; 32]),
        provider_id: ProviderId::new("scripted").expect("provider"),
        kind: StoredHarnessSessionOperationKind::Resume(
            ProviderSessionId::new("exact-session").expect("session"),
        ),
        state: StoredHarnessSessionOperationState::Prepared,
    };
    store
        .apply_harness_state(StoredHarnessStateMutation::QueueSessionOperation(
            operation.clone(),
        ))
        .expect("operation queues");
    store
        .apply_harness_state(StoredHarnessStateMutation::QueueSessionOperation(
            operation.clone(),
        ))
        .expect("exact replay is idempotent");
    let mut changed = operation.clone();
    changed.request_digest = CommandDigest::from_bytes([99; 32]);
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::QueueSessionOperation(changed))
            .expect_err("changed operation identity collides")
            .class(),
        StoreErrorClass::HarnessStateConflict
    );
    store
        .apply_harness_state(StoredHarnessStateMutation::SetSessionOperationState {
            operation_id: operation.operation_id,
            state: StoredHarnessSessionOperationState::Uncertain,
        })
        .expect("uncertainty precedes provider I/O");
    let ready = ProviderSessionId::new("exact-session").expect("session");
    store
        .apply_harness_state(StoredHarnessStateMutation::SetSessionOperationState {
            operation_id: operation.operation_id,
            state: StoredHarnessSessionOperationState::Ready(ready.clone()),
        })
        .expect("readiness commits");
    store.close().expect("store closes");

    let reopened = open_store(&database);
    let retained = reopened
        .harness_state_handle()
        .session_operation(operation.operation_id)
        .expect("operation loads")
        .expect("operation remains");
    assert_eq!(
        retained.state,
        StoredHarnessSessionOperationState::Ready(ready)
    );
    assert_eq!(retained.request_digest, operation.request_digest);
    assert_eq!(
        reopened
            .apply_harness_state(StoredHarnessStateMutation::SetSessionOperationState {
                operation_id: operation.operation_id,
                state: StoredHarnessSessionOperationState::Stopped,
            })
            .expect_err("terminal operation cannot change meaning")
            .class(),
        StoreErrorClass::HarnessStateConflict
    );
}

fn delivery() -> StoredHarnessDelivery {
    StoredHarnessDelivery {
        agent_id: AgentId::from_bytes([1; 32]),
        provider_id: ProviderId::new("scripted").expect("provider validates"),
        session_id: ProviderSessionId::new("durable-session").expect("session validates"),
        submission_id: MessageId::from_bytes([4; 32]),
        digest: CommandDigest::from_bytes([5; 32]),
        operation_id: OperationId::from_bytes([6; 32]),
        body: ContentText::new("durable exact input").expect("body validates"),
        queued_at_millis: 7,
        state: StoredHarnessDeliveryState::Pending,
    }
}

fn assert_terminal_delivery_replay(
    store: &hq_store::Store,
    delivery: &StoredHarnessDelivery,
    owner_token: [u8; 32],
) {
    store
        .apply_harness_state(StoredHarnessStateMutation::QueueDelivery(delivery.clone()))
        .expect("exact replay preserves the advanced durable state");
    assert_eq!(
        store
            .harness_state_handle()
            .delivery(delivery.agent_id, delivery.submission_id)
            .expect("exact delivery loads")
            .expect("delivery remains")
            .state,
        StoredHarnessDeliveryState::Accepted
    );
    assert!(
        store
            .harness_state_handle()
            .runnable_deliveries(delivery.agent_id, 16)
            .expect("bounded runnable query succeeds")
            .is_empty(),
        "terminal rows cannot starve a repair page"
    );
    assert_eq!(
        store
            .apply_harness_state(StoredHarnessStateMutation::SetDeliveryState {
                agent_id: delivery.agent_id,
                submission_id: delivery.submission_id,
                owner_token,
                state: StoredHarnessDeliveryState::Rejected,
            })
            .expect_err("accepted delivery cannot become rejected")
            .class(),
        StoreErrorClass::HarnessStateConflict
    );
}
