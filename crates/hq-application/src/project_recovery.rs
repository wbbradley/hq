//! Exact passive project runtime evidence independent of provider diagnostics.
use hq_domain::{AssignmentBinding, MessageId, OperationId, ProjectId, ResourceLocator, ThreadId};
use std::{fmt, num::NonZeroU64};

macro_rules! runtime_identity {
    ($name:ident, $documentation:literal) => {
        #[doc = $documentation]
        #[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; 32]);
        impl $name {
            /// Validates an injected nonzero opaque identity.
            pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
                (bytes != [0; 32]).then_some(Self(bytes))
            }
            /// Borrows exact bytes for trusted persistence and correlation adapters.
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([redacted])"))
            }
        }
    };
}
runtime_identity!(
    RuntimeGenerationId,
    "Identity of one node-owned runtime generation."
);
runtime_identity!(
    RuntimeWorkerOwner,
    "Opaque identity of one exact live runtime worker owner."
);

/// Durable recovery scope; restarting a node does not create a new assignment retry budget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeScope {
    /// Exact canonical project.
    pub project_id: ProjectId,
    /// Exact assignment, agent, provider and saved session.
    pub binding: AssignmentBinding,
    /// Exact selected conversation thread.
    pub thread_id: ThreadId,
}

/// Exact saved-session readiness request, never a request to start a fresh conversation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectReadinessRequest {
    /// Exact node generation admitted before the external boundary.
    pub generation: RuntimeGenerationId,
    /// Complete canonical correlation revalidated after the external boundary.
    pub scope: ProjectRuntimeScope,
    /// Canonically validated launch directory.
    pub launch_directory: ResourceLocator,
}

/// Point-in-time current worker evidence; persistence does not make it current after restart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeReady {
    /// Exact assignment/session for which readiness was requested.
    pub scope: ProjectRuntimeScope,
    /// Node-owned generation that observed the actual worker.
    pub generation: RuntimeGenerationId,
    /// Actual live worker owner returned by the runtime.
    pub owner: RuntimeWorkerOwner,
}

/// Passive retained lease evidence, not a statement that a worker is live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeLeaseEvidence {
    /// Exact retained owner.
    pub owner: RuntimeWorkerOwner,
    /// Absolute Unix millisecond expiry, retained even after it expires.
    pub expires_at_millis: u64,
}

/// Closed body-free runtime failure classifications; never parse provider prose for policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFailureReason {
    /// Node runtime generation changed after this attempt was admitted.
    GenerationChanged,
    /// Invalid neutral request.
    InvalidInput,
    /// Required provider capability is unsupported.
    Unsupported,
    /// Selected provider is not registered.
    ProviderNotRegistered,
    /// Provider registration conflicts.
    RegistrationConflict,
    /// Stable submission recovery is unavailable.
    UnsafeRecovery,
    /// Provider acknowledged a different exact session.
    SessionIdentityMismatch,
    /// The exact saved session does not exist.
    SessionNotFound,
    /// Submission identity was reused unequally.
    SubmissionIdentityConflict,
    /// Interactive response identity was already consumed.
    InteractiveAlreadyAnswered,
    /// A secret-bearing request failed the neutral boundary.
    SecretInputRejected,
    /// Runtime intake has closed.
    IntakeClosed,
    /// Provider instance ended unexpectedly.
    Crashed,
    /// Provider protocol violated a closed contract.
    ProtocolViolation,
    /// Provider transport closed independently of confirmed exit.
    TransportClosed,
    /// Owned process exited unsuccessfully.
    ProcessFailed,
    /// Provider protocol requires an unsupported capability.
    CompatibilityMismatch,
    /// Runtime cannot currently perform or determine readiness.
    Unavailable,
    /// Previous owner cleanup did not complete safely.
    CleanupFailed,
    /// Another worker owns the agent or capacity is unavailable.
    OwnershipConflict,
    /// Bounded runtime capacity is temporarily full.
    Backpressure,
    /// Stable persistence identity conflicts.
    PersistenceCollision,
}
impl RuntimeFailureReason {
    /// Stable machine code for diagnostics and strict codecs; policy matches the enum directly.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GenerationChanged => "generation_changed",
            Self::InvalidInput => "invalid_input",
            Self::Unsupported => "unsupported",
            Self::ProviderNotRegistered => "provider_not_registered",
            Self::RegistrationConflict => "registration_conflict",
            Self::UnsafeRecovery => "unsafe_recovery",
            Self::SessionIdentityMismatch => "session_identity_mismatch",
            Self::SessionNotFound => "session_not_found",
            Self::SubmissionIdentityConflict => "submission_identity_conflict",
            Self::InteractiveAlreadyAnswered => "interactive_already_answered",
            Self::SecretInputRejected => "secret_input_rejected",
            Self::IntakeClosed => "intake_closed",
            Self::Crashed => "crashed",
            Self::ProtocolViolation => "protocol_violation",
            Self::TransportClosed => "transport_closed",
            Self::ProcessFailed => "process_failed",
            Self::CompatibilityMismatch => "compatibility_mismatch",
            Self::Unavailable => "unavailable",
            Self::CleanupFailed => "cleanup_failed",
            Self::OwnershipConflict => "ownership_conflict",
            Self::Backpressure => "backpressure",
            Self::PersistenceCollision => "persistence_collision",
        }
    }
}

/// Exact readiness failure evidence used by durable recovery policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeFailure {
    /// Closed failure classification from the neutral runtime boundary.
    pub reason: RuntimeFailureReason,
    /// Exact lease evidence when ownership blocks readiness; never inferred from timestamps.
    pub lease: Option<RuntimeLeaseEvidence>,
}

/// Readiness observation distinct from provider delivery acceptance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectReadinessOutcome {
    /// Exact current worker evidence.
    Ready(ProjectRuntimeReady),
    /// Readiness could not be established; retained input still belongs to the project.
    Failed(ProjectRuntimeFailure),
}

/// Why automatic recovery is blocked; an explicit scoped retry is a separate command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRecoveryStop {
    /// Runtime failure requires intervention rather than automatic replay.
    PermanentFailure,
    /// Durable attempt budget has been consumed.
    AttemptsExhausted,
    /// No representable strictly later retry deadline exists.
    ClockRange,
    /// Policy inputs or a failed-attempt count are invalid.
    InvalidPolicy,
}

/// Durable recovery disposition; persisted ready evidence is not current liveness after restart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRecoveryState {
    /// Work may attempt recovery at this absolute Unix millisecond deadline.
    Waiting {
        /// Earliest permitted next attempt.
        retry_at_millis: u64,
    },
    /// Attempt intent committed before the external boundary.
    Attempting {
        /// Node generation that owns the attempt.
        generation: RuntimeGenerationId,
        /// Earliest recovery deadline if this attempt loses its process or result.
        recover_at_millis: u64,
    },
    /// Automatic recovery stopped pending an explicit exact-scope action.
    Blocked {
        /// Closed reason automatic retries stopped.
        reason: RuntimeRecoveryStop,
    },
    /// Exact worker was observed ready in this generation.
    Ready {
        /// Observing node generation.
        generation: RuntimeGenerationId,
        /// Actual observed worker owner.
        owner: RuntimeWorkerOwner,
        /// Recovery deadline while this input still lacks a committed canonical dispatch.
        recover_at_millis: u64,
    },
    /// Exact input dispatch committed after proven provider acceptance.
    Completed,
    /// The original scope is no longer eligible for this recovery operation.
    Cancelled,
}
impl ProjectRecoveryState {
    /// Returns an absolute deadline only for schedulable states.
    pub const fn deadline(&self) -> Option<u64> {
        match self {
            Self::Waiting { retry_at_millis } => Some(*retry_at_millis),
            Self::Attempting {
                recover_at_millis, ..
            }
            | Self::Ready {
                recover_at_millis, ..
            } => Some(*recover_at_millis),
            Self::Blocked { .. } | Self::Completed | Self::Cancelled => None,
        }
    }
    /// Whether this recovery still reserves its project against a competing recovery operation.
    pub const fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Waiting { .. }
                | Self::Attempting { .. }
                | Self::Blocked { .. }
                | Self::Ready { .. }
        )
    }
}

/// Exact pending input and owning workflow for a recovery episode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecoveryInput {
    /// Workflow to continue when this episode becomes due.
    pub saga_operation_id: OperationId,
    /// Stable provider submission identity of the selected pending input.
    pub submission_id: MessageId,
    /// Authoritative project input sequence, never storage or page order.
    pub sequence: NonZeroU64,
}

/// One recovery episode linked to an exact durable saga and immutable assignment/session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecoveryRecord {
    /// Exact input recovery episode, stable across response loss and node restart.
    pub operation_id: OperationId,
    /// Exact pending input and owning saga; other messages do not spend this budget.
    pub input: ProjectRecoveryInput,
    /// Immutable canonical recovery scope; never inferred from presentation or timestamps.
    pub scope: ProjectRuntimeScope,
    /// Monotonic compare-and-swap revision, independent of wall clock time.
    pub revision: u64,
    /// Attempts durably begun in this episode, incremented before external readiness calls.
    pub attempts: u32,
    /// Durable scheduling disposition and exact observation evidence.
    pub state: ProjectRecoveryState,
    /// Last closed failure with optional exact retained lease evidence.
    pub failure: Option<ProjectRuntimeFailure>,
}

/// Exact conditional write result; a stale result cannot overwrite newer recovery state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectRecoveryWriteOutcome {
    /// The new state committed.
    Applied,
    /// This exact state had already committed before a response was lost.
    AlreadyApplied,
    /// Expected revision, immutable identity or transition no longer matches.
    Conflict,
}

/// Exact user-requested reset of one blocked recovery budget.
/// Canonical scope eligibility must be checked by the workflow before requesting it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecoveryRetryRequest {
    /// Active human requesting another bounded recovery attempt.
    pub account_id: hq_domain::AccountId,
    /// Exact authoritative home presented with the retry action.
    pub home: hq_domain::InstallationId,

    /// Stable identity of this explicit retry, retained for response-loss replay.
    pub retry_id: OperationId,
    /// Exact original saga recovery operation.
    pub operation_id: OperationId,
    /// Complete immutable scope presented for approval.
    pub scope: ProjectRuntimeScope,
    /// Exact blocked revision observed by the user.
    pub expected_revision: u64,
}

/// Current injected runtime generation and clock used to persist an attempt before execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeRecoveryContext {
    /// Node-owned runtime generation that will execute the attempt.
    pub generation: RuntimeGenerationId,
    /// Current Unix milliseconds from the injected runtime clock.
    pub now_millis: u64,
}

/// Body-free hint that an exact project worker stopped; consumers reread current authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeStopped {
    /// Project owned by the removed worker.
    pub project_id: ProjectId,
    /// Named worker identity.
    pub agent_id: hq_domain::AgentId,
    /// Exact provider and saved session.
    pub provider: hq_domain::ProviderId,
    /// Exact saved provider conversation.
    pub session: hq_domain::ProviderSessionId,
    /// Node lifetime that owned the worker.
    pub generation: RuntimeGenerationId,
    /// Exact removed worker owner, never a display-derived identity.
    pub owner: RuntimeWorkerOwner,
    /// Stable terminal class without event bodies or provider prose.
    pub reason: RuntimeFailureReason,
}

/// Current local worker evidence, distinct from persisted attempt history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRuntimeWorkerState {
    /// No local worker exists for this node lifetime.
    Stopped,
    /// A matching local worker owns an unexpired lease at observation time.
    Ready {
        /// Exact live worker ownership at observation time.
        owner: RuntimeWorkerOwner,
    },
    /// A matching live worker has a typed currently running operation.
    Working {
        /// Exact live worker ownership at observation time.
        owner: RuntimeWorkerOwner,
        /// Authoritative running agent-turn operation identity.
        operation_id: OperationId,
        /// Source sequence of the running-turn evidence.
        sequence: NonZeroU64,
    },
    /// A runtime transition prevents immediate observation; this does not establish liveness.
    Busy,
    /// Exact ownership or session evidence does not permit readiness.
    Failed(ProjectRuntimeFailure),
}

/// Read-only observation correlated with a requested assignment and node lifetime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeObservation {
    /// Exact canonical scope supplied by the caller.
    pub scope: ProjectRuntimeScope,
    /// Current node lifetime, absent when no supervisor is running or observable.
    pub generation: Option<RuntimeGenerationId>,
    /// Current worker evidence; never reconstructed from a persisted Ready row.
    pub worker: ProjectRuntimeWorkerState,
}

/// Result of scheduling an explicit recovery retry; provider work runs in the background.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRecoveryRetryOutcome {
    /// This explicit retry was durably scheduled.
    Scheduled,
    /// The same explicit retry had already committed; its budget was not reset again.
    AlreadyScheduled,
    /// Authority, eligibility, identity, or the observed revision no longer permits retry.
    Rejected(hq_domain::DomainError),
}

/// Human-authorized, exact-scope retry capability.
pub trait RetryProjectRuntime {
    /// Validates current canonical authority before scheduling one bounded retry budget.
    fn retry_project_runtime(
        &self,
        request: ProjectRecoveryRetryRequest,
    ) -> Result<ProjectRecoveryRetryOutcome, crate::ApplicationError>;
}

/// Authorized read of current recovery evidence for one project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecoveryQuery {
    /// Human viewing the project.
    pub account_id: hq_domain::AccountId,
    /// Expected authoritative home.
    pub home: hq_domain::InstallationId,
    /// Project to observe.
    pub project_id: ProjectId,
}

/// Canonically fenced runtime and recovery evidence, independent of device connectivity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecoveryView {
    /// Canonical head checked again after collecting runtime evidence.
    pub head: hq_domain::FactId,
    /// Current exact assignment observation; absent without a runnable bound assignment.
    pub observation: Option<ProjectRuntimeObservation>,
    /// Active episode only when its exact scope and input remain canonical.
    pub recovery: Option<ProjectRecoveryRecord>,
    /// Whether the viewing human may request a reset of the displayed blocked revision.
    pub retry_allowed: bool,
}

/// Read capability independent of serialized workflow execution.
pub trait QueryProjectRecovery {
    /// Returns authorized, current-scope evidence without launching a runtime.
    fn query_project_recovery(
        &self,
        request: ProjectRecoveryQuery,
    ) -> Result<ProjectRecoveryView, crate::ApplicationError>;
}
