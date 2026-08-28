//! Durable project workflow checkpoint and reservation contracts.

#![allow(clippy::expect_used)]

use hq_application::ProjectCommandStage;
use hq_domain::{
    AccountId, BoundedText, CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode,
    FactId, InstallationId, OperationId, ProjectId, ProviderSessionId, ResourceLocator,
    ResourceScheme, ThreadId, Timestamp,
};
use hq_store::{
    StoreErrorClass, StoredProjectEffectState, StoredProjectSaga, StoredProjectSagaBegin,
    StoredProjectSagaState,
};

mod support;

use support::{TestDirectory, authority_policy, open_store};

fn destination(path: &str) -> ResourceLocator {
    ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new(path).expect("destination validates"),
    )
}

fn saga(operation: u8, digest: u8, project: u8) -> StoredProjectSaga {
    StoredProjectSaga {
        operation_id: OperationId::from_bytes([operation; 32]),
        command_id: CommandId::from_bytes([operation; 32]),
        request_digest: CommandDigest::from_bytes([digest; 32]),
        account_id: AccountId::from_bytes([1; 32]),
        project_id: ProjectId::from_bytes([project; 32]),
        home: InstallationId::from_bytes([2; 32]),
        expected_head: Some(FactId::from_bytes([3; 32])),
        issued_at: Timestamp::from_unix_millis(4),
        command_body: b"hq-project-command-v1:open".to_vec(),
        state: StoredProjectSagaState::Running(ProjectCommandStage::Accepted),
        runtime_operation_id: None,
        runtime_effect: StoredProjectEffectState::NotStarted,
        runtime_session: None,
        selected_thread: None,
        opened_by_workflow: false,
        failure: None,
        pending_canonical_mutation: None,
        dispatch_operation_id: None,
        dispatch_effect: StoredProjectEffectState::NotStarted,
        git_operation_id: None,
        git_effect: StoredProjectEffectState::NotStarted,
        resource_operation_id: None,
        resource_effect: StoredProjectEffectState::NotStarted,
        reservation: None,
        updated_at_millis: 5,
    }
}

#[test]
fn exact_begin_replays_and_changed_identity_or_busy_project_is_typed() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let first = saga(10, 11, 12);

    assert_eq!(
        store
            .begin_project_saga(first.clone())
            .expect("first begin"),
        StoredProjectSagaBegin::Inserted(first.clone())
    );
    assert_eq!(
        store
            .begin_project_saga(first.clone())
            .expect("exact replay"),
        StoredProjectSagaBegin::Existing(first.clone())
    );
    assert_eq!(
        store
            .begin_project_saga(saga(10, 99, 12))
            .expect("collision is a disposition"),
        StoredProjectSagaBegin::IdentityConflict
    );
    assert_eq!(
        store
            .begin_project_saga(saga(20, 21, 12))
            .expect("busy project is a disposition"),
        StoredProjectSagaBegin::ProjectBusy
    );

    let mut reserved = saga(30, 31, 32);
    reserved.reservation = Some(destination("/repo/worktrees/feature"));
    assert!(matches!(
        store
            .begin_project_saga(reserved)
            .expect("first destination reserves"),
        StoredProjectSagaBegin::Inserted(_)
    ));
    let mut competing = saga(40, 41, 42);
    competing.reservation = Some(destination("/repo/worktrees/feature"));
    assert_eq!(
        store
            .begin_project_saga(competing)
            .expect("reservation conflict is typed"),
        StoredProjectSagaBegin::ProjectBusy
    );
}

#[test]
fn terminal_reservations_release_only_when_external_state_is_owned_or_absent() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let reserved_destination = destination("/repo/worktrees/reusable");

    let mut rejected_without_git = saga(50, 51, 52);
    rejected_without_git.expected_head = None;
    rejected_without_git.reservation = Some(reserved_destination.clone());
    store
        .begin_project_saga(rejected_without_git.clone())
        .expect("destination reserves");
    rejected_without_git.state = StoredProjectSagaState::Rejected(DomainError::new(
        ErrorCategory::InvalidInput,
        ErrorCode::new("git_request_rejected").expect("code"),
    ));
    rejected_without_git.updated_at_millis += 1;
    store
        .replace_project_saga(rejected_without_git)
        .expect("definite no-effect rejection releases");

    let mut completed_after_git = saga(53, 54, 55);
    completed_after_git.expected_head = None;
    completed_after_git.reservation = Some(reserved_destination.clone());
    completed_after_git.git_operation_id = Some(OperationId::from_bytes([56; 32]));
    completed_after_git.git_effect = StoredProjectEffectState::Accepted;
    assert!(matches!(
        store
            .begin_project_saga(completed_after_git.clone())
            .expect("released destination can be reserved again"),
        StoredProjectSagaBegin::Inserted(_)
    ));
    completed_after_git.state = StoredProjectSagaState::Completed(FactId::from_bytes([57; 32]));
    completed_after_git.updated_at_millis += 1;
    store
        .replace_project_saga(completed_after_git)
        .expect("canonical ownership releases protected reservation");

    let mut next = saga(58, 59, 60);
    next.expected_head = None;
    next.reservation = Some(reserved_destination);
    assert!(matches!(
        store
            .begin_project_saga(next)
            .expect("completed project no longer needs the saga reservation"),
        StoredProjectSagaBegin::Inserted(_)
    ));
}

#[test]
fn rejected_saga_retains_a_reservation_once_git_may_have_created_state() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let reserved_destination = destination("/repo/worktrees/orphaned");
    let mut protected = saga(61, 62, 63);
    protected.expected_head = None;
    protected.reservation = Some(reserved_destination.clone());
    protected.git_operation_id = Some(OperationId::from_bytes([64; 32]));
    protected.git_effect = StoredProjectEffectState::Accepted;
    store
        .begin_project_saga(protected.clone())
        .expect("protected destination reserves");
    protected.state = StoredProjectSagaState::Rejected(DomainError::new(
        ErrorCategory::Conflict,
        ErrorCode::new("canonical_project_conflict").expect("code"),
    ));
    protected.updated_at_millis += 1;
    store
        .replace_project_saga(protected)
        .expect("terminal failure persists");

    let mut competing = saga(65, 66, 67);
    competing.expected_head = None;
    competing.reservation = Some(reserved_destination);
    assert_eq!(
        store
            .begin_project_saga(competing)
            .expect("external worktree keeps destination protected"),
        StoredProjectSagaBegin::ProjectBusy
    );
}

#[test]
fn checkpoints_are_monotonic_bounded_and_restart_durable() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let mut record = saga(10, 11, 12);
    store
        .begin_project_saga(record.clone())
        .expect("saga begins");

    record.state = StoredProjectSagaState::Running(ProjectCommandStage::Opening);
    record.runtime_operation_id = Some(OperationId::from_bytes([40; 32]));
    record.runtime_effect = StoredProjectEffectState::Pending;
    record.updated_at_millis = 6;
    store
        .replace_project_saga(record.clone())
        .expect("checkpoint advances");
    let uncertain = DomainError::new(
        ErrorCategory::Unresolved,
        ErrorCode::new("runtime_acceptance_unknown").expect("error code validates"),
    );
    record.runtime_effect = StoredProjectEffectState::Uncertain(uncertain.clone());
    record.runtime_session = Some(ProviderSessionId::new("session-ready").expect("session"));
    record.selected_thread = Some(ThreadId::from_bytes([41; 32]));
    record.opened_by_workflow = true;
    record.failure = Some(DomainError::new(
        ErrorCategory::Unresolved,
        ErrorCode::new("activation_failed").expect("error code validates"),
    ));
    record.pending_canonical_mutation = Some(b"canonical-mutation-v1".to_vec());
    record.state = StoredProjectSagaState::Reconcilable {
        stage: ProjectCommandStage::ReconciliationRequired,
        error: uncertain,
    };
    record.updated_at_millis = 7;
    store
        .replace_project_saga(record.clone())
        .expect("exact uncertainty persists");

    let mut regression = record.clone();
    regression.state = StoredProjectSagaState::Running(ProjectCommandStage::Accepted);
    assert_eq!(
        store
            .replace_project_saga(regression)
            .expect_err("stage cannot regress")
            .class(),
        StoreErrorClass::ProjectSagaConflict
    );
    assert_eq!(
        store
            .load_runnable_project_sagas(1)
            .expect("bounded recovery scan"),
        vec![record.clone()]
    );
    let before_repair = store
        .load_runnable_project_sagas(16)
        .expect("checkpoint loads before repair");
    store
        .repair(authority_policy())
        .expect("projection repair succeeds");
    assert_eq!(
        store
            .load_runnable_project_sagas(16)
            .expect("checkpoint survives repair"),
        before_repair
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_runnable_project_sagas(16)
            .expect("checkpoint reopens"),
        vec![record]
    );
}
