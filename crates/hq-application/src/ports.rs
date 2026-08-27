//! Consumer-owned capability ports and neutral external-operation values.

use std::collections::BTreeSet;

use hq_domain::{
    AgentId, CommandDigest, ContentText, OperationId, Page, PageCursor, ProjectId, ProviderId,
    ProviderSessionId, ResourceHealth, ResourceId, ResourceLocator, Revision, Timestamp,
};
use hq_reducer::ConversationKey;

use crate::{
    ApplicationError, ApplicationValueError, AuthoritativeSnapshot, ConversationEntry,
    FactMutation, MutationAttempt,
};

/// Query capability expressed in normalized semantic values rather than storage operations.
pub trait QueryDomain {
    /// Loads all authoritative projection packages and their one serialized revision.
    fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError>;

    /// Loads one bounded reducer-ordered conversation page.
    fn conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<ConversationEntry>, ApplicationError>;
}

/// Atomic retryable canonical-fact commit capability.
pub trait CommitFacts {
    /// Executes or reconciles one stable fact-backed mutation.
    fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError>;
}

/// Disposition of a non-blocking post-commit work notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeDisposition {
    /// A new wake was scheduled.
    Scheduled,
    /// Equivalent pending work already covers this wake.
    Coalesced,
}

/// Non-blocking capability for prompting durable relay and reconciliation work.
pub trait PublishWake {
    /// Prompts work for a committed revision without changing its durable outcome.
    fn publish_wake(&self, revision: Revision) -> Result<WakeDisposition, ApplicationError>;
}

/// Stable request envelope for work that crosses an external-effect boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectRequest<T> {
    operation_id: OperationId,
    request_digest: CommandDigest,
    issued_at: Timestamp,
    body: T,
}

impl<T> EffectRequest<T> {
    /// Constructs an external request from explicit stable inputs.
    pub const fn new(
        operation_id: OperationId,
        request_digest: CommandDigest,
        issued_at: Timestamp,
        body: T,
    ) -> Self {
        Self {
            operation_id,
            request_digest,
            issued_at,
            body,
        }
    }

    /// Returns the stable external-operation identity.
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    /// Returns the digest of the exact external request.
    pub const fn request_digest(&self) -> CommandDigest {
        self.request_digest
    }

    /// Returns the caller-supplied issue time.
    pub const fn issued_at(&self) -> Timestamp {
        self.issued_at
    }

    /// Borrows the typed operation body.
    pub const fn body(&self) -> &T {
        &self.body
    }
}

/// Explicit result of crossing an idempotent external-effect boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectOutcome<T> {
    /// The adapter authoritatively accepted and completed the represented boundary.
    Accepted(T),
    /// The adapter authoritatively rejected the operation.
    Rejected(hq_domain::DomainError),
    /// Completion is unknown and the stable operation must be reconciled before retry.
    Uncertain(OperationId),
}

/// Relay read/write policy independent of any relay client library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayAccess {
    /// Receive retained and live input only.
    Read,
    /// Publish durable outbound work only.
    Write,
    /// Receive and publish.
    ReadWrite,
}

/// Relay authentication policy independent of protocol credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayAuthentication {
    /// Do not authenticate this endpoint.
    Disabled,
    /// Authenticate when the relay requests it.
    OnChallenge,
    /// Require an authenticated session before ordinary work.
    Required,
}

/// Typed local relay configuration request without transport implementation details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayConfiguration {
    /// Typed relay endpoint locator.
    pub endpoint: ResourceLocator,
    /// Allowed synchronization direction.
    pub access: RelayAccess,
    /// Connection authentication policy.
    pub authentication: RelayAuthentication,
}

impl RelayConfiguration {
    /// Constructs one relay policy.
    pub const fn new(
        endpoint: ResourceLocator,
        access: RelayAccess,
        authentication: RelayAuthentication,
    ) -> Self {
        Self {
            endpoint,
            access,
            authentication,
        }
    }
}

/// Scope of an explicit prompt to perform synchronization work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SynchronizationRequest {
    /// Prompt every configured synchronization owner.
    All,
    /// Prompt work for one typed endpoint.
    Relay(ResourceLocator),
}

/// Relay configuration and explicit synchronization capability.
pub trait ConfigureRelays {
    /// Applies or reconciles one stable relay configuration operation.
    fn configure_relay(
        &self,
        request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError>;

    /// Prompts or reconciles one stable synchronization operation.
    fn synchronize(
        &self,
        request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError>;
}

/// Neutral durable-agent session control action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionControl {
    /// Start a new provider session.
    Start,
    /// Resume exactly one known provider session.
    Resume {
        /// Durable provider session identity that must exist.
        session: ProviderSessionId,
    },
    /// Stop the current local runtime without retiring durable history.
    Stop,
}

/// Application-level named-agent session request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionRequest {
    agent_id: AgentId,
    provider: ProviderId,
    control: SessionControl,
}

impl AgentSessionRequest {
    /// Constructs a neutral session request.
    pub const fn new(agent_id: AgentId, provider: ProviderId, control: SessionControl) -> Self {
        Self {
            agent_id,
            provider,
            control,
        }
    }

    /// Returns the durable named-agent identity.
    pub const fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// Returns the neutral provider identity selected by configuration.
    pub const fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Returns the requested lifecycle action.
    pub const fn control(&self) -> &SessionControl {
        &self.control
    }
}

/// Authoritative result at the application-to-runtime control boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentSessionResult {
    /// A nonempty durable provider session was acknowledged ready.
    Ready(ProviderSessionId),
    /// The local runtime stopped and durable history remains.
    Stopped,
}

/// Neutral managed-runtime control capability.
pub trait ControlHarness {
    /// Performs or reconciles one stable named-agent session operation.
    fn control_harness(
        &self,
        request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError>;
}

/// Read-only external resource inspection request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceInspectionRequest {
    project_id: ProjectId,
    resource_id: ResourceId,
    locator: ResourceLocator,
}

impl ResourceInspectionRequest {
    /// Constructs a resource inspection with explicit semantic identity and locator.
    pub const fn new(
        project_id: ProjectId,
        resource_id: ResourceId,
        locator: ResourceLocator,
    ) -> Self {
        Self {
            project_id,
            resource_id,
            locator,
        }
    }

    /// Returns the owning project identity.
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    /// Returns the resource identity.
    pub const fn resource_id(&self) -> ResourceId {
        self.resource_id
    }

    /// Returns the typed resource locator.
    pub const fn locator(&self) -> &ResourceLocator {
        &self.locator
    }
}

/// Typed resource observation suitable for a later canonical health fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceInspectionResult {
    health: ResourceHealth,
    details: Option<ContentText>,
    checked_at: Timestamp,
}

impl ResourceInspectionResult {
    /// Constructs one inert external observation.
    pub const fn new(
        health: ResourceHealth,
        details: Option<ContentText>,
        checked_at: Timestamp,
    ) -> Self {
        Self {
            health,
            details,
            checked_at,
        }
    }

    /// Returns the typed health classification.
    pub const fn health(&self) -> ResourceHealth {
        self.health
    }

    /// Returns bounded inert details.
    pub const fn details(&self) -> Option<&ContentText> {
        self.details.as_ref()
    }

    /// Returns the explicit observation time.
    pub const fn checked_at(&self) -> Timestamp {
        self.checked_at
    }
}

/// External resource observation capability.
pub trait InspectResource {
    /// Inspects or reconciles one stable resource observation operation.
    fn inspect_resource(
        &self,
        request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError>;
}

/// Coarse invalidation topic used only to decide which authoritative view to refresh.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SubscriptionTopic {
    /// Any relevant change.
    All,
    /// Authority and configuration projections.
    Authority,
    /// Messages and durable activity.
    Conversation,
    /// Named agents and session selection.
    Agent,
    /// Projects and resource observations.
    Project,
    /// Durable operational and external-work state.
    Operations,
}

/// Maximum distinct topics in one subscription.
pub const MAX_SUBSCRIPTION_TOPICS: usize = 6;

/// Stable pending subscription registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriptionRequest {
    operation_id: OperationId,
    topics: BTreeSet<SubscriptionTopic>,
}

impl SubscriptionRequest {
    /// Validates a nonempty bounded set of invalidation topics.
    pub fn new(
        operation_id: OperationId,
        topics: impl IntoIterator<Item = SubscriptionTopic>,
    ) -> Result<Self, ApplicationValueError> {
        let topics = topics.into_iter().collect::<BTreeSet<_>>();
        if topics.is_empty() {
            return Err(ApplicationValueError::Empty);
        }
        if topics.len() > MAX_SUBSCRIPTION_TOPICS {
            return Err(ApplicationValueError::TooManyItems {
                maximum: MAX_SUBSCRIPTION_TOPICS,
                actual: topics.len(),
            });
        }
        Ok(Self {
            operation_id,
            topics,
        })
    }

    /// Returns the stable registration identity.
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    /// Returns the subscribed invalidation topics.
    pub const fn topics(&self) -> &BTreeSet<SubscriptionTopic> {
        &self.topics
    }
}

/// Pending registration lifecycle used to close snapshot revision races.
pub trait ObserveRevisions {
    /// Registers pending observation before an acknowledged snapshot is read.
    fn register_subscription(&self, request: &SubscriptionRequest) -> Result<(), ApplicationError>;

    /// Activates delivery only after the caller has written its acknowledgement.
    fn activate_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError>;

    /// Cancels pending or active observation idempotently.
    fn cancel_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError>;
}

/// Complete capability bundle required by the transport-independent application service.
pub trait ApplicationPorts:
    QueryDomain
    + CommitFacts
    + PublishWake
    + ConfigureRelays
    + ControlHarness
    + InspectResource
    + ObserveRevisions
{
}
