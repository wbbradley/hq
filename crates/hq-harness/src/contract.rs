//! Object-safe provider-neutral lifecycle, submission, and event vocabulary.

use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroU64, time::Duration};

use hq_domain::{
    ActivityKind, ActivityStatus, AgentId, BoundedVec, CommandDigest, ContentText, MessageId,
    OperationId, ProjectId, ProviderSessionId, ShortText,
};

use crate::HarnessEnvironment;

/// Maximum choices carried by one structured interactive request.
pub const MAX_INTERACTIVE_CHOICES: usize = 64;

/// One independently advertised neutral adapter capability.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HarnessCapability {
    /// The adapter can create a new durable session.
    StartSessions,
    /// The adapter can resume one exact known durable session.
    ResumeSessions,
    /// Repeating the same stable submission identity and digest is provider-idempotent.
    StableSubmissionIdempotency,
    /// The adapter can authoritatively look up stable submission acceptance.
    SubmissionLookup,
    /// The adapter can cancel an active operation with an explicit outcome.
    OperationCancellation,
    /// The adapter can surface and answer structured non-secret requests.
    InteractiveRequests,
}

/// Passive declaration of one adapter's neutral behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessCapabilities {
    /// Complete set of independently supported behavior.
    pub supported: BTreeSet<HarnessCapability>,
}

/// Stable neutral failure classifications; provider prose is not retained here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessErrorClass {
    /// A passive request violated a neutral contract invariant.
    InvalidInput,
    /// A requested capability is not advertised by this adapter.
    Unsupported,
    /// No registered provider owns the requested namespace.
    ProviderNotRegistered,
    /// A provider namespace was registered more than once.
    RegistrationConflict,
    /// The adapter offers no safe stable-submission recovery mechanism.
    UnsafeRecovery,
    /// A resumed session acknowledged a different durable identity.
    SessionIdentityMismatch,
    /// The exact requested durable session does not exist.
    SessionNotFound,
    /// A stable submission identity was reused for changed input.
    SubmissionIdentityConflict,
    /// One structured interactive request was answered more than once.
    InteractiveAlreadyAnswered,
    /// A secret-bearing request cannot cross or persist at the neutral boundary.
    SecretInputRejected,
    /// New request intake has already closed.
    IntakeClosed,
    /// The provider instance or session ended unexpectedly.
    Crashed,
    /// The provider protocol violated a closed framing, envelope, identity, or DTO invariant.
    ProtocolViolation,
    /// The provider transport closed or failed independently of a confirmed process exit.
    TransportClosed,
    /// The owned provider process exited unsuccessfully.
    ProcessFailed,
    /// A provider introduced an unsupported authority-bearing protocol method.
    CompatibilityMismatch,
    /// The adapter cannot currently determine or perform the requested effect.
    Unavailable,
    /// A mismatched or failed owner could not be force-stopped cleanly.
    CleanupFailed,
    /// Another exact live worker token owns the named agent.
    OwnershipConflict,
    /// A bounded neutral queue cannot accept another distinct item.
    Backpressure,
    /// A stable normalized persistence identity was reused unequally.
    PersistenceCollision,
}

/// One typed neutral harness failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HarnessError {
    /// Stable classification without provider diagnostic text.
    pub class: HarnessErrorClass,
}

impl HarnessError {
    /// Constructs a failure from its stable class.
    pub const fn new(class: HarnessErrorClass) -> Self {
        Self { class }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "managed runtime failed: {:?}", self.class)
    }
}

impl Error for HarnessError {}

/// Passive identity and optional project binding for one logical runtime owner.
pub struct HarnessInstanceRequest {
    /// Durable named-agent identity owning the runtime.
    pub agent_id: AgentId,
    /// Optional project binding; absence denotes a direct named-agent worker.
    pub project_id: Option<ProjectId>,
    /// Memory-only copied launch environment; values are redacted and never durable.
    pub environment: HarnessEnvironment,
}

impl fmt::Debug for HarnessInstanceRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HarnessInstanceRequest")
            .field("agent_id", &self.agent_id)
            .field("project_id", &self.project_id)
            .field("environment", &self.environment)
            .finish()
    }
}

/// Exact new or resumed durable-session request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessSessionRequest {
    /// Create and acknowledge one new provider-scoped durable session.
    Start,
    /// Resume only this exact existing provider-scoped durable session.
    Resume {
        /// Durable identity that must be acknowledged unchanged.
        session_id: ProviderSessionId,
    },
}

/// Stable provider submission input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessSubmission {
    /// Stable HQ message identity supplied to provider idempotency/reconciliation.
    pub submission_id: MessageId,
    /// Digest of the complete exact neutral input under this stable identity.
    pub digest: CommandDigest,
    /// HQ-managed operation identity used by normalized correlation.
    pub operation_id: OperationId,
    /// Bounded user-facing input body.
    pub body: ContentText,
}

/// Definite or explicitly uncertain result of one provider submission call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessSubmissionOutcome {
    /// The provider definitely accepted this stable identity and input.
    Accepted,
    /// The provider definitely did not accept it.
    Rejected(HarnessErrorClass),
    /// The call crossed a boundary after which acceptance is unknown.
    Uncertain(HarnessErrorClass),
}

/// Authoritative lookup result for one stable provider submission identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessSubmissionLookup {
    /// Provider history contains the exact stable identity and digest.
    Accepted,
    /// Provider history definitely does not contain the stable identity.
    Missing,
}

/// Explicit result of requesting provider-operation cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessCancellationOutcome {
    /// Cancellation was accepted for an active operation.
    Cancelled,
    /// The operation was already terminal before cancellation.
    AlreadyFinished,
    /// The provider definitely rejected cancellation.
    Rejected(HarnessErrorClass),
    /// Whether cancellation occurred is unknown and requires observation.
    Uncertain(HarnessErrorClass),
}

/// Opaque stable identity for one provider-originated interactive request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HarnessRequestId([u8; 32]);

impl HarnessRequestId {
    /// Constructs an identity from its opaque stable bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrows the opaque stable bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Supported authority-bearing interactive request class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessRequestKind {
    /// Ask for bounded non-secret human text or a selected choice.
    Question,
    /// Request explicit approval or denial.
    Approval,
    /// Request one bounded permission scope.
    Permission,
}

/// One stable value and display label in a structured request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessRequestChoice {
    /// Stable non-secret value returned to the adapter.
    pub value: ShortText,
    /// Human-readable non-secret label.
    pub label: ShortText,
}

/// Structured non-secret request emitted in provider source order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessInteractiveRequest {
    /// Stable request identity for exact-once response.
    pub request_id: HarnessRequestId,
    /// Operation blocked by this request.
    pub operation_id: OperationId,
    /// Closed request class.
    pub kind: HarnessRequestKind,
    /// Bounded non-secret prompt.
    pub prompt: ContentText,
    /// Bounded structured choices; empty means free text or boolean response.
    pub choices: BoundedVec<HarnessRequestChoice, MAX_INTERACTIVE_CHOICES>,
}

/// Neutral non-secret response shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessInteractiveResponse {
    /// Bounded free-text answer.
    Text(ContentText),
    /// Stable value selected from the offered choices.
    Choice(ShortText),
    /// Explicit approval or denial.
    Approval(bool),
    /// Cancel the interactive request without an affirmative answer.
    Cancelled,
}

/// One correlated exact-once interactive response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessInteractiveAnswer {
    /// Request identity being answered.
    pub request_id: HarnessRequestId,
    /// Non-secret typed response.
    pub response: HarnessInteractiveResponse,
}

/// User-facing normalized output presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessOutputKind {
    /// Intermediate user-facing update.
    Update,
    /// Terminal final-answer candidate.
    FinalAnswer,
}

/// Bounded normalized provider output independent of any wire DTO.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessOutput {
    /// Stable output identity; unequal reuse is a collision at persistence.
    pub output_id: MessageId,
    /// HQ operation correlation.
    pub operation_id: OperationId,
    /// Typed presentation without parsing the body.
    pub kind: HarnessOutputKind,
    /// Typed lifecycle status.
    pub status: ActivityStatus,
    /// Bounded user-facing body.
    pub body: ContentText,
}

/// Bounded normalized non-actionable provider activity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessActivity {
    /// HQ operation correlation.
    pub operation_id: OperationId,
    /// Optional provider-item identity represented as bounded opaque text.
    pub item: Option<ShortText>,
    /// Reducer-owned activity class.
    pub kind: ActivityKind,
    /// Stable coalescing/history key within the operation.
    pub logical_key: ShortText,
    /// Bounded runtime display identity.
    pub runtime: ShortText,
    /// Positive semantic sequence, independent of arrival or display time.
    pub sequence: NonZeroU64,
    /// Typed activity state.
    pub status: ActivityStatus,
    /// Bounded user-facing content.
    pub content: ContentText,
    /// Whether normalization explicitly shortened provider content.
    pub truncated: bool,
}

/// One source-ordered normalized provider event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessEvent {
    /// Actionable provider output.
    Output(HarnessOutput),
    /// Non-actionable provider activity.
    Activity(HarnessActivity),
    /// Structured authority-bearing request requiring one answer.
    InteractiveRequest(HarnessInteractiveRequest),
}

/// Result of one bounded event poll.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessEventPoll {
    /// One indivisible event in provider source order.
    Event(HarnessEvent),
    /// The bounded wait expired without an event.
    TimedOut,
    /// The session ended normally after all preceding events.
    Closed,
}

/// Explicit provider drain observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessDrainOutcome {
    /// No accepted event or interactive response remains owned by the adapter.
    Complete,
    /// The bounded wait expired with neutral work still pending.
    Pending {
        /// Accepted normalized events not yet delivered.
        event_count: usize,
        /// Interactive requests still awaiting a terminal response.
        request_count: usize,
    },
}

/// Adapter factory for one registered provider namespace.
pub trait HarnessFactory: Send + Sync {
    /// Creates one independently owned logical provider instance.
    fn create_instance(
        &self,
        request: HarnessInstanceRequest,
    ) -> Result<Box<dyn HarnessInstance>, HarnessError>;
}

/// One logical runtime instance before durable-session readiness.
pub trait HarnessInstance: Send {
    /// Starts or resumes a session and returns ownership only after exact readiness.
    fn open_session(
        self: Box<Self>,
        request: HarnessSessionRequest,
    ) -> Result<OpenedHarnessSession, HarnessError>;
}

/// Ready durable session and its sole mutable runtime capability owner.
pub struct OpenedHarnessSession {
    /// Acknowledged provider-scoped durable identity.
    pub session_id: ProviderSessionId,
    /// Sole session owner.
    pub session: Box<dyn HarnessSession>,
}

impl fmt::Debug for OpenedHarnessSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenedHarnessSession")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// Sole mutable owner of one ready provider session.
pub trait HarnessSession: Send {
    /// Submits or safely repeats one stable exact input.
    fn submit(
        &mut self,
        submission: HarnessSubmission,
    ) -> Result<HarnessSubmissionOutcome, HarnessError>;

    /// Reconciles authoritative provider acceptance for one complete durable submission.
    fn lookup_submission(
        &mut self,
        submission: &HarnessSubmission,
    ) -> Result<HarnessSubmissionLookup, HarnessError>;

    /// Requests cancellation of one exact HQ-correlated operation.
    fn cancel_operation(
        &mut self,
        operation_id: OperationId,
    ) -> Result<HarnessCancellationOutcome, HarnessError>;

    /// Polls at most `wait` for one source-ordered neutral event.
    fn poll_event(&mut self, wait: Duration) -> Result<HarnessEventPoll, HarnessError>;

    /// Answers one structured request exactly once.
    fn answer_interactive(&mut self, answer: HarnessInteractiveAnswer) -> Result<(), HarnessError>;

    /// Rejects future submission and interactive-response intake.
    fn stop_intake(&mut self) -> Result<(), HarnessError>;

    /// Waits at most `wait` for all accepted neutral work to leave the adapter.
    fn drain(&mut self, wait: Duration) -> Result<HarnessDrainOutcome, HarnessError>;

    /// Idempotently terminates remaining adapter I/O or runtime ownership.
    fn force_stop(&mut self) -> Result<(), HarnessError>;
}
