//! Provider-neutral passive interaction observations and exact response controls.

use hq_domain::{
    AgentId, BoundedVec, CommandDigest, ContentText, OperationId, ProjectId, ProviderId,
    ProviderSessionId, ShortText,
};
use sha2::{Digest, Sha256};

use crate::ApplicationError;

/// Maximum interaction choices carried across an application boundary.
pub const MAX_INTERACTION_CHOICES: usize = 64;
/// Maximum pending interactions returned by one bounded query.
pub const MAX_PENDING_INTERACTIONS: usize = 256;

/// Opaque provider-originated request identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InteractionId([u8; 32]);

impl InteractionId {
    /// Constructs an identity from exact neutral bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrows the exact neutral bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Closed provider-neutral interaction class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionKind {
    /// Ask for bounded text or one offered choice.
    Question,
    /// Approve or deny command execution.
    CommandApproval,
    /// Approve or deny file changes.
    FileApproval,
    /// Grant or deny a permission scope.
    Permission,
    /// Approve, decline, or cancel an MCP URL request.
    McpUrl,
    /// Supply or cancel bounded structured MCP form input.
    McpForm,
}

/// One exact stable value with a human-facing label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionChoice {
    /// Untouched stable response value.
    pub value: ShortText,
    /// Human-readable label.
    pub label: ShortText,
}

/// One memory-only pending provider interaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingInteraction {
    /// Named agent awaiting the response.
    pub agent_id: AgentId,
    /// Project whose work is blocked, when the worker is project-assigned.
    pub project_id: Option<ProjectId>,
    /// Neutral provider namespace.
    pub provider: ProviderId,
    /// Exact live provider session.
    pub session: ProviderSessionId,
    /// Provider-originated stable request identity.
    pub request_id: InteractionId,
    /// Operation blocked on this request.
    pub operation_id: OperationId,
    /// Typed request family.
    pub kind: InteractionKind,
    /// Exact bounded non-secret prompt.
    pub prompt: ContentText,
    /// Source-ordered stable choices.
    pub choices: BoundedVec<InteractionChoice, MAX_INTERACTION_CHOICES>,
    /// Whether bounded free-text input is permitted.
    pub allow_text: bool,
}

/// Closed non-secret human response shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionResponse {
    /// Bounded free text or encoded structured form content.
    Text(ContentText),
    /// One untouched offered stable value.
    Choice(ShortText),
    /// Explicit approval or denial.
    Approval(bool),
    /// Explicit cancellation without affirmative authority.
    Cancelled,
}

/// Stable exact-once command answering one pending interaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionAnswerRequest {
    /// Caller-selected command identity used for response-loss replay.
    command_id: OperationId,
    /// Digest of every exact request field and typed response.
    request_digest: CommandDigest,
    /// Named agent that owns the provider session.
    agent_id: AgentId,
    /// Provider-originated request identity.
    request_id: InteractionId,
    /// Complete typed terminal response.
    response: InteractionResponse,
}

impl InteractionAnswerRequest {
    /// Constructs one command and derives its digest from every exact semantic field.
    pub fn new(
        command_id: OperationId,
        agent_id: AgentId,
        request_id: InteractionId,
        response: InteractionResponse,
    ) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"hq-interaction-answer-v1\0");
        digest.update(agent_id.as_bytes());
        digest.update(request_id.as_bytes());
        match &response {
            InteractionResponse::Text(value) => {
                digest.update([1]);
                digest.update(value.as_str().len().to_be_bytes());
                digest.update(value.as_str().as_bytes());
            }
            InteractionResponse::Choice(value) => {
                digest.update([2]);
                digest.update(value.as_str().len().to_be_bytes());
                digest.update(value.as_str().as_bytes());
            }
            InteractionResponse::Approval(value) => digest.update([3, u8::from(*value)]),
            InteractionResponse::Cancelled => digest.update([4]),
        }
        Self {
            command_id,
            request_digest: CommandDigest::from_bytes(digest.finalize().into()),
            agent_id,
            request_id,
            response,
        }
    }

    /// Returns the stable command identity.
    pub const fn command_id(&self) -> OperationId {
        self.command_id
    }

    /// Returns the digest of every exact semantic request field.
    pub const fn request_digest(&self) -> CommandDigest {
        self.request_digest
    }

    /// Returns the named owner agent.
    pub const fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// Returns the provider-originated request identity.
    pub const fn request_id(&self) -> InteractionId {
        self.request_id
    }

    /// Borrows the complete typed response.
    pub const fn response(&self) -> &InteractionResponse {
        &self.response
    }
}

/// Terminal answer command outcome retained for equal replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionAnswerOutcome {
    /// The response was delivered to the sole provider-session owner.
    Answered,
    /// The request was already absent or terminal.
    Stale,
}

/// Active registration lease owned by one interactive local session.
pub trait InteractionResponderLease: Send {
    /// Activates response availability only after the acknowledgement frame is written.
    fn activate(&mut self) -> Result<(), ApplicationError>;
}

/// Bounded passive interaction observation capability.
pub trait QueryInteractions {
    /// Loads pending interactions in stable source-owner order.
    fn pending_interactions(
        &self,
        _limit: usize,
    ) -> Result<Vec<PendingInteraction>, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

/// Exact answer and session-scoped responder registration capability.
pub trait ControlInteractions {
    /// Executes or reconciles one exact terminal interaction response.
    fn answer_interaction(
        &self,
        _request: InteractionAnswerRequest,
    ) -> Result<InteractionAnswerOutcome, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Prepares a pending responder whose owned lease cancels on drop.
    fn prepare_interaction_responder(
        &self,
        _responder_id: OperationId,
    ) -> Result<Box<dyn InteractionResponderLease>, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}
