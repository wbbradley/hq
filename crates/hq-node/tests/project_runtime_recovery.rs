//! Durable input recovery admission over the real node/store adapter.
#![allow(clippy::expect_used)]
use hq_application::{
    ProjectRecoveryInput, ProjectRecoveryState, ProjectRuntimeFailure, ProjectRuntimeReady,
    ProjectRuntimeScope, RuntimeFailureReason, RuntimeGenerationId, RuntimeRecoveryContext,
    RuntimeWorkerOwner,
};
use hq_domain::{
    AgentId, AssignmentBinding, AssignmentId, MessageId, OperationId, ProjectId, ProviderId,
    ProviderSessionId, ThreadId,
};
use hq_node::ProjectSagaStoreAdapter;
use hq_projects::{ProjectRecoveryCoordinator, ProjectRecoveryStore, RuntimeRecoveryPolicy};
use hq_store::Store;
use std::num::{NonZeroU64, NonZeroUsize};
mod support;
use support::TestDirectory;

fn scope() -> ProjectRuntimeScope {
    ProjectRuntimeScope {
        project_id: ProjectId::from_bytes([1; 32]),
        binding: AssignmentBinding {
            assignment_id: AssignmentId::from_bytes([2; 32]),
            agent_id: AgentId::from_bytes([3; 32]),
            provider: ProviderId::new("provider").expect("provider"),
            session: ProviderSessionId::new("saved").expect("session"),
        },
        thread_id: ThreadId::from_bytes([4; 32]),
    }
}
fn input(id: u8) -> ProjectRecoveryInput {
    ProjectRecoveryInput {
        saga_operation_id: OperationId::from_bytes([5; 32]),
        submission_id: MessageId::from_bytes([id; 32]),
        sequence: NonZeroU64::new(u64::from(id)).expect("sequence"),
    }
}
fn context(generation: u8, now_millis: u64) -> RuntimeRecoveryContext {
    RuntimeRecoveryContext {
        generation: RuntimeGenerationId::from_bytes([generation; 32]).expect("generation"),
        now_millis,
    }
}
fn adapter(store: &Store) -> ProjectSagaStoreAdapter {
    ProjectSagaStoreAdapter::new(
        store.project_saga_state_handle(),
        store.project_recovery_state_handle(),
    )
}

#[test]
fn failure_budget_survives_restart_and_exhaustion_does_not_reacquire_an_attempt() {
    let directory = TestDirectory::new();
    let path = directory.path().join("state").join("recovery.sqlite3");
    let store = Store::open(&path, NonZeroUsize::MIN).expect("store");
    let state = adapter(&store);
    let coordinator = ProjectRecoveryCoordinator::new(&state, RuntimeRecoveryPolicy::default());
    let attempt = coordinator
        .admit(&scope(), &input(1), context(10, 10))
        .expect("admit")
        .into_attempt()
        .expect("first attempt");
    let operation = attempt.operation_id;
    assert_eq!(
        state.recovery_find(operation).expect("intent persisted"),
        Some(attempt.clone())
    );
    coordinator
        .failed(
            attempt,
            ProjectRuntimeFailure {
                reason: RuntimeFailureReason::TransportClosed,
                lease: None,
            },
            10,
        )
        .expect("failure")
        .expect("stored");
    store.close().expect("close");
    let reopened = Store::open(&path, NonZeroUsize::MIN).expect("reopen");
    let state = adapter(&reopened);
    let coordinator = ProjectRecoveryCoordinator::new(&state, RuntimeRecoveryPolicy::default());
    assert!(
        coordinator
            .admit(&scope(), &input(1), context(11, 1_009))
            .expect("not due")
            .into_attempt()
            .is_none()
    );
    let mut now = 1_010;
    for count in 2..=5 {
        let attempt = coordinator
            .admit(&scope(), &input(1), context(11, now))
            .expect("due")
            .into_attempt()
            .expect("attempt");
        assert_eq!(attempt.attempts, count);
        let failed = coordinator
            .failed(
                attempt,
                ProjectRuntimeFailure {
                    reason: RuntimeFailureReason::TransportClosed,
                    lease: None,
                },
                now,
            )
            .expect("failure")
            .expect("stored");
        if let Some(deadline) = failed.state.deadline() {
            now = deadline;
        }
    }
    assert!(matches!(
        state
            .recovery_find(operation)
            .expect("record")
            .expect("blocked")
            .state,
        ProjectRecoveryState::Blocked {
            reason: hq_application::RuntimeRecoveryStop::AttemptsExhausted
        }
    ));
    assert_eq!(
        state.recovery_deadline().expect("no blocked deadline"),
        None
    );
    assert!(
        coordinator
            .admit(&scope(), &input(1), context(12, u64::MAX))
            .expect("still blocked")
            .into_attempt()
            .is_none()
    );
}

#[test]
fn readiness_retains_deadline_and_only_completed_input_releases_the_next_budget() {
    let directory = TestDirectory::new();
    let path = directory.path().join("state").join("recovery.sqlite3");
    let store = Store::open(&path, NonZeroUsize::MIN).expect("store");
    let state = adapter(&store);
    let coordinator = ProjectRecoveryCoordinator::new(&state, RuntimeRecoveryPolicy::default());
    let ctx = context(10, 10);
    let attempt = coordinator
        .admit(&scope(), &input(1), ctx)
        .expect("admit")
        .into_attempt()
        .expect("attempt");
    let operation = attempt.operation_id;
    let ready = coordinator
        .ready(
            attempt,
            &ProjectRuntimeReady {
                scope: scope(),
                generation: ctx.generation,
                owner: RuntimeWorkerOwner::from_bytes([20; 32]).expect("owner"),
            },
        )
        .expect("ready")
        .expect("stored");
    assert_eq!(
        state.recovery_deadline().expect("still recoverable"),
        Some(30_010)
    );
    assert!(coordinator.admit(&scope(), &input(2), ctx).is_err());
    coordinator
        .completed(ready)
        .expect("canonical dispatch committed")
        .expect("completed checkpoint");
    assert!(
        coordinator
            .admit(&scope(), &input(1), ctx)
            .expect("completed replay")
            .into_attempt()
            .is_none()
    );
    let next = coordinator
        .admit(&scope(), &input(2), ctx)
        .expect("next input")
        .into_attempt()
        .expect("new attempt");
    assert_eq!(next.attempts, 1);
    assert_ne!(next.operation_id, operation);
    assert_eq!(next.input.saga_operation_id, input(1).saga_operation_id);
}

#[test]
fn identical_concurrent_admissions_grant_only_one_runtime_attempt() {
    let directory = TestDirectory::new();
    let path = directory.path().join("state").join("recovery.sqlite3");
    let store = Store::open(&path, NonZeroUsize::MIN).expect("store");
    let state = adapter(&store);
    let outcomes = std::thread::scope(|threads| {
        let jobs = [0, 1].map(|_| {
            threads.spawn(|| {
                ProjectRecoveryCoordinator::new(&state, RuntimeRecoveryPolicy::default())
                    .admit(&scope(), &input(1), context(10, 10))
                    .expect("admit")
                    .into_attempt()
                    .is_some()
            })
        });
        jobs.map(|job| job.join().expect("join"))
    });
    assert_eq!(outcomes.into_iter().filter(|admitted| *admitted).count(), 1);
}
