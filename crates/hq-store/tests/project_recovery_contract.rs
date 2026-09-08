//! Exact durable runtime recovery and indexed deadline scheduling.
#![allow(clippy::expect_used)]
use hq_application::{ProjectRuntimeScope, RuntimeGenerationId};
use hq_domain::{
    AgentId, AssignmentBinding, AssignmentId, OperationId, ProjectId, ProviderId,
    ProviderSessionId, ThreadId,
};
use hq_store::{ProjectRecoveryRecord, ProjectRecoveryState, ProjectRecoveryWriteOutcome};
mod support;
use support::{TestDirectory, open_store};

fn pending(id: u8, due: u64) -> ProjectRecoveryRecord {
    ProjectRecoveryRecord {
        operation_id: OperationId::from_bytes([id; 32]),
        input: hq_application::ProjectRecoveryInput {
            saga_operation_id: OperationId::from_bytes([50; 32]),
            submission_id: hq_domain::MessageId::from_bytes([id; 32]),
            sequence: std::num::NonZeroU64::new(u64::from(id)).expect("sequence"),
        },
        scope: ProjectRuntimeScope {
            project_id: ProjectId::from_bytes([id; 32]),
            binding: AssignmentBinding {
                assignment_id: AssignmentId::from_bytes([id; 32]),
                agent_id: AgentId::from_bytes([id; 32]),
                provider: ProviderId::new("provider").expect("provider"),
                session: ProviderSessionId::new("saved").expect("session"),
            },
            thread_id: ThreadId::from_bytes([id; 32]),
        },
        revision: 0,
        attempts: 0,
        state: ProjectRecoveryState::Waiting {
            retry_at_millis: due,
        },
        failure: None,
    }
}

#[test]
fn recovery_attempt_and_deadline_survive_reopen_without_reset_or_stale_overwrite() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let handle = store.project_recovery_state_handle();
    let initial = pending(1, 10);
    assert_eq!(
        handle
            .compare_exchange(None, initial.clone(), 10)
            .expect("insert"),
        ProjectRecoveryWriteOutcome::Applied
    );
    let mut attempt = initial.clone();
    attempt.revision = 1;
    attempt.attempts = 1;
    attempt.state = ProjectRecoveryState::Attempting {
        generation: RuntimeGenerationId::from_bytes([2; 32]).expect("generation"),
        recover_at_millis: 100,
    };
    assert_eq!(
        handle
            .compare_exchange(Some(0), attempt.clone(), 10)
            .expect("claim attempt"),
        ProjectRecoveryWriteOutcome::Applied
    );
    assert_eq!(
        handle
            .compare_exchange(Some(0), attempt.clone(), 10)
            .expect("lost response"),
        ProjectRecoveryWriteOutcome::AlreadyApplied
    );
    store.close().expect("close");
    let reopened = open_store(&database);
    let handle = reopened.project_recovery_state_handle();
    assert_eq!(
        handle.find(initial.operation_id).expect("reopen"),
        Some(attempt.clone())
    );
    assert_eq!(handle.next_deadline().expect("deadline"), Some(100));
    assert!(handle.due(99, 1).expect("before due").is_empty());
    assert_eq!(handle.due(100, 1).expect("due"), vec![attempt]);
    assert_eq!(
        handle
            .compare_exchange(None, initial, 100)
            .expect("replayed intake"),
        ProjectRecoveryWriteOutcome::Conflict
    );
}

#[test]
fn deadline_queries_select_due_work_beyond_an_unrelated_prefix() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let handle = store.project_recovery_state_handle();
    for id in 1..=8 {
        handle
            .compare_exchange(None, pending(id, 1_000), 0)
            .expect("later record");
    }
    let urgent = pending(9, 20);
    handle
        .compare_exchange(None, urgent.clone(), 0)
        .expect("earlier deadline");
    assert_eq!(handle.next_deadline().expect("minimum deadline"), Some(20));
    assert_eq!(handle.due(20, 1).expect("exact due index"), vec![urgent]);
    assert!(handle.due(20, 0).is_err());
}

#[test]
#[allow(clippy::too_many_lines)]
fn active_scope_reservation_and_terminal_fences_reject_competing_or_stale_attempts() {
    use hq_application::{
        ProjectRuntimeFailure, RuntimeFailureReason, RuntimeRecoveryStop, RuntimeWorkerOwner,
    };
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let handle = store.project_recovery_state_handle();
    let initial = pending(1, 10);
    exchange(&handle, None, initial.clone(), 0);
    let mut competing = initial.clone();
    competing.operation_id = OperationId::from_bytes([2; 32]);
    competing.input.submission_id = hq_domain::MessageId::from_bytes([2; 32]);
    competing.input.sequence = std::num::NonZeroU64::new(2).expect("next input");
    assert_eq!(
        exchange(&handle, None, competing.clone(), 10),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let mut attempt = initial.clone();
    attempt.revision = 1;
    attempt.attempts = 1;
    let generation = RuntimeGenerationId::from_bytes([3; 32]).expect("generation");
    attempt.state = ProjectRecoveryState::Attempting {
        generation,
        recover_at_millis: 100,
    };
    assert_eq!(
        exchange(&handle, Some(0), attempt.clone(), 9),
        ProjectRecoveryWriteOutcome::Conflict
    );
    assert_eq!(
        exchange(&handle, Some(0), attempt.clone(), 10),
        ProjectRecoveryWriteOutcome::Applied
    );
    let mut ready = attempt.clone();
    ready.revision = 2;
    ready.state = ProjectRecoveryState::Ready {
        generation: RuntimeGenerationId::from_bytes([4; 32]).expect("wrong generation"),
        owner: RuntimeWorkerOwner::from_bytes([5; 32]).expect("owner"),
        recover_at_millis: 100,
    };
    assert_eq!(
        exchange(&handle, Some(1), ready.clone(), 11),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let mut blocked = attempt.clone();
    blocked.revision = 2;
    blocked.failure = Some(ProjectRuntimeFailure {
        reason: RuntimeFailureReason::SessionNotFound,
        lease: None,
    });
    blocked.state = ProjectRecoveryState::Blocked {
        reason: RuntimeRecoveryStop::PermanentFailure,
    };
    assert_eq!(
        exchange(&handle, Some(1), blocked.clone(), 11),
        ProjectRecoveryWriteOutcome::Applied
    );
    assert_eq!(handle.next_deadline().expect("blocked has no timer"), None);
    assert!(
        handle
            .due(u64::MAX, 1)
            .expect("no automatic blocked work")
            .is_empty()
    );
    let mut reset = blocked.clone();
    reset.revision = 3;
    reset.attempts = 0;
    reset.state = ProjectRecoveryState::Waiting {
        retry_at_millis: 12,
    };
    assert_eq!(
        exchange(&handle, Some(2), reset, 11),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let mut changed_scope = blocked.clone();
    changed_scope.revision = 3;
    changed_scope.scope.binding.assignment_id = AssignmentId::from_bytes([99; 32]);
    changed_scope.state = ProjectRecoveryState::Cancelled;
    assert_eq!(
        exchange(&handle, Some(2), changed_scope, 11),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let mut cancelled = blocked;
    cancelled.revision = 3;
    cancelled.state = ProjectRecoveryState::Cancelled;
    assert_eq!(
        exchange(&handle, Some(2), cancelled, 11),
        ProjectRecoveryWriteOutcome::Applied
    );
    assert_eq!(
        exchange(&handle, None, competing, 11),
        ProjectRecoveryWriteOutcome::Applied
    );
    ready.state = ProjectRecoveryState::Ready {
        generation,
        owner: RuntimeWorkerOwner::from_bytes([5; 32]).expect("owner"),
        recover_at_millis: 100,
    };
    assert_eq!(
        exchange(&handle, Some(1), ready, 12),
        ProjectRecoveryWriteOutcome::Conflict
    );
}

#[test]
fn all_closed_failures_and_exact_lease_evidence_survive_reopen() {
    use hq_application::{
        ProjectRuntimeFailure, RuntimeFailureReason, RuntimeLeaseEvidence, RuntimeWorkerOwner,
    };
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let handle = store.project_recovery_state_handle();
    let reasons = [
        RuntimeFailureReason::GenerationChanged,
        RuntimeFailureReason::InvalidInput,
        RuntimeFailureReason::Unsupported,
        RuntimeFailureReason::ProviderNotRegistered,
        RuntimeFailureReason::RegistrationConflict,
        RuntimeFailureReason::UnsafeRecovery,
        RuntimeFailureReason::SessionIdentityMismatch,
        RuntimeFailureReason::SessionNotFound,
        RuntimeFailureReason::SubmissionIdentityConflict,
        RuntimeFailureReason::InteractiveAlreadyAnswered,
        RuntimeFailureReason::SecretInputRejected,
        RuntimeFailureReason::IntakeClosed,
        RuntimeFailureReason::Crashed,
        RuntimeFailureReason::ProtocolViolation,
        RuntimeFailureReason::TransportClosed,
        RuntimeFailureReason::ProcessFailed,
        RuntimeFailureReason::CompatibilityMismatch,
        RuntimeFailureReason::Unavailable,
        RuntimeFailureReason::CleanupFailed,
        RuntimeFailureReason::OwnershipConflict,
        RuntimeFailureReason::Backpressure,
        RuntimeFailureReason::PersistenceCollision,
    ];
    let mut retained = Vec::new();
    for (index, reason) in reasons.into_iter().enumerate() {
        let mut record = pending(u8::try_from(index + 1).expect("bounded id"), 10);
        handle
            .compare_exchange(None, record.clone(), 10)
            .expect("intake");
        record.revision = 1;
        record.attempts = 1;
        record.state = ProjectRecoveryState::Attempting {
            generation: RuntimeGenerationId::from_bytes([90; 32]).expect("generation"),
            recover_at_millis: 100,
        };
        handle
            .compare_exchange(Some(0), record.clone(), 10)
            .expect("intent");
        record.revision = 2;
        record.state = ProjectRecoveryState::Waiting {
            retry_at_millis: 1_000,
        };
        record.failure = Some(ProjectRuntimeFailure {
            reason,
            lease: (reason == RuntimeFailureReason::OwnershipConflict).then_some(
                RuntimeLeaseEvidence {
                    owner: RuntimeWorkerOwner::from_bytes([91; 32]).expect("owner"),
                    expires_at_millis: 1_000,
                },
            ),
        });
        assert_eq!(
            handle
                .compare_exchange(Some(1), record.clone(), 11)
                .expect("failure evidence"),
            ProjectRecoveryWriteOutcome::Applied
        );
        retained.push(record);
    }
    store.close().expect("close");
    let reopened = open_store(&database);
    let handle = reopened.project_recovery_state_handle();
    for record in retained {
        assert_eq!(
            handle
                .find(record.operation_id)
                .expect("exact restored failure"),
            Some(record)
        );
    }
}

fn exchange(
    handle: &hq_store::ProjectRecoveryStateHandle,
    expected: Option<u64>,
    record: ProjectRecoveryRecord,
    now: u64,
) -> ProjectRecoveryWriteOutcome {
    handle
        .compare_exchange(expected, record, now)
        .expect("recovery transition")
}

#[test]
fn explicit_retry_resets_once_and_rejects_changed_or_stale_consent() {
    use hq_application::{ProjectRuntimeFailure, RuntimeFailureReason, RuntimeRecoveryStop};
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let handle = store.project_recovery_state_handle();
    let mut record = pending(1, 10);
    exchange(&handle, None, record.clone(), 10);
    record.revision = 1;
    record.attempts = 1;
    record.state = ProjectRecoveryState::Attempting {
        generation: RuntimeGenerationId::from_bytes([3; 32]).expect("generation"),
        recover_at_millis: 100,
    };
    assert_eq!(
        exchange(&handle, Some(0), record.clone(), 10),
        ProjectRecoveryWriteOutcome::Applied
    );
    record.revision = 2;
    record.failure = Some(ProjectRuntimeFailure {
        reason: RuntimeFailureReason::Unavailable,
        lease: None,
    });
    record.state = ProjectRecoveryState::Blocked {
        reason: RuntimeRecoveryStop::AttemptsExhausted,
    };
    assert_eq!(
        exchange(&handle, Some(1), record.clone(), 10),
        ProjectRecoveryWriteOutcome::Applied
    );
    let request = hq_store::ProjectRecoveryRetryRequest {
        account_id: hq_domain::AccountId::from_bytes([40; 32]),
        home: hq_domain::InstallationId::from_bytes([41; 32]),
        retry_id: OperationId::from_bytes([9; 32]),
        operation_id: record.operation_id,
        scope: record.scope.clone(),
        expected_revision: 2,
    };
    assert_eq!(
        handle.retry(request.clone(), 20).expect("explicit retry"),
        ProjectRecoveryWriteOutcome::Applied
    );
    let mut changed_actor = request.clone();
    changed_actor.account_id = hq_domain::AccountId::from_bytes([42; 32]);
    assert_eq!(
        handle.retry(changed_actor, 21).expect("changed actor"),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let mut changed_home = request.clone();
    changed_home.home = hq_domain::InstallationId::from_bytes([42; 32]);
    assert_eq!(
        handle.retry(changed_home, 21).expect("changed home"),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let after = handle
        .find(record.operation_id)
        .expect("reset")
        .expect("retained");
    assert_eq!(after.revision, 3);
    assert_eq!(after.attempts, 0);
    assert_eq!(
        after.state,
        ProjectRecoveryState::Waiting {
            retry_at_millis: 20
        }
    );
    store.close().expect("close after retry commit");
    let reopened = open_store(&database);
    let handle = reopened.project_recovery_state_handle();
    assert_eq!(
        handle
            .retry(request.clone(), 90)
            .expect("lost retry response"),
        ProjectRecoveryWriteOutcome::AlreadyApplied
    );
    assert_eq!(
        handle.find(record.operation_id).expect("unchanged"),
        Some(after)
    );
    let mut changed = request.clone();
    changed.expected_revision = 3;
    assert_eq!(
        handle.retry(changed, 100).expect("changed request"),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let mut stale = request;
    stale.retry_id = OperationId::from_bytes([10; 32]);
    assert_eq!(
        handle.retry(stale, 100).expect("stale consent"),
        ProjectRecoveryWriteOutcome::Conflict
    );
}

#[test]
fn process_loss_reclaims_due_attempt_without_accepting_an_old_generation_result() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let handle = store.project_recovery_state_handle();
    let mut record = pending(1, 0);
    exchange(&handle, None, record.clone(), 0);
    record.revision = 1;
    record.attempts = 1;
    let generation = RuntimeGenerationId::from_bytes([2; 32]).expect("generation");
    record.state = ProjectRecoveryState::Attempting {
        generation,
        recover_at_millis: 100,
    };
    assert_eq!(
        exchange(&handle, Some(0), record.clone(), 0),
        ProjectRecoveryWriteOutcome::Applied
    );
    let mut reclaimed = record.clone();
    reclaimed.revision = 2;
    reclaimed.attempts = 2;
    reclaimed.state = ProjectRecoveryState::Attempting {
        generation: RuntimeGenerationId::from_bytes([3; 32]).expect("new generation"),
        recover_at_millis: 200,
    };
    assert_eq!(
        exchange(&handle, Some(1), reclaimed.clone(), 99),
        ProjectRecoveryWriteOutcome::Conflict
    );
    assert_eq!(
        exchange(&handle, Some(1), reclaimed.clone(), 100),
        ProjectRecoveryWriteOutcome::Applied
    );
    record.revision = 2;
    record.state = ProjectRecoveryState::Ready {
        generation,
        owner: hq_application::RuntimeWorkerOwner::from_bytes([4; 32]).expect("owner"),
        recover_at_millis: 100,
    };
    assert_eq!(
        exchange(&handle, Some(1), record, 101),
        ProjectRecoveryWriteOutcome::Conflict
    );
    assert_eq!(
        handle
            .find(reclaimed.operation_id)
            .expect("retained newer attempt"),
        Some(reclaimed)
    );
}

#[test]
fn concurrent_attempt_claims_consume_one_revision_and_one_budget_slot() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let handle = store.project_recovery_state_handle();
    let initial = pending(1, 0);
    exchange(&handle, None, initial.clone(), 0);
    let outcomes = std::thread::scope(|scope| {
        let jobs = [2_u8, 3].map(|identity| {
            let handle = handle.clone();
            let mut proposed = initial.clone();
            proposed.revision = 1;
            proposed.attempts = 1;
            proposed.state = ProjectRecoveryState::Attempting {
                generation: RuntimeGenerationId::from_bytes([identity; 32]).expect("generation"),
                recover_at_millis: 100,
            };
            scope.spawn(move || (exchange(&handle, Some(0), proposed.clone(), 0), proposed))
        });
        jobs.map(|job| job.join().expect("claim joins"))
    });
    assert_eq!(
        outcomes
            .iter()
            .filter(|(result, _)| *result == ProjectRecoveryWriteOutcome::Applied)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|(result, _)| *result == ProjectRecoveryWriteOutcome::Conflict)
            .count(),
        1
    );
    let (_, winner) = outcomes
        .into_iter()
        .find(|(result, _)| *result == ProjectRecoveryWriteOutcome::Applied)
        .expect("one winner");
    assert_eq!(
        handle.find(initial.operation_id).expect("exact winner"),
        Some(winner)
    );
}

#[test]
fn active_project_lookup_releases_cancelled_scope_and_rejects_stale_attempts() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let handle = store.project_recovery_state_handle();
    let old = pending(1, 10);
    handle
        .compare_exchange(None, old.clone(), 10)
        .expect("old reservation");
    assert_eq!(
        handle.active(old.scope.project_id).expect("active"),
        Some(old.clone())
    );
    assert_eq!(
        handle
            .active(ProjectId::from_bytes([99; 32]))
            .expect("other project"),
        None
    );
    let mut cancelled = old.clone();
    cancelled.revision += 1;
    cancelled.state = ProjectRecoveryState::Cancelled;
    handle
        .compare_exchange(Some(old.revision), cancelled, 10)
        .expect("cancel");
    assert_eq!(handle.active(old.scope.project_id).expect("released"), None);
    let mut stale = old.clone();
    stale.revision += 1;
    stale.attempts = 1;
    stale.state = ProjectRecoveryState::Attempting {
        generation: RuntimeGenerationId::from_bytes([2; 32]).expect("generation"),
        recover_at_millis: 100,
    };
    assert_eq!(
        handle
            .compare_exchange(Some(old.revision), stale, 10)
            .expect("stale attempt"),
        ProjectRecoveryWriteOutcome::Conflict
    );
    let mut replacement = pending(2, 10);
    replacement.scope.project_id = old.scope.project_id;
    handle
        .compare_exchange(None, replacement.clone(), 10)
        .expect("replacement reservation");
    assert_eq!(
        handle.active(old.scope.project_id).expect("current"),
        Some(replacement)
    );
}
