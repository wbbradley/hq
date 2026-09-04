//! Consumer-owned capability ports and neutral external-operation values.

use std::{collections::BTreeSet, fmt};

use hq_domain::{
    AgentId, CommandDigest, ContentText, FactId, OperationId, Page, PageCursor, ProjectId,
    ProviderId, ProviderSessionId, ResourceHealth, ResourceId, ResourceLocator, Revision,
    ShortText, Timestamp,
};

/// Passive exact canonical evidence crossing an adapter boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalEvidence {
    /// Verified canonical fact identity.
    pub fact_id: FactId,
    /// Exact signed outer event bytes.
    pub exact_event: Vec<u8>,
}

/// Passive result of one reverified idempotent evidence import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvidenceIngestOutcome {
    /// Verified canonical fact identity.
    pub fact_id: FactId,
    /// Original canonical commit revision.
    pub revision: Revision,
    /// Whether this call inserted previously unknown evidence.
    pub inserted: bool,
}
use hq_reducer::ConversationKey;

use crate::{
    ApplicationError, ApplicationErrorCode, ApplicationValueError, AuthoritativeConversationView,
    AuthoritativeSnapshot, ConversationEntry, ConversationPageSelection, FactMutation,
    MutationAttempt,
};

/// Maximum relay policies in one bounded application observation.
pub const MAX_RELAY_STATUS_POLICIES: usize = 256;
/// Maximum provider registrations in one passive application observation.
pub const MAX_PROVIDER_CATALOG_ITEMS: usize = 32;

/// One neutral provider registration suitable for user choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAvailability {
    /// Stable neutral provider identity.
    pub provider: ProviderId,
    /// User-facing provider name.
    pub name: ShortText,
    /// Whether new sessions can currently be started.
    pub available: bool,
}

/// Complete bounded provider catalog for one running installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCatalog {
    /// Provider registrations in stable identity order.
    pub providers: Vec<ProviderAvailability>,
    /// Installation-local configured preference, including stale identities.
    pub default_provider: Option<ProviderId>,
}

impl ProviderCatalog {
    /// Constructs a bounded, uniquely ordered passive catalog.
    pub fn new(
        providers: Vec<ProviderAvailability>,
        default_provider: Option<ProviderId>,
    ) -> Result<Self, ApplicationValueError> {
        if providers.len() > MAX_PROVIDER_CATALOG_ITEMS {
            return Err(ApplicationValueError::TooManyItems {
                maximum: MAX_PROVIDER_CATALOG_ITEMS,
                actual: providers.len(),
            });
        }
        if providers
            .windows(2)
            .any(|pair| pair[0].provider >= pair[1].provider)
        {
            return Err(ApplicationValueError::InvalidEncoding);
        }
        Ok(Self {
            providers,
            default_provider,
        })
    }
}

/// Read-only runtime-provider discovery capability.
pub trait QueryProviders {
    /// Loads providers registered with the running installation and its configured preference.
    fn provider_catalog(&self) -> Result<ProviderCatalog, ApplicationError>;
}

/// Query capability expressed in normalized semantic values rather than storage operations.
pub trait QueryDomain {
    /// Loads all authoritative projection packages and their one serialized revision.
    fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError>;

    /// Loads one snapshot and optional selected first page from one authoritative state boundary.
    fn authoritative_conversation_view(
        &self,
        _selection: Option<&ConversationPageSelection>,
    ) -> Result<AuthoritativeConversationView, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Loads one bounded reducer-ordered conversation page.
    fn conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<ConversationEntry>, ApplicationError>;

    /// Loads the exact transitive canonical ancestry of bounded root facts.
    fn canonical_evidence(
        &self,
        _roots: &BTreeSet<FactId>,
        _maximum_facts: usize,
        _maximum_bytes: usize,
    ) -> Result<Vec<CanonicalEvidence>, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Loads current normalized domain health without mutating it.
    fn state_health(&self) -> Result<StateHealth, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Reverifies the immutable corpus and atomically replaces rebuildable state.
    fn repair_state(
        &self,
        _operation_id: OperationId,
    ) -> Result<StateRepairReport, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

/// Atomic retryable canonical-fact commit capability.
pub trait CommitFacts {
    /// Executes or reconciles one stable fact-backed mutation.
    fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError>;

    /// Reverifies and idempotently ingests bounded exact canonical evidence.
    fn ingest_canonical_evidence(
        &self,
        _evidence: &[CanonicalEvidence],
    ) -> Result<Vec<EvidenceIngestOutcome>, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

/// Installation-local draft persistence and authoritative mailbox-command capability.
pub trait ControlMailbox {
    /// Loads every bounded local draft in stable identity order.
    fn mailbox_drafts(&self) -> Result<Vec<crate::MailboxDraft>, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Creates or optimistically replaces one complete local draft.
    fn save_mailbox_draft(
        &self,
        _request: crate::MailboxDraftSaveRequest,
    ) -> Result<crate::MailboxDraftSaveOutcome, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Idempotently and optimistically deletes one local draft.
    fn delete_mailbox_draft(
        &self,
        _request: crate::MailboxDraftDeleteRequest,
    ) -> Result<crate::MailboxDraftDeleteOutcome, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Executes or reconciles one node-resolved mailbox command.
    fn control_mailbox(
        &self,
        _request: crate::MailboxCommandRequest,
    ) -> Result<MutationAttempt, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
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
    /// Stable external-operation identity.
    pub operation_id: OperationId,
    /// Digest of the exact external request.
    pub request_digest: CommandDigest,
    /// Caller-supplied issue time.
    pub issued_at: Timestamp,
    /// Typed operation body.
    pub body: T,
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
    /// Whether a relay session owner should exist.
    pub enabled: bool,
}

impl RelayConfiguration {
    /// Constructs one relay policy.
    pub const fn new(
        endpoint: ResourceLocator,
        access: RelayAccess,
        authentication: RelayAuthentication,
        enabled: bool,
    ) -> Self {
        Self {
            endpoint,
            access,
            authentication,
            enabled,
        }
    }
}

/// Passive current durable policy for one relay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayPolicyStatus {
    /// Typed relay endpoint.
    pub endpoint: ResourceLocator,
    /// Enabled synchronization direction.
    pub access: RelayAccess,
    /// Connection authentication policy.
    pub authentication: RelayAuthentication,
    /// Whether a relay session owner should exist.
    pub enabled: bool,
    /// Positive durable policy generation.
    pub generation: u64,
}

/// Passive bounded durable relay and delivery health observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayStatus {
    /// Current durable policies in endpoint order.
    pub policies: Vec<RelayPolicyStatus>,
    /// Queued canonical delivery intents in the observed page.
    pub queued: usize,
    /// Prepared exact delivery lineages in the observed page.
    pub prepared: usize,
    /// Relay attempts with uncertain disposition in the observed page.
    pub uncertain: usize,
    /// Relay attempts with explicit rejection in the observed page.
    pub rejected: usize,
    /// Relay attempts with positive acceptance in the observed page.
    pub accepted: usize,
    /// Transient inbound wrappers awaiting retry in the observed page.
    pub staged: usize,
    /// Permanently rejected bounded evidence in the observed page.
    pub quarantined: usize,
    /// Whether additional durable rows exist beyond the bounded observation.
    pub truncated: bool,
}

/// Stable reducer domain name used by administrative health output.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HealthDomain {
    /// Installation, mailbox, peer, capability, and account authority.
    Authority,
    /// Conversation, message, and activity state.
    Conversation,
    /// Named agents and provider sessions.
    Agent,
    /// Projects, resources, assignments, and control.
    Project,
}

/// Passive decision counts for one authoritative reducer domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainHealth {
    /// Stable domain name.
    pub domain: HealthDomain,
    /// Admitted semantic facts.
    pub projected: u64,
    /// Facts waiting on missing or unusable dependencies.
    pub unresolved: u64,
    /// Facts rejected by authority policy.
    pub unauthorized: u64,
    /// Facts participating in explicit conflicts.
    pub conflicted: u64,
    /// Intrinsically invalid facts.
    pub invalid: u64,
    /// Verified but unsupported semantic families.
    pub unsupported: u64,
    /// Normalized aggregate/global conflicts.
    pub conflicts: u64,
}

/// Passive authoritative domain-health observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateHealth {
    /// Serialized local revision paired with the index at one adapter serialization point.
    pub revision: Revision,
    /// Complete fixed domain catalog in stable order.
    pub domains: Vec<DomainHealth>,
}

/// Passive result of one explicit complete rebuildable-state repair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateRepairReport {
    /// Caller-selected stable audit identity.
    pub operation_id: OperationId,
    /// Serialized local revision observed after repair.
    pub revision: Revision,
    /// Complete repaired domain health.
    pub domains: Vec<DomainHealth>,
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

    /// Loads one bounded authoritative relay/delivery health observation.
    fn relay_status(&self) -> Result<RelayStatus, ApplicationError> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
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

/// Maximum copied environment entries accepted at one local control boundary.
pub const MAX_LAUNCH_ENVIRONMENT_ENTRIES: usize = 512;
/// Maximum bytes accepted in one environment name.
pub const MAX_LAUNCH_ENVIRONMENT_NAME_BYTES: usize = 256;
/// Maximum bytes accepted in one environment value.
pub const MAX_LAUNCH_ENVIRONMENT_VALUE_BYTES: usize = 32_768;
/// Maximum aggregate bytes accepted across one copied environment.
pub const MAX_LAUNCH_ENVIRONMENT_BYTES: usize = 1_048_576;

struct SecretLaunchEnvironmentEntry {
    name: String,
    value: Vec<u8>,
}

impl Drop for SecretLaunchEnvironmentEntry {
    fn drop(&mut self) {
        self.value.fill(0);
    }
}

/// Opaque copied launch environment whose secret values are redacted and zeroed on drop.
#[derive(Default)]
pub struct LaunchEnvironment {
    entries: Vec<SecretLaunchEnvironmentEntry>,
}

impl LaunchEnvironment {
    /// Copies and validates a complete caller environment.
    pub fn copy_from<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<Self, ApplicationError> {
        let mut copied = Vec::new();
        let mut total = 0_usize;
        for (name, value) in entries {
            if copied.len() == MAX_LAUNCH_ENVIRONMENT_ENTRIES
                || name.is_empty()
                || name.len() > MAX_LAUNCH_ENVIRONMENT_NAME_BYTES
                || name.as_bytes().contains(&0)
                || name.as_bytes().contains(&b'=')
                || value.len() > MAX_LAUNCH_ENVIRONMENT_VALUE_BYTES
                || value.contains(&0)
            {
                return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
            }
            total = total
                .checked_add(name.len())
                .and_then(|size| size.checked_add(value.len()))
                .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::InvalidRequest))?;
            if total > MAX_LAUNCH_ENVIRONMENT_BYTES {
                return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
            }
            copied.push(SecretLaunchEnvironmentEntry {
                name: name.to_owned(),
                value: value.to_vec(),
            });
        }
        copied.sort_by(|left, right| left.name.cmp(&right.name));
        if copied.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
        }
        Ok(Self { entries: copied })
    }

    /// Visits copied values without transferring or retaining ownership.
    pub fn visit(&self, mut visitor: impl FnMut(&str, &[u8])) {
        for entry in &self.entries {
            visitor(&entry.name, &entry.value);
        }
    }

    /// Returns the number of copied entries without exposing values.
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports whether the copied environment is empty.
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Debug for LaunchEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchEnvironment")
            .field("entry_count", &self.entries.len())
            .finish_non_exhaustive()
    }
}

/// Memory-only caller launch context for a start or exact resume.
#[derive(Debug)]
pub struct AgentLaunchContext {
    /// Absolute caller-selected launch directory.
    pub directory: ResourceLocator,
    /// Complete copied caller environment.
    pub environment: LaunchEnvironment,
}

/// Application-level named-agent session request.
#[derive(Debug)]
pub struct AgentSessionRequest {
    /// Durable named-agent identity.
    pub agent_id: AgentId,
    /// Neutral provider identity selected by configuration.
    pub provider: ProviderId,
    /// Requested lifecycle action.
    pub control: SessionControl,
    /// Required memory-only launch context for start/resume; absent for stop.
    pub launch: Option<AgentLaunchContext>,
}

impl AgentSessionRequest {
    /// Constructs a neutral session request.
    pub const fn new(
        agent_id: AgentId,
        provider: ProviderId,
        control: SessionControl,
        launch: Option<AgentLaunchContext>,
    ) -> Self {
        Self {
            agent_id,
            provider,
            control,
            launch,
        }
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
    /// Owning project identity.
    pub project_id: ProjectId,
    /// Stable resource identity.
    pub resource_id: ResourceId,
    /// Normalized human-selected spelling to re-resolve.
    pub display_locator: ResourceLocator,
    /// Immutable canonical identity expected for revalidation, absent during discovery.
    pub canonical_locator: Option<ResourceLocator>,
}

/// Typed resource observation suitable for a later canonical health fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceInspectionResult {
    /// Exact adapter-neutral condition supporting user-facing recovery.
    pub condition: ResourceCondition,
    /// Typed health classification.
    pub health: ResourceHealth,
    /// Current canonical identity when it could be observed.
    pub observed_canonical: Option<ResourceLocator>,
    /// Fresh read-only release classification.
    pub release: ResourceReleaseState,
    /// Bounded inert details.
    pub details: Option<ContentText>,
    /// Explicit observation time.
    pub checked_at: Timestamp,
}

/// Closed resource condition independent of adapter implementation details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceCondition {
    /// The resource exists as the expected directory identity.
    Healthy,
    /// One or more selected path components do not exist.
    Missing,
    /// The resource cannot be inspected with current process authority.
    Inaccessible,
    /// The selected locator could not be resolved safely.
    Malformed,
    /// The selected entry exists but is not a directory.
    NotDirectory,
    /// The observed identity differs from the expected identity.
    IdentityChanged,
    /// Inspection failed without a more precise safe classification.
    Unknown,
}

/// Closed resource-release classification independent of adapter details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceReleaseState {
    /// The resource may be released without force.
    Clean,
    /// The resource contains changes and requires force.
    Dirty,
    /// Safety could not be established and force is required.
    Unknown,
    /// The resource kind has no applicable release check.
    NotApplicable,
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
    + ControlMailbox
    + PublishWake
    + ConfigureRelays
    + QueryProviders
    + ControlHarness
    + crate::ControlProjects
    + crate::RetireAgents
    + InspectResource
    + ObserveRevisions
    + crate::QueryInteractions
    + crate::ControlInteractions
{
}
