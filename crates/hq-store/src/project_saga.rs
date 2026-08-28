//! Store-owned durable project workflow records.

use hq_application::ProjectCommandStage;
use hq_domain::{
    AccountId, CommandDigest, CommandId, DomainError, FactId, InstallationId, OperationId,
    ProjectId, ProviderSessionId, ResourceLocator, ThreadId, Timestamp,
};

/// Maximum opaque canonical project-command body bytes retained per saga.
pub const MAX_PROJECT_COMMAND_BODY_BYTES: usize = 65_536;
/// Maximum project saga rows returned by one deterministic recovery scan.
pub const MAX_PROJECT_SAGA_QUERY_ITEMS: usize = 1_024;

/// Definite or unknown observation of one external project workflow effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoredProjectEffectState {
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

/// Durable terminal or recoverable project workflow disposition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoredProjectSagaState {
    /// Bounded execution may continue from this checkpoint.
    Running(ProjectCommandStage),
    /// A canonical stable state committed at this project head.
    Completed(FactId),
    /// The command definitely failed without unknown external truth.
    Rejected(DomainError),
    /// Exact external reconciliation is required before retry.
    Reconcilable {
        /// Workflow checkpoint at the uncertain boundary.
        stage: ProjectCommandStage,
        /// Stable typed reason.
        error: DomainError,
    },
}

/// Passive durable project workflow record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredProjectSaga {
    /// Stable workflow identity.
    pub operation_id: OperationId,
    /// Stable canonical-command identity.
    pub command_id: CommandId,
    /// Digest of every exact request field.
    pub request_digest: CommandDigest,
    /// Active human account authorizing the request.
    pub account_id: AccountId,
    /// Target or to-be-created project.
    pub project_id: ProjectId,
    /// Immutable project home.
    pub home: InstallationId,
    /// Expected canonical project head.
    pub expected_head: FactId,
    /// Caller-supplied semantic time.
    pub issued_at: Timestamp,
    /// Strict versioned command bytes owned by the workflow codec.
    pub command_body: Vec<u8>,
    /// Current durable state.
    pub state: StoredProjectSagaState,
    /// Stable runtime boundary correlation, when derived.
    pub runtime_operation_id: Option<OperationId>,
    /// Runtime boundary disposition.
    pub runtime_effect: StoredProjectEffectState,
    /// Exact acknowledged runtime session, when ready.
    pub runtime_session: Option<ProviderSessionId>,
    /// Exact selected project thread, when known.
    pub selected_thread: Option<ThreadId>,
    /// Whether this workflow conditionally opened the project.
    pub opened_by_workflow: bool,
    /// Original definite failure retained while compensation reconciles.
    pub failure: Option<DomainError>,
    /// Strict workflow-owned encoding of an in-flight canonical compare-and-swap.
    pub pending_canonical_mutation: Option<Vec<u8>>,
    /// Stable provider-delivery correlation, when derived.
    pub dispatch_operation_id: Option<OperationId>,
    /// Provider-delivery boundary disposition.
    pub dispatch_effect: StoredProjectEffectState,
    /// Stable Git boundary correlation, when derived.
    pub git_operation_id: Option<OperationId>,
    /// Git boundary disposition.
    pub git_effect: StoredProjectEffectState,
    /// Stable resource-observation correlation, when derived.
    pub resource_operation_id: Option<OperationId>,
    /// Resource-observation boundary disposition.
    pub resource_effect: StoredProjectEffectState,
    /// Normalized worktree destination protected by this saga, when any.
    pub reservation: Option<ResourceLocator>,
    /// Injected ordering key for bounded deterministic recovery scans.
    pub updated_at_millis: u64,
}

/// Atomic result of beginning one exact stored saga.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoredProjectSagaBegin {
    /// This call inserted the exact record.
    Inserted(StoredProjectSaga),
    /// The exact immutable request already exists.
    Existing(StoredProjectSaga),
    /// The operation or command identity exists with unequal immutable input.
    IdentityConflict,
    /// Another unresolved command currently serializes this project.
    ProjectBusy,
}
