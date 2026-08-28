//! Durable project workflow checkpoints and worktree reservations.

use hq_application::ProjectCommandStage;
use hq_domain::{
    AccountId, BoundedText, CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode,
    FactId, InstallationId, OperationId, ProjectId, ProviderSessionId, ResourceLocator,
    ResourceScheme, ThreadId, Timestamp,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use crate::{
    MAX_PROJECT_COMMAND_BODY_BYTES, MAX_PROJECT_SAGA_QUERY_ITEMS, StoreError, StoreErrorClass,
    StoredProjectEffectState, StoredProjectSaga, StoredProjectSagaBegin, StoredProjectSagaState,
};

pub(super) fn begin(
    connection: &mut Connection,
    proposed: &StoredProjectSaga,
) -> Result<StoredProjectSagaBegin, StoreError> {
    begin_with_failpoint(connection, proposed, ProjectSagaFailpoint::Never)
}

fn begin_with_failpoint(
    connection: &mut Connection,
    proposed: &StoredProjectSaga,
    failpoint: ProjectSagaFailpoint,
) -> Result<StoredProjectSagaBegin, StoreError> {
    validate_record(proposed)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database)?;
    if let Some(existing) = load_operation(&transaction, proposed.operation_id)? {
        transaction.commit().map_err(database)?;
        return Ok(if same_request(&existing, proposed) {
            StoredProjectSagaBegin::Existing(existing)
        } else {
            StoredProjectSagaBegin::IdentityConflict
        });
    }
    if command_exists(&transaction, proposed.command_id)? {
        transaction.commit().map_err(database)?;
        return Ok(StoredProjectSagaBegin::IdentityConflict);
    }
    if project_is_busy(&transaction, proposed.project_id)?
        || reservation_is_busy(&transaction, proposed)?
    {
        transaction.commit().map_err(database)?;
        return Ok(StoredProjectSagaBegin::ProjectBusy);
    }
    insert_record(&transaction, proposed)?;
    fail_at(failpoint, ProjectSagaFailpoint::AfterRecordWrite)?;
    reserve(&transaction, proposed)?;
    fail_at(failpoint, ProjectSagaFailpoint::AfterReservationWrite)?;
    fail_at(failpoint, ProjectSagaFailpoint::BeforeCommit)?;
    transaction.commit().map_err(database)?;
    fail_at(failpoint, ProjectSagaFailpoint::AfterCommit)?;
    Ok(StoredProjectSagaBegin::Inserted(proposed.clone()))
}

#[allow(clippy::too_many_lines)]
pub(super) fn replace(
    connection: &mut Connection,
    proposed: &StoredProjectSaga,
) -> Result<(), StoreError> {
    replace_with_failpoint(connection, proposed, ProjectSagaFailpoint::Never)
}

#[allow(clippy::too_many_lines)]
fn replace_with_failpoint(
    connection: &mut Connection,
    proposed: &StoredProjectSaga,
    failpoint: ProjectSagaFailpoint,
) -> Result<(), StoreError> {
    validate_record(proposed)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database)?;
    let existing = load_operation(&transaction, proposed.operation_id)?.ok_or_else(conflict)?;
    if !same_request(&existing, proposed)
        || proposed.updated_at_millis < existing.updated_at_millis
        || !state_advances(&existing.state, &proposed.state)
        || !effect_advances(&existing.runtime_effect, &proposed.runtime_effect)
        || !effect_advances(&existing.dispatch_effect, &proposed.dispatch_effect)
        || !effect_advances(&existing.git_effect, &proposed.git_effect)
        || !effect_advances(&existing.resource_effect, &proposed.resource_effect)
        || !optional_identity_advances(existing.runtime_operation_id, proposed.runtime_operation_id)
        || !optional_identity_advances(
            existing.dispatch_operation_id,
            proposed.dispatch_operation_id,
        )
        || !optional_identity_advances(existing.git_operation_id, proposed.git_operation_id)
        || !optional_identity_advances(
            existing.resource_operation_id,
            proposed.resource_operation_id,
        )
        || !optional_session_advances(
            existing.runtime_session.as_ref(),
            proposed.runtime_session.as_ref(),
        )
        || !optional_thread_advances(existing.selected_thread, proposed.selected_thread)
        || (existing.opened_by_workflow && !proposed.opened_by_workflow)
        || !optional_error_advances(existing.failure.as_ref(), proposed.failure.as_ref())
        || !optional_reservation_advances(
            existing.reservation.as_ref(),
            proposed.reservation.as_ref(),
        )
    {
        return Err(conflict());
    }
    reserve(&transaction, proposed)?;
    fail_at(failpoint, ProjectSagaFailpoint::AfterReservationWrite)?;
    let (state_kind, stage, project_head, error_category, error_code) =
        encode_state(&proposed.state);
    let (runtime_effect, runtime_error_category, runtime_error_code) =
        encode_effect(&proposed.runtime_effect);
    let (dispatch_effect, dispatch_error_category, dispatch_error_code) =
        encode_effect(&proposed.dispatch_effect);
    let (git_effect, git_error_category, git_error_code) = encode_effect(&proposed.git_effect);
    let (resource_effect, resource_error_category, resource_error_code) =
        encode_effect(&proposed.resource_effect);
    let (reservation_scheme, reservation_value) = proposed
        .reservation
        .as_ref()
        .map_or((None, None), |locator| {
            (Some(encode_scheme(locator.scheme())), Some(locator.value()))
        });
    let (failure_category, failure_code) =
        proposed.failure.as_ref().map_or((None, None), |error| {
            (
                Some(encode_error_category(error.category())),
                Some(error.code().as_str()),
            )
        });
    transaction
        .execute(
            "UPDATE project_sagas SET state_kind = ?2, stage = ?3, project_head = ?4, \
             error_category = ?5, error_code = ?6, runtime_operation_id = ?7, \
             runtime_effect = ?8, runtime_error_category = ?9, runtime_error_code = ?10, \
             dispatch_operation_id = ?11, dispatch_effect = ?12, \
             dispatch_error_category = ?13, dispatch_error_code = ?14, \
             git_operation_id = ?15, git_effect = ?16, git_error_category = ?17, \
             git_error_code = ?18, resource_operation_id = ?19, resource_effect = ?20, \
             resource_error_category = ?21, resource_error_code = ?22, \
             reservation_scheme = ?23, reservation_value = ?24, \
             updated_at_millis = ?25, runtime_session = ?26, selected_thread = ?27, \
             opened_by_workflow = ?28, failure_category = ?29, \
             failure_code = ?30, pending_canonical_mutation = ?31 WHERE operation_id = ?1",
            params![
                proposed.operation_id.as_bytes().as_slice(),
                state_kind,
                stage,
                project_head
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                error_category,
                error_code,
                optional_operation_bytes(proposed.runtime_operation_id),
                runtime_effect,
                runtime_error_category,
                runtime_error_code,
                optional_operation_bytes(proposed.dispatch_operation_id),
                dispatch_effect,
                dispatch_error_category,
                dispatch_error_code,
                optional_operation_bytes(proposed.git_operation_id),
                git_effect,
                git_error_category,
                git_error_code,
                optional_operation_bytes(proposed.resource_operation_id),
                resource_effect,
                resource_error_category,
                resource_error_code,
                reservation_scheme,
                reservation_value,
                proposed.updated_at_millis.to_be_bytes().as_slice(),
                proposed
                    .runtime_session
                    .as_ref()
                    .map(ProviderSessionId::as_str),
                proposed
                    .selected_thread
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                i64::from(proposed.opened_by_workflow),
                failure_category,
                failure_code,
                proposed.pending_canonical_mutation.as_deref(),
            ],
        )
        .map_err(database)?;
    fail_at(failpoint, ProjectSagaFailpoint::AfterRecordWrite)?;
    if proposed.reservation.is_some()
        && matches!(
            &proposed.git_effect,
            StoredProjectEffectState::Accepted | StoredProjectEffectState::Uncertain(_)
        )
    {
        transaction
            .execute(
                "UPDATE project_saga_reservations SET protects_external_state = 1 \
                 WHERE operation_id = ?1",
                [proposed.operation_id.as_bytes().as_slice()],
            )
            .map_err(database)?;
    }
    fail_at(failpoint, ProjectSagaFailpoint::AfterProtectionWrite)?;
    release_terminal_reservation(&transaction, proposed)?;
    fail_at(failpoint, ProjectSagaFailpoint::AfterReservationRelease)?;
    fail_at(failpoint, ProjectSagaFailpoint::BeforeCommit)?;
    transaction.commit().map_err(database)?;
    fail_at(failpoint, ProjectSagaFailpoint::AfterCommit)
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "non-Never variants are exercised by unit failpoint tests"
)]
enum ProjectSagaFailpoint {
    Never,
    AfterReservationWrite,
    AfterRecordWrite,
    AfterProtectionWrite,
    AfterReservationRelease,
    BeforeCommit,
    AfterCommit,
}

fn fail_at(actual: ProjectSagaFailpoint, expected: ProjectSagaFailpoint) -> Result<(), StoreError> {
    if actual == expected {
        Err(StoreError::new(StoreErrorClass::DatabaseUnavailable))
    } else {
        Ok(())
    }
}

pub(super) fn load_runnable(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<StoredProjectSaga>, StoreError> {
    let limit = bounded_limit(limit)?;
    let mut statement = connection
        .prepare(
            "SELECT operation_id, command_id, request_digest, account_id, project_id, home, \
             expected_head, issued_at_millis, command_body, state_kind, stage, project_head, \
             error_category, error_code, runtime_operation_id, runtime_effect, \
             runtime_error_category, runtime_error_code, dispatch_operation_id, dispatch_effect, \
             dispatch_error_category, dispatch_error_code, git_operation_id, git_effect, \
             git_error_category, git_error_code, resource_operation_id, resource_effect, \
             resource_error_category, resource_error_code, reservation_scheme, reservation_value, \
             updated_at_millis, runtime_session, selected_thread, opened_by_workflow, \
             failure_category, failure_code, pending_canonical_mutation \
             FROM project_sagas WHERE state_kind IN (1, 4) \
             ORDER BY updated_at_millis, operation_id LIMIT ?1",
        )
        .map_err(database)?;
    let rows = statement
        .query_map([limit], decode_record)
        .map_err(database)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(database)?);
    }
    Ok(records)
}

fn insert_record(
    transaction: &Transaction<'_>,
    record: &StoredProjectSaga,
) -> Result<(), StoreError> {
    let (state_kind, stage, project_head, error_category, error_code) = encode_state(&record.state);
    let (runtime_effect, runtime_error_category, runtime_error_code) =
        encode_effect(&record.runtime_effect);
    let (dispatch_effect, dispatch_error_category, dispatch_error_code) =
        encode_effect(&record.dispatch_effect);
    let (git_effect, git_error_category, git_error_code) = encode_effect(&record.git_effect);
    let (resource_effect, resource_error_category, resource_error_code) =
        encode_effect(&record.resource_effect);
    let (reservation_scheme, reservation_value) =
        record.reservation.as_ref().map_or((None, None), |locator| {
            (Some(encode_scheme(locator.scheme())), Some(locator.value()))
        });
    let (failure_category, failure_code) = record.failure.as_ref().map_or((None, None), |error| {
        (
            Some(encode_error_category(error.category())),
            Some(error.code().as_str()),
        )
    });
    transaction
        .execute(
            "INSERT INTO project_sagas(operation_id, command_id, request_digest, account_id, \
             project_id, home, expected_head, issued_at_millis, command_body, state_kind, stage, \
             project_head, error_category, error_code, runtime_operation_id, runtime_effect, \
             runtime_error_category, runtime_error_code, dispatch_operation_id, dispatch_effect, \
             dispatch_error_category, dispatch_error_code, git_operation_id, git_effect, \
             git_error_category, git_error_code, resource_operation_id, resource_effect, \
             resource_error_category, resource_error_code, reservation_scheme, reservation_value, \
             updated_at_millis, runtime_session, selected_thread, opened_by_workflow, \
             failure_category, failure_code, pending_canonical_mutation) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, \
             ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, \
             ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39)",
            params![
                record.operation_id.as_bytes().as_slice(),
                record.command_id.as_bytes().as_slice(),
                record.request_digest.as_bytes().as_slice(),
                record.account_id.as_bytes().as_slice(),
                record.project_id.as_bytes().as_slice(),
                record.home.as_bytes().as_slice(),
                record
                    .expected_head
                    .as_ref()
                    .map(|head| head.as_bytes().as_slice()),
                record.issued_at.as_unix_millis().to_be_bytes().as_slice(),
                &record.command_body,
                state_kind,
                stage,
                project_head
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                error_category,
                error_code,
                optional_operation_bytes(record.runtime_operation_id),
                runtime_effect,
                runtime_error_category,
                runtime_error_code,
                optional_operation_bytes(record.dispatch_operation_id),
                dispatch_effect,
                dispatch_error_category,
                dispatch_error_code,
                optional_operation_bytes(record.git_operation_id),
                git_effect,
                git_error_category,
                git_error_code,
                optional_operation_bytes(record.resource_operation_id),
                resource_effect,
                resource_error_category,
                resource_error_code,
                reservation_scheme,
                reservation_value,
                record.updated_at_millis.to_be_bytes().as_slice(),
                record
                    .runtime_session
                    .as_ref()
                    .map(ProviderSessionId::as_str),
                record
                    .selected_thread
                    .as_ref()
                    .map(|value| value.as_bytes().as_slice()),
                i64::from(record.opened_by_workflow),
                failure_category,
                failure_code,
                record.pending_canonical_mutation.as_deref(),
            ],
        )
        .map_err(database)?;
    Ok(())
}

pub(super) fn load_operation(
    connection: &Connection,
    operation_id: OperationId,
) -> Result<Option<StoredProjectSaga>, StoreError> {
    connection
        .query_row(
            "SELECT operation_id, command_id, request_digest, account_id, project_id, home, \
             expected_head, issued_at_millis, command_body, state_kind, stage, project_head, \
             error_category, error_code, runtime_operation_id, runtime_effect, \
             runtime_error_category, runtime_error_code, dispatch_operation_id, dispatch_effect, \
             dispatch_error_category, dispatch_error_code, git_operation_id, git_effect, \
             git_error_category, git_error_code, resource_operation_id, resource_effect, \
             resource_error_category, resource_error_code, reservation_scheme, reservation_value, \
             updated_at_millis, runtime_session, selected_thread, opened_by_workflow, \
             failure_category, failure_code, pending_canonical_mutation \
             FROM project_sagas WHERE operation_id = ?1",
            [operation_id.as_bytes().as_slice()],
            decode_record,
        )
        .optional()
        .map_err(database)
}

#[allow(clippy::too_many_lines)]
fn decode_record(row: &Row<'_>) -> rusqlite::Result<StoredProjectSaga> {
    let operation_id = OperationId::from_bytes(blob32(row.get(0)?)?);
    let command_id = CommandId::from_bytes(blob32(row.get(1)?)?);
    let request_digest = CommandDigest::from_bytes(blob32(row.get(2)?)?);
    let account_id = AccountId::from_bytes(blob32(row.get(3)?)?);
    let project_id = ProjectId::from_bytes(blob32(row.get(4)?)?);
    let home = InstallationId::from_bytes(blob32(row.get(5)?)?);
    let expected_head = optional_blob32(row.get(6)?)?.map(FactId::from_bytes);
    let issued_at_millis = i64::from_be_bytes(blob8(row.get(7)?)?);
    let command_body: Vec<u8> = row.get(8)?;
    let state_kind: i64 = row.get(9)?;
    let stage = decode_stage(row.get(10)?)?;
    let project_head = optional_blob32(row.get(11)?)?.map(FactId::from_bytes);
    let error_category = row.get::<_, Option<i64>>(12)?;
    let error_code = row.get::<_, Option<String>>(13)?;
    let workflow_state = decode_state(state_kind, stage, project_head, error_category, error_code)?;
    let runtime_operation_id = optional_blob32(row.get(14)?)?.map(OperationId::from_bytes);
    let runtime_effect = decode_effect(row.get(15)?, row.get(16)?, row.get(17)?)?;
    let dispatch_operation_id = optional_blob32(row.get(18)?)?.map(OperationId::from_bytes);
    let dispatch_effect = decode_effect(row.get(19)?, row.get(20)?, row.get(21)?)?;
    let git_operation_id = optional_blob32(row.get(22)?)?.map(OperationId::from_bytes);
    let git_effect = decode_effect(row.get(23)?, row.get(24)?, row.get(25)?)?;
    let resource_operation_id = optional_blob32(row.get(26)?)?.map(OperationId::from_bytes);
    let resource_effect = decode_effect(row.get(27)?, row.get(28)?, row.get(29)?)?;
    let reservation_scheme = row.get::<_, Option<i64>>(30)?;
    let reservation_value = row.get::<_, Option<String>>(31)?;
    let reservation = decode_reservation(reservation_scheme, reservation_value)?;
    let updated_at_millis = u64::from_be_bytes(blob8(row.get(32)?)?);
    let runtime_session = row
        .get::<_, Option<String>>(33)?
        .map(ProviderSessionId::new)
        .transpose()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let selected_thread = optional_blob32(row.get(34)?)?.map(ThreadId::from_bytes);
    let opened_by_workflow = match row.get::<_, i64>(35)? {
        0 => false,
        1 => true,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let failure_category = row.get::<_, Option<i64>>(36)?;
    let failure_code = row.get::<_, Option<String>>(37)?;
    let failure = match (failure_category, failure_code) {
        (Some(category), Some(code)) => Some(decode_domain_error(category, code)?),
        (None, None) => None,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let pending_canonical_mutation = row.get::<_, Option<Vec<u8>>>(38)?;
    Ok(StoredProjectSaga {
        operation_id,
        command_id,
        request_digest,
        account_id,
        project_id,
        home,
        expected_head,
        issued_at: Timestamp::from_unix_millis(issued_at_millis),
        command_body,
        state: workflow_state,
        runtime_operation_id,
        runtime_effect,
        runtime_session,
        selected_thread,
        opened_by_workflow,
        failure,
        pending_canonical_mutation,
        dispatch_operation_id,
        dispatch_effect,
        git_operation_id,
        git_effect,
        resource_operation_id,
        resource_effect,
        reservation,
        updated_at_millis,
    })
}

fn reserve(transaction: &Transaction<'_>, record: &StoredProjectSaga) -> Result<(), StoreError> {
    let Some(locator) = &record.reservation else {
        return Ok(());
    };
    let existing = transaction
        .query_row(
            "SELECT operation_id FROM project_saga_reservations \
             WHERE home = ?1 AND locator_scheme = ?2 AND locator_value = ?3",
            params![
                record.home.as_bytes().as_slice(),
                encode_scheme(locator.scheme()),
                locator.value(),
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(database)?;
    if let Some(existing) = existing {
        return if blob32(existing).map_err(|_| corrupt())? == *record.operation_id.as_bytes() {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    transaction
        .execute(
            "INSERT INTO project_saga_reservations(home, locator_scheme, locator_value, \
             operation_id, protects_external_state) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.home.as_bytes().as_slice(),
                encode_scheme(locator.scheme()),
                locator.value(),
                record.operation_id.as_bytes().as_slice(),
                i64::from(matches!(
                    &record.git_effect,
                    StoredProjectEffectState::Accepted | StoredProjectEffectState::Uncertain(_)
                )),
            ],
        )
        .map_err(database)?;
    Ok(())
}

fn reservation_is_busy(
    transaction: &Transaction<'_>,
    record: &StoredProjectSaga,
) -> Result<bool, StoreError> {
    let Some(locator) = &record.reservation else {
        return Ok(false);
    };
    let operation = transaction
        .query_row(
            "SELECT operation_id FROM project_saga_reservations \
             WHERE home = ?1 AND locator_scheme = ?2 AND locator_value = ?3",
            params![
                record.home.as_bytes().as_slice(),
                encode_scheme(locator.scheme()),
                locator.value(),
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(database)?;
    operation
        .map(|bytes| blob32(bytes).map(|value| value != *record.operation_id.as_bytes()))
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(|_| corrupt())
}

fn release_terminal_reservation(
    transaction: &Transaction<'_>,
    record: &StoredProjectSaga,
) -> Result<(), StoreError> {
    let predicate = match &record.state {
        StoredProjectSagaState::Completed(_) => "operation_id = ?1",
        StoredProjectSagaState::Rejected(_) => "operation_id = ?1 AND protects_external_state = 0",
        StoredProjectSagaState::Running(_) | StoredProjectSagaState::Reconcilable { .. } => {
            return Ok(());
        }
    };
    transaction
        .execute(
            &format!("DELETE FROM project_saga_reservations WHERE {predicate}"),
            [record.operation_id.as_bytes().as_slice()],
        )
        .map(|_| ())
        .map_err(database)
}

fn command_exists(connection: &Connection, command_id: CommandId) -> Result<bool, StoreError> {
    connection
        .query_row(
            "SELECT count(*) FROM project_sagas WHERE command_id = ?1",
            [command_id.as_bytes().as_slice()],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count != 0)
        .map_err(database)
}

fn project_is_busy(connection: &Connection, project_id: ProjectId) -> Result<bool, StoreError> {
    connection
        .query_row(
            "SELECT count(*) FROM project_sagas WHERE project_id = ?1 AND state_kind IN (1, 4)",
            [project_id.as_bytes().as_slice()],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count != 0)
        .map_err(database)
}

fn validate_record(record: &StoredProjectSaga) -> Result<(), StoreError> {
    if record.command_body.is_empty()
        || record.command_body.len() > MAX_PROJECT_COMMAND_BODY_BYTES
        || record
            .pending_canonical_mutation
            .as_ref()
            .is_some_and(|body| body.is_empty() || body.len() > MAX_PROJECT_COMMAND_BODY_BYTES)
        || operation_state_mismatch(record.runtime_operation_id, &record.runtime_effect)
        || operation_state_mismatch(record.dispatch_operation_id, &record.dispatch_effect)
        || operation_state_mismatch(record.git_operation_id, &record.git_effect)
        || operation_state_mismatch(record.resource_operation_id, &record.resource_effect)
    {
        return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
    }
    Ok(())
}

fn operation_state_mismatch(
    operation_id: Option<OperationId>,
    effect: &StoredProjectEffectState,
) -> bool {
    operation_id.is_none() && !matches!(effect, StoredProjectEffectState::NotStarted)
}

fn same_request(left: &StoredProjectSaga, right: &StoredProjectSaga) -> bool {
    left.operation_id == right.operation_id
        && left.command_id == right.command_id
        && left.request_digest == right.request_digest
        && left.account_id == right.account_id
        && left.project_id == right.project_id
        && left.home == right.home
        && left.expected_head == right.expected_head
        && left.issued_at == right.issued_at
        && left.command_body == right.command_body
}

fn state_advances(old: &StoredProjectSagaState, new: &StoredProjectSagaState) -> bool {
    match (old, new) {
        (StoredProjectSagaState::Running(old), StoredProjectSagaState::Running(new)) => {
            encode_stage(*new) >= encode_stage(*old)
        }
        (
            StoredProjectSagaState::Reconcilable { stage: old, .. },
            StoredProjectSagaState::Reconcilable { stage: new, .. },
        )
        | (
            StoredProjectSagaState::Reconcilable { stage: old, .. },
            StoredProjectSagaState::Running(new),
        ) => encode_stage(*new) >= encode_stage(*old),
        (
            StoredProjectSagaState::Running(_) | StoredProjectSagaState::Reconcilable { .. },
            StoredProjectSagaState::Completed(_) | StoredProjectSagaState::Rejected(_),
        )
        | (StoredProjectSagaState::Running(_), StoredProjectSagaState::Reconcilable { .. }) => true,
        (StoredProjectSagaState::Completed(old), StoredProjectSagaState::Completed(new)) => {
            old == new
        }
        (StoredProjectSagaState::Rejected(old), StoredProjectSagaState::Rejected(new)) => {
            old == new
        }
        _ => false,
    }
}

fn effect_advances(old: &StoredProjectEffectState, new: &StoredProjectEffectState) -> bool {
    match old {
        StoredProjectEffectState::NotStarted => true,
        StoredProjectEffectState::Pending => !matches!(new, StoredProjectEffectState::NotStarted),
        StoredProjectEffectState::Uncertain(old_error) => match new {
            StoredProjectEffectState::Uncertain(new_error) => old_error == new_error,
            StoredProjectEffectState::Accepted | StoredProjectEffectState::Rejected(_) => true,
            StoredProjectEffectState::NotStarted | StoredProjectEffectState::Pending => false,
        },
        StoredProjectEffectState::Accepted => matches!(new, StoredProjectEffectState::Accepted),
        StoredProjectEffectState::Rejected(old_error) => {
            matches!(new, StoredProjectEffectState::Rejected(new_error) if old_error == new_error)
        }
    }
}

fn optional_identity_advances(old: Option<OperationId>, new: Option<OperationId>) -> bool {
    old.is_none() || old == new
}

fn optional_session_advances(
    old: Option<&ProviderSessionId>,
    new: Option<&ProviderSessionId>,
) -> bool {
    old.is_none() || old == new
}

fn optional_thread_advances(old: Option<ThreadId>, new: Option<ThreadId>) -> bool {
    old.is_none() || old == new
}

fn optional_error_advances(old: Option<&DomainError>, new: Option<&DomainError>) -> bool {
    old.is_none() || old == new
}

fn optional_reservation_advances(
    old: Option<&ResourceLocator>,
    new: Option<&ResourceLocator>,
) -> bool {
    old.is_none() || old == new
}

fn encode_state(
    workflow_state: &StoredProjectSagaState,
) -> (i64, i64, Option<FactId>, Option<i64>, Option<&str>) {
    match workflow_state {
        StoredProjectSagaState::Running(stage) => (1, encode_stage(*stage), None, None, None),
        StoredProjectSagaState::Completed(head) => (
            2,
            encode_stage(ProjectCommandStage::Complete),
            Some(*head),
            None,
            None,
        ),
        StoredProjectSagaState::Rejected(error) => (
            3,
            encode_stage(ProjectCommandStage::Complete),
            None,
            Some(encode_error_category(error.category())),
            Some(error.code().as_str()),
        ),
        StoredProjectSagaState::Reconcilable { stage, error } => (
            4,
            encode_stage(*stage),
            None,
            Some(encode_error_category(error.category())),
            Some(error.code().as_str()),
        ),
    }
}

fn decode_state(
    kind: i64,
    stage: ProjectCommandStage,
    head: Option<FactId>,
    error_category: Option<i64>,
    error_code: Option<String>,
) -> rusqlite::Result<StoredProjectSagaState> {
    match (kind, head, error_category, error_code) {
        (1, None, None, None) => Ok(StoredProjectSagaState::Running(stage)),
        (2, Some(head), None, None) if stage == ProjectCommandStage::Complete => {
            Ok(StoredProjectSagaState::Completed(head))
        }
        (3, None, Some(category), Some(code)) if stage == ProjectCommandStage::Complete => Ok(
            StoredProjectSagaState::Rejected(decode_domain_error(category, code)?),
        ),
        (4, None, Some(category), Some(code)) => Ok(StoredProjectSagaState::Reconcilable {
            stage,
            error: decode_domain_error(category, code)?,
        }),
        _ => Err(decode_error()),
    }
}

fn encode_stage(stage: ProjectCommandStage) -> i64 {
    match stage {
        ProjectCommandStage::Accepted => 1,
        ProjectCommandStage::AwaitingHome => 2,
        ProjectCommandStage::ReceivedAtHome => 3,
        ProjectCommandStage::ValidatingResources => 4,
        ProjectCommandStage::Opening => 5,
        ProjectCommandStage::ConfiguringAssignment => 6,
        ProjectCommandStage::StartingRuntime => 7,
        ProjectCommandStage::ValidatingLaunchDirectory => 8,
        ProjectCommandStage::MakingRunnable => 9,
        ProjectCommandStage::DispatchingInputs => 10,
        ProjectCommandStage::AssessingRelease => 11,
        ProjectCommandStage::QuiescingRuntime => 12,
        ProjectCommandStage::EndingAssignment => 13,
        ProjectCommandStage::Closing => 14,
        ProjectCommandStage::UpdatingProject => 15,
        ProjectCommandStage::ReservingDestination => 16,
        ProjectCommandStage::ReconcilingGit => 17,
        ProjectCommandStage::CreatingWorktree => 18,
        ProjectCommandStage::IdentifyingResource => 19,
        ProjectCommandStage::CreatingProject => 20,
        ProjectCommandStage::Compensating => 21,
        ProjectCommandStage::ReconciliationRequired => 22,
        ProjectCommandStage::Complete => 23,
    }
}

fn decode_stage(value: i64) -> rusqlite::Result<ProjectCommandStage> {
    Ok(match value {
        1 => ProjectCommandStage::Accepted,
        2 => ProjectCommandStage::AwaitingHome,
        3 => ProjectCommandStage::ReceivedAtHome,
        4 => ProjectCommandStage::ValidatingResources,
        5 => ProjectCommandStage::Opening,
        6 => ProjectCommandStage::ConfiguringAssignment,
        7 => ProjectCommandStage::StartingRuntime,
        8 => ProjectCommandStage::ValidatingLaunchDirectory,
        9 => ProjectCommandStage::MakingRunnable,
        10 => ProjectCommandStage::DispatchingInputs,
        11 => ProjectCommandStage::AssessingRelease,
        12 => ProjectCommandStage::QuiescingRuntime,
        13 => ProjectCommandStage::EndingAssignment,
        14 => ProjectCommandStage::Closing,
        15 => ProjectCommandStage::UpdatingProject,
        16 => ProjectCommandStage::ReservingDestination,
        17 => ProjectCommandStage::ReconcilingGit,
        18 => ProjectCommandStage::CreatingWorktree,
        19 => ProjectCommandStage::IdentifyingResource,
        20 => ProjectCommandStage::CreatingProject,
        21 => ProjectCommandStage::Compensating,
        22 => ProjectCommandStage::ReconciliationRequired,
        23 => ProjectCommandStage::Complete,
        _ => return Err(decode_error()),
    })
}

fn encode_effect(state: &StoredProjectEffectState) -> (i64, Option<i64>, Option<&str>) {
    match state {
        StoredProjectEffectState::NotStarted => (1, None, None),
        StoredProjectEffectState::Pending => (2, None, None),
        StoredProjectEffectState::Accepted => (3, None, None),
        StoredProjectEffectState::Rejected(error) => (
            4,
            Some(encode_error_category(error.category())),
            Some(error.code().as_str()),
        ),
        StoredProjectEffectState::Uncertain(error) => (
            5,
            Some(encode_error_category(error.category())),
            Some(error.code().as_str()),
        ),
    }
}

fn decode_effect(
    value: i64,
    error_category: Option<i64>,
    error_code: Option<String>,
) -> rusqlite::Result<StoredProjectEffectState> {
    match (value, error_category, error_code) {
        (1, None, None) => Ok(StoredProjectEffectState::NotStarted),
        (2, None, None) => Ok(StoredProjectEffectState::Pending),
        (3, None, None) => Ok(StoredProjectEffectState::Accepted),
        (4, Some(category), Some(code)) => Ok(StoredProjectEffectState::Rejected(
            decode_domain_error(category, code)?,
        )),
        (5, Some(category), Some(code)) => Ok(StoredProjectEffectState::Uncertain(
            decode_domain_error(category, code)?,
        )),
        _ => Err(decode_error()),
    }
}

const fn encode_error_category(category: ErrorCategory) -> i64 {
    match category {
        ErrorCategory::InvalidInput => 1,
        ErrorCategory::Conflict => 2,
        ErrorCategory::Unauthorized => 3,
        ErrorCategory::Unresolved => 4,
        ErrorCategory::NotFound => 5,
        ErrorCategory::InvariantViolation => 6,
    }
}

fn decode_domain_error(category: i64, code: String) -> rusqlite::Result<DomainError> {
    let category = match category {
        1 => ErrorCategory::InvalidInput,
        2 => ErrorCategory::Conflict,
        3 => ErrorCategory::Unauthorized,
        4 => ErrorCategory::Unresolved,
        5 => ErrorCategory::NotFound,
        6 => ErrorCategory::InvariantViolation,
        _ => return Err(decode_error()),
    };
    let code = ErrorCode::new(code).map_err(|_| decode_error())?;
    Ok(DomainError::new(category, code))
}

const fn encode_scheme(scheme: ResourceScheme) -> i64 {
    match scheme {
        ResourceScheme::GitRepository => 1,
        ResourceScheme::WorkingTree => 2,
        ResourceScheme::Container => 3,
        ResourceScheme::Opaque => 4,
    }
}

fn decode_reservation(
    scheme: Option<i64>,
    value: Option<String>,
) -> rusqlite::Result<Option<ResourceLocator>> {
    match (scheme, value) {
        (None, None) => Ok(None),
        (Some(scheme), Some(value)) => {
            let scheme = match scheme {
                1 => ResourceScheme::GitRepository,
                2 => ResourceScheme::WorkingTree,
                3 => ResourceScheme::Container,
                4 => ResourceScheme::Opaque,
                _ => return Err(decode_error()),
            };
            let value = BoundedText::new(value).map_err(|_| decode_error())?;
            Ok(Some(ResourceLocator::new(scheme, value)))
        }
        _ => Err(decode_error()),
    }
}

fn optional_operation_bytes(value: Option<OperationId>) -> Option<Vec<u8>> {
    value.map(|value| value.as_bytes().to_vec())
}

fn bounded_limit(limit: usize) -> Result<i64, StoreError> {
    if !(1..=MAX_PROJECT_SAGA_QUERY_ITEMS).contains(&limit) {
        return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
    }
    i64::try_from(limit).map_err(|_| StoreError::new(StoreErrorClass::InvalidOperationalRequest))
}

fn blob32(value: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    value.try_into().map_err(|_| decode_error())
}

fn optional_blob32(value: Option<Vec<u8>>) -> rusqlite::Result<Option<[u8; 32]>> {
    value.map(blob32).transpose()
}

fn blob8(value: Vec<u8>) -> rusqlite::Result<[u8; 8]> {
    value.try_into().map_err(|_| decode_error())
}

fn decode_error() -> rusqlite::Error {
    rusqlite::Error::InvalidQuery
}

fn conflict() -> StoreError {
    StoreError::new(StoreErrorClass::ProjectSagaConflict)
}

fn corrupt() -> StoreError {
    StoreError::new(StoreErrorClass::OperationalStateCorrupt)
}

fn database(_: rusqlite::Error) -> StoreError {
    StoreError::new(StoreErrorClass::DatabaseUnavailable)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().expect("memory database opens");
        connection
            .execute_batch(super::super::SCHEMA)
            .expect("schema creates");
        connection
    }

    fn saga() -> StoredProjectSaga {
        StoredProjectSaga {
            operation_id: OperationId::from_bytes([1; 32]),
            command_id: CommandId::from_bytes([2; 32]),
            request_digest: CommandDigest::from_bytes([3; 32]),
            account_id: AccountId::from_bytes([4; 32]),
            project_id: ProjectId::from_bytes([5; 32]),
            home: InstallationId::from_bytes([6; 32]),
            expected_head: None,
            issued_at: Timestamp::from_unix_millis(7),
            command_body: b"provision-worktree".to_vec(),
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
            reservation: Some(ResourceLocator::new(
                ResourceScheme::WorkingTree,
                BoundedText::new("/repo/worktrees/feature").expect("locator"),
            )),
            updated_at_millis: 8,
        }
    }

    fn row_counts(connection: &Connection) -> (i64, i64) {
        let sagas = connection
            .query_row("SELECT count(*) FROM project_sagas", [], |row| row.get(0))
            .expect("saga count reads");
        let reservations = connection
            .query_row(
                "SELECT count(*) FROM project_saga_reservations",
                [],
                |row| row.get(0),
            )
            .expect("reservation count reads");
        (sagas, reservations)
    }

    #[test]
    fn begin_failpoints_roll_back_record_and_reservation_or_replay_post_commit() {
        let proposed = saga();
        for failpoint in [
            ProjectSagaFailpoint::AfterRecordWrite,
            ProjectSagaFailpoint::AfterReservationWrite,
            ProjectSagaFailpoint::BeforeCommit,
        ] {
            let mut connection = connection();
            let error = begin_with_failpoint(&mut connection, &proposed, failpoint)
                .expect_err("pre-commit failpoint interrupts begin");
            assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
            assert_eq!(row_counts(&connection), (0, 0));
        }

        let mut connection = connection();
        let error = begin_with_failpoint(
            &mut connection,
            &proposed,
            ProjectSagaFailpoint::AfterCommit,
        )
        .expect_err("response loss follows commit");
        assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
        assert_eq!(row_counts(&connection), (1, 1));
        assert_eq!(
            begin(&mut connection, &proposed).expect("exact retry reconciles"),
            StoredProjectSagaBegin::Existing(proposed)
        );
    }

    #[test]
    fn replace_failpoints_preserve_the_old_pair_or_the_complete_terminal_pair() {
        let original = saga();
        let mut completed = original.clone();
        completed.git_operation_id = Some(OperationId::from_bytes([9; 32]));
        completed.git_effect = StoredProjectEffectState::Accepted;
        completed.state = StoredProjectSagaState::Completed(FactId::from_bytes([10; 32]));
        completed.updated_at_millis += 1;

        for failpoint in [
            ProjectSagaFailpoint::AfterReservationWrite,
            ProjectSagaFailpoint::AfterRecordWrite,
            ProjectSagaFailpoint::AfterProtectionWrite,
            ProjectSagaFailpoint::AfterReservationRelease,
            ProjectSagaFailpoint::BeforeCommit,
        ] {
            let mut connection = connection();
            begin(&mut connection, &original).expect("initial pair commits");
            let error = replace_with_failpoint(&mut connection, &completed, failpoint)
                .expect_err("pre-commit failpoint interrupts replace");
            assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
            assert_eq!(
                load_operation(&connection, original.operation_id),
                Ok(Some(original.clone()))
            );
            assert_eq!(row_counts(&connection), (1, 1));
        }

        let mut connection = connection();
        begin(&mut connection, &original).expect("initial pair commits");
        let error = replace_with_failpoint(
            &mut connection,
            &completed,
            ProjectSagaFailpoint::AfterCommit,
        )
        .expect_err("response loss follows terminal commit");
        assert_eq!(error.class(), StoreErrorClass::DatabaseUnavailable);
        assert_eq!(
            load_operation(&connection, original.operation_id),
            Ok(Some(completed.clone()))
        );
        assert_eq!(row_counts(&connection), (1, 0));
        replace(&mut connection, &completed).expect("exact terminal retry reconciles");
        assert_eq!(row_counts(&connection), (1, 0));
    }
}
