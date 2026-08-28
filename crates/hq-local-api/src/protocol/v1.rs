//! Strict local API v1 messages and length-delimited framing.

use std::{error::Error, fmt, num::NonZeroU64};

use hq_application::FactPlan;
use hq_domain::{
    CONTENT_MAX_BYTES, CommandDigest, CommandId, ERROR_CODE_MAX_BYTES, PROVIDER_ID_MAX_BYTES,
    PROVIDER_SESSION_ID_MAX_BYTES, RESOURCE_LOCATOR_MAX_BYTES, SHORT_TEXT_MAX_BYTES,
};
use hq_protocol::{CanonicalEventPlan, FailureClass, MAX_CONTENT_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Local API protocol version implemented by this module.
pub const V1: u16 = 1;
/// Maximum JSON body bytes in one local API frame.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
/// Maximum bytes retained while incrementally decoding one frame.
pub const MAX_BUFFERED_BYTES: usize = MAX_FRAME_BYTES + 4;
/// Maximum bytes in one build metadata text field.
pub const MAX_BUILD_FIELD_BYTES: usize = 128;
/// Maximum requested conversation page size.
pub const MAX_PAGE_ITEMS: u16 = 256;
/// Maximum opaque cursor bytes.
pub const MAX_CURSOR_BYTES: usize = 512;
/// Maximum broad topics in a subscription request.
pub const MAX_TOPICS: usize = 6;
/// Maximum projection items in one full client snapshot.
pub const MAX_SNAPSHOT_ITEMS: usize = 16_384;
/// Maximum canonical evidence items in one transfer.
pub const MAX_CANONICAL_EVIDENCE_ITEMS: usize = 64;
/// Maximum aggregate exact event bytes in one transfer.
pub const MAX_CANONICAL_EVIDENCE_BYTES: usize = 512 * 1024;

const MUTATION_DIGEST_DOMAIN: &[u8] = b"hq-local-api-v1-mutation\0";

/// Fixed-width identifier representation owned by local API v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Id32([u8; 32]);

impl Id32 {
    /// Constructs an identifier from exact semantic bytes.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the exact identifier bytes.
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Inclusive protocol-version range advertised during negotiation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRange {
    minimum: u16,
    maximum: u16,
}

impl VersionRange {
    /// Constructs a nonzero ordered inclusive version range.
    pub const fn new(minimum: u16, maximum: u16) -> Result<Self, ValueError> {
        if minimum == 0 || minimum > maximum {
            return Err(ValueError::InvalidVersionRange);
        }
        Ok(Self { minimum, maximum })
    }

    /// Returns the oldest supported version.
    pub const fn minimum(self) -> u16 {
        self.minimum
    }

    /// Returns the newest supported version.
    pub const fn maximum(self) -> u16 {
        self.maximum
    }

    const fn is_valid(self) -> bool {
        self.minimum != 0 && self.minimum <= self.maximum
    }
}

/// Chooses the highest protocol version supported by both peers.
pub const fn negotiate(client: VersionRange, server: VersionRange) -> Result<u16, ValueError> {
    if !client.is_valid() || !server.is_valid() {
        return Err(ValueError::InvalidVersionRange);
    }
    let minimum = if client.minimum > server.minimum {
        client.minimum
    } else {
        server.minimum
    };
    let maximum = if client.maximum < server.maximum {
        client.maximum
    } else {
        server.maximum
    };
    if minimum > maximum {
        Err(ValueError::NoCommonVersion)
    } else {
        Ok(maximum)
    }
}

/// Diagnostic build identity exchanged without granting any compatibility authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildMetadata {
    name: String,
    version: String,
    commit: Option<String>,
}

impl BuildMetadata {
    /// Constructs bounded, nonempty build metadata.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        commit: Option<impl Into<String>>,
    ) -> Result<Self, ValueError> {
        let value = Self {
            name: name.into(),
            version: version.into(),
            commit: commit.map(Into::into),
        };
        value.validate()?;
        Ok(value)
    }

    /// Returns the executable/product name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the human-readable build version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns optional source revision metadata.
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    fn validate(&self) -> Result<(), ValueError> {
        validate_build_field(&self.name)?;
        validate_build_field(&self.version)?;
        if let Some(commit) = &self.commit {
            validate_build_field(commit)?;
        }
        Ok(())
    }
}

fn validate_build_field(value: &str) -> Result<(), ValueError> {
    if value.is_empty()
        || value.len() > MAX_BUILD_FIELD_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ValueError::InvalidBuildMetadata);
    }
    Ok(())
}

/// First message sent by a newly connected client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHello {
    /// Inclusive versions understood by the client.
    pub versions: VersionRange,
    /// Diagnostic client build identity.
    pub build: BuildMetadata,
}

impl ClientHello {
    /// Constructs a client negotiation offer.
    pub const fn new(versions: VersionRange, build: BuildMetadata) -> Self {
        Self { versions, build }
    }
}

/// Successful server negotiation response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerHello {
    /// Exact version selected for the connection.
    pub selected_version: u16,
    /// Diagnostic server build identity.
    pub build: BuildMetadata,
    /// Ephemeral connection-session identity.
    pub session_id: Id32,
}

impl ServerHello {
    /// Constructs a negotiated server response.
    pub const fn new(selected_version: u16, build: BuildMetadata, session_id: Id32) -> Self {
        Self {
            selected_version,
            build,
            session_id,
        }
    }
}

/// Negotiation failure returned before closing an incompatible connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRejected {
    /// Inclusive versions understood by the server.
    pub supported: VersionRange,
    /// Diagnostic server build identity.
    pub build: BuildMetadata,
}

impl VersionRejected {
    /// Constructs a version rejection naming the server range and build.
    pub const fn new(supported: VersionRange, build: BuildMetadata) -> Self {
        Self { supported, build }
    }
}

/// Nonzero identity pairing exactly one response with one request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestId(NonZeroU64);

impl RequestId {
    /// Constructs a nonzero request identity.
    pub const fn new(value: u64) -> Result<Self, ValueError> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(ValueError::ZeroRequestId),
        }
    }

    /// Returns the numeric request identity.
    pub const fn value(self) -> u64 {
        self.0.get()
    }
}

/// Node lifecycle operations exposed to local clients.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleRequest {
    /// Query current node lifecycle and readiness.
    Status,
    /// Wait for the currently starting node to report ready or a typed failure.
    Readiness,
    /// Begin orderly node drain and stop.
    Stop,
    /// Orderly restart through the sole node owner.
    Restart,
}

/// Exact retryable mutation input transported without signing authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationRequest {
    command_id: Id32,
    request_digest: Id32,
    canonical_plan: Vec<u8>,
    auxiliary_randomness: [u8; 32],
}

impl MutationRequest {
    /// Encodes one application fact plan and derives its exact retry digest.
    pub fn from_plan(command_id: CommandId, plan: FactPlan) -> Result<Self, ValueError> {
        let (author, authored_at, scope, causal, payload, auxiliary_randomness) = plan.into_parts();
        let canonical_plan = CanonicalEventPlan::new(author, authored_at, scope, causal, payload)
            .encode_content()
            .map_err(ValueError::from)?;
        let request_digest = mutation_digest(&canonical_plan, &auxiliary_randomness);
        Ok(Self {
            command_id: Id32::new(*command_id.as_bytes()),
            request_digest: Id32::new(*request_digest.as_bytes()),
            canonical_plan,
            auxiliary_randomness,
        })
    }

    /// Returns the stable command identity.
    pub const fn command_id(&self) -> CommandId {
        CommandId::from_bytes(self.command_id.bytes())
    }

    /// Returns the digest binding the exact retry input.
    pub const fn request_digest(&self) -> CommandDigest {
        CommandDigest::from_bytes(self.request_digest.bytes())
    }

    /// Reports whether the retained digest matches the exact encoded input.
    pub fn validate_digest(&self) -> bool {
        mutation_digest(&self.canonical_plan, &self.auxiliary_randomness) == self.request_digest()
    }

    /// Strictly decodes the exact unsigned plan into an application mutation plan.
    pub fn into_plan(self) -> Result<FactPlan, ValueError> {
        if !self.validate_digest() || self.canonical_plan.len() > MAX_CONTENT_BYTES {
            return Err(ValueError::MutationDigestMismatch);
        }
        let (author, authored_at, scope, causal, payload) =
            CanonicalEventPlan::decode_content(&self.canonical_plan)
                .map_err(ValueError::from)?
                .into_parts();
        Ok(FactPlan::new(
            author,
            authored_at,
            scope,
            causal,
            payload,
            self.auxiliary_randomness,
        ))
    }

    fn validate(&self) -> Result<(), ValueError> {
        if self.canonical_plan.is_empty() || self.canonical_plan.len() > MAX_CONTENT_BYTES {
            return Err(ValueError::InvalidCanonicalPlan);
        }
        if !self.validate_digest() {
            return Err(ValueError::MutationDigestMismatch);
        }
        CanonicalEventPlan::decode_content(&self.canonical_plan).map_err(ValueError::from)?;
        Ok(())
    }
}

fn mutation_digest(content: &[u8], auxiliary_randomness: &[u8; 32]) -> CommandDigest {
    let mut hasher = Sha256::new();
    hasher.update(MUTATION_DIGEST_DOMAIN);
    hasher.update(
        u32::try_from(content.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    hasher.update(content);
    hasher.update(auxiliary_randomness);
    CommandDigest::from_bytes(hasher.finalize().into())
}

/// Conversation identity used by bounded page queries.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationKeyDto {
    /// One causal thread with an exact counterparty mailbox.
    Thread {
        /// Counterparty installation identity.
        counterparty_installation: Id32,
        /// Counterparty mailbox identity.
        counterparty_mailbox: Id32,
        /// Stable causal thread identity.
        thread: Id32,
    },
    /// One durable provider session with an exact counterparty mailbox.
    ProviderSession {
        /// Counterparty installation identity.
        counterparty_installation: Id32,
        /// Counterparty mailbox identity.
        counterparty_mailbox: Id32,
        /// Provider namespace.
        provider: String,
        /// Provider-scoped durable session.
        session: String,
    },
}

/// One bounded reducer-ordered conversation page request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPageRequest {
    /// Typed conversation identity.
    pub key: ConversationKeyDto,
    /// Nonzero inclusive page limit.
    pub limit: u16,
    /// Opaque continuation cursor.
    pub cursor: Option<String>,
}

impl ConversationPageRequest {
    /// Constructs a page request with a nonzero bounded limit and cursor.
    pub fn new(
        key: ConversationKeyDto,
        limit: u16,
        cursor: Option<String>,
    ) -> Result<Self, ValueError> {
        let request = Self { key, limit, cursor };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), ValueError> {
        validate_conversation_key(&self.key)?;
        if self.limit == 0 || self.limit > MAX_PAGE_ITEMS {
            return Err(ValueError::InvalidPageLimit);
        }
        if self
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES)
        {
            return Err(ValueError::InvalidCursor);
        }
        Ok(())
    }
}

/// Broad invalidation topic; notifications never carry projection rows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationTopic {
    /// Any relevant state.
    All,
    /// Authority and configuration state.
    Authority,
    /// Conversations and durable activity.
    Conversation,
    /// Named agents and session selection.
    Agent,
    /// Projects and resources.
    Project,
    /// Operational and external work state.
    Operations,
}

/// Stable subscription registration input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRequestDto {
    /// Stable subscription registration identity.
    pub subscription_id: Id32,
    /// Sorted, unique, nonempty broad topics.
    pub topics: Vec<InvalidationTopic>,
}

impl SubscriptionRequestDto {
    /// Constructs a subscription with sorted, unique, nonempty broad topics.
    pub fn new(
        subscription_id: Id32,
        mut topics: Vec<InvalidationTopic>,
    ) -> Result<Self, ValueError> {
        topics.sort_unstable();
        topics.dedup();
        validate_topics(&topics)?;
        Ok(Self {
            subscription_id,
            topics,
        })
    }
}

/// Resource-locator scheme named independently by local API v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceSchemeDto {
    /// Canonical Git repository identity.
    GitRepository,
    /// Canonical working-tree identity.
    WorkingTree,
    /// Container or sandbox identity.
    Container,
    /// Adapter-defined opaque identity.
    Opaque,
}

/// Typed external resource locator.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLocatorDto {
    /// Typed locator scheme.
    pub scheme: ResourceSchemeDto,
    /// Canonical bounded locator text.
    pub value: String,
}

impl ResourceLocatorDto {
    /// Constructs a typed locator after enforcing its text bound.
    pub fn new(scheme: ResourceSchemeDto, value: String) -> Result<Self, ValueError> {
        let locator = Self { scheme, value };
        validate_locator(&locator)?;
        Ok(locator)
    }
}

/// Stable request envelope for an idempotent external-effect boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRequestDto<T> {
    /// Stable external-operation identity.
    pub operation_id: Id32,
    /// Digest of the exact external request.
    pub request_digest: Id32,
    /// Caller-supplied issue time.
    pub issued_at_unix_millis: i64,
    /// Typed external-operation body.
    pub body: T,
}

impl<T> EffectRequestDto<T> {
    /// Constructs one stable external-effect request.
    pub const fn new(
        operation_id: Id32,
        request_digest: Id32,
        issued_at_unix_millis: i64,
        body: T,
    ) -> Self {
        Self {
            operation_id,
            request_digest,
            issued_at_unix_millis,
            body,
        }
    }
}

/// Relay read/write policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayAccessDto {
    /// Receive only.
    Read,
    /// Publish only.
    Write,
    /// Receive and publish.
    ReadWrite,
}

/// Relay authentication policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayAuthenticationDto {
    /// Never authenticate.
    Disabled,
    /// Authenticate when challenged.
    OnChallenge,
    /// Require authentication before ordinary work.
    Required,
}

/// Typed relay configuration body.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfigurationDto {
    /// Typed relay endpoint.
    pub endpoint: ResourceLocatorDto,
    /// Allowed relay direction.
    pub access: RelayAccessDto,
    /// Relay authentication policy.
    pub authentication: RelayAuthenticationDto,
}

impl RelayConfigurationDto {
    /// Constructs one relay configuration body.
    pub const fn new(
        endpoint: ResourceLocatorDto,
        access: RelayAccessDto,
        authentication: RelayAuthenticationDto,
    ) -> Self {
        Self {
            endpoint,
            access,
            authentication,
        }
    }
}

/// Explicit synchronization scope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "scope", content = "relay", rename_all = "snake_case")]
pub enum SynchronizationRequestDto {
    /// Prompt all configured synchronization owners.
    All,
    /// Prompt exactly one endpoint.
    Relay(ResourceLocatorDto),
}

/// Provider-neutral named-agent session action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", content = "session", rename_all = "snake_case")]
pub enum SessionControlDto {
    /// Start a new provider session.
    Start,
    /// Resume a known provider session.
    Resume(String),
    /// Stop the current local runtime.
    Stop,
}

/// Provider-neutral named-agent control body.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionRequestDto {
    /// Durable named-agent identity.
    pub agent_id: Id32,
    /// Provider namespace.
    pub provider: String,
    /// Provider-neutral session action.
    pub control: SessionControlDto,
}

impl AgentSessionRequestDto {
    /// Constructs a provider-neutral session control body.
    pub fn new(
        agent_id: Id32,
        provider: String,
        control: SessionControlDto,
    ) -> Result<Self, ValueError> {
        validate_text(&provider, PROVIDER_ID_MAX_BYTES)?;
        if let SessionControlDto::Resume(session) = &control {
            validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
        }
        Ok(Self {
            agent_id,
            provider,
            control,
        })
    }
}

/// Read-only project resource inspection body.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceInspectionRequestDto {
    /// Owning project identity.
    pub project_id: Id32,
    /// Stable resource identity.
    pub resource_id: Id32,
    /// Normalized human-selected spelling to re-resolve.
    pub display_locator: ResourceLocatorDto,
    /// Immutable canonical identity expected after re-resolution.
    pub canonical_locator: ResourceLocatorDto,
}

/// Desired project resource carried by a control request.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectResourceDto {
    pub resource_id: Id32,
    pub display_locator: ResourceLocatorDto,
    pub canonical_locator: ResourceLocatorDto,
    pub health: ResourceHealthDto,
}

/// Exact worktree provisioning input.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeProvisioningRequestDto {
    pub mailbox_id: Id32,
    pub project_name: String,
    pub brief: Option<String>,
    pub source: ResourceLocatorDto,
    pub destination: ResourceLocatorDto,
    pub branch: String,
    pub create_branch: bool,
}

/// Closed project action catalog for local API v1.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum ProjectCommandActionDto {
    Open,
    Activate {
        agent_id: Id32,
        provider: String,
        resume_session: Option<String>,
        resume_thread: Option<Id32>,
        launch_directory: ResourceLocatorDto,
    },
    DispatchPending,
    Close {
        force: bool,
    },
    SetArchived {
        archived: bool,
    },
    Handoff {
        agent_id: Id32,
        provider: String,
        resume_session: Option<String>,
        thread_id: Id32,
        launch_directory: ResourceLocatorDto,
        force_takeover: bool,
    },
    RetireAgent {
        agent_id: Id32,
        force: bool,
    },
    AddResource {
        resource: ProjectResourceDto,
        make_primary: bool,
    },
    RemoveResource {
        resource_id: Id32,
        force: bool,
    },
    ReplaceResource {
        old_resource_id: Id32,
        new_resource: ProjectResourceDto,
    },
    ProvisionWorktree(WorktreeProvisioningRequestDto),
}

/// Stable exact project command envelope.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCommandRequestDto {
    pub command_id: Id32,
    pub operation_id: Id32,
    pub request_digest: Id32,
    pub account_id: Id32,
    pub project_id: Id32,
    pub home: Id32,
    pub expected_head: Option<Id32>,
    pub issued_at_unix_millis: i64,
    pub action: ProjectCommandActionDto,
}

/// Durable project workflow checkpoint.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectCommandStageDto {
    Accepted,
    AwaitingHome,
    ReceivedAtHome,
    ValidatingResources,
    Opening,
    ConfiguringAssignment,
    StartingRuntime,
    ValidatingLaunchDirectory,
    MakingRunnable,
    DispatchingInputs,
    AssessingRelease,
    QuiescingRuntime,
    EndingAssignment,
    Closing,
    UpdatingProject,
    ReservingDestination,
    ReconcilingGit,
    CreatingWorktree,
    IdentifyingResource,
    CreatingProject,
    Compensating,
    ReconciliationRequired,
    Complete,
}

/// External runtime truth retained without inference.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "code", rename_all = "snake_case")]
pub enum RuntimeObservationDto {
    Succeeded,
    Failed(String),
    Uncertain(String),
}

/// Typed project command submission or progress result.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProjectCommandOutcomeDto {
    Accepted {
        operation_id: Id32,
        stage: ProjectCommandStageDto,
    },
    Running {
        operation_id: Id32,
        stage: ProjectCommandStageDto,
    },
    Completed {
        operation_id: Id32,
        project_head: Id32,
        runtime: Option<RuntimeObservationDto>,
    },
    Rejected {
        operation_id: Id32,
        error: DomainErrorDto,
        runtime: Option<RuntimeObservationDto>,
    },
    Reconcilable {
        operation_id: Id32,
        stage: ProjectCommandStageDto,
        error: DomainErrorDto,
    },
}

/// Terminal remote-control result.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum RemoteCommandResultDto {
    Committed(Id32),
    Rejected(String),
}

/// Authoritative remote-control checkpoint with exact fact attribution.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum RemoteCommandProgressDto {
    Queued,
    Received {
        receipt_fact: Id32,
        received_head: Id32,
        received_at_unix_millis: i64,
    },
    Terminal {
        receipt_fact: Id32,
        received_head: Id32,
        received_at_unix_millis: i64,
        outcome_fact: Id32,
        result: RemoteCommandResultDto,
        runtime: Option<RuntimeObservationDto>,
    },
    Conflicted,
}

/// One exact signed canonical event paired with its verified fact identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalEvidenceDto {
    /// Verified canonical fact identity.
    pub fact_id: Id32,
    /// Exact UTF-8 outer event JSON.
    pub exact_event: String,
}

/// Bounded roots for one transitive canonical evidence query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalEvidenceRequestDto {
    /// Sorted unique root facts whose complete ancestry is requested.
    pub roots: Vec<Id32>,
}

/// Result of one reverified idempotent evidence import.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceIngestOutcomeDto {
    /// Verified canonical fact identity.
    pub fact_id: Id32,
    /// Original canonical commit revision.
    pub revision: u64,
    /// Whether this call inserted previously unknown evidence.
    pub inserted: bool,
}

/// Closed local API v1 request families.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// Query or control node lifecycle.
    Lifecycle(LifecycleRequest),
    /// Load one complete authoritative client snapshot.
    AuthoritativeSnapshot,
    /// Load one bounded reducer-ordered conversation page.
    ConversationPage(ConversationPageRequest),
    /// Execute or reconcile one exact retryable mutation.
    Mutation(MutationRequest),
    /// Load bounded exact transitive canonical evidence.
    CanonicalEvidence(CanonicalEvidenceRequestDto),
    /// Reverify and idempotently import bounded exact canonical evidence.
    IngestCanonicalEvidence(Vec<CanonicalEvidenceDto>),
    /// Apply or reconcile one typed relay configuration effect.
    ConfigureRelay(EffectRequestDto<RelayConfigurationDto>),
    /// Prompt or reconcile explicit relay synchronization.
    Synchronize(EffectRequestDto<SynchronizationRequestDto>),
    /// Control one provider-neutral named-agent session.
    ControlAgentSession(EffectRequestDto<AgentSessionRequestDto>),
    /// Inspect one typed project resource.
    InspectResource(EffectRequestDto<ResourceInspectionRequestDto>),
    /// Execute, route, or reconcile one exact project command.
    ControlProject(Box<ProjectCommandRequestDto>),
    /// Register a revision invalidation subscription.
    Subscribe(SubscriptionRequestDto),
    /// Cancel one pending or active subscription idempotently.
    CancelSubscription {
        /// Stable subscription identity.
        subscription_id: Id32,
    },
}

/// One correlated request message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    /// Request correlation identity.
    pub id: RequestId,
    /// Typed request body.
    pub request: Request,
}

impl RequestEnvelope {
    /// Constructs a request envelope.
    pub const fn new(id: RequestId, request: Request) -> Self {
        Self { id, request }
    }
}

/// Client-visible node lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// Node startup is in progress.
    Starting,
    /// Node accepts ordinary requests.
    Ready,
    /// Node is orderly draining.
    Draining,
    /// Node is stopped.
    Stopped,
    /// Node failed to become or remain ready.
    Failed,
}

/// Typed lifecycle status response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleStatus {
    /// Current node lifecycle state.
    pub state: LifecycleState,
    /// Diagnostic node build identity.
    pub build: BuildMetadata,
    /// Authoritative revision when durable state is available.
    pub revision: Option<u64>,
    /// Optional bounded inert readiness or failure detail.
    pub detail: Option<String>,
}

impl LifecycleStatus {
    /// Constructs a lifecycle response with bounded diagnostic detail.
    pub fn new(
        state: LifecycleState,
        build: BuildMetadata,
        revision: Option<u64>,
        detail: Option<String>,
    ) -> Result<Self, ValueError> {
        if let Some(detail) = &detail {
            validate_text(detail, CONTENT_MAX_BYTES)?;
        }
        Ok(Self {
            state,
            build,
            revision,
            detail,
        })
    }
}

/// One typed client-facing authoritative projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnapshotItem {
    /// Installation identity and display metadata.
    Installation {
        /// Installation identity.
        installation_id: Id32,
        /// Exact unique installation-root fact.
        root_fact: Id32,
        /// Installation signing key.
        signing_key: Id32,
        /// Installation encryption key.
        encryption_key: Id32,
        /// Optional bounded display label.
        label: Option<String>,
    },
    /// Installation-qualified mailbox identity.
    Mailbox {
        /// Owning installation.
        installation_id: Id32,
        /// Mailbox identity.
        mailbox_id: Id32,
        /// Exact mailbox creation fact.
        create_fact: Id32,
        /// Stable mailbox kind name.
        mailbox_kind: String,
        /// Optional bounded display label.
        label: Option<String>,
    },
    /// Human account presentation.
    Account {
        /// Account identity.
        account_id: Id32,
        /// Exact unique account-root fact.
        root_fact: Id32,
        /// Creator installation.
        creator_installation: Id32,
        /// Optional display label.
        label: Option<String>,
        /// Whether this account is the local active selection.
        selected: bool,
    },
    /// Current directional peer route.
    PeerRoute {
        /// Local route owner.
        owner: Id32,
        /// Remote installation.
        peer: Id32,
        /// Stable route state name.
        state: String,
        /// Causal-maximal supporting facts.
        frontier: Vec<Id32>,
    },
    /// Directional mailbox capability lineage.
    MailboxCapability {
        /// Stable grant identity.
        grant_id: Id32,
        /// Mailbox-owning installation.
        mailbox_installation: Id32,
        /// Target mailbox identity.
        mailbox_id: Id32,
        /// Grantee installation.
        grantee_installation: Id32,
        /// Whether the capability is active.
        active: bool,
    },
    /// Human device membership state.
    Membership {
        /// Human account identity.
        account_id: Id32,
        /// Member installation.
        device: Id32,
        /// Stable membership state name.
        state: String,
        /// Complete causal-maximal grant/accept/revoke frontier.
        frontier: Vec<Id32>,
        /// Complete creator-issued grant history.
        grants: Vec<DeviceGrantDto>,
        /// Every usable exact acceptance fact.
        acceptances: Vec<Id32>,
        /// Every usable exact revoke fact.
        revokes: Vec<Id32>,
        /// Exact active acceptance authorities.
        active_acceptances: Vec<Id32>,
    },
    /// Local default-account selection register.
    AccountSelection {
        /// Selecting installation.
        installation_id: Id32,
        /// Causal-maximal account candidates.
        candidates: Vec<Id32>,
        /// Unique active account when resolved.
        active: Option<Id32>,
        /// Complete causal-maximal selection fact frontier.
        frontier: Vec<Id32>,
    },
    /// Conversation discovery and unread/open summary; entries are loaded by bounded page query.
    Conversation {
        /// Stable conversation identity.
        key: ConversationKeyDto,
        /// Canonically latest presented fact, when nonempty.
        latest_fact: Option<Id32>,
        /// Number of currently open actionable messages.
        open_messages: u32,
    },
    /// Named agent current presentation state.
    Agent {
        /// Agent identity.
        agent_id: Id32,
        /// Candidate permanent names.
        names: Vec<String>,
        /// Stable lifecycle name.
        lifecycle: String,
        /// Whether the agent is presently runnable.
        runnable: bool,
    },
    /// Immutable provider-session binding state.
    AgentSession {
        /// Provider namespace.
        provider: String,
        /// Provider-scoped session identity.
        session: String,
        /// Unique bound mailbox installation, when unconflicted.
        mailbox_installation: Option<Id32>,
        /// Unique bound mailbox identity, when unconflicted.
        mailbox_id: Option<Id32>,
        /// Whether incompatible bindings exist.
        conflicted: bool,
    },
    /// Durable selected-session register for one named agent.
    AgentSelection {
        /// Named-agent identity.
        agent_id: Id32,
        /// Selected provider, when resolved.
        provider: Option<String>,
        /// Selected provider session, when resolved.
        session: Option<String>,
        /// Whether distinct causal maxima remain.
        conflicted: bool,
    },
    /// Independent provider-session display-name register.
    AgentSessionName {
        /// Named-agent identity.
        agent_id: Id32,
        /// Provider namespace.
        provider: String,
        /// Provider-scoped session identity.
        session: String,
        /// Whether the register has one resolved value.
        resolved: bool,
        /// Resolved display name or explicit clear.
        display_name: Option<String>,
    },
    /// Project current presentation state.
    Project {
        /// Project identity.
        project_id: Id32,
        /// Immutable home installation.
        home: Id32,
        /// Display name.
        name: String,
        /// Stable lifecycle name.
        lifecycle: String,
        /// Whether the project is archived.
        archived: bool,
        /// Whether all desired resources are claimable.
        claimable: bool,
        /// Last unique admitted project head.
        head: Id32,
        /// Last accepted contiguous input sequence.
        input_sequence: u64,
    },
    /// One desired project path and its advisory claim state.
    ProjectResource {
        /// Owning project identity.
        project_id: Id32,
        /// Stable resource identity.
        resource_id: Id32,
        /// Normalized human-selected spelling.
        display_locator: ResourceLocatorDto,
        /// Home-local canonical claim identity.
        canonical_locator: ResourceLocatorDto,
        /// Latest typed health observation.
        health: ResourceHealthDto,
        /// Whether this resource is the explicit project primary.
        primary: bool,
        /// Whether this resource currently has a conflict-free active advisory claim.
        active_claim: bool,
        /// Sorted projects with overlapping claims.
        conflicting_projects: Vec<Id32>,
    },
    /// Immutable accepted project input attribution.
    ProjectInput {
        /// Project identity.
        project_id: Id32,
        /// Stable public message identity.
        message_id: Id32,
        /// Home-assigned contiguous sequence.
        sequence: u64,
        /// Acceptance fact.
        accepted_fact: Id32,
    },
    /// Immutable at-most-once project dispatch attribution.
    ProjectDispatch {
        /// Stable dispatch identity.
        dispatch_id: Id32,
        /// Project input identity.
        message_id: Id32,
        /// Home sequence.
        sequence: u64,
        /// Dispatch fact.
        fact_id: Id32,
        /// Whether changed duplicate dispatches exist.
        conflicted: bool,
    },
    /// Retained project output attribution.
    ProjectOutput {
        /// Stable output message identity.
        output_id: Id32,
        /// Originating dispatch.
        dispatch_id: Id32,
        /// Stable current/late/conflicted status name.
        status: String,
        /// Bounded output content.
        content: String,
    },
    /// Remote project command progress.
    RemoteCommand {
        /// Stable command identity.
        command_id: Id32,
        /// Exact request digest.
        request_digest: Id32,
        /// Active human account authorizing the request.
        account_id: Id32,
        /// Target project.
        project_id: Id32,
        /// Immutable authoritative installation.
        target_home: Id32,
        /// Caller-observed canonical head.
        expected_head: Id32,
        /// Provider namespace used for durable routing correlation.
        operation_provider: String,
        /// Provider session used for durable routing correlation.
        operation_session: String,
        /// Stable workflow operation identity.
        operation_id: Id32,
        /// Strict versioned command body retained by the projection.
        body: String,
        /// Request semantic time.
        issued_at_unix_millis: i64,
        /// Exact request fact.
        request_fact: Id32,
        /// Structured authoritative progress.
        progress: Box<RemoteCommandProgressDto>,
    },
}

/// Passive creator-issued human-device grant history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceGrantDto {
    /// Stable grant identity.
    pub grant_id: Id32,
    /// Exact signed grant fact.
    pub grant_fact: Id32,
    /// Invited installation.
    pub device: Id32,
    /// Exact invited signing key.
    pub signing_key: Id32,
    /// Optional signed label.
    pub label: Option<String>,
    /// Signed non-authority relay hints.
    pub relay_hints: Vec<ResourceLocatorDto>,
    /// Whether this grant is currently causal-maximal membership history.
    pub frontier_member: bool,
    /// Whether a current active acceptance cites this grant identity.
    pub active: bool,
}

/// Complete revisioned client-facing authoritative snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeSnapshotDto {
    /// Serialized authoritative local revision.
    pub revision: u64,
    /// Bounded typed client projection items.
    pub items: Vec<SnapshotItem>,
}

impl AuthoritativeSnapshotDto {
    /// Constructs a bounded authoritative snapshot.
    pub fn new(revision: u64, items: Vec<SnapshotItem>) -> Result<Self, ValueError> {
        let snapshot = Self { revision, items };
        validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }
}

/// One typed reducer-ordered conversation page item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationEntryDto {
    /// Durable message presentation.
    Message {
        /// Canonical fact identity.
        fact_id: Id32,
        /// Stable public message identity.
        message_id: Id32,
        /// Causal thread identity.
        thread_id: Id32,
        /// Bounded message content.
        content: String,
        /// Whether the message remains open.
        open: bool,
        /// Whether the message was absorbing-rejected.
        rejected: bool,
    },
    /// Durable or selected harness activity presentation.
    Activity {
        /// Canonical fact identity.
        fact_id: Id32,
        /// Positive source sequence.
        sequence: u64,
        /// Stable status name.
        status: String,
        /// Bounded display content.
        content: String,
        /// Whether authoring truncated the content.
        truncated: bool,
    },
}

/// Bounded page response with a stable continuation cursor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPageDto {
    /// Reducer-ordered bounded conversation entries.
    pub items: Vec<ConversationEntryDto>,
    /// Opaque continuation cursor when more entries exist.
    pub next_cursor: Option<String>,
}

impl ConversationPageDto {
    /// Constructs a bounded reducer-ordered page.
    pub fn new(
        items: Vec<ConversationEntryDto>,
        next_cursor: Option<String>,
    ) -> Result<Self, ValueError> {
        let page = Self { items, next_cursor };
        validate_page(&page)?;
        Ok(page)
    }
}

/// Stable semantic mutation outcome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MutationOutcomeDto {
    /// A canonical fact or durable semantic no-op committed.
    Committed,
    /// Domain policy authoritatively rejected the command.
    Rejected {
        /// Stable domain error category.
        category: String,
        /// Stable domain error code.
        code: String,
    },
}

/// Retry-safe mutation result including explicit uncertainty.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MutationAttemptDto {
    /// A retained receipt is authoritative.
    Completed {
        /// Stable command identity.
        command_id: Id32,
        /// Exact request digest.
        request_digest: Id32,
        /// Durable transaction revision.
        revision: u64,
        /// Stable semantic outcome.
        outcome: MutationOutcomeDto,
    },
    /// Completion is unknown and exact retry must reconcile it.
    Uncertain {
        /// Stable command identity.
        command_id: Id32,
        /// Exact request digest that must be reused.
        request_digest: Id32,
    },
}

/// Stable domain rejection at an external-effect boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DomainErrorDto {
    /// Stable domain error category.
    pub category: String,
    /// Stable domain error code.
    pub code: String,
}

impl DomainErrorDto {
    /// Constructs a stable bounded domain error.
    pub fn new(category: String, code: String) -> Result<Self, ValueError> {
        validate_text(&category, SHORT_TEXT_MAX_BYTES)?;
        validate_text(&code, SHORT_TEXT_MAX_BYTES)?;
        Ok(Self { category, code })
    }
}

/// Explicit idempotent external-effect outcome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum EffectOutcomeDto<T> {
    /// The adapter authoritatively accepted and completed the effect.
    Accepted(T),
    /// The adapter authoritatively rejected the effect.
    Rejected(DomainErrorDto),
    /// Completion is unknown and must be reconciled before retry.
    Uncertain(Id32),
}

/// Provider-neutral named-agent session result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "session", rename_all = "snake_case")]
pub enum AgentSessionResultDto {
    /// A nonempty durable provider session is ready.
    Ready(String),
    /// The local runtime stopped while durable history remains.
    Stopped,
}

/// Resource health classification.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceHealthDto {
    /// The resource has not been checked.
    Unknown,
    /// The resource is available.
    Healthy,
    /// The resource exists but needs attention.
    Degraded,
    /// The resource is unavailable.
    Unavailable,
}

/// Typed inert project-resource observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceInspectionResultDto {
    /// Typed resource health.
    pub health: ResourceHealthDto,
    /// Current canonical identity when it could be observed.
    pub observed_canonical: Option<ResourceLocatorDto>,
    /// Optional bounded inert observation detail.
    pub details: Option<String>,
    /// Explicit observation time.
    pub checked_at_unix_millis: i64,
}

impl ResourceInspectionResultDto {
    /// Constructs a bounded inert resource observation.
    pub fn new(
        health: ResourceHealthDto,
        observed_canonical: Option<ResourceLocatorDto>,
        details: Option<String>,
        checked_at_unix_millis: i64,
    ) -> Result<Self, ValueError> {
        if let Some(details) = &details {
            validate_text(details, CONTENT_MAX_BYTES)?;
        }
        Ok(Self {
            health,
            observed_canonical,
            details,
            checked_at_unix_millis,
        })
    }
}

/// Acknowledgement of a pending subscription and its race-free snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionAcknowledgement {
    /// Stable pending subscription identity.
    pub subscription_id: Id32,
    /// Snapshot loaded after pending registration.
    pub snapshot: AuthoritativeSnapshotDto,
}

impl SubscriptionAcknowledgement {
    /// Constructs a pending-registration acknowledgement with its authoritative snapshot.
    pub const fn new(subscription_id: Id32, snapshot: AuthoritativeSnapshotDto) -> Self {
        Self {
            subscription_id,
            snapshot,
        }
    }
}

/// Successful local API v1 response families.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum ResponseResult {
    /// Lifecycle status or control acknowledgement.
    Lifecycle(LifecycleStatus),
    /// Complete authoritative snapshot.
    AuthoritativeSnapshot(AuthoritativeSnapshotDto),
    /// Bounded conversation page.
    ConversationPage(ConversationPageDto),
    /// Retry-safe mutation attempt.
    Mutation(MutationAttemptDto),
    /// Bounded exact canonical evidence closure.
    CanonicalEvidence(Vec<CanonicalEvidenceDto>),
    /// Per-fact idempotent evidence import outcomes.
    EvidenceIngest(Vec<EvidenceIngestOutcomeDto>),
    /// Relay configuration or synchronization effect outcome.
    EmptyEffect(EffectOutcomeDto<()>),
    /// Named-agent session effect outcome.
    AgentSession(EffectOutcomeDto<AgentSessionResultDto>),
    /// Resource inspection effect outcome.
    ResourceInspection(EffectOutcomeDto<ResourceInspectionResultDto>),
    /// Project command submission or durable progress.
    ProjectCommand(ProjectCommandOutcomeDto),
    /// Pending subscription acknowledgement.
    Subscription(SubscriptionAcknowledgement),
    /// Successful operation without a value.
    Empty,
}

/// Stable client-visible error class.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// Request bytes or semantic inputs are invalid.
    InvalidInput,
    /// Request conflicts with authoritative state.
    Conflict,
    /// Caller or domain author lacks authority.
    Unauthorized,
    /// Required state is absent or unresolved.
    NotFound,
    /// Service is temporarily unavailable or draining.
    Unavailable,
    /// An internal invariant or adapter failed.
    Internal,
}

/// Closed error response carrying stable machine data and bounded inert detail.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    /// Stable client-visible error class.
    pub class: ErrorClass,
    /// Stable machine error code.
    pub code: String,
    /// Optional bounded inert diagnostic detail.
    pub detail: Option<String>,
}

impl ErrorResponse {
    /// Constructs a stable machine error and optional bounded inert detail.
    pub fn new(
        class: ErrorClass,
        code: String,
        detail: Option<String>,
    ) -> Result<Self, ValueError> {
        validate_text(&code, SHORT_TEXT_MAX_BYTES)?;
        if let Some(detail) = &detail {
            validate_text(detail, CONTENT_MAX_BYTES)?;
        }
        Ok(Self {
            class,
            code,
            detail,
        })
    }
}

/// Closed response outcome with language-independent wire tags.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Response {
    /// Typed successful result.
    Success(ResponseResult),
    /// Typed error result.
    Error(ErrorResponse),
}

/// One correlated success or error response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseEnvelope {
    /// Request correlation identity.
    pub id: RequestId,
    /// Typed success or error outcome.
    pub response: Response,
}

impl ResponseEnvelope {
    /// Constructs a successful response.
    pub const fn success(id: RequestId, result: ResponseResult) -> Self {
        Self {
            id,
            response: Response::Success(result),
        }
    }

    /// Constructs an error response.
    pub const fn error(id: RequestId, error: ErrorResponse) -> Self {
        Self {
            id,
            response: Response::Error(error),
        }
    }
}

/// Revision-only invalidation notification with no projection data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionInvalidation {
    /// Stable active subscription identity.
    pub subscription_id: Id32,
    /// Newest coalesced committed revision.
    pub revision: u64,
    /// Sorted, unique broad invalidation topics.
    pub topics: Vec<InvalidationTopic>,
    /// Whether the client must refresh its complete snapshot.
    pub full_snapshot: bool,
}

impl RevisionInvalidation {
    /// Constructs a sorted, unique, bounded invalidation notification.
    pub fn new(
        subscription_id: Id32,
        revision: u64,
        mut topics: Vec<InvalidationTopic>,
        full_snapshot: bool,
    ) -> Result<Self, ValueError> {
        if revision == 0 {
            return Err(ValueError::InvalidRevision);
        }
        topics.sort_unstable();
        topics.dedup();
        validate_topics(&topics)?;
        Ok(Self {
            subscription_id,
            revision,
            topics,
            full_snapshot,
        })
    }
}

/// Closed top-level local API v1 message union.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum WireMessage {
    /// Initial client negotiation offer.
    ClientHello(ClientHello),
    /// Successful server negotiation response.
    ServerHello(ServerHello),
    /// Incompatible-version response followed by connection close.
    VersionRejected(VersionRejected),
    /// Correlated client request.
    Request(RequestEnvelope),
    /// Correlated server response.
    Response(ResponseEnvelope),
    /// Revision invalidation for an active subscription.
    Invalidation(RevisionInvalidation),
}

impl WireMessage {
    /// Encodes one canonical JSON message behind a four-byte big-endian length prefix.
    pub fn encode_frame(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate().map_err(EncodeError::InvalidValue)?;
        let body = serde_json::to_vec(self).map_err(|_| EncodeError::Serialization)?;
        if body.len() > MAX_FRAME_BYTES {
            return Err(EncodeError::FrameTooLarge);
        }
        let length = u32::try_from(body.len()).map_err(|_| EncodeError::FrameTooLarge)?;
        let mut frame = Vec::with_capacity(body.len() + 4);
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&body);
        Ok(frame)
    }

    /// Strictly decodes exactly one complete canonical frame.
    pub fn decode_frame(frame: &[u8]) -> Result<Self, DecodeError> {
        if frame.len() < 4 {
            return Err(DecodeError::Truncated);
        }
        let declared =
            usize::try_from(u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]))
                .map_err(|_| DecodeError::FrameTooLarge)?;
        if declared > MAX_FRAME_BYTES {
            return Err(DecodeError::FrameTooLarge);
        }
        let actual = frame.len() - 4;
        if actual < declared {
            return Err(DecodeError::Truncated);
        }
        if actual > declared {
            return Err(DecodeError::TrailingData);
        }
        let body = &frame[4..];
        let message: Self = serde_json::from_slice(body).map_err(|_| DecodeError::Malformed)?;
        message.validate().map_err(DecodeError::InvalidValue)?;
        let canonical = serde_json::to_vec(&message).map_err(|_| DecodeError::Malformed)?;
        if canonical != body {
            return Err(DecodeError::NonCanonical);
        }
        Ok(message)
    }

    fn validate(&self) -> Result<(), ValueError> {
        match self {
            Self::ClientHello(hello) => {
                if !hello.versions.is_valid() {
                    return Err(ValueError::InvalidVersionRange);
                }
                hello.build.validate()
            }
            Self::ServerHello(hello) => {
                if hello.selected_version != V1 {
                    return Err(ValueError::InvalidVersionRange);
                }
                hello.build.validate()
            }
            Self::VersionRejected(rejected) => {
                if !rejected.supported.is_valid() {
                    return Err(ValueError::InvalidVersionRange);
                }
                rejected.build.validate()
            }
            Self::Request(envelope) => match &envelope.request {
                Request::ConversationPage(request) => request.validate(),
                Request::Mutation(request) => request.validate(),
                Request::CanonicalEvidence(request) => {
                    if request.roots.is_empty() {
                        return Err(ValueError::TooManyItems);
                    }
                    validate_id_set(&request.roots, MAX_CANONICAL_EVIDENCE_ITEMS)
                }
                Request::IngestCanonicalEvidence(evidence) => validate_evidence(evidence),
                Request::Subscribe(request) => validate_topics(&request.topics),
                Request::ConfigureRelay(request) => validate_locator(&request.body.endpoint),
                Request::Synchronize(request) => match &request.body {
                    SynchronizationRequestDto::All => Ok(()),
                    SynchronizationRequestDto::Relay(locator) => validate_locator(locator),
                },
                Request::ControlAgentSession(request) => {
                    validate_text(&request.body.provider, PROVIDER_ID_MAX_BYTES)?;
                    if let SessionControlDto::Resume(session) = &request.body.control {
                        validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                    }
                    Ok(())
                }
                Request::InspectResource(request) => {
                    validate_locator(&request.body.display_locator)?;
                    validate_locator(&request.body.canonical_locator)?;
                    if request.body.display_locator.scheme != request.body.canonical_locator.scheme
                    {
                        return Err(ValueError::InvalidValueCombination);
                    }
                    Ok(())
                }
                Request::ControlProject(request) => validate_project_request(request),
                Request::Lifecycle(_)
                | Request::AuthoritativeSnapshot
                | Request::CancelSubscription { .. } => Ok(()),
            },
            Self::Invalidation(invalidation) => {
                if invalidation.revision == 0 {
                    return Err(ValueError::InvalidRevision);
                }
                validate_topics(&invalidation.topics)
            }
            Self::Response(response) => validate_response(response),
        }
    }
}

fn validate_locator(locator: &ResourceLocatorDto) -> Result<(), ValueError> {
    validate_text(&locator.value, RESOURCE_LOCATOR_MAX_BYTES)
}

fn validate_text(value: &str, maximum: usize) -> Result<(), ValueError> {
    if value.is_empty() || value.len() > maximum {
        return Err(ValueError::InvalidText);
    }
    Ok(())
}

fn validate_response(response: &ResponseEnvelope) -> Result<(), ValueError> {
    match &response.response {
        Response::Success(ResponseResult::Lifecycle(status)) => {
            status.build.validate()?;
            if let Some(detail) = &status.detail {
                validate_text(detail, CONTENT_MAX_BYTES)?;
            }
            Ok(())
        }
        Response::Success(ResponseResult::AuthoritativeSnapshot(snapshot)) => {
            validate_snapshot(snapshot)
        }
        Response::Success(ResponseResult::ConversationPage(page)) => validate_page(page),
        Response::Success(ResponseResult::Mutation(attempt)) => validate_mutation_attempt(attempt),
        Response::Success(ResponseResult::CanonicalEvidence(evidence)) => {
            validate_evidence(evidence)
        }
        Response::Success(ResponseResult::EvidenceIngest(outcomes)) => {
            if outcomes.is_empty() || outcomes.len() > MAX_CANONICAL_EVIDENCE_ITEMS {
                return Err(ValueError::TooManyItems);
            }
            let mut previous = None;
            for outcome in outcomes {
                if outcome.revision == 0 || previous.is_some_and(|value| value >= outcome.fact_id) {
                    return Err(ValueError::InvalidValueCombination);
                }
                previous = Some(outcome.fact_id);
            }
            Ok(())
        }
        Response::Success(ResponseResult::EmptyEffect(outcome)) => {
            validate_effect_outcome(outcome, |()| Ok(()))
        }
        Response::Success(ResponseResult::AgentSession(outcome)) => {
            validate_effect_outcome(outcome, |result| {
                if let AgentSessionResultDto::Ready(session) = result {
                    validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                }
                Ok(())
            })
        }
        Response::Success(ResponseResult::Subscription(acknowledgement)) => {
            validate_snapshot(&acknowledgement.snapshot)
        }
        Response::Success(ResponseResult::ProjectCommand(outcome)) => {
            validate_project_outcome(outcome)
        }
        Response::Success(ResponseResult::Empty) => Ok(()),
        Response::Success(ResponseResult::ResourceInspection(outcome)) => match outcome {
            EffectOutcomeDto::Accepted(result) => {
                if let Some(observed) = &result.observed_canonical {
                    validate_locator(observed)?;
                }
                if let Some(details) = &result.details {
                    validate_text(details, CONTENT_MAX_BYTES)?;
                }
                Ok(())
            }
            EffectOutcomeDto::Rejected(error) => validate_domain_error(error),
            EffectOutcomeDto::Uncertain(_) => Ok(()),
        },
        Response::Error(error) => {
            validate_text(&error.code, SHORT_TEXT_MAX_BYTES)?;
            if let Some(detail) = &error.detail {
                validate_text(detail, CONTENT_MAX_BYTES)?;
            }
            Ok(())
        }
    }
}

fn validate_evidence(evidence: &[CanonicalEvidenceDto]) -> Result<(), ValueError> {
    if evidence.is_empty() || evidence.len() > MAX_CANONICAL_EVIDENCE_ITEMS {
        return Err(ValueError::TooManyItems);
    }
    let mut total = 0_usize;
    let mut previous = None;
    for item in evidence {
        if item.exact_event.is_empty() || previous.is_some_and(|value| value >= item.fact_id) {
            return Err(ValueError::InvalidValueCombination);
        }
        total = total
            .checked_add(item.exact_event.len())
            .ok_or(ValueError::TooManyItems)?;
        if total > MAX_CANONICAL_EVIDENCE_BYTES {
            return Err(ValueError::TooManyItems);
        }
        previous = Some(item.fact_id);
    }
    Ok(())
}

fn validate_project_request(request: &ProjectCommandRequestDto) -> Result<(), ValueError> {
    let provisioning = matches!(
        &request.action,
        ProjectCommandActionDto::ProvisionWorktree(_)
    );
    if provisioning == request.expected_head.is_some() {
        return Err(ValueError::InvalidValueCombination);
    }
    match &request.action {
        ProjectCommandActionDto::Open
        | ProjectCommandActionDto::DispatchPending
        | ProjectCommandActionDto::Close { .. }
        | ProjectCommandActionDto::SetArchived { .. }
        | ProjectCommandActionDto::RetireAgent { .. }
        | ProjectCommandActionDto::RemoveResource { .. } => Ok(()),
        ProjectCommandActionDto::Activate {
            provider,
            resume_session,
            launch_directory,
            ..
        }
        | ProjectCommandActionDto::Handoff {
            provider,
            resume_session,
            launch_directory,
            ..
        } => {
            validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
            if let Some(session) = resume_session {
                validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
            }
            validate_locator(launch_directory)
        }
        ProjectCommandActionDto::AddResource { resource, .. }
        | ProjectCommandActionDto::ReplaceResource {
            new_resource: resource,
            ..
        } => validate_project_resource(resource),
        ProjectCommandActionDto::ProvisionWorktree(request) => {
            validate_text(&request.project_name, SHORT_TEXT_MAX_BYTES)?;
            if let Some(brief) = &request.brief {
                validate_text(brief, CONTENT_MAX_BYTES)?;
            }
            validate_locator(&request.source)?;
            validate_locator(&request.destination)?;
            validate_text(&request.branch, SHORT_TEXT_MAX_BYTES)
        }
    }
}

fn validate_project_resource(resource: &ProjectResourceDto) -> Result<(), ValueError> {
    validate_locator(&resource.display_locator)?;
    validate_locator(&resource.canonical_locator)?;
    if resource.display_locator.scheme != resource.canonical_locator.scheme {
        return Err(ValueError::InvalidValueCombination);
    }
    Ok(())
}

fn validate_runtime(runtime: &RuntimeObservationDto) -> Result<(), ValueError> {
    match runtime {
        RuntimeObservationDto::Succeeded => Ok(()),
        RuntimeObservationDto::Failed(code) | RuntimeObservationDto::Uncertain(code) => {
            validate_text(code, ERROR_CODE_MAX_BYTES)
        }
    }
}

fn validate_project_outcome(outcome: &ProjectCommandOutcomeDto) -> Result<(), ValueError> {
    match outcome {
        ProjectCommandOutcomeDto::Accepted { .. } | ProjectCommandOutcomeDto::Running { .. } => {
            Ok(())
        }
        ProjectCommandOutcomeDto::Completed { runtime, .. } => {
            runtime.as_ref().map_or(Ok(()), validate_runtime)
        }
        ProjectCommandOutcomeDto::Rejected { error, runtime, .. } => {
            validate_domain_error(error)?;
            runtime.as_ref().map_or(Ok(()), validate_runtime)
        }
        ProjectCommandOutcomeDto::Reconcilable { error, .. } => validate_domain_error(error),
    }
}

fn validate_mutation_attempt(attempt: &MutationAttemptDto) -> Result<(), ValueError> {
    match attempt {
        MutationAttemptDto::Completed { outcome, .. } => match outcome {
            MutationOutcomeDto::Committed => Ok(()),
            MutationOutcomeDto::Rejected { category, code } => {
                validate_text(category, SHORT_TEXT_MAX_BYTES)?;
                validate_text(code, SHORT_TEXT_MAX_BYTES)
            }
        },
        MutationAttemptDto::Uncertain { .. } => Ok(()),
    }
}

fn validate_effect_outcome<T>(
    outcome: &EffectOutcomeDto<T>,
    accepted: impl FnOnce(&T) -> Result<(), ValueError>,
) -> Result<(), ValueError> {
    match outcome {
        EffectOutcomeDto::Accepted(value) => accepted(value),
        EffectOutcomeDto::Rejected(error) => validate_domain_error(error),
        EffectOutcomeDto::Uncertain(_) => Ok(()),
    }
}

fn validate_domain_error(error: &DomainErrorDto) -> Result<(), ValueError> {
    validate_text(&error.category, SHORT_TEXT_MAX_BYTES)?;
    validate_text(&error.code, SHORT_TEXT_MAX_BYTES)
}

// Keeping the closed projection catalog in one exhaustive match makes newly added wire variants
// fail compilation until their bounds are classified.
#[allow(clippy::too_many_lines)]
fn validate_snapshot(snapshot: &AuthoritativeSnapshotDto) -> Result<(), ValueError> {
    if snapshot.items.len() > MAX_SNAPSHOT_ITEMS {
        return Err(ValueError::TooManyItems);
    }
    for item in &snapshot.items {
        match item {
            SnapshotItem::Installation { label, .. }
            | SnapshotItem::Mailbox { label, .. }
            | SnapshotItem::Account { label, .. } => {
                if let Some(label) = label {
                    validate_text(label, SHORT_TEXT_MAX_BYTES)?;
                }
            }
            SnapshotItem::PeerRoute {
                state, frontier, ..
            } => {
                if !matches!(state.as_str(), "routable" | "blocked" | "conflicted") {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_id_set(frontier, 64)?;
            }
            SnapshotItem::MailboxCapability { .. } => {}
            SnapshotItem::AccountSelection {
                candidates,
                frontier,
                ..
            } => {
                validate_id_set(candidates, 64)?;
                validate_id_set(frontier, 64)?;
            }
            SnapshotItem::Conversation { key, .. } => validate_conversation_key(key)?,
            SnapshotItem::ProjectInput { sequence, .. }
            | SnapshotItem::ProjectDispatch { sequence, .. } => {
                if *sequence == 0 {
                    return Err(ValueError::InvalidSequence);
                }
            }
            SnapshotItem::Membership {
                state,
                frontier,
                grants,
                acceptances,
                revokes,
                active_acceptances,
                ..
            } => {
                if !matches!(state.as_str(), "pending" | "active" | "revoked") {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_id_set(frontier, 64)?;
                if grants.len() > 64 {
                    return Err(ValueError::TooManyItems);
                }
                if grants
                    .windows(2)
                    .any(|pair| pair[0].grant_id >= pair[1].grant_id)
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                for grant in grants {
                    if let Some(label) = &grant.label {
                        validate_text(label, SHORT_TEXT_MAX_BYTES)?;
                    }
                    if grant.relay_hints.len() > hq_domain::MAX_RELAY_HINTS
                        || grant.relay_hints.windows(2).any(|pair| pair[0] >= pair[1])
                    {
                        return Err(ValueError::InvalidValueCombination);
                    }
                    for locator in &grant.relay_hints {
                        validate_locator(locator)?;
                    }
                }
                validate_id_set(acceptances, 64)?;
                validate_id_set(revokes, 64)?;
                let has_active_grant = grants.iter().any(|grant| grant.active);
                if (state == "active") != has_active_grant
                    || (state == "pending" && !grants.iter().any(|grant| grant.frontier_member))
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_id_set(active_acceptances, 64)?;
                if !active_acceptances
                    .iter()
                    .all(|acceptance| acceptances.contains(acceptance))
                    || (state == "revoked" && revokes.is_empty())
                {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::Agent {
                names, lifecycle, ..
            } => {
                if names.len() > 64 {
                    return Err(ValueError::TooManyItems);
                }
                for name in names {
                    validate_text(name, SHORT_TEXT_MAX_BYTES)?;
                }
                if names.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_text(lifecycle, SHORT_TEXT_MAX_BYTES)?;
            }
            SnapshotItem::AgentSession {
                provider,
                session,
                mailbox_installation,
                mailbox_id,
                ..
            } => {
                validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                if mailbox_installation.is_some() != mailbox_id.is_some() {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::AgentSelection {
                provider, session, ..
            } => {
                if let Some(provider) = provider {
                    validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
                }
                if let Some(session) = session {
                    validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                }
                if provider.is_some() != session.is_some() {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::AgentSessionName {
                provider,
                session,
                resolved,
                display_name,
                ..
            } => {
                validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                if let Some(display_name) = display_name {
                    validate_text(display_name, SHORT_TEXT_MAX_BYTES)?;
                }
                if !resolved && display_name.is_some() {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::Project {
                name, lifecycle, ..
            } => {
                validate_text(name, SHORT_TEXT_MAX_BYTES)?;
                validate_text(lifecycle, SHORT_TEXT_MAX_BYTES)?;
            }
            SnapshotItem::ProjectResource {
                display_locator,
                canonical_locator,
                conflicting_projects,
                ..
            } => {
                validate_locator(display_locator)?;
                validate_locator(canonical_locator)?;
                if display_locator.scheme != canonical_locator.scheme {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_id_set(conflicting_projects, MAX_SNAPSHOT_ITEMS)?;
            }
            SnapshotItem::ProjectOutput {
                status, content, ..
            } => {
                validate_text(status, SHORT_TEXT_MAX_BYTES)?;
                validate_text(content, CONTENT_MAX_BYTES)?;
            }
            SnapshotItem::RemoteCommand {
                operation_provider,
                operation_session,
                body,
                progress,
                ..
            } => {
                validate_text(operation_provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(operation_session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                validate_text(body, CONTENT_MAX_BYTES)?;
                if let RemoteCommandProgressDto::Terminal {
                    result: RemoteCommandResultDto::Rejected(code),
                    runtime,
                    ..
                } = progress.as_ref()
                {
                    validate_text(code, ERROR_CODE_MAX_BYTES)?;
                    if let Some(runtime) = runtime {
                        validate_runtime(runtime)?;
                    }
                } else if let RemoteCommandProgressDto::Terminal {
                    runtime: Some(runtime),
                    ..
                } = progress.as_ref()
                {
                    validate_runtime(runtime)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_id_set(ids: &[Id32], maximum: usize) -> Result<(), ValueError> {
    if ids.len() > maximum {
        return Err(ValueError::TooManyItems);
    }
    if ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(ValueError::InvalidValueCombination);
    }
    Ok(())
}

fn validate_conversation_key(key: &ConversationKeyDto) -> Result<(), ValueError> {
    if let ConversationKeyDto::ProviderSession {
        provider, session, ..
    } = key
    {
        validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
        validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
    }
    Ok(())
}

fn validate_page(page: &ConversationPageDto) -> Result<(), ValueError> {
    if page.items.len() > usize::from(MAX_PAGE_ITEMS) {
        return Err(ValueError::TooManyItems);
    }
    if page
        .next_cursor
        .as_ref()
        .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES)
    {
        return Err(ValueError::InvalidCursor);
    }
    for item in &page.items {
        match item {
            ConversationEntryDto::Message { content, .. }
            | ConversationEntryDto::Activity { content, .. } => {
                validate_text(content, CONTENT_MAX_BYTES)?;
            }
        }
        if matches!(item, ConversationEntryDto::Activity { sequence: 0, .. }) {
            return Err(ValueError::InvalidSequence);
        }
    }
    Ok(())
}

fn validate_topics(topics: &[InvalidationTopic]) -> Result<(), ValueError> {
    if topics.is_empty() || topics.len() > MAX_TOPICS {
        return Err(ValueError::InvalidTopics);
    }
    if topics.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(ValueError::InvalidTopics);
    }
    Ok(())
}

/// Incremental decoder retaining one bounded partial frame and any later frame bytes.
#[derive(Clone, Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    /// Constructs an empty incremental decoder.
    pub const fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Appends bytes and returns the next complete message, retaining any later frame bytes.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<WireMessage>, DecodeError> {
        if self.buffer.len().saturating_add(bytes.len()) > MAX_BUFFERED_BYTES {
            return Err(DecodeError::FrameTooLarge);
        }
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let declared = usize::try_from(u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]))
        .map_err(|_| DecodeError::FrameTooLarge)?;
        if declared > MAX_FRAME_BYTES {
            return Err(DecodeError::FrameTooLarge);
        }
        let frame_len = declared + 4;
        if self.buffer.len() < frame_len {
            return Ok(None);
        }
        let message = WireMessage::decode_frame(&self.buffer[..frame_len])?;
        self.buffer.drain(..frame_len);
        Ok(Some(message))
    }

    /// Returns bytes retained for an incomplete or later frame.
    pub const fn buffered_len(&self) -> usize {
        self.buffer.len()
    }
}

/// Invalid semantic values in local API v1 DTOs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValueError {
    /// A version range is zero or reversed.
    InvalidVersionRange,
    /// Peers share no protocol version.
    NoCommonVersion,
    /// Build metadata is empty, oversized, or contains controls.
    InvalidBuildMetadata,
    /// A request correlation identity was zero.
    ZeroRequestId,
    /// A page limit was zero or exceeded its protocol maximum.
    InvalidPageLimit,
    /// A page cursor was empty or oversized.
    InvalidCursor,
    /// An unsigned canonical plan was empty or oversized.
    InvalidCanonicalPlan,
    /// Exact mutation bytes do not match their stable digest.
    MutationDigestMismatch,
    /// The unsigned canonical semantic plan was invalid.
    CanonicalPlan(FailureClass),
    /// Subscription topics were empty, duplicated, unsorted, or oversized.
    InvalidTopics,
    /// A revision that must represent committed state was zero.
    InvalidRevision,
    /// A text field was empty or exceeded its type-specific byte bound.
    InvalidText,
    /// A bounded DTO collection exceeded its item limit.
    TooManyItems,
    /// A positive sequence was zero.
    InvalidSequence,
    /// Optional fields whose presence must agree were inconsistent.
    InvalidValueCombination,
}

impl From<hq_protocol::ProtocolError> for ValueError {
    fn from(error: hq_protocol::ProtocolError) -> Self {
        Self::CanonicalPlan(error.class())
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid local API v1 value: {self:?}")
    }
}

impl Error for ValueError {}

/// Frame encoding failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodeError {
    /// A DTO violated a semantic protocol bound.
    InvalidValue(ValueError),
    /// JSON serialization failed.
    Serialization,
    /// Encoded JSON exceeded the frame limit.
    FrameTooLarge,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local API v1 encode failed: {self:?}")
    }
}

impl Error for EncodeError {}

/// Strict frame decoding failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The prefix declared more than the protocol maximum.
    FrameTooLarge,
    /// The frame ended before its declared body.
    Truncated,
    /// Bytes remained after the one declared body.
    TrailingData,
    /// JSON shape, UTF-8, field, or enum data was invalid.
    Malformed,
    /// The DTO violated a semantic protocol bound.
    InvalidValue(ValueError),
    /// JSON was semantically decodable but not in canonical v1 form.
    NonCanonical,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local API v1 decode failed: {self:?}")
    }
}

impl Error for DecodeError {}
