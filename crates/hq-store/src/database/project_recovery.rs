//! Indexed exact runtime recovery records and revision-fenced attempt transitions.
use crate::{
    MAX_PROJECT_RECOVERY_QUERY_ITEMS, ProjectRecoveryRecord, ProjectRecoveryState as State,
    ProjectRecoveryWriteOutcome as Write, StoreError, StoreErrorClass,
};
use hq_application::{
    ProjectRuntimeFailure, ProjectRuntimeScope, RuntimeFailureReason, RuntimeGenerationId,
    RuntimeLeaseEvidence, RuntimeRecoveryStop, RuntimeWorkerOwner,
};
use hq_domain::{
    AgentId, AssignmentBinding, AssignmentId, MessageId, OperationId, ProjectId, ProviderId,
    ProviderSessionId, ThreadId,
};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

const COLUMNS: &str = "operation_id, project_id, assignment_id, agent_id, thread_id, provider, session, revision, attempts, state_kind, due_at, generation, owner, failure_reason, lease_owner, lease_deadline, blocked_reason, saga_operation_id, submission_id, input_sequence";

pub(super) fn find(
    connection: &Connection,
    operation: OperationId,
) -> Result<Option<ProjectRecoveryRecord>, StoreError> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM project_runtime_recovery WHERE operation_id = ?1"),
            [operation.as_bytes().as_slice()],
            |row| Ok(decode(row)),
        )
        .optional()
        .map_err(database)?
        .transpose()
}

pub(super) fn active(
    connection: &Connection,
    project: ProjectId,
) -> Result<Option<ProjectRecoveryRecord>, StoreError> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM project_runtime_recovery WHERE project_id = ?1 AND state_kind IN (1, 2, 3, 4)"),
            [project.as_bytes().as_slice()],
            |row| Ok(decode(row)),
        )
        .optional()
        .map_err(database)?
        .transpose()
}

pub(super) fn compare_exchange(
    connection: &mut Connection,
    expected: Option<u64>,
    proposed: &ProjectRecoveryRecord,
    now: u64,
) -> Result<Write, StoreError> {
    if !valid(proposed) {
        return Err(invalid());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database)?;
    let existing = find(&transaction, proposed.operation_id)?;
    if existing.as_ref() == Some(proposed) {
        return Ok(Write::AlreadyApplied);
    }
    let accepts = match &existing {
        None => {
            expected.is_none()
                && proposed.revision == 0
                && proposed.attempts == 0
                && proposed.failure.is_none()
                && matches!(proposed.state, State::Waiting { .. })
        }
        Some(old) => {
            expected == Some(old.revision)
                && old.revision.checked_add(1) == Some(proposed.revision)
                && old.scope == proposed.scope
                && old.input == proposed.input
                && transition(old, proposed, now)
        }
    };
    if !accepts {
        return Ok(Write::Conflict);
    }
    let reused_input: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM project_runtime_recovery WHERE project_id = ?1 AND assignment_id = ?2 AND submission_id = ?3 AND operation_id != ?4)",
        params![proposed.scope.project_id.as_bytes().as_slice(), proposed.scope.binding.assignment_id.as_bytes().as_slice(), proposed.input.submission_id.as_bytes().as_slice(), proposed.operation_id.as_bytes().as_slice()],
        |row| row.get(0),
    ).map_err(database)?;
    if reused_input {
        return Ok(Write::Conflict);
    }
    if proposed.state.is_active() {
        let busy: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM project_runtime_recovery WHERE project_id = ?1 AND operation_id != ?2 AND state_kind IN (1, 2, 3, 4))",
            params![proposed.scope.project_id.as_bytes().as_slice(), proposed.operation_id.as_bytes().as_slice()], |row| row.get(0)).map_err(database)?;
        if busy {
            return Ok(Write::Conflict);
        }
    }
    write(&transaction, proposed)?;
    transaction.commit().map_err(database)?;
    Ok(Write::Applied)
}

pub(super) fn retry(
    connection: &mut Connection,
    request: &crate::ProjectRecoveryRetryRequest,
    now: u64,
) -> Result<Write, StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database)?;
    let Some(mut record) = find(&transaction, request.operation_id)? else {
        return Ok(Write::Conflict);
    };
    if record.scope != request.scope {
        return Ok(Write::Conflict);
    }
    let receipt = transaction.query_row(
        "SELECT operation_id, expected_revision, account_id, home FROM project_runtime_recovery_retries WHERE retry_id = ?1",
        [request.retry_id.as_bytes().as_slice()], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Vec<u8>>(2)?, row.get::<_, Vec<u8>>(3)?))
    ).optional().map_err(database)?;
    if let Some((operation, revision, account, home)) = receipt {
        return Ok(
            if OperationId::from_bytes(fixed(operation)?) == request.operation_id
                && decode_u64(revision)? == request.expected_revision
                && hq_domain::AccountId::from_bytes(fixed(account)?) == request.account_id
                && hq_domain::InstallationId::from_bytes(fixed(home)?) == request.home
            {
                Write::AlreadyApplied
            } else {
                Write::Conflict
            },
        );
    }
    if record.revision != request.expected_revision
        || !matches!(record.state, State::Blocked { .. })
    {
        return Ok(Write::Conflict);
    }
    let Some(revision) = record.revision.checked_add(1) else {
        return Ok(Write::Conflict);
    };
    record.revision = revision;
    record.attempts = 0;
    record.state = State::Waiting {
        retry_at_millis: now,
    };
    write(&transaction, &record)?;
    transaction.execute("INSERT INTO project_runtime_recovery_retries(retry_id, operation_id, expected_revision, account_id, home) VALUES (?1, ?2, ?3, ?4, ?5)", params![
        request.retry_id.as_bytes().as_slice(), request.operation_id.as_bytes().as_slice(), request.expected_revision.to_be_bytes().as_slice(), request.account_id.as_bytes().as_slice(), request.home.as_bytes().as_slice(),
    ]).map_err(database)?;
    transaction.commit().map_err(database)?;
    Ok(Write::Applied)
}

fn valid(record: &ProjectRecoveryRecord) -> bool {
    match record.state {
        State::Attempting { .. } => record.attempts > 0,
        State::Ready { .. } => record.attempts > 0 && record.failure.is_none(),
        State::Blocked { .. } => record.failure.is_some(),
        State::Completed => record.failure.is_none(),
        State::Waiting { .. } | State::Cancelled => true,
    }
}

fn transition(old: &ProjectRecoveryRecord, new: &ProjectRecoveryRecord, now: u64) -> bool {
    if matches!(old.state, State::Cancelled | State::Completed) {
        return false;
    }
    if matches!(new.state, State::Completed) {
        return new.attempts == old.attempts;
    }
    if matches!(new.state, State::Cancelled) {
        return new.attempts == old.attempts;
    }
    match (&old.state, &new.state) {
        (
            State::Ready {
                recover_at_millis: due,
                ..
            },
            State::Attempting {
                recover_at_millis, ..
            },
        )
        | (
            State::Waiting {
                retry_at_millis: due,
            },
            State::Attempting {
                recover_at_millis, ..
            },
        )
        | (
            State::Attempting {
                recover_at_millis: due,
                ..
            },
            State::Attempting {
                recover_at_millis, ..
            },
        ) => {
            *due <= now
                && *recover_at_millis > now
                && old.attempts.checked_add(1) == Some(new.attempts)
        }
        (State::Attempting { .. } | State::Ready { .. }, State::Waiting { retry_at_millis }) => {
            new.attempts == old.attempts && *retry_at_millis > now && new.failure.is_some()
        }
        (
            State::Attempting { .. } | State::Waiting { .. } | State::Ready { .. },
            State::Blocked { .. },
        ) => new.attempts == old.attempts,
        (State::Attempting { .. }, State::Ready { generation, .. }) => {
            new.attempts == old.attempts
                && matches!(&old.state, State::Attempting { generation: old, .. } if old == generation)
        }
        _ => false,
    }
}

pub(super) fn due(
    connection: &Connection,
    now: u64,
    limit: usize,
) -> Result<Vec<ProjectRecoveryRecord>, StoreError> {
    if limit == 0 || limit > MAX_PROJECT_RECOVERY_QUERY_ITEMS {
        return Err(invalid());
    }
    let mut statement = connection.prepare(&format!("SELECT {COLUMNS} FROM project_runtime_recovery WHERE due_at IS NOT NULL AND due_at <= ?1 ORDER BY due_at, operation_id LIMIT ?2")).map_err(database)?;
    statement
        .query_map(
            params![
                now.to_be_bytes().as_slice(),
                i64::try_from(limit).map_err(|_| invalid())?
            ],
            |row| Ok(decode(row)),
        )
        .map_err(database)?
        .map(|row| row.map_err(database)?)
        .collect()
}

pub(super) fn next_deadline(connection: &Connection) -> Result<Option<u64>, StoreError> {
    connection.query_row("SELECT due_at FROM project_runtime_recovery WHERE due_at IS NOT NULL ORDER BY due_at, operation_id LIMIT 1", [],
        |row| row.get::<_, Vec<u8>>(0)).optional().map_err(database)?.map(decode_u64).transpose()
}

fn write(connection: &Connection, record: &ProjectRecoveryRecord) -> Result<(), StoreError> {
    let (kind, generation, owner, blocked) = match record.state {
        State::Waiting { .. } => (1, None, None, None),
        State::Attempting { generation, .. } => (2, Some(generation), None, None),
        State::Blocked { reason } => (3, None, None, Some(encode_stop(reason))),
        State::Ready {
            generation, owner, ..
        } => (4, Some(generation), Some(owner), None),
        State::Cancelled => (5, None, None, None),
        State::Completed => (6, None, None, None),
    };
    let lease = record.failure.as_ref().and_then(|failure| failure.lease);
    connection.execute(&format!("INSERT INTO project_runtime_recovery ({COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20) ON CONFLICT(operation_id) DO UPDATE SET revision=excluded.revision, attempts=excluded.attempts, state_kind=excluded.state_kind, due_at=excluded.due_at, generation=excluded.generation, owner=excluded.owner, failure_reason=excluded.failure_reason, lease_owner=excluded.lease_owner, lease_deadline=excluded.lease_deadline, blocked_reason=excluded.blocked_reason"), params![
        record.operation_id.as_bytes().as_slice(), record.scope.project_id.as_bytes().as_slice(),
        record.scope.binding.assignment_id.as_bytes().as_slice(), record.scope.binding.agent_id.as_bytes().as_slice(), record.scope.thread_id.as_bytes().as_slice(),
        record.scope.binding.provider.as_str(), record.scope.binding.session.as_str(), record.revision.to_be_bytes().as_slice(), i64::from(record.attempts), kind,
        record.state.deadline().map(|value| value.to_be_bytes().to_vec()), generation.map(|value| value.as_bytes().to_vec()), owner.map(|value| value.as_bytes().to_vec()),
        record.failure.as_ref().map(|failure| failure.reason.as_str()), lease.map(|value| value.owner.as_bytes().to_vec()), lease.map(|value| value.expires_at_millis.to_be_bytes().to_vec()), blocked,
        record.input.saga_operation_id.as_bytes().as_slice(), record.input.submission_id.as_bytes().as_slice(), record.input.sequence.get().to_be_bytes().as_slice(),
    ]).map_err(database)?;
    Ok(())
}

fn decode(row: &Row<'_>) -> Result<ProjectRecoveryRecord, StoreError> {
    let bytes = |name| row.get::<_, Vec<u8>>(name).map_err(database);
    let optional_bytes = |name| row.get::<_, Option<Vec<u8>>>(name).map_err(database);
    let generation = optional_bytes("generation")?
        .map(|bytes| RuntimeGenerationId::from_bytes(fixed(bytes)?).ok_or_else(corrupt))
        .transpose()?;
    let owner = optional_bytes("owner")?
        .map(|bytes| RuntimeWorkerOwner::from_bytes(fixed(bytes)?).ok_or_else(corrupt))
        .transpose()?;
    let deadline = optional_bytes("due_at")?.map(decode_u64).transpose()?;
    let kind: i64 = row.get("state_kind").map_err(database)?;
    let blocked: Option<i64> = row.get("blocked_reason").map_err(database)?;
    let state = match (kind, deadline, generation, owner, blocked) {
        (1, Some(retry_at_millis), None, None, None) => State::Waiting { retry_at_millis },
        (2, Some(recover_at_millis), Some(generation), None, None) => State::Attempting {
            generation,
            recover_at_millis,
        },
        (3, None, None, None, Some(reason)) => State::Blocked {
            reason: decode_stop(reason)?,
        },
        (4, Some(recover_at_millis), Some(generation), Some(owner), None) => State::Ready {
            generation,
            owner,
            recover_at_millis,
        },
        (5, None, None, None, None) => State::Cancelled,
        (6, None, None, None, None) => State::Completed,
        _ => return Err(corrupt()),
    };
    let failure = decode_failure(row)?;
    let record = ProjectRecoveryRecord {
        operation_id: OperationId::from_bytes(fixed(bytes("operation_id")?)?),
        input: hq_application::ProjectRecoveryInput {
            saga_operation_id: OperationId::from_bytes(fixed(bytes("saga_operation_id")?)?),
            submission_id: MessageId::from_bytes(fixed(bytes("submission_id")?)?),
            sequence: std::num::NonZeroU64::new(decode_u64(bytes("input_sequence")?)?)
                .ok_or_else(corrupt)?,
        },
        scope: ProjectRuntimeScope {
            project_id: ProjectId::from_bytes(fixed(bytes("project_id")?)?),
            binding: AssignmentBinding {
                assignment_id: AssignmentId::from_bytes(fixed(bytes("assignment_id")?)?),
                agent_id: AgentId::from_bytes(fixed(bytes("agent_id")?)?),
                provider: ProviderId::new(row.get::<_, String>("provider").map_err(database)?)
                    .map_err(|_| corrupt())?,
                session: ProviderSessionId::new(row.get::<_, String>("session").map_err(database)?)
                    .map_err(|_| corrupt())?,
            },
            thread_id: ThreadId::from_bytes(fixed(bytes("thread_id")?)?),
        },
        revision: decode_u64(bytes("revision")?)?,
        attempts: u32::try_from(row.get::<_, i64>("attempts").map_err(database)?)
            .map_err(|_| corrupt())?,
        state,
        failure,
    };
    if !valid(&record) {
        return Err(corrupt());
    }
    Ok(record)
}
fn decode_failure(row: &Row<'_>) -> Result<Option<ProjectRuntimeFailure>, StoreError> {
    let reason: Option<String> = row.get("failure_reason").map_err(database)?;
    let owner: Option<Vec<u8>> = row.get("lease_owner").map_err(database)?;
    let deadline: Option<Vec<u8>> = row.get("lease_deadline").map_err(database)?;
    match (reason, owner, deadline) {
        (None, None, None) => Ok(None),
        (Some(reason), None, None) => Ok(Some(ProjectRuntimeFailure {
            reason: decode_reason(&reason)?,
            lease: None,
        })),
        (Some(reason), Some(owner), Some(deadline)) => Ok(Some(ProjectRuntimeFailure {
            reason: decode_reason(&reason)?,
            lease: Some(RuntimeLeaseEvidence {
                owner: RuntimeWorkerOwner::from_bytes(fixed(owner)?).ok_or_else(corrupt)?,
                expires_at_millis: decode_u64(deadline)?,
            }),
        })),
        _ => Err(corrupt()),
    }
}
const fn encode_stop(reason: RuntimeRecoveryStop) -> i64 {
    match reason {
        RuntimeRecoveryStop::PermanentFailure => 1,
        RuntimeRecoveryStop::AttemptsExhausted => 2,
        RuntimeRecoveryStop::ClockRange => 3,
        RuntimeRecoveryStop::InvalidPolicy => 4,
    }
}
fn decode_stop(reason: i64) -> Result<RuntimeRecoveryStop, StoreError> {
    match reason {
        1 => Ok(RuntimeRecoveryStop::PermanentFailure),
        2 => Ok(RuntimeRecoveryStop::AttemptsExhausted),
        3 => Ok(RuntimeRecoveryStop::ClockRange),
        4 => Ok(RuntimeRecoveryStop::InvalidPolicy),
        _ => Err(corrupt()),
    }
}
fn decode_reason(reason: &str) -> Result<RuntimeFailureReason, StoreError> {
    match reason {
        "generation_changed" => Ok(RuntimeFailureReason::GenerationChanged),
        "invalid_input" => Ok(RuntimeFailureReason::InvalidInput),
        "unsupported" => Ok(RuntimeFailureReason::Unsupported),
        "provider_not_registered" => Ok(RuntimeFailureReason::ProviderNotRegistered),
        "registration_conflict" => Ok(RuntimeFailureReason::RegistrationConflict),
        "unsafe_recovery" => Ok(RuntimeFailureReason::UnsafeRecovery),
        "session_identity_mismatch" => Ok(RuntimeFailureReason::SessionIdentityMismatch),
        "session_not_found" => Ok(RuntimeFailureReason::SessionNotFound),
        "submission_identity_conflict" => Ok(RuntimeFailureReason::SubmissionIdentityConflict),
        "interactive_already_answered" => Ok(RuntimeFailureReason::InteractiveAlreadyAnswered),
        "secret_input_rejected" => Ok(RuntimeFailureReason::SecretInputRejected),
        "intake_closed" => Ok(RuntimeFailureReason::IntakeClosed),
        "crashed" => Ok(RuntimeFailureReason::Crashed),
        "protocol_violation" => Ok(RuntimeFailureReason::ProtocolViolation),
        "transport_closed" => Ok(RuntimeFailureReason::TransportClosed),
        "process_failed" => Ok(RuntimeFailureReason::ProcessFailed),
        "compatibility_mismatch" => Ok(RuntimeFailureReason::CompatibilityMismatch),
        "unavailable" => Ok(RuntimeFailureReason::Unavailable),
        "cleanup_failed" => Ok(RuntimeFailureReason::CleanupFailed),
        "ownership_conflict" => Ok(RuntimeFailureReason::OwnershipConflict),
        "backpressure" => Ok(RuntimeFailureReason::Backpressure),
        "persistence_collision" => Ok(RuntimeFailureReason::PersistenceCollision),
        _ => Err(corrupt()),
    }
}
fn decode_u64(bytes: Vec<u8>) -> Result<u64, StoreError> {
    Ok(u64::from_be_bytes(fixed(bytes)?))
}
fn fixed<const SIZE: usize>(bytes: Vec<u8>) -> Result<[u8; SIZE], StoreError> {
    bytes.try_into().map_err(|_| corrupt())
}
fn database(_: rusqlite::Error) -> StoreError {
    StoreError::new(StoreErrorClass::DatabaseUnavailable)
}
const fn invalid() -> StoreError {
    StoreError::new(StoreErrorClass::InvalidOperationalRequest)
}
const fn corrupt() -> StoreError {
    StoreError::new(StoreErrorClass::OperationalStateCorrupt)
}
