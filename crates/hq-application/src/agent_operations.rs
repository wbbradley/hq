//! Provider-neutral control targets derived from canonical recipient and runtime evidence.

use std::num::NonZeroU64;

use hq_domain::{
    AccountId, ActivityStatus, AgentId, AssignmentId, InstallationId, MailboxAddress, OperationId,
    ProjectId, ProviderId, ProviderSessionId, ThreadId,
};

use crate::{ConversationKey, RuntimeFailureReason, RuntimeGenerationId, RuntimeWorkerOwner};

/// Exact conversation whose recipient capabilities are being inspected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentOperationQuery {
    /// Human requesting control.
    pub account_id: AccountId,
    /// Expected local authoritative installation.
    pub home: InstallationId,
    /// Exact selected conversation; a human recipient has no agent operation capability.
    pub conversation: ConversationKey,
    /// Optional prior operation whose terminal evidence is still being observed.
    pub tracked_operation: Option<OperationId>,
}

/// Current canonical project assignment authorizing an agent's work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentOperationProject {
    /// Exact project.
    pub project_id: ProjectId,
    /// Exact current assignment.
    pub assignment_id: AssignmentId,
    /// Exact current conversation.
    pub thread_id: ThreadId,
}

/// Neutral recipient identity independent of provider wire types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentOperationScope {
    /// Named active worker.
    pub agent_id: AgentId,
    /// Singular canonical agent mailbox.
    pub mailbox: MailboxAddress,
    /// Provider namespace.
    pub provider: ProviderId,
    /// Exact durable provider session.
    pub session: ProviderSessionId,
    /// Assignment evidence for a project worker, absent for direct agent sessions.
    pub project: Option<AgentOperationProject>,
}

/// Body-free, exact live target; clients cannot retarget it to a newer operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentOperationTarget {
    /// Human and conversation authority.
    pub query: AgentOperationQuery,
    /// Current canonical worker binding.
    pub scope: AgentOperationScope,
    /// Node generation observing the worker.
    pub generation: RuntimeGenerationId,
    /// Actual worker lease owner.
    pub owner: RuntimeWorkerOwner,
    /// Exact running operation.
    pub operation_id: OperationId,
    /// Source sequence of the running evidence.
    pub sequence: NonZeroU64,
}

/// Canonical operation state, never inferred from request acknowledgement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentOperationStatus {
    /// Exact operation.
    pub operation_id: OperationId,
    /// Latest authoritative source sequence.
    pub sequence: NonZeroU64,
    /// Typed state selected from agent-turn activity.
    pub status: ActivityStatus,
}

/// Capability observation and optional terminal evidence for a previously targeted operation.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct AgentOperationView {
    /// Present only with an authorized recipient, live independent control, and exact running work.
    pub target: Option<AgentOperationTarget>,
    /// Authoritative evidence for the explicitly tracked operation.
    pub tracked: Option<AgentOperationStatus>,
}

/// Stable cancellation command retained across response loss.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCancellationRequest {
    /// Stable identity; replay must carry the identical target.
    pub request_id: OperationId,
    /// Complete displayed target.
    pub target: AgentOperationTarget,
}

/// Request progress; terminal work status comes from `AgentOperationView`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentCancellationState {
    /// Admitted to independent bounded dispatch.
    Queued,
    /// Provider acknowledged the interrupt; work may still be stopping.
    Requested,
    /// The exact operation had already finished.
    AlreadyFinished,
    /// Definite refusal with a body-free reason.
    Rejected(RuntimeFailureReason),
    /// Delivery or acknowledgement is unknown; only the same target may be retried.
    Uncertain(RuntimeFailureReason),
}

/// Authorized canonical evidence used before and after independent runtime observation.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct AgentOperationCanonical {
    /// Current eligible binding, absent for human or unsupported conversation kinds.
    pub scope: Option<AgentOperationScope>,
    /// Singular currently running operation in that exact binding.
    pub running: Option<AgentOperationStatus>,
    /// Evidence for the specifically tracked operation, including terminal state.
    pub tracked: Option<AgentOperationStatus>,
}
