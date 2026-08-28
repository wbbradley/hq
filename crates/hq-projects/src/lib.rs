//! Durable, home-authoritative project workflow orchestration.
//!
//! Canonical project facts remain authoritative. Records in this crate only retain enough
//! coordination state to decide whether an external effect is safe to start, retry, or reconcile.

mod canonical;
mod command_codec;
mod git_worktree;
mod remote;
mod remote_canonical;
mod workflow;

use hq_application::{
    ApplicationError, ApplicationErrorCode, ControlProjects, ProjectCommandAction,
    ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
};
use hq_domain::{
    AccountId, CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode, FactId,
    InstallationId, OperationId, ProjectId, ProviderSessionId, ThreadId, Timestamp,
};

pub use canonical::ApplicationCanonicalProjectPort;
pub use command_codec::{
    ProjectCommandCodecError, agent_retirement_request_digest, decode_canonical_project_mutation,
    decode_project_command_action, encode_canonical_project_mutation,
    encode_project_command_action, project_command_request_digest,
};
pub use git_worktree::{GitWorktreeAdapter, GitWorktreeAdapterConfig};
pub use remote::*;
pub use remote_canonical::ApplicationRemoteProjectCommandPort;
pub use workflow::*;

/// Maximum records returned by one startup recovery scan.
pub const MAX_RUNNABLE_SAGAS: usize = 1_024;

/// Bounded local workflow recovery used by the node-owned project worker.
pub trait RepairLocalProjectWorkflows: ControlProjects {
    /// Repairs one deterministic bounded prefix of durable local workflows.
    fn repair_local(&self, limit: usize) -> Result<Vec<ProjectCommandOutcome>, ApplicationError>;
}

/// Complete project intake plus local and home-targeted startup recovery.
pub trait ProjectWorkerPort: ControlProjects {
    /// Repairs bounded local and remote workflow prefixes at one explicit semantic time.
    fn repair_pending(
        &self,
        received_at: Timestamp,
        limit: usize,
    ) -> Result<Vec<ProjectCommandOutcome>, ApplicationError>;
}

/// Definite or unknown observation of one external workflow effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SagaEffectState {
    /// The effect has not crossed its boundary.
    NotStarted,
    /// Durable intent exists and the effect may be in progress.
    Pending,
    /// The exact effect was authoritatively accepted.
    Accepted,
    /// The exact effect was authoritatively rejected.
    Rejected(DomainError),
    /// Whether the exact effect happened is unknown; lookup is mandatory.
    Uncertain(DomainError),
}

/// Durable project workflow state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectSagaState {
    /// Intake committed before any further boundary.
    Running(ProjectCommandStage),
    /// The canonical project transition committed at this head.
    Completed {
        /// Resulting authoritative project head.
        project_head: FactId,
    },
    /// The command definitely did not perform an unknown external effect.
    Rejected(DomainError),
    /// External truth is unknown and exact reconciliation remains possible.
    Reconcilable {
        /// Checkpoint from which reconciliation resumes.
        stage: ProjectCommandStage,
        /// Typed stable reason.
        error: DomainError,
    },
}

impl ProjectSagaState {
    /// Reports whether no further workflow work is permitted or required.
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Rejected(_))
    }
}

/// Passive durable coordination record for one exact command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSagaRecord {
    /// Stable command identity used for canonical replay.
    pub command_id: CommandId,
    /// Stable workflow and external correlation identity.
    pub operation_id: OperationId,
    /// Digest of every exact request field.
    pub request_digest: CommandDigest,
    /// Active human account authorizing the command.
    pub account_id: AccountId,
    /// Target or to-be-created project.
    pub project_id: ProjectId,
    /// Immutable project home.
    pub home: InstallationId,
    /// Expected canonical project head, absent only for new-project provisioning.
    pub expected_head: Option<FactId>,
    /// Caller-supplied semantic time.
    pub issued_at: Timestamp,
    /// Closed requested behavior.
    pub action: ProjectCommandAction,
    /// Current durable state.
    pub state: ProjectSagaState,
    /// Runtime start/resume/stop boundary state.
    pub runtime_operation_id: Option<OperationId>,
    /// Runtime start/resume/stop boundary state.
    pub runtime_effect: SagaEffectState,
    /// Exact provider session acknowledged by runtime readiness.
    pub runtime_session: Option<ProviderSessionId>,
    /// Exact project thread selected before the runnable transition.
    pub selected_thread: Option<ThreadId>,
    /// Whether this workflow opened a project that was closed at intake.
    pub opened_by_workflow: bool,
    /// Original definite activation failure retained across compensation retries.
    pub failure: Option<DomainError>,
    /// Exact in-flight canonical compare-and-swap retained until its result is definite.
    pub pending_canonical_mutation: Option<CanonicalProjectMutation>,
    /// Provider submission boundary state.
    pub dispatch_operation_id: Option<OperationId>,
    /// Provider submission boundary state.
    pub dispatch_effect: SagaEffectState,
    /// Git lookup/create boundary state.
    pub git_operation_id: Option<OperationId>,
    /// Git lookup/create boundary state.
    pub git_effect: SagaEffectState,
    /// Resource observation boundary state.
    pub resource_operation_id: Option<OperationId>,
    /// Resource observation boundary state.
    pub resource_effect: SagaEffectState,
    /// Normalized destination protected for provisioning, when any.
    pub reservation: Option<hq_domain::ResourceLocator>,
    /// Injected deterministic recovery ordering key.
    pub updated_at_millis: u64,
}

impl ProjectSagaRecord {
    /// Creates the initial durable record for an exact request.
    pub fn from_request(request: ProjectCommandRequest) -> Self {
        let reservation = match &request.action {
            ProjectCommandAction::ProvisionWorktree(provisioning) => {
                Some(provisioning.destination.clone())
            }
            _ => None,
        };
        Self {
            command_id: request.command_id,
            operation_id: request.operation_id,
            request_digest: request.request_digest,
            account_id: request.account_id,
            project_id: request.project_id,
            home: request.home,
            expected_head: request.expected_head,
            issued_at: request.issued_at,
            action: request.action,
            state: ProjectSagaState::Running(ProjectCommandStage::Accepted),
            runtime_operation_id: None,
            runtime_effect: SagaEffectState::NotStarted,
            runtime_session: None,
            selected_thread: None,
            opened_by_workflow: false,
            failure: None,
            pending_canonical_mutation: None,
            dispatch_operation_id: None,
            dispatch_effect: SagaEffectState::NotStarted,
            git_operation_id: None,
            git_effect: SagaEffectState::NotStarted,
            resource_operation_id: None,
            resource_effect: SagaEffectState::NotStarted,
            reservation,
            updated_at_millis: u64::try_from(request.issued_at.as_unix_millis())
                .unwrap_or_default(),
        }
    }
}

/// Atomic result of beginning one exact saga.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BeginSagaOutcome {
    /// This call durably inserted the command.
    Inserted(ProjectSagaRecord),
    /// The exact command and digest already exist.
    Existing(ProjectSagaRecord),
    /// The operation identity exists with a different digest or immutable request.
    IdentityConflict,
    /// Another unresolved state-changing command already owns this project.
    ProjectBusy,
}

/// Closed persistence-boundary failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SagaStoreError {
    /// A monotonic transition or stable identity conflicted with retained state.
    Conflict,
    /// Durable state is temporarily unavailable.
    Unavailable,
    /// Durable state failed strict validation.
    Corrupt,
}

/// Durable exact-replay project workflow state capability.
pub trait ProjectSagaStore {
    /// Loads one exact retained workflow by its stable operation identity.
    fn find(&self, operation_id: OperationId) -> Result<Option<ProjectSagaRecord>, SagaStoreError>;

    /// Inserts one command atomically or returns its exact retained disposition.
    fn begin(&self, record: ProjectSagaRecord) -> Result<BeginSagaOutcome, SagaStoreError>;

    /// Replaces one record only through a validated monotonic transition.
    fn replace(&self, record: ProjectSagaRecord) -> Result<(), SagaStoreError>;

    /// Loads a deterministic bounded set of nonterminal or reconcilable records.
    fn runnable(&self, limit: usize) -> Result<Vec<ProjectSagaRecord>, SagaStoreError>;
}

/// Serialized owner of project command intake and recovery.
pub struct ProjectSagaManager<S> {
    store: S,
}

impl<S: ProjectSagaStore> ProjectSagaManager<S> {
    /// Creates a manager around its durable coordination capability.
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    /// Durably accepts one exact command without crossing an external-effect boundary.
    pub fn accept(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        let operation_id = request.operation_id;
        let proposed = ProjectSagaRecord::from_request(request);
        match self
            .store
            .begin(proposed)
            .map_err(application_store_error)?
        {
            BeginSagaOutcome::Inserted(record) | BeginSagaOutcome::Existing(record) => {
                Ok(outcome(&record))
            }
            BeginSagaOutcome::IdentityConflict => Ok(ProjectCommandOutcome::Rejected {
                operation_id,
                error: domain_error(ErrorCategory::Conflict, "project_command_identity_conflict")?,
                runtime: None,
            }),
            BeginSagaOutcome::ProjectBusy => Ok(ProjectCommandOutcome::Rejected {
                operation_id,
                error: domain_error(ErrorCategory::Conflict, "project_command_in_progress")?,
                runtime: None,
            }),
        }
    }

    /// Loads bounded startup work, rejecting unbounded adapter requests.
    pub fn runnable(&self, limit: usize) -> Result<Vec<ProjectSagaRecord>, ApplicationError> {
        if limit == 0 || limit > MAX_RUNNABLE_SAGAS {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
        }
        self.store.runnable(limit).map_err(application_store_error)
    }
}

fn outcome(record: &ProjectSagaRecord) -> ProjectCommandOutcome {
    match &record.state {
        ProjectSagaState::Running(ProjectCommandStage::Accepted) => {
            ProjectCommandOutcome::Accepted {
                operation_id: record.operation_id,
                stage: ProjectCommandStage::Accepted,
            }
        }
        ProjectSagaState::Running(stage) => ProjectCommandOutcome::Running {
            operation_id: record.operation_id,
            stage: *stage,
        },
        ProjectSagaState::Completed { project_head } => ProjectCommandOutcome::Completed {
            operation_id: record.operation_id,
            project_head: *project_head,
            runtime: None,
        },
        ProjectSagaState::Rejected(error) => ProjectCommandOutcome::Rejected {
            operation_id: record.operation_id,
            error: error.clone(),
            runtime: None,
        },
        ProjectSagaState::Reconcilable { stage, error } => ProjectCommandOutcome::Reconcilable {
            operation_id: record.operation_id,
            stage: *stage,
            error: error.clone(),
        },
    }
}

fn domain_error(
    category: ErrorCategory,
    code: &'static str,
) -> Result<DomainError, ApplicationError> {
    ErrorCode::new(code)
        .map(|code| DomainError::new(category, code))
        .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvariantViolation))
}

const fn application_store_error(error: SagaStoreError) -> ApplicationError {
    match error {
        SagaStoreError::Conflict => {
            ApplicationError::new(ApplicationErrorCode::StateIdentityConflict)
        }
        SagaStoreError::Unavailable => {
            ApplicationError::new(ApplicationErrorCode::AdapterUnavailable)
        }
        SagaStoreError::Corrupt => ApplicationError::new(ApplicationErrorCode::StateCorrupt),
    }
}
