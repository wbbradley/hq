//! Mapping between workflow-owned checkpoints and store-owned durable records.

use hq_projects::{
    BeginSagaOutcome, ProjectSagaRecord, ProjectSagaState, ProjectSagaStore, SagaEffectState,
    SagaStoreError, decode_project_command_action, encode_project_command_action,
};
use hq_store::{
    ProjectSagaStateHandle, StoreError, StoreErrorClass, StoredProjectEffectState,
    StoredProjectSaga, StoredProjectSagaBegin, StoredProjectSagaState,
};

/// Durable project-workflow adapter without owning store shutdown.
#[derive(Clone, Debug)]
pub struct ProjectSagaStoreAdapter {
    state: ProjectSagaStateHandle,
}

impl ProjectSagaStoreAdapter {
    /// Creates an adapter around the narrow store capability.
    pub const fn new(state: ProjectSagaStateHandle) -> Self {
        Self { state }
    }
}

impl ProjectSagaStore for ProjectSagaStoreAdapter {
    fn begin(&self, record: ProjectSagaRecord) -> Result<BeginSagaOutcome, SagaStoreError> {
        match self
            .state
            .begin(encode_record(record)?)
            .map_err(map_store_error)?
        {
            StoredProjectSagaBegin::Inserted(record) => {
                decode_record(record).map(BeginSagaOutcome::Inserted)
            }
            StoredProjectSagaBegin::Existing(record) => {
                decode_record(record).map(BeginSagaOutcome::Existing)
            }
            StoredProjectSagaBegin::IdentityConflict => Ok(BeginSagaOutcome::IdentityConflict),
            StoredProjectSagaBegin::ProjectBusy => Ok(BeginSagaOutcome::ProjectBusy),
        }
    }

    fn replace(&self, record: ProjectSagaRecord) -> Result<(), SagaStoreError> {
        self.state
            .replace(encode_record(record)?)
            .map_err(map_store_error)
    }

    fn runnable(&self, limit: usize) -> Result<Vec<ProjectSagaRecord>, SagaStoreError> {
        self.state
            .load_runnable(limit)
            .map_err(map_store_error)?
            .into_iter()
            .map(decode_record)
            .collect()
    }
}

fn encode_record(record: ProjectSagaRecord) -> Result<StoredProjectSaga, SagaStoreError> {
    let command_body = encode_project_command_action(&record.action)
        .map_err(|_| SagaStoreError::Corrupt)?
        .as_str()
        .as_bytes()
        .to_vec();
    Ok(StoredProjectSaga {
        operation_id: record.operation_id,
        command_id: record.command_id,
        request_digest: record.request_digest,
        account_id: record.account_id,
        project_id: record.project_id,
        home: record.home,
        expected_head: record.expected_head,
        issued_at: record.issued_at,
        command_body,
        state: encode_state(record.state),
        runtime_operation_id: record.runtime_operation_id,
        runtime_effect: encode_effect(&record.runtime_effect),
        runtime_session: record.runtime_session,
        selected_thread: record.selected_thread,
        opened_by_workflow: record.opened_by_workflow,
        failure: record.failure,
        pending_canonical_mutation: record
            .pending_canonical_mutation
            .as_ref()
            .map(hq_projects::encode_canonical_project_mutation)
            .transpose()
            .map_err(|_| SagaStoreError::Corrupt)?,
        dispatch_operation_id: record.dispatch_operation_id,
        dispatch_effect: encode_effect(&record.dispatch_effect),
        git_operation_id: record.git_operation_id,
        git_effect: encode_effect(&record.git_effect),
        resource_operation_id: record.resource_operation_id,
        resource_effect: encode_effect(&record.resource_effect),
        reservation: record.reservation,
        updated_at_millis: record.updated_at_millis,
    })
}

fn decode_record(record: StoredProjectSaga) -> Result<ProjectSagaRecord, SagaStoreError> {
    let command_body =
        String::from_utf8(record.command_body).map_err(|_| SagaStoreError::Corrupt)?;
    let command_body =
        hq_domain::ContentText::new(command_body).map_err(|_| SagaStoreError::Corrupt)?;
    let action =
        decode_project_command_action(&command_body).map_err(|_| SagaStoreError::Corrupt)?;
    Ok(ProjectSagaRecord {
        command_id: record.command_id,
        operation_id: record.operation_id,
        request_digest: record.request_digest,
        account_id: record.account_id,
        project_id: record.project_id,
        home: record.home,
        expected_head: record.expected_head,
        issued_at: record.issued_at,
        action,
        state: decode_state(record.state),
        runtime_operation_id: record.runtime_operation_id,
        runtime_effect: decode_effect(record.runtime_effect),
        runtime_session: record.runtime_session,
        selected_thread: record.selected_thread,
        opened_by_workflow: record.opened_by_workflow,
        failure: record.failure,
        pending_canonical_mutation: record
            .pending_canonical_mutation
            .as_deref()
            .map(hq_projects::decode_canonical_project_mutation)
            .transpose()
            .map_err(|_| SagaStoreError::Corrupt)?,
        dispatch_operation_id: record.dispatch_operation_id,
        dispatch_effect: decode_effect(record.dispatch_effect),
        git_operation_id: record.git_operation_id,
        git_effect: decode_effect(record.git_effect),
        resource_operation_id: record.resource_operation_id,
        resource_effect: decode_effect(record.resource_effect),
        reservation: record.reservation,
        updated_at_millis: record.updated_at_millis,
    })
}

fn encode_state(workflow_state: ProjectSagaState) -> StoredProjectSagaState {
    match workflow_state {
        ProjectSagaState::Running(checkpoint) => StoredProjectSagaState::Running(checkpoint),
        ProjectSagaState::Completed { project_head } => {
            StoredProjectSagaState::Completed(project_head)
        }
        ProjectSagaState::Rejected(error) => StoredProjectSagaState::Rejected(error),
        ProjectSagaState::Reconcilable { stage, error } => {
            StoredProjectSagaState::Reconcilable { stage, error }
        }
    }
}

fn decode_state(workflow_state: StoredProjectSagaState) -> ProjectSagaState {
    match workflow_state {
        StoredProjectSagaState::Running(checkpoint) => ProjectSagaState::Running(checkpoint),
        StoredProjectSagaState::Completed(project_head) => {
            ProjectSagaState::Completed { project_head }
        }
        StoredProjectSagaState::Rejected(error) => ProjectSagaState::Rejected(error),
        StoredProjectSagaState::Reconcilable { stage, error } => {
            ProjectSagaState::Reconcilable { stage, error }
        }
    }
}

fn encode_effect(state: &SagaEffectState) -> StoredProjectEffectState {
    match state {
        SagaEffectState::NotStarted => StoredProjectEffectState::NotStarted,
        SagaEffectState::Pending => StoredProjectEffectState::Pending,
        SagaEffectState::Accepted => StoredProjectEffectState::Accepted,
        SagaEffectState::Rejected(error) => StoredProjectEffectState::Rejected(error.clone()),
        SagaEffectState::Uncertain(error) => StoredProjectEffectState::Uncertain(error.clone()),
    }
}

fn decode_effect(state: StoredProjectEffectState) -> SagaEffectState {
    match state {
        StoredProjectEffectState::NotStarted => SagaEffectState::NotStarted,
        StoredProjectEffectState::Pending => SagaEffectState::Pending,
        StoredProjectEffectState::Accepted => SagaEffectState::Accepted,
        StoredProjectEffectState::Rejected(error) => SagaEffectState::Rejected(error),
        StoredProjectEffectState::Uncertain(error) => SagaEffectState::Uncertain(error),
    }
}

const fn map_store_error(error: StoreError) -> SagaStoreError {
    match error.class() {
        StoreErrorClass::ProjectSagaConflict
        | StoreErrorClass::MutationConflict
        | StoreErrorClass::IdentityCollision => SagaStoreError::Conflict,
        StoreErrorClass::CorruptDatabase
        | StoreErrorClass::InvalidEvidence
        | StoreErrorClass::OperationalStateCorrupt
        | StoreErrorClass::RebuildableStateCorrupt
        | StoreErrorClass::IncompatibleSchema => SagaStoreError::Corrupt,
        _ => SagaStoreError::Unavailable,
    }
}
