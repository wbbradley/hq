//! Project workflow/store composition contract.

#![allow(clippy::expect_used)]

use std::num::NonZeroUsize;

use hq_application::{
    ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
};
use hq_domain::{
    AccountId, CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode, FactId,
    InstallationId, OperationId, ProjectId, ProviderSessionId, ThreadId, Timestamp,
};
use hq_node::ProjectSagaStoreAdapter;
use hq_projects::{
    CanonicalProjectMutation, CanonicalProjectMutationAction, ProjectSagaManager, ProjectSagaState,
    ProjectSagaStore, SagaEffectState,
};
use hq_store::Store;

mod support;

use support::TestDirectory;

fn request() -> ProjectCommandRequest {
    ProjectCommandRequest {
        command_id: CommandId::from_bytes([1; 32]),
        operation_id: OperationId::from_bytes([2; 32]),
        request_digest: CommandDigest::from_bytes([3; 32]),
        account_id: AccountId::from_bytes([4; 32]),
        project_id: ProjectId::from_bytes([5; 32]),
        home: InstallationId::from_bytes([6; 32]),
        expected_head: Some(FactId::from_bytes([7; 32])),
        issued_at: Timestamp::from_unix_millis(8),
        action: ProjectCommandAction::Open,
    }
}

#[test]
fn exact_command_checkpoint_survives_store_and_adapter_restart() {
    let directory = TestDirectory::new();
    let database = directory.path().join("state").join("project-saga.sqlite3");
    let store = Store::open(&database, NonZeroUsize::MIN).expect("store opens");
    let adapter = ProjectSagaStoreAdapter::new(store.project_saga_state_handle());
    let manager = ProjectSagaManager::new(adapter.clone());
    let first = manager.accept(request()).expect("command accepts");
    assert!(matches!(first, ProjectCommandOutcome::Accepted { .. }));
    let mut checkpoint = manager.runnable(16).expect("checkpoint loads").remove(0);
    let uncertain = DomainError::new(
        ErrorCategory::Unresolved,
        ErrorCode::new("runtime_acceptance_unknown").expect("error code validates"),
    );
    checkpoint.runtime_operation_id = Some(OperationId::from_bytes([9; 32]));
    checkpoint.runtime_effect = SagaEffectState::Uncertain(uncertain.clone());
    checkpoint.runtime_session = Some(ProviderSessionId::new("session-ready").expect("session"));
    checkpoint.selected_thread = Some(ThreadId::from_bytes([10; 32]));
    checkpoint.opened_by_workflow = true;
    checkpoint.failure = Some(DomainError::new(
        ErrorCategory::Unresolved,
        ErrorCode::new("activation_failed").expect("error code validates"),
    ));
    checkpoint.pending_canonical_mutation = Some(CanonicalProjectMutation {
        command_id: CommandId::from_bytes([11; 32]),
        request_digest: CommandDigest::from_bytes([12; 32]),
        account_id: checkpoint.account_id,
        project_id: checkpoint.project_id,
        home: checkpoint.home,
        expected_head: checkpoint.expected_head,
        issued_at: checkpoint.issued_at,
        action: CanonicalProjectMutationAction::Open,
    });
    checkpoint.state = ProjectSagaState::Reconcilable {
        stage: ProjectCommandStage::ReconciliationRequired,
        error: uncertain,
    };
    checkpoint.updated_at_millis = 10;
    adapter
        .replace(checkpoint.clone())
        .expect("uncertain effect checkpoints");
    drop(manager);
    store.close().expect("store closes");

    let reopened = Store::open(&database, NonZeroUsize::MIN).expect("store reopens");
    let reopened_adapter = ProjectSagaStoreAdapter::new(reopened.project_saga_state_handle());
    assert_eq!(
        reopened_adapter
            .find(checkpoint.operation_id)
            .expect("exact checkpoint lookup"),
        Some(checkpoint.clone())
    );
    let recovered = ProjectSagaManager::new(reopened_adapter);
    assert!(matches!(
        recovered.accept(request()).expect("exact command replays"),
        ProjectCommandOutcome::Reconcilable {
            stage: ProjectCommandStage::ReconciliationRequired,
            ..
        }
    ));
    assert_eq!(
        recovered.runnable(16).expect("recovery checkpoint loads"),
        vec![checkpoint]
    );
}
