//! Provider-neutral operation-control DTOs for local protocol v1.

use super::{ActivityStatusDto, ConversationKeyDto, Id32, RuntimeFailureReasonDto};
use serde::{Deserialize, Serialize};

/// Exact actor and selected conversation, with optional prior operation observation.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationQueryDto {
    pub account_id: Id32,
    pub home: Id32,
    pub conversation: ConversationKeyDto,
    pub tracked_operation: Option<Id32>,
}

/// Current project assignment authorizing the target.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationProjectDto {
    pub project_id: Id32,
    pub assignment_id: Id32,
    pub thread_id: Id32,
}

/// Exact canonical agent and session binding.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationScopeDto {
    pub agent_id: Id32,
    pub mailbox_installation: Id32,
    pub mailbox_id: Id32,
    pub provider: String,
    pub session: String,
    pub project: Option<AgentOperationProjectDto>,
}

/// Complete live cancellation target; replay cannot select newer work.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationTargetDto {
    pub query: AgentOperationQueryDto,
    pub scope: AgentOperationScopeDto,
    pub generation: Id32,
    pub owner: Id32,
    pub operation_id: Id32,
    pub sequence: u64,
}

/// Canonical work status, independent of cancellation acknowledgement.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationStatusDto {
    pub operation_id: Id32,
    pub sequence: u64,
    pub status: ActivityStatusDto,
}

/// Current optional capability and previously targeted operation state.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationViewDto {
    pub target: Option<AgentOperationTargetDto>,
    pub tracked: Option<AgentOperationStatusDto>,
}

/// Stable cancellation identity plus exact target for submit or observation.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCancellationRequestDto {
    pub request_id: Id32,
    pub target: AgentOperationTargetDto,
}

/// Admission/delivery progress; Requested does not claim terminal interruption.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentCancellationStateDto {
    Queued,
    Requested,
    AlreadyFinished,
    Rejected { reason: RuntimeFailureReasonDto },
    Uncertain { reason: RuntimeFailureReasonDto },
}
