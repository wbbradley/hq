//! Strict local API v1 messages and length-delimited framing.

use std::{error::Error, fmt, num::NonZeroU64};

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
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
/// Maximum first-page size embedded in one materialized authoritative view.
pub const MAX_MATERIALIZED_CONVERSATION_PAGE_ITEMS: u16 = 200;
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
/// Maximum relay policies returned in one health observation.
pub const MAX_RELAY_STATUS_POLICIES: usize = hq_application::MAX_RELAY_STATUS_POLICIES;
/// Maximum provider registrations returned by one passive catalog query.
pub const MAX_PROVIDER_CATALOG_ITEMS: usize = 32;

const MUTATION_DIGEST_DOMAIN: &[u8] = b"hq-local-api-v1-mutation\0";
const MAILBOX_COMMAND_DIGEST_DOMAIN: &[u8] = b"hq-local-api-v1-mailbox-command\0";
const AGENT_SESSION_DIGEST_DOMAIN: &[u8] = b"hq-local-api-v1-agent-session\0";
const RESOURCE_INSPECTION_DIGEST_DOMAIN: &[u8] = b"hq-local-api-v1-resource-inspection\0";

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

/// Explicit local mailbox-draft target.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MailboxDraftTargetDto {
    Reply {
        message_id: Id32,
    },
    Direct {
        installation_id: Id32,
        mailbox_id: Id32,
    },
    SelfNote,
    Project {
        project_id: Id32,
        thread_id: Option<Id32>,
    },
}

/// Complete passive local mailbox draft.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxDraftDto {
    pub draft_id: Id32,
    pub target: MailboxDraftTargetDto,
    pub content: String,
    pub version: u64,
}

/// Optimistic mailbox-draft autosave request.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxDraftSaveRequestDto {
    pub draft_id: Id32,
    pub target: MailboxDraftTargetDto,
    pub content: String,
    pub expected_version: Option<u64>,
}

/// Optimistic mailbox-draft autosave result.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "draft", rename_all = "snake_case")]
pub enum MailboxDraftSaveOutcomeDto {
    Saved(MailboxDraftDto),
    Conflict(MailboxDraftDto),
}

/// Optimistic mailbox-draft deletion request.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxDraftDeleteRequestDto {
    pub draft_id: Id32,
    pub expected_version: u64,
}

/// Optimistic mailbox-draft deletion result.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "draft", rename_all = "snake_case")]
pub enum MailboxDraftDeleteOutcomeDto {
    Deleted,
    NotFound,
    Conflict(MailboxDraftDto),
}

/// Passive node-resolved mailbox command action.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MailboxCommandActionDto {
    Reply {
        target_message: Id32,
        message_id: Id32,
    },
    Direct {
        recipient_installation: Id32,
        recipient_mailbox: Id32,
        message_id: Id32,
    },
    SelfNote {
        message_id: Id32,
    },
    Project {
        project_id: Id32,
        thread_id: Option<Id32>,
        message_id: Id32,
    },
    Archive {
        target_message: Id32,
    },
    Restore {
        target_message: Id32,
    },
}

/// Stable retry envelope for one authoritative mailbox command.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxCommandRequestDto {
    pub command_id: Id32,
    pub request_digest: Id32,
    pub draft_id: Option<Id32>,
    pub action: MailboxCommandActionDto,
    pub content: Option<String>,
    pub authored_at_millis: i64,
    pub auxiliary_randomness: [u8; 32],
}

impl MailboxCommandRequestDto {
    /// Binds a caller-selected stable identity to the exact command input.
    pub fn new(
        command_id: Id32,
        draft_id: Option<Id32>,
        action: MailboxCommandActionDto,
        content: Option<String>,
        authored_at_millis: i64,
        auxiliary_randomness: [u8; 32],
    ) -> Self {
        let mut request = Self {
            command_id,
            request_digest: Id32::new([0; 32]),
            draft_id,
            action,
            content,
            authored_at_millis,
            auxiliary_randomness,
        };
        request.request_digest = Id32::new(*mailbox_command_digest(&request).as_bytes());
        request
    }

    /// Returns the stable retry identity.
    pub const fn command_id(&self) -> CommandId {
        CommandId::from_bytes(self.command_id.bytes())
    }

    /// Returns the digest binding every exact input field.
    pub const fn request_digest(&self) -> CommandDigest {
        CommandDigest::from_bytes(self.request_digest.bytes())
    }

    pub(crate) fn validate(&self) -> Result<(), ValueError> {
        if self
            .content
            .as_ref()
            .is_some_and(|value| value.len() > CONTENT_MAX_BYTES)
            || mailbox_command_digest(self) != self.request_digest()
        {
            return Err(ValueError::MutationDigestMismatch);
        }
        let message_action = matches!(
            self.action,
            MailboxCommandActionDto::Reply { .. }
                | MailboxCommandActionDto::Direct { .. }
                | MailboxCommandActionDto::SelfNote { .. }
                | MailboxCommandActionDto::Project { .. }
        );
        if message_action != (self.draft_id.is_some() ^ self.content.is_some()) {
            return Err(ValueError::InvalidCanonicalPlan);
        }
        Ok(())
    }
}

fn mailbox_command_digest(request: &MailboxCommandRequestDto) -> CommandDigest {
    let mut hasher = Sha256::new();
    hasher.update(MAILBOX_COMMAND_DIGEST_DOMAIN);
    match &request.action {
        MailboxCommandActionDto::Reply {
            target_message,
            message_id,
        } => {
            hasher.update([1]);
            hasher.update(target_message.bytes());
            hasher.update(message_id.bytes());
        }
        MailboxCommandActionDto::Direct {
            recipient_installation,
            recipient_mailbox,
            message_id,
        } => {
            hasher.update([2]);
            hasher.update(recipient_installation.bytes());
            hasher.update(recipient_mailbox.bytes());
            hasher.update(message_id.bytes());
        }
        MailboxCommandActionDto::SelfNote { message_id } => {
            hasher.update([3]);
            hasher.update(message_id.bytes());
        }
        MailboxCommandActionDto::Archive { target_message } => {
            hasher.update([4]);
            hasher.update(target_message.bytes());
        }
        MailboxCommandActionDto::Restore { target_message } => {
            hasher.update([5]);
            hasher.update(target_message.bytes());
        }
        MailboxCommandActionDto::Project {
            project_id,
            thread_id,
            message_id,
        } => {
            hasher.update([6]);
            hasher.update(project_id.bytes());
            match thread_id {
                Some(thread_id) => {
                    hasher.update([1]);
                    hasher.update(thread_id.bytes());
                }
                None => hasher.update([0]),
            }
            hasher.update(message_id.bytes());
        }
    }
    match request.draft_id {
        Some(draft_id) => {
            hasher.update([1]);
            hasher.update(draft_id.bytes());
        }
        None => hasher.update([0]),
    }
    match &request.content {
        Some(content) => {
            hasher.update([1]);
            hasher.update(
                u64::try_from(content.len())
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
            );
            hasher.update(content.as_bytes());
        }
        None => hasher.update([0]),
    }
    hasher.update(request.authored_at_millis.to_be_bytes());
    hasher.update(request.auxiliary_randomness);
    CommandDigest::from_bytes(hasher.finalize().into())
}

/// Conversation identity used by bounded page queries.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationKeyDto {
    /// One independently initiated project exchange.
    ProjectThread {
        /// Stable project identity.
        project: Id32,
        /// Stable initiating thread identity.
        thread: Id32,
    },
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

/// Exact participant evidence paired with an optional human-facing name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationParticipantDto {
    /// Singular named-agent identity when resolved.
    pub agent: Option<Id32>,
    /// Participant installation when an exact mailbox is available.
    pub installation: Option<Id32>,
    /// Participant mailbox when an exact mailbox is available.
    pub mailbox: Option<Id32>,
    /// Singular bounded authoritative name when resolved.
    pub name: Option<String>,
}

/// Closed human-facing context for one conversation summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationContextDto {
    /// A local human message to their own mailbox.
    Personal,
    /// A direct exchange with one exact or unresolved counterparty.
    Direct {
        /// Counterparty identity evidence.
        participant: ConversationParticipantDto,
    },
    /// One independently initiated exchange belonging to a project.
    Project {
        /// Stable project identity.
        project: Id32,
        /// Singular bounded project name when resolved.
        name: Option<String>,
        /// Historical or current worker evidence when resolved.
        participant: Option<ConversationParticipantDto>,
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

/// One bounded first-page interest for a materialized authoritative view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPageSelectionDto {
    /// Typed conversation identity.
    pub key: ConversationKeyDto,
    /// Nonzero inclusive first-page limit.
    pub limit: u16,
}

impl ConversationPageSelectionDto {
    /// Constructs one bounded typed selection.
    pub fn new(key: ConversationKeyDto, limit: u16) -> Result<Self, ValueError> {
        let selection = Self { key, limit };
        selection.validate()?;
        Ok(selection)
    }

    fn validate(&self) -> Result<(), ValueError> {
        validate_conversation_key(&self.key)?;
        if self.limit == 0 || self.limit > MAX_MATERIALIZED_CONVERSATION_PAGE_ITEMS {
            return Err(ValueError::InvalidPageLimit);
        }
        Ok(())
    }
}

/// Request for one coherent snapshot and optional selected first page.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeConversationViewRequestDto {
    /// Selected first-page interest, or no conversation detail.
    pub conversation: Option<ConversationPageSelectionDto>,
}

impl AuthoritativeConversationViewRequestDto {
    /// Constructs one materialized-view request.
    pub const fn new(conversation: Option<ConversationPageSelectionDto>) -> Self {
        Self { conversation }
    }

    fn validate(&self) -> Result<(), ValueError> {
        self.conversation
            .as_ref()
            .map_or(Ok(()), ConversationPageSelectionDto::validate)
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
    /// Optional selected first-page interest for materialized refreshes.
    pub conversation: Option<ConversationPageSelectionDto>,
}

impl SubscriptionRequestDto {
    /// Constructs a subscription with sorted, unique, nonempty broad topics.
    pub fn new(
        subscription_id: Id32,
        mut topics: Vec<InvalidationTopic>,
        conversation: Option<ConversationPageSelectionDto>,
    ) -> Result<Self, ValueError> {
        topics.sort_unstable();
        topics.dedup();
        validate_topics(&topics)?;
        if let Some(conversation) = &conversation {
            conversation.validate()?;
        }
        Ok(Self {
            subscription_id,
            topics,
            conversation,
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
    /// Whether a relay session owner should exist.
    pub enabled: bool,
}

impl RelayConfigurationDto {
    /// Constructs one relay configuration body.
    pub const fn new(
        endpoint: ResourceLocatorDto,
        access: RelayAccessDto,
        authentication: RelayAuthenticationDto,
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelayPolicyStatusDto {
    /// Typed relay endpoint.
    pub endpoint: ResourceLocatorDto,
    /// Enabled synchronization direction.
    pub access: RelayAccessDto,
    /// Connection authentication policy.
    pub authentication: RelayAuthenticationDto,
    /// Whether a relay session owner should exist.
    pub enabled: bool,
    /// Positive durable policy generation.
    pub generation: u64,
}

/// Passive bounded relay and delivery health observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelayStatusDto {
    /// Current durable relay policies.
    pub policies: Vec<RelayPolicyStatusDto>,
    /// Queued canonical delivery intents in the observed page.
    pub queued: u64,
    /// Prepared exact delivery lineages in the observed page.
    pub prepared: u64,
    /// Uncertain relay attempts in the observed page.
    pub uncertain: u64,
    /// Explicitly rejected relay attempts in the observed page.
    pub rejected: u64,
    /// Positively accepted relay attempts in the observed page.
    pub accepted: u64,
    /// Transient inbound wrappers in the observed page.
    pub staged: u64,
    /// Permanently rejected evidence in the observed page.
    pub quarantined: u64,
    /// Whether additional durable rows exist beyond the observation.
    pub truncated: bool,
}

/// Stable reducer domain name used by health output.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthDomainDto {
    /// Installation, mailbox, peer, capability, and account authority.
    Authority,
    /// Conversation, message, and activity state.
    Conversation,
    /// Named agents and provider sessions.
    Agent,
    /// Projects, resources, assignments, and control.
    Project,
}

/// Passive decision counts for one reducer domain.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DomainHealthDto {
    /// Stable domain name.
    pub domain: HealthDomainDto,
    /// Admitted facts.
    pub projected: u64,
    /// Dependency-incomplete facts.
    pub unresolved: u64,
    /// Authority-rejected facts.
    pub unauthorized: u64,
    /// Explicitly conflicted facts.
    pub conflicted: u64,
    /// Intrinsically invalid facts.
    pub invalid: u64,
    /// Unsupported facts.
    pub unsupported: u64,
    /// Normalized aggregate/global conflicts.
    pub conflicts: u64,
}

/// Passive authoritative domain health observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateHealthDto {
    /// Serialized local revision.
    pub revision: u64,
    /// Complete fixed domain catalog.
    pub domains: Vec<DomainHealthDto>,
}

/// Passive explicit repair result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateRepairReportDto {
    /// Caller-selected stable audit identity.
    pub operation_id: Id32,
    /// Serialized local revision after repair.
    pub revision: u64,
    /// Complete repaired domain health.
    pub domains: Vec<DomainHealthDto>,
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

/// One sensitive environment entry encoded without requiring UTF-8 values.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchEnvironmentEntryDto {
    name: String,
    #[serde(with = "environment_value")]
    value: Vec<u8>,
}

impl Drop for LaunchEnvironmentEntryDto {
    fn drop(&mut self) {
        self.value.fill(0);
    }
}

/// Opaque sensitive environment snapshot with redacted diagnostics.
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LaunchEnvironmentDto(Vec<LaunchEnvironmentEntryDto>);

impl LaunchEnvironmentDto {
    /// Copies a caller environment into independently owned wire values.
    pub fn copy_from<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<Self, ValueError> {
        let copied = hq_application::LaunchEnvironment::copy_from(entries)
            .map_err(|_| ValueError::InvalidText)?;
        let mut values = Vec::with_capacity(copied.len());
        copied.visit(|name, value| {
            values.push(LaunchEnvironmentEntryDto {
                name: name.to_owned(),
                value: value.to_vec(),
            });
        });
        Ok(Self(values))
    }

    /// Visits sensitive values without transferring ownership.
    pub fn visit(&self, mut visitor: impl FnMut(&str, &[u8])) {
        for entry in &self.0 {
            visitor(&entry.name, &entry.value);
        }
    }

    /// Returns the number of copied entries without exposing names or values.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Reports whether the copied environment is empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for LaunchEnvironmentDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchEnvironmentDto")
            .field("entry_count", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Sensitive start/resume context interpreted only by the owning node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLaunchContextDto {
    /// Absolute launch directory selected by the caller.
    pub directory: ResourceLocatorDto,
    /// Complete copied caller environment.
    pub environment: LaunchEnvironmentDto,
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
    /// Required sensitive launch context for start/resume; absent for stop.
    pub launch: Option<AgentLaunchContextDto>,
}

impl AgentSessionRequestDto {
    /// Constructs a provider-neutral session control body.
    pub fn new(
        agent_id: Id32,
        provider: String,
        control: SessionControlDto,
        launch: Option<AgentLaunchContextDto>,
    ) -> Result<Self, ValueError> {
        let request = Self {
            agent_id,
            provider,
            control,
            launch,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), ValueError> {
        validate_text(&self.provider, PROVIDER_ID_MAX_BYTES)?;
        if let SessionControlDto::Resume(session) = &self.control {
            validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
        }
        if matches!(self.control, SessionControlDto::Stop) != self.launch.is_none() {
            return Err(ValueError::InvalidText);
        }
        if let Some(launch) = &self.launch {
            validate_locator(&launch.directory)?;
            let mut entries = Vec::with_capacity(launch.environment.len());
            launch
                .environment
                .visit(|name, value| entries.push((name.to_owned(), value.to_vec())));
            hq_application::LaunchEnvironment::copy_from(
                entries
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_slice())),
            )
            .map_err(|_| ValueError::InvalidText)?;
        }
        Ok(())
    }
}

/// Derives the exact digest for one retry-safe managed-session request.
pub fn agent_session_request_digest(
    request: &EffectRequestDto<AgentSessionRequestDto>,
) -> Result<CommandDigest, ValueError> {
    request.body.validate()?;
    let mut digest = Sha256::new();
    digest.update(AGENT_SESSION_DIGEST_DOMAIN);
    digest.update(request.operation_id.bytes());
    digest.update(request.issued_at_unix_millis.to_be_bytes());
    digest.update(request.body.agent_id.bytes());
    update_sized(&mut digest, request.body.provider.as_bytes())?;
    match &request.body.control {
        SessionControlDto::Start => digest.update([0]),
        SessionControlDto::Resume(session) => {
            digest.update([1]);
            update_sized(&mut digest, session.as_bytes())?;
        }
        SessionControlDto::Stop => digest.update([2]),
    }
    match &request.body.launch {
        None => digest.update([0]),
        Some(launch) => {
            digest.update([1]);
            digest.update([match launch.directory.scheme {
                ResourceSchemeDto::GitRepository => 0,
                ResourceSchemeDto::WorkingTree => 1,
                ResourceSchemeDto::Container => 2,
                ResourceSchemeDto::Opaque => 3,
            }]);
            update_sized(&mut digest, launch.directory.value.as_bytes())?;
            let count =
                u32::try_from(launch.environment.len()).map_err(|_| ValueError::InvalidText)?;
            digest.update(count.to_be_bytes());
            launch.environment.visit(|name, value| {
                // Validation above proves these conversions fit in u32.
                let name_len = u32::try_from(name.len()).unwrap_or(u32::MAX);
                let value_len = u32::try_from(value.len()).unwrap_or(u32::MAX);
                digest.update(name_len.to_be_bytes());
                digest.update(name.as_bytes());
                digest.update(value_len.to_be_bytes());
                digest.update(value);
            });
        }
    }
    Ok(CommandDigest::from_bytes(digest.finalize().into()))
}

fn update_sized(digest: &mut Sha256, value: &[u8]) -> Result<(), ValueError> {
    let len = u32::try_from(value.len()).map_err(|_| ValueError::InvalidText)?;
    digest.update(len.to_be_bytes());
    digest.update(value);
    Ok(())
}

mod environment_value {
    use super::*;
    use serde::{Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD_NO_PAD.encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let decoded = STANDARD_NO_PAD
            .decode(encoded.as_bytes())
            .map_err(D::Error::custom)?;
        if STANDARD_NO_PAD.encode(&decoded) != encoded {
            return Err(D::Error::custom("noncanonical environment value"));
        }
        Ok(decoded)
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

/// Derives the exact digest for one read-only resource inspection request.
pub fn resource_inspection_request_digest(
    request: &EffectRequestDto<ResourceInspectionRequestDto>,
) -> Result<CommandDigest, ValueError> {
    validate_locator(&request.body.display_locator)?;
    validate_locator(&request.body.canonical_locator)?;
    if request.body.display_locator.scheme != request.body.canonical_locator.scheme {
        return Err(ValueError::InvalidValueCombination);
    }
    let mut digest = Sha256::new();
    digest.update(RESOURCE_INSPECTION_DIGEST_DOMAIN);
    digest.update(request.operation_id.bytes());
    digest.update(request.issued_at_unix_millis.to_be_bytes());
    digest.update(request.body.project_id.bytes());
    digest.update(request.body.resource_id.bytes());
    for locator in [
        &request.body.display_locator,
        &request.body.canonical_locator,
    ] {
        digest.update([match locator.scheme {
            ResourceSchemeDto::GitRepository => 0,
            ResourceSchemeDto::WorkingTree => 1,
            ResourceSchemeDto::Container => 2,
            ResourceSchemeDto::Opaque => 3,
        }]);
        update_sized(&mut digest, locator.value.as_bytes())?;
    }
    Ok(CommandDigest::from_bytes(digest.finalize().into()))
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
    pub base: Option<String>,
    pub create_branch: bool,
}

/// Exact existing-resource project creation input.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCreationRequestDto {
    pub mailbox_id: Id32,
    pub project_name: String,
    pub brief: Option<String>,
    pub resource_id: Id32,
    pub resource: ResourceLocatorDto,
}

/// Closed project action catalog for local API v1.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum ProjectCommandActionDto {
    Create(ProjectCreationRequestDto),
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
        resource_id: Id32,
        resource: ResourceLocatorDto,
        make_primary: bool,
    },
    RemoveResource {
        resource_id: Id32,
        force: bool,
    },
    ReplaceResource {
        old_resource_id: Id32,
        new_resource_id: Id32,
        resource: ResourceLocatorDto,
    },
    SetPrimaryResource {
        resource_id: Id32,
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

/// Stable exact node-owned named-agent retirement request.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRetirementRequestDto {
    pub command_id: Id32,
    pub operation_id: Id32,
    pub request_digest: Id32,
    pub account_id: Id32,
    pub agent_id: Id32,
    pub expected_claim: Id32,
    pub home: Id32,
    pub issued_at_unix_millis: i64,
    pub force: bool,
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
        external_state_warning: Option<ProjectExternalStateWarningDto>,
    },
    Reconcilable {
        operation_id: Id32,
        stage: ProjectCommandStageDto,
        error: DomainErrorDto,
        external_state_warning: Option<ProjectExternalStateWarningDto>,
    },
}

/// External state deliberately retained after a project workflow boundary.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectExternalStateWarningDto {
    WorktreeMayExist {
        destination: ResourceLocatorDto,
        branch: String,
    },
}

/// Typed node-owned named-agent retirement progress or terminal result.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentRetirementOutcomeDto {
    Running {
        operation_id: Id32,
        stage: ProjectCommandStageDto,
    },
    Completed {
        operation_id: Id32,
        project_id: Option<Id32>,
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
    Rejected {
        error: String,
        external_state_warning: Option<ProjectExternalStateWarningDto>,
    },
}

/// Authoritative remote-control checkpoint with exact fact attribution.
#[allow(missing_docs)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum RemoteCommandProgressDto {
    Queued,
    Received {
        receipt_fact: Id32,
        received_head: Option<Id32>,
        received_at_unix_millis: i64,
    },
    Terminal {
        receipt_fact: Id32,
        received_head: Option<Id32>,
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

/// Closed provider-neutral interaction class.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKindDto {
    /// Ask for text or one offered choice.
    Question,
    /// Approve command execution.
    CommandApproval,
    /// Approve file changes.
    FileApproval,
    /// Grant a permission scope.
    Permission,
    /// Resolve an MCP URL request.
    McpUrl,
    /// Supply an MCP form response.
    McpForm,
}

/// One stable response value and human-facing label.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionChoiceDto {
    /// Untouched stable value.
    pub value: String,
    /// Human-facing label.
    pub label: String,
}

/// One pending memory-only provider interaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingInteractionDto {
    /// Named agent awaiting the response.
    pub agent_id: Id32,
    /// Project whose work is blocked, when present.
    pub project_id: Option<Id32>,
    /// Neutral provider namespace.
    pub provider: String,
    /// Exact live provider session.
    pub session: String,
    /// Provider-originated request identity.
    pub request_id: Id32,
    /// Operation blocked by the request.
    pub operation_id: Id32,
    /// Typed request family.
    pub kind: InteractionKindDto,
    /// Exact bounded non-secret prompt.
    pub prompt: String,
    /// Source-ordered bounded choices.
    pub choices: Vec<InteractionChoiceDto>,
    /// Whether bounded free-text input is permitted.
    pub allow_text: bool,
}

/// Bounded pending-interaction query.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingInteractionsRequestDto {
    /// Inclusive maximum rows requested.
    pub limit: u16,
}

/// Closed non-secret terminal response shape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum InteractionResponseDto {
    /// Bounded free text or encoded structured form content.
    Text(String),
    /// One untouched stable offered value.
    Choice(String),
    /// Explicit approval or denial.
    Approval(bool),
    /// Explicit cancellation.
    Cancelled,
}

/// Exact-once interaction answer command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionAnswerRequestDto {
    /// Caller-selected command identity.
    pub command_id: Id32,
    /// Named agent owning the request.
    pub agent_id: Id32,
    /// Provider-originated request identity.
    pub request_id: Id32,
    /// Complete typed terminal response.
    pub response: InteractionResponseDto,
}

/// Terminal response command outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionAnswerOutcomeDto {
    /// The sole provider-session owner accepted the response.
    Answered,
    /// The request was already absent or terminal.
    Stale,
}

/// Written acknowledgement for one pending responder registration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionResponderAcknowledgement {
    /// Stable session-owned responder identity.
    pub responder_id: Id32,
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
    /// Load one coherent authoritative snapshot and optional selected first page.
    AuthoritativeConversationView(AuthoritativeConversationViewRequestDto),
    /// Load providers registered with this running installation.
    ProviderCatalog,
    /// Load one bounded reducer-ordered conversation page.
    ConversationPage(ConversationPageRequest),
    /// Load every bounded installation-local mailbox draft.
    MailboxDrafts,
    /// Create or optimistically replace one complete mailbox draft.
    SaveMailboxDraft(MailboxDraftSaveRequestDto),
    /// Idempotently and optimistically delete one mailbox draft.
    DeleteMailboxDraft(MailboxDraftDeleteRequestDto),
    /// Execute or reconcile one node-resolved mailbox command.
    ControlMailbox(Box<MailboxCommandRequestDto>),
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
    /// Load bounded authoritative relay and delivery health.
    RelayStatus,
    /// Load authoritative reducer-domain health.
    StateHealth,
    /// Explicitly reverify the corpus and repair rebuildable state.
    RepairState {
        /// Stable caller-selected audit identity.
        operation_id: Id32,
    },
    /// Control one provider-neutral named-agent session.
    ControlAgentSession(Box<EffectRequestDto<AgentSessionRequestDto>>),
    /// Inspect one typed project resource.
    InspectResource(EffectRequestDto<ResourceInspectionRequestDto>),
    /// Execute, route, or reconcile one exact project command.
    ControlProject(Box<ProjectCommandRequestDto>),
    /// Execute or reconcile one exact node-owned named-agent retirement.
    RetireAgent(Box<AgentRetirementRequestDto>),
    /// Load one bounded passive pending-interaction view.
    PendingInteractions(PendingInteractionsRequestDto),
    /// Execute or reconcile one exact terminal interaction response.
    AnswerInteraction(InteractionAnswerRequestDto),
    /// Prepare a responder registration pending its written acknowledgement.
    RegisterInteractionResponder {
        /// Stable session-owned responder identity.
        responder_id: Id32,
    },
    /// Cancel one pending or active responder registration idempotently.
    CancelInteractionResponder {
        /// Stable session-owned responder identity.
        responder_id: Id32,
    },
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
    /// Boot-local generation returned by the live protocol peer.
    pub generation: Option<Id32>,
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
            generation: None,
            detail,
        })
    }

    /// Binds this live response to one boot-local readiness generation.
    #[must_use]
    pub const fn with_generation(mut self, generation: Id32) -> Self {
        self.generation = Some(generation);
        self
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
        /// Complete signed route-set history.
        routes: Vec<PeerRouteCandidateDto>,
        /// Complete signed route-block history.
        blocks: Vec<PeerRouteBlockDto>,
    },
    /// Directional mailbox capability lineage.
    MailboxCapability {
        /// Stable grant identity.
        grant_id: Id32,
        /// Exact supporting capability-grant fact.
        grant_fact: Id32,
        /// Mailbox-owning installation.
        mailbox_installation: Id32,
        /// Target mailbox identity.
        mailbox_id: Id32,
        /// Grantee installation.
        grantee_installation: Id32,
        /// Exact grantee signing key.
        grantee_signing_key: Id32,
        /// Whether the capability is active.
        active: bool,
        /// Causal-maximal revoke facts.
        revoke_frontier: Vec<Id32>,
        /// Owner-observed action identities retained by history.
        observed_actions: Vec<Id32>,
        /// Complete projection support including observation facts.
        support: Vec<Id32>,
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
        /// Typed authoritative human-facing context.
        context: ConversationContextDto,
        /// Exact reserved local-human mailbox for author presentation.
        local_human: MailboxAddressDto,
        /// Stable initiating message identity for a project thread.
        root_message: Option<Id32>,
        /// Sanitized bounded one-line message preview.
        preview: Option<String>,
        /// Canonically latest presented fact, when nonempty.
        latest_fact: Option<Id32>,
        /// Number of currently open actionable messages.
        open_messages: u32,
        /// Number of messages outside the open view.
        archived_messages: u32,
        /// Number of messages authored by the reserved local human mailbox.
        sent_messages: u32,
    },
    /// Inert message whose required causal history is incomplete.
    IncompleteMessage {
        /// Canonical message-bearing fact.
        fact_id: Id32,
        /// Stable public message identity.
        message_id: Id32,
        /// Declared or derived causal thread identity.
        thread_id: Id32,
        /// Sender installation identity.
        sender_installation: Id32,
        /// Sender mailbox identity.
        sender_mailbox: Id32,
        /// Recipient installation identity when directly addressed.
        recipient_installation: Option<Id32>,
        /// Recipient mailbox identity when directly addressed.
        recipient_mailbox: Option<Id32>,
        /// Bounded message body.
        content: String,
        /// Typed semantic purpose.
        purpose: MessagePurposeDto,
        /// Typed presentation behavior.
        presentation: PresentationKindDto,
        /// Correlated provider namespace when present.
        correlation_provider: Option<String>,
        /// Correlated provider session when present.
        correlation_session: Option<String>,
        /// Correlated operation identity when present.
        correlation_operation: Option<Id32>,
        /// Optional project association.
        project_id: Option<Id32>,
        /// Required causal identities that are absent.
        missing_dependencies: Vec<Id32>,
        /// Present causal identities that are unusable.
        unusable_dependencies: Vec<Id32>,
    },
    /// More dependency-incomplete addressed messages exist beyond the diagnostic bound.
    IncompleteMessagesTruncated,
    /// Named agent current presentation state.
    Agent {
        /// Agent identity.
        agent_id: Id32,
        /// Exact compatible permanent-name claim facts.
        claims: Vec<Id32>,
        /// Candidate permanent names.
        names: Vec<String>,
        /// Candidate installation-qualified agent mailboxes.
        mailboxes: Vec<MailboxAddressDto>,
        /// Absorbing retirement facts.
        retirements: Vec<Id32>,
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
        /// Exact immutable binding facts and their candidate mailboxes.
        bindings: Vec<AgentSessionBindingDto>,
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
        /// Causal-maximal candidate values and their exact facts.
        candidates: Vec<AgentSelectionCandidateDto>,
        /// Selected provider, when resolved.
        provider: Option<String>,
        /// Selected provider session, when resolved.
        session: Option<String>,
        /// Exact causal-maximal selection facts.
        frontier: Vec<Id32>,
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
        /// Causal-maximal display-name candidates and their exact facts.
        candidates: Vec<AgentSessionNameCandidateDto>,
        /// Exact causal-maximal rename facts.
        frontier: Vec<Id32>,
        /// Whether the register has one resolved value.
        resolved: bool,
        /// Resolved display name or explicit clear.
        display_name: Option<String>,
    },
    /// Grow-only repository-context history for one provider session.
    AgentContext {
        /// Context-owning mailbox installation.
        mailbox_installation: Id32,
        /// Context-owning mailbox identity.
        mailbox_id: Id32,
        /// Complete typed context history in fact order.
        history: Vec<RepositoryContextDto>,
        /// Exact causal-maximal context fact frontier.
        frontier: Vec<Id32>,
    },
    /// Permanent direct provider-session mailbox binding.
    AgentDirectSession {
        /// Provider namespace.
        provider: String,
        /// Provider-scoped session identity.
        session: String,
        /// Bound mailbox installation.
        mailbox_installation: Id32,
        /// Bound mailbox identity.
        mailbox_id: Id32,
        /// Unique compatible named agent when present.
        named_agent: Option<Id32>,
        /// Whether incompatible binding history blocks use.
        conflicted: bool,
    },
    /// Project current presentation state.
    Project {
        /// Project identity.
        project_id: Id32,
        /// Immutable home installation.
        home: Id32,
        /// Immutable human account whose devices address the project.
        account_id: Id32,
        /// Immutable project mailbox on the home installation.
        mailbox_id: Id32,
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
    /// Current authoritative project assignment.
    ProjectAssignment {
        /// Owning project.
        project_id: Id32,
        /// Immutable assignment epoch.
        assignment_id: Id32,
        /// Assigned named agent.
        agent_id: Id32,
        /// Selected provider namespace.
        provider: String,
        /// Acknowledged provider session, when startup reached one.
        session: Option<String>,
        /// Stable configuring, runnable, or blocked phase.
        phase: String,
        /// Runnable project thread, when startup completed.
        thread_id: Option<Id32>,
        /// Runnable launch directory, when startup completed.
        launch_directory: Option<ResourceLocatorDto>,
        /// Stable blocking error code, when blocked.
        blocked: Option<String>,
        /// Whether project/agent cardinality is conflicted.
        cardinality_conflicted: bool,
        /// Whether the assignment is currently runnable.
        runnable: bool,
        /// Exact supporting fact set.
        support: Vec<Id32>,
    },
    /// One exact historical provider-session/project-thread binding.
    ProjectThread {
        /// Owning project.
        project_id: Id32,
        /// Named agent that owned the thread.
        agent_id: Id32,
        /// Provider namespace.
        provider: String,
        /// Exact provider session.
        session: String,
        /// Immutable project thread.
        thread_id: Id32,
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
        /// Immutable causal thread containing this accepted input.
        thread_id: Id32,
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
        /// Caller-observed canonical head, absent for creation.
        expected_head: Option<Id32>,
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

/// One immutable repository context observation and its canonical fact identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryContextDto {
    /// Canonical context fact identity.
    pub fact_id: Id32,
    /// Canonical working-directory locator.
    pub directory: ResourceLocatorDto,
    /// Optional canonical repository identity.
    pub repository: Option<ResourceLocatorDto>,
    /// Optional canonical worktree identity.
    pub worktree: Option<ResourceLocatorDto>,
    /// Optional bounded display branch.
    pub branch: Option<String>,
}

/// Passive installation-qualified mailbox identity inside an administrative projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxAddressDto {
    /// Owning installation.
    pub installation_id: Id32,
    /// Mailbox identity.
    pub mailbox_id: Id32,
}

/// One causal-maximal durable provider-session selection candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSelectionCandidateDto {
    /// Exact selection fact.
    pub fact_id: Id32,
    /// Neutral provider namespace.
    pub provider: String,
    /// Exact provider-scoped session.
    pub session: String,
}

/// One exact immutable provider-session binding candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionBindingDto {
    /// Exact binding fact.
    pub fact_id: Id32,
    /// Candidate installation-qualified mailbox.
    pub mailbox: MailboxAddressDto,
}

/// One causal-maximal provider-session display-name candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionNameCandidateDto {
    /// Exact rename fact.
    pub fact_id: Id32,
    /// Candidate display name, or `None` for an explicit clear.
    pub display_name: Option<String>,
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

/// Passive exact directional peer-route candidate history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRouteCandidateDto {
    /// Exact route-set fact.
    pub fact_id: Id32,
    /// Exact peer signing key.
    pub signing_key: Id32,
    /// Exact peer transport encryption key.
    pub encryption_key: Id32,
    /// Optional signed display label.
    pub label: Option<String>,
    /// Signed non-authority relay hints.
    pub relay_hints: Vec<ResourceLocatorDto>,
    /// Whether this route set is a causal maximum.
    pub frontier_member: bool,
}

/// Passive exact directional peer-route block history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRouteBlockDto {
    /// Exact route-block fact.
    pub fact_id: Id32,
    /// Stable signed block reason.
    pub reason: String,
    /// Whether this block is a causal maximum.
    pub frontier_member: bool,
}

/// One provider registration exposed without concrete adapter details.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAvailabilityDto {
    /// Stable neutral provider namespace.
    pub provider: String,
    /// User-facing provider name.
    pub name: String,
    /// Whether the running node can start new sessions with this provider.
    pub available: bool,
}

impl ProviderAvailabilityDto {
    /// Constructs one bounded provider presentation.
    pub fn new(
        provider: impl Into<String>,
        name: impl Into<String>,
        available: bool,
    ) -> Result<Self, ValueError> {
        let value = Self {
            provider: provider.into(),
            name: name.into(),
            available,
        };
        validate_provider_availability(&value)?;
        Ok(value)
    }
}

/// Complete installation-local provider catalog and configured preference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogDto {
    /// Provider registrations in stable namespace order.
    pub providers: Vec<ProviderAvailabilityDto>,
    /// Configured preference, including a stale value no longer registered.
    pub default_provider: Option<String>,
}

impl ProviderCatalogDto {
    /// Constructs one bounded, uniquely ordered provider catalog.
    pub fn new(
        providers: Vec<ProviderAvailabilityDto>,
        default_provider: Option<String>,
    ) -> Result<Self, ValueError> {
        let value = Self {
            providers,
            default_provider,
        };
        validate_provider_catalog(&value)?;
        Ok(value)
    }
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
    Message(Box<ConversationMessageDto>),
    /// Durable or selected harness activity presentation.
    Activity(Box<ConversationActivityDto>),
}

/// Durable or selected harness activity presentation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationActivityDto {
    /// Canonical fact identity.
    pub fact_id: Id32,
    /// Closed activity family independent from display content.
    pub activity_kind: ConversationActivityKindDto,
    /// Positive source sequence.
    pub sequence: u64,
    /// Exact source installation identity.
    pub source_installation: Id32,
    /// Exact source mailbox identity.
    pub source_mailbox: Id32,
    /// Exact provider namespace.
    pub provider: String,
    /// Exact provider-scoped session identity.
    pub session: String,
    /// Exact operation identity.
    pub operation: Id32,
    /// Optional provider item identity.
    pub item: Option<String>,
    /// Stable logical key within the operation.
    pub logical_key: String,
    /// Bounded runtime identity.
    pub runtime: String,
    /// Signed occurrence time in Unix milliseconds.
    pub occurred_at_unix_ms: i64,
    /// Typed activity status and optional stable failure reason.
    pub status: ActivityStatusDto,
    /// Bounded display content.
    pub content: String,
    /// Whether authoring truncated the content.
    pub truncated: bool,
    /// Structured presentation for a durable completed item.
    pub completed: Option<CompletedItemPresentationDto>,
}

/// One changed-file presentation record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedFileChangeDto {
    /// Provider-reported changed path.
    pub path: String,
    /// Optional bounded diff retained for technical detail.
    pub diff: Option<String>,
    /// Whether the path was shortened.
    pub path_truncated: bool,
    /// Whether the diff was shortened.
    pub diff_truncated: bool,
}

/// Closed completed-item presentation carried without parsing flattened prose.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompletedItemPresentationDto {
    /// Completed command execution.
    Command {
        /// Multiline command source.
        command: String,
        /// Optional aggregated command output.
        output: Option<String>,
        /// Provider-reported exit code.
        exit_code: Option<i64>,
        /// Whether command source was shortened.
        command_truncated: bool,
        /// Whether output was shortened.
        output_truncated: bool,
    },
    /// Completed file changes.
    FileChange {
        /// Bounded per-file records.
        changes: Vec<CompletedFileChangeDto>,
        /// Whether additional file records were omitted.
        changes_truncated: bool,
    },
    /// Completed tool call.
    Tool {
        /// Retained server/tool or tool-family name.
        name: String,
        /// Whether the name was shortened.
        name_truncated: bool,
    },
    /// Completed web search.
    WebSearch {
        /// Retained query.
        query: String,
        /// Whether the query was shortened.
        query_truncated: bool,
    },
    /// Explicit unknown completed family.
    Unknown,
}

/// Closed conversation-activity family on local API v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationActivityKindDto {
    /// Generic operation status.
    Status,
    /// Provider-neutral agent-turn lifecycle.
    AgentTurn,
    /// Incremental progress.
    Progress,
    /// Plan or task state.
    Plan,
    /// Proposed-change snapshot.
    Diff,
    /// Durable completed command, file, or tool item.
    CompletedItem,
}

/// Typed activity status on the stable local protocol.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ActivityStatusDto {
    /// Informational snapshot without a lifecycle claim.
    Snapshot,
    /// Correlated work remains active.
    Running,
    /// Correlated work completed successfully.
    Succeeded,
    /// Correlated work failed with a stable reason code.
    Failed {
        /// Stable bounded failure reason.
        reason: String,
    },
    /// Correlated work was explicitly interrupted.
    Interrupted,
}

/// Passive typed message presentation carried by one conversation page item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ConversationMessageDto {
    /// Canonical fact identity.
    pub fact_id: Id32,
    /// Stable public message identity.
    pub message_id: Id32,
    /// Causal thread identity.
    pub thread_id: Id32,
    /// Bounded message content.
    pub content: String,
    /// Sender installation identity.
    pub sender_installation: Id32,
    /// Sender mailbox identity.
    pub sender_mailbox: Id32,
    /// Recipient installation identity when directly addressed.
    pub recipient_installation: Option<Id32>,
    /// Recipient mailbox identity when directly addressed.
    pub recipient_mailbox: Option<Id32>,
    /// Typed semantic message purpose.
    pub purpose: MessagePurposeDto,
    /// Typed presentation behavior.
    pub presentation: PresentationKindDto,
    /// Correlated provider namespace when present.
    pub correlation_provider: Option<String>,
    /// Correlated provider session when present.
    pub correlation_session: Option<String>,
    /// Correlated operation identity when present.
    pub correlation_operation: Option<Id32>,
    /// Optional project association.
    pub project_id: Option<Id32>,
    /// Whether the message remains open.
    pub open: bool,
    /// Whether the message was absorbing-rejected.
    pub rejected: bool,
    /// Exact causal-maximal reversible-state facts.
    pub state_frontier: Vec<Id32>,
    /// Peer-authored usable children proving receipt.
    pub peer_received_by: Vec<Id32>,
    /// Exact question root fact when normalized thread state exists.
    pub root_fact: Option<Id32>,
    /// Stable root message identity when normalized thread state exists.
    pub root_message: Option<Id32>,
    /// Whether this fact is currently a ready answer.
    pub ready_answer: bool,
    /// Whether at least one valid thread cancellation exists.
    pub thread_cancelled: bool,
}

/// Typed message purpose on the stable local protocol.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagePurposeDto {
    /// A question expects an answer.
    Question,
    /// An asynchronous message does not imply a blocking wait.
    Asynchronous,
    /// Output produced for a project input.
    ProjectOutput,
}

/// Typed message presentation on the stable local protocol.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationKindDto {
    /// Ordinary user or agent prose.
    Message,
    /// Final answer from a managed operation.
    FinalAnswer,
    /// Concise status or progress notice.
    Status,
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

/// One bounded selected conversation page in a materialized authoritative view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedConversationPageDto {
    /// Stable typed conversation identity.
    pub key: ConversationKeyDto,
    /// Bounded reducer-ordered first page.
    pub page: ConversationPageDto,
}

impl SelectedConversationPageDto {
    /// Constructs one selected first page.
    pub const fn new(key: ConversationKeyDto, page: ConversationPageDto) -> Self {
        Self { key, page }
    }
}

/// One authoritative snapshot and optional selected page from one serialized state boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeConversationViewDto {
    /// Complete authoritative projection snapshot.
    pub snapshot: AuthoritativeSnapshotDto,
    /// Selected first page, when requested.
    pub conversation: Option<SelectedConversationPageDto>,
}

impl AuthoritativeConversationViewDto {
    /// Constructs and validates one bounded coherent view.
    pub fn new(
        snapshot: AuthoritativeSnapshotDto,
        conversation: Option<SelectedConversationPageDto>,
    ) -> Result<Self, ValueError> {
        let view = Self {
            snapshot,
            conversation,
        };
        validate_authoritative_conversation_view(&view)?;
        Ok(view)
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
    /// Fresh clean, dirty, unknown, or not-applicable release classification.
    pub release: ResourceReleaseStateDto,
    /// Optional bounded inert observation detail.
    pub details: Option<String>,
    /// Explicit observation time.
    pub checked_at_unix_millis: i64,
}

/// Closed resource-release classification.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceReleaseStateDto {
    /// Safe to release without force.
    Clean,
    /// Contains changes and requires force.
    Dirty,
    /// Safety could not be established.
    Unknown,
    /// No release assessment applies.
    NotApplicable,
}

impl ResourceInspectionResultDto {
    /// Constructs a bounded inert resource observation.
    pub fn new(
        health: ResourceHealthDto,
        observed_canonical: Option<ResourceLocatorDto>,
        release: ResourceReleaseStateDto,
        details: Option<String>,
        checked_at_unix_millis: i64,
    ) -> Result<Self, ValueError> {
        if let Some(details) = &details {
            validate_text(details, CONTENT_MAX_BYTES)?;
        }
        Ok(Self {
            health,
            observed_canonical,
            release,
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
    /// Materialized view loaded after pending registration.
    pub view: AuthoritativeConversationViewDto,
}

impl SubscriptionAcknowledgement {
    /// Constructs a pending-registration acknowledgement with its authoritative snapshot.
    pub const fn new(subscription_id: Id32, view: AuthoritativeConversationViewDto) -> Self {
        Self {
            subscription_id,
            view,
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
    /// Coherent authoritative snapshot and optional selected first page.
    AuthoritativeConversationView(AuthoritativeConversationViewDto),
    /// Passive provider registrations and configured preference.
    ProviderCatalog(ProviderCatalogDto),
    /// Bounded conversation page.
    ConversationPage(ConversationPageDto),
    /// Every bounded installation-local mailbox draft.
    MailboxDrafts(Vec<MailboxDraftDto>),
    /// Optimistic mailbox-draft autosave outcome.
    MailboxDraftSave(MailboxDraftSaveOutcomeDto),
    /// Optimistic mailbox-draft deletion outcome.
    MailboxDraftDelete(MailboxDraftDeleteOutcomeDto),
    /// Retry-safe mutation attempt.
    Mutation(MutationAttemptDto),
    /// Bounded exact canonical evidence closure.
    CanonicalEvidence(Vec<CanonicalEvidenceDto>),
    /// Per-fact idempotent evidence import outcomes.
    EvidenceIngest(Vec<EvidenceIngestOutcomeDto>),
    /// Relay configuration or synchronization effect outcome.
    EmptyEffect(EffectOutcomeDto<()>),
    /// Bounded authoritative relay and delivery health.
    RelayStatus(RelayStatusDto),
    /// Authoritative reducer-domain health.
    StateHealth(StateHealthDto),
    /// Explicit rebuildable-state repair result.
    StateRepair(StateRepairReportDto),
    /// Named-agent session effect outcome.
    AgentSession(EffectOutcomeDto<AgentSessionResultDto>),
    /// Resource inspection effect outcome.
    ResourceInspection(EffectOutcomeDto<ResourceInspectionResultDto>),
    /// Project command submission or durable progress.
    ProjectCommand(ProjectCommandOutcomeDto),
    /// Named-agent retirement progress or terminal result.
    AgentRetirement(AgentRetirementOutcomeDto),
    /// Bounded passive pending interactions.
    PendingInteractions(Vec<PendingInteractionDto>),
    /// Exact interaction-answer terminal outcome.
    InteractionAnswer(InteractionAnswerOutcomeDto),
    /// Pending responder acknowledgement.
    InteractionResponder(InteractionResponderAcknowledgement),
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
                Request::AuthoritativeConversationView(request) => request.validate(),
                Request::SaveMailboxDraft(request) => {
                    if request.content.len() > CONTENT_MAX_BYTES
                        || request.expected_version == Some(0)
                    {
                        Err(ValueError::InvalidCanonicalPlan)
                    } else {
                        Ok(())
                    }
                }
                Request::DeleteMailboxDraft(request) => {
                    if request.expected_version == 0 {
                        Err(ValueError::InvalidCanonicalPlan)
                    } else {
                        Ok(())
                    }
                }
                Request::ControlMailbox(request) => request.validate(),
                Request::Mutation(request) => request.validate(),
                Request::CanonicalEvidence(request) => {
                    if request.roots.is_empty() {
                        return Err(ValueError::TooManyItems);
                    }
                    validate_id_set(&request.roots, MAX_CANONICAL_EVIDENCE_ITEMS)
                }
                Request::IngestCanonicalEvidence(evidence) => validate_evidence(evidence),
                Request::Subscribe(request) => {
                    validate_topics(&request.topics)?;
                    request
                        .conversation
                        .as_ref()
                        .map_or(Ok(()), ConversationPageSelectionDto::validate)
                }
                Request::PendingInteractions(request) => validate_pending_interactions(*request),
                Request::AnswerInteraction(request) => {
                    validate_interaction_response(&request.response)
                }
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
                Request::MailboxDrafts
                | Request::RetireAgent(_)
                | Request::Lifecycle(_)
                | Request::AuthoritativeSnapshot
                | Request::ProviderCatalog
                | Request::RelayStatus
                | Request::StateHealth
                | Request::RepairState { .. }
                | Request::RegisterInteractionResponder { .. }
                | Request::CancelInteractionResponder { .. }
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

fn validate_provider_availability(provider: &ProviderAvailabilityDto) -> Result<(), ValueError> {
    validate_text(&provider.provider, PROVIDER_ID_MAX_BYTES)?;
    validate_text(&provider.name, SHORT_TEXT_MAX_BYTES)
}

fn validate_provider_catalog(catalog: &ProviderCatalogDto) -> Result<(), ValueError> {
    if catalog.providers.len() > MAX_PROVIDER_CATALOG_ITEMS
        || catalog
            .providers
            .windows(2)
            .any(|pair| pair[0].provider >= pair[1].provider)
    {
        return Err(ValueError::InvalidValueCombination);
    }
    for provider in &catalog.providers {
        validate_provider_availability(provider)?;
    }
    if let Some(provider) = &catalog.default_provider {
        validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
    }
    Ok(())
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

#[allow(clippy::too_many_lines)]
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
        Response::Success(ResponseResult::ProviderCatalog(catalog)) => {
            validate_provider_catalog(catalog)
        }
        Response::Success(ResponseResult::ConversationPage(page)) => validate_page(page),
        Response::Success(ResponseResult::MailboxDrafts(drafts)) => {
            if drafts.len() > hq_application::MAX_MAILBOX_DRAFTS
                || drafts
                    .iter()
                    .any(|draft| draft.version == 0 || draft.content.len() > CONTENT_MAX_BYTES)
            {
                Err(ValueError::InvalidCanonicalPlan)
            } else {
                Ok(())
            }
        }
        Response::Success(ResponseResult::MailboxDraftSave(outcome)) => {
            let draft = match outcome {
                MailboxDraftSaveOutcomeDto::Saved(draft)
                | MailboxDraftSaveOutcomeDto::Conflict(draft) => draft,
            };
            if draft.version == 0 || draft.content.len() > CONTENT_MAX_BYTES {
                Err(ValueError::InvalidCanonicalPlan)
            } else {
                Ok(())
            }
        }
        Response::Success(ResponseResult::MailboxDraftDelete(outcome)) => match outcome {
            MailboxDraftDeleteOutcomeDto::Deleted | MailboxDraftDeleteOutcomeDto::NotFound => {
                Ok(())
            }
            MailboxDraftDeleteOutcomeDto::Conflict(draft)
                if draft.version > 0 && draft.content.len() <= CONTENT_MAX_BYTES =>
            {
                Ok(())
            }
            MailboxDraftDeleteOutcomeDto::Conflict(_) => Err(ValueError::InvalidCanonicalPlan),
        },
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
        Response::Success(ResponseResult::RelayStatus(status)) => {
            if status.policies.len() > MAX_RELAY_STATUS_POLICIES
                || status
                    .policies
                    .windows(2)
                    .any(|pair| pair[0].endpoint >= pair[1].endpoint)
            {
                return Err(ValueError::InvalidValueCombination);
            }
            for policy in &status.policies {
                validate_locator(&policy.endpoint)?;
                if policy.generation == 0 {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            Ok(())
        }
        Response::Success(ResponseResult::StateHealth(status)) => {
            validate_state_health(status.revision, &status.domains)
        }
        Response::Success(ResponseResult::StateRepair(report)) => {
            validate_state_health(report.revision, &report.domains)
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
            validate_authoritative_conversation_view(&acknowledgement.view)
        }
        Response::Success(ResponseResult::AuthoritativeConversationView(view)) => {
            validate_authoritative_conversation_view(view)
        }
        Response::Success(ResponseResult::ProjectCommand(outcome)) => {
            validate_project_outcome(outcome)
        }
        Response::Success(ResponseResult::AgentRetirement(outcome)) => {
            validate_agent_retirement_outcome(outcome)
        }
        Response::Success(ResponseResult::PendingInteractions(interactions)) => {
            if interactions.len() > hq_application::MAX_PENDING_INTERACTIONS {
                return Err(ValueError::TooManyItems);
            }
            for interaction in interactions {
                validate_text(&interaction.provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(&interaction.session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                validate_text(&interaction.prompt, CONTENT_MAX_BYTES)?;
                if interaction.choices.len() > hq_application::MAX_INTERACTION_CHOICES {
                    return Err(ValueError::TooManyItems);
                }
                for choice in &interaction.choices {
                    validate_text(&choice.value, SHORT_TEXT_MAX_BYTES)?;
                    validate_text(&choice.label, SHORT_TEXT_MAX_BYTES)?;
                }
            }
            Ok(())
        }
        Response::Success(
            ResponseResult::InteractionAnswer(_)
            | ResponseResult::InteractionResponder(_)
            | ResponseResult::Empty,
        ) => Ok(()),
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

fn validate_pending_interactions(request: PendingInteractionsRequestDto) -> Result<(), ValueError> {
    if request.limit == 0 || usize::from(request.limit) > hq_application::MAX_PENDING_INTERACTIONS {
        Err(ValueError::TooManyItems)
    } else {
        Ok(())
    }
}

fn validate_interaction_response(response: &InteractionResponseDto) -> Result<(), ValueError> {
    match response {
        InteractionResponseDto::Text(value) => validate_text(value, CONTENT_MAX_BYTES),
        InteractionResponseDto::Choice(value) => validate_text(value, SHORT_TEXT_MAX_BYTES),
        InteractionResponseDto::Approval(_) | InteractionResponseDto::Cancelled => Ok(()),
    }
}

fn validate_state_health(revision: u64, domains: &[DomainHealthDto]) -> Result<(), ValueError> {
    if revision == 0
        || domains.len() != 4
        || !matches!(domains[0].domain, HealthDomainDto::Authority)
        || !matches!(domains[1].domain, HealthDomainDto::Conversation)
        || !matches!(domains[2].domain, HealthDomainDto::Agent)
        || !matches!(domains[3].domain, HealthDomainDto::Project)
    {
        return Err(ValueError::InvalidValueCombination);
    }
    Ok(())
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
    let creation = matches!(
        &request.action,
        ProjectCommandActionDto::Create(_) | ProjectCommandActionDto::ProvisionWorktree(_)
    );
    if creation == request.expected_head.is_some() {
        return Err(ValueError::InvalidValueCombination);
    }
    match &request.action {
        ProjectCommandActionDto::Open
        | ProjectCommandActionDto::DispatchPending
        | ProjectCommandActionDto::Close { .. }
        | ProjectCommandActionDto::SetArchived { .. }
        | ProjectCommandActionDto::RetireAgent { .. }
        | ProjectCommandActionDto::RemoveResource { .. }
        | ProjectCommandActionDto::SetPrimaryResource { .. } => Ok(()),
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
        | ProjectCommandActionDto::ReplaceResource { resource, .. } => validate_locator(resource),
        ProjectCommandActionDto::Create(request) => {
            validate_text(&request.project_name, SHORT_TEXT_MAX_BYTES)?;
            if let Some(brief) = &request.brief {
                validate_text(brief, CONTENT_MAX_BYTES)?;
            }
            validate_locator(&request.resource)
        }
        ProjectCommandActionDto::ProvisionWorktree(request) => {
            validate_text(&request.project_name, SHORT_TEXT_MAX_BYTES)?;
            if let Some(brief) = &request.brief {
                validate_text(brief, CONTENT_MAX_BYTES)?;
            }
            validate_locator(&request.source)?;
            validate_locator(&request.destination)?;
            validate_text(&request.branch, SHORT_TEXT_MAX_BYTES)?;
            if let Some(base) = &request.base {
                validate_text(base, SHORT_TEXT_MAX_BYTES)?;
            }
            if request.create_branch != request.base.is_some() {
                return Err(ValueError::InvalidValueCombination);
            }
            Ok(())
        }
    }
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
        ProjectCommandOutcomeDto::Rejected {
            error,
            runtime,
            external_state_warning,
            ..
        } => {
            validate_domain_error(error)?;
            runtime.as_ref().map_or(Ok(()), validate_runtime)?;
            external_state_warning
                .as_ref()
                .map_or(Ok(()), validate_project_external_state_warning)
        }
        ProjectCommandOutcomeDto::Reconcilable {
            error,
            external_state_warning,
            ..
        } => {
            validate_domain_error(error)?;
            external_state_warning
                .as_ref()
                .map_or(Ok(()), validate_project_external_state_warning)
        }
    }
}

fn validate_project_external_state_warning(
    warning: &ProjectExternalStateWarningDto,
) -> Result<(), ValueError> {
    match warning {
        ProjectExternalStateWarningDto::WorktreeMayExist {
            destination,
            branch,
        } => {
            validate_locator(destination)?;
            validate_text(branch, SHORT_TEXT_MAX_BYTES)
        }
    }
}

fn validate_agent_retirement_outcome(
    outcome: &AgentRetirementOutcomeDto,
) -> Result<(), ValueError> {
    match outcome {
        AgentRetirementOutcomeDto::Running { .. } => Ok(()),
        AgentRetirementOutcomeDto::Completed { runtime, .. } => {
            runtime.as_ref().map_or(Ok(()), validate_runtime)
        }
        AgentRetirementOutcomeDto::Rejected { error, runtime, .. } => {
            validate_domain_error(error)?;
            runtime.as_ref().map_or(Ok(()), validate_runtime)
        }
        AgentRetirementOutcomeDto::Reconcilable { error, .. } => validate_domain_error(error),
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
                state,
                frontier,
                routes,
                blocks,
                ..
            } => {
                if !matches!(state.as_str(), "routable" | "blocked" | "conflicted") {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_id_set(frontier, 64)?;
                if routes.len() > 64
                    || blocks.len() > 64
                    || routes
                        .windows(2)
                        .any(|pair| pair[0].fact_id >= pair[1].fact_id)
                    || blocks
                        .windows(2)
                        .any(|pair| pair[0].fact_id >= pair[1].fact_id)
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                for route in routes {
                    if route.frontier_member != frontier.contains(&route.fact_id)
                        || blocks.iter().any(|block| block.fact_id == route.fact_id)
                    {
                        return Err(ValueError::InvalidValueCombination);
                    }
                    if let Some(label) = &route.label {
                        validate_text(label, SHORT_TEXT_MAX_BYTES)?;
                    }
                    if route.relay_hints.len() > hq_domain::MAX_RELAY_HINTS
                        || route.relay_hints.windows(2).any(|pair| pair[0] >= pair[1])
                    {
                        return Err(ValueError::InvalidValueCombination);
                    }
                    for locator in &route.relay_hints {
                        validate_locator(locator)?;
                    }
                }
                for block in blocks {
                    if block.frontier_member != frontier.contains(&block.fact_id) {
                        return Err(ValueError::InvalidValueCombination);
                    }
                    validate_text(&block.reason, SHORT_TEXT_MAX_BYTES)?;
                }
                let frontier_routes = routes.iter().filter(|route| route.frontier_member).count();
                let frontier_blocks = blocks.iter().filter(|block| block.frontier_member).count();
                let expected_state = if frontier_blocks > 0 {
                    "blocked"
                } else if frontier_routes == 1 {
                    "routable"
                } else {
                    "conflicted"
                };
                if state != expected_state || frontier.len() != frontier_routes + frontier_blocks {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::MailboxCapability {
                grant_fact,
                active,
                revoke_frontier,
                observed_actions,
                support,
                ..
            } => {
                validate_id_set(revoke_frontier, 64)?;
                validate_id_set(observed_actions, 64)?;
                validate_id_set(support, 64)?;
                if *active != revoke_frontier.is_empty()
                    || !support.contains(grant_fact)
                    || revoke_frontier.iter().any(|fact| !support.contains(fact))
                {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::AccountSelection {
                candidates,
                frontier,
                ..
            } => {
                validate_id_set(candidates, 64)?;
                validate_id_set(frontier, 64)?;
            }
            SnapshotItem::Conversation {
                key,
                context,
                root_message,
                preview,
                ..
            } => validate_conversation_summary(key, context, *root_message, preview.as_deref())?,
            SnapshotItem::IncompleteMessage {
                recipient_installation,
                recipient_mailbox,
                content,
                missing_dependencies,
                unusable_dependencies,
                correlation_provider,
                correlation_session,
                correlation_operation,
                ..
            } => {
                if recipient_installation.is_some() != recipient_mailbox.is_some() {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_text(content, CONTENT_MAX_BYTES)?;
                validate_id_set(missing_dependencies, 64)?;
                validate_id_set(unusable_dependencies, 64)?;
                validate_optional_correlation(
                    correlation_provider.as_deref(),
                    correlation_session.as_deref(),
                    correlation_operation.as_ref(),
                )?;
            }
            SnapshotItem::IncompleteMessagesTruncated => {}
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
                claims,
                names,
                mailboxes,
                retirements,
                lifecycle,
                ..
            } => {
                validate_id_set(claims, 64)?;
                if names.len() > 64 {
                    return Err(ValueError::TooManyItems);
                }
                for name in names {
                    validate_text(name, SHORT_TEXT_MAX_BYTES)?;
                }
                if names.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(ValueError::InvalidValueCombination);
                }
                if mailboxes.len() > 64
                    || mailboxes.windows(2).any(|pair| {
                        (&pair[0].installation_id, &pair[0].mailbox_id)
                            >= (&pair[1].installation_id, &pair[1].mailbox_id)
                    })
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_id_set(retirements, 64)?;
                validate_text(lifecycle, SHORT_TEXT_MAX_BYTES)?;
            }
            SnapshotItem::AgentSession {
                provider,
                session,
                bindings,
                mailbox_installation,
                mailbox_id,
                conflicted,
            } => {
                validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                if bindings.is_empty()
                    || bindings.len() > 64
                    || bindings
                        .windows(2)
                        .any(|pair| pair[0].fact_id >= pair[1].fact_id)
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                if mailbox_installation.is_some() != mailbox_id.is_some() {
                    return Err(ValueError::InvalidValueCombination);
                }
                if *conflicted != mailbox_id.is_none() {
                    return Err(ValueError::InvalidValueCombination);
                }
                if let (Some(installation), Some(mailbox_id)) = (mailbox_installation, mailbox_id)
                    && bindings.iter().any(|binding| {
                        binding.mailbox.installation_id != *installation
                            || binding.mailbox.mailbox_id != *mailbox_id
                    })
                {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::AgentSelection {
                candidates,
                provider,
                session,
                frontier,
                ..
            } => {
                if candidates.len() > 64
                    || candidates
                        .windows(2)
                        .any(|pair| pair[0].fact_id >= pair[1].fact_id)
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                for candidate in candidates {
                    validate_text(&candidate.provider, PROVIDER_ID_MAX_BYTES)?;
                    validate_text(&candidate.session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                }
                validate_id_set(frontier, 64)?;
                if frontier.iter().any(|fact_id| {
                    !candidates
                        .iter()
                        .any(|candidate| candidate.fact_id == *fact_id)
                }) {
                    return Err(ValueError::InvalidValueCombination);
                }
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
                candidates,
                frontier,
                resolved,
                display_name,
                ..
            } => {
                validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                if candidates.len() > 64
                    || candidates
                        .windows(2)
                        .any(|pair| pair[0].fact_id >= pair[1].fact_id)
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                for candidate in candidates {
                    if let Some(display_name) = &candidate.display_name {
                        validate_text(display_name, SHORT_TEXT_MAX_BYTES)?;
                    }
                }
                validate_id_set(frontier, 64)?;
                if frontier.iter().any(|fact_id| {
                    !candidates
                        .iter()
                        .any(|candidate| candidate.fact_id == *fact_id)
                }) {
                    return Err(ValueError::InvalidValueCombination);
                }
                if let Some(display_name) = display_name {
                    validate_text(display_name, SHORT_TEXT_MAX_BYTES)?;
                }
                if !resolved && display_name.is_some() {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::AgentContext {
                history, frontier, ..
            } => {
                if history.len() > MAX_SNAPSHOT_ITEMS
                    || history
                        .windows(2)
                        .any(|pair| pair[0].fact_id >= pair[1].fact_id)
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                for context in history {
                    validate_locator(&context.directory)?;
                    if let Some(repository) = &context.repository {
                        validate_locator(repository)?;
                    }
                    if let Some(worktree) = &context.worktree {
                        validate_locator(worktree)?;
                    }
                    if let Some(branch) = &context.branch {
                        validate_text(branch, SHORT_TEXT_MAX_BYTES)?;
                    }
                }
                validate_id_set(frontier, 64)?;
                if frontier
                    .iter()
                    .any(|fact_id| !history.iter().any(|context| context.fact_id == *fact_id))
                {
                    return Err(ValueError::InvalidValueCombination);
                }
            }
            SnapshotItem::AgentDirectSession {
                provider, session, ..
            }
            | SnapshotItem::ProjectThread {
                provider, session, ..
            } => {
                validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
            }
            SnapshotItem::Project {
                name, lifecycle, ..
            } => {
                validate_text(name, SHORT_TEXT_MAX_BYTES)?;
                validate_text(lifecycle, SHORT_TEXT_MAX_BYTES)?;
            }
            SnapshotItem::ProjectAssignment {
                provider,
                session,
                phase,
                thread_id,
                launch_directory,
                blocked,
                runnable,
                support,
                ..
            } => {
                validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(phase, SHORT_TEXT_MAX_BYTES)?;
                if let Some(session) = session {
                    validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                }
                if let Some(directory) = launch_directory {
                    validate_locator(directory)?;
                }
                if let Some(blocked) = blocked {
                    validate_text(blocked, ERROR_CODE_MAX_BYTES)?;
                }
                validate_id_set(support, MAX_SNAPSHOT_ITEMS)?;
                let runnable_phase = phase == "runnable"
                    && session.is_some()
                    && thread_id.is_some()
                    && launch_directory.is_some()
                    && blocked.is_none();
                let configuring = phase == "configuring"
                    && !*runnable
                    && session.is_none()
                    && thread_id.is_none()
                    && launch_directory.is_none()
                    && blocked.is_none();
                let blocked_state = phase == "blocked"
                    && !*runnable
                    && thread_id.is_none()
                    && launch_directory.is_none()
                    && blocked.is_some();
                if !(runnable_phase || configuring || blocked_state) {
                    return Err(ValueError::InvalidValueCombination);
                }
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
                    result:
                        RemoteCommandResultDto::Rejected {
                            error: code,
                            external_state_warning,
                        },
                    runtime,
                    ..
                } = progress.as_ref()
                {
                    validate_text(code, ERROR_CODE_MAX_BYTES)?;
                    if let Some(warning) = external_state_warning {
                        validate_project_external_state_warning(warning)?;
                    }
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

fn validate_conversation_summary(
    key: &ConversationKeyDto,
    context: &ConversationContextDto,
    root_message: Option<Id32>,
    preview: Option<&str>,
) -> Result<(), ValueError> {
    validate_conversation_key(key)?;
    if let Some(preview) = preview {
        validate_text(preview, SHORT_TEXT_MAX_BYTES)?;
    }
    match (key, context) {
        (
            ConversationKeyDto::ProjectThread { project, .. },
            ConversationContextDto::Project {
                project: context_project,
                name,
                participant,
            },
        ) if project == context_project && root_message.is_some() => {
            if let Some(name) = name {
                validate_text(name, SHORT_TEXT_MAX_BYTES)?;
            }
            if let Some(participant) = participant {
                validate_conversation_participant(participant, false)?;
            }
            Ok(())
        }
        (
            ConversationKeyDto::Thread { .. } | ConversationKeyDto::ProviderSession { .. },
            ConversationContextDto::Direct { participant },
        ) if root_message.is_none() => validate_conversation_participant(participant, true),
        (ConversationKeyDto::Thread { .. }, ConversationContextDto::Personal)
            if root_message.is_none() =>
        {
            Ok(())
        }
        _ => Err(ValueError::InvalidValueCombination),
    }
}

fn validate_conversation_participant(
    participant: &ConversationParticipantDto,
    mailbox_required: bool,
) -> Result<(), ValueError> {
    if participant.installation.is_some() != participant.mailbox.is_some()
        || (mailbox_required && participant.mailbox.is_none())
        || (participant.agent.is_none() && participant.mailbox.is_none())
        || (participant.name.is_some() && participant.agent.is_none())
    {
        return Err(ValueError::InvalidValueCombination);
    }
    if let Some(name) = &participant.name {
        validate_text(name, SHORT_TEXT_MAX_BYTES)?;
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
            ConversationEntryDto::Message(message) => {
                validate_text(&message.content, CONTENT_MAX_BYTES)?;
                if message.recipient_installation.is_some() != message.recipient_mailbox.is_some()
                    || message.root_fact.is_some() != message.root_message.is_some()
                    || (message.ready_answer && message.root_fact.is_none())
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                validate_id_set(&message.state_frontier, 64)?;
                validate_id_set(&message.peer_received_by, 64)?;
                validate_optional_correlation(
                    message.correlation_provider.as_deref(),
                    message.correlation_session.as_deref(),
                    message.correlation_operation.as_ref(),
                )?;
            }
            ConversationEntryDto::Activity(activity) => {
                validate_text(&activity.content, CONTENT_MAX_BYTES)?;
                validate_text(&activity.provider, PROVIDER_ID_MAX_BYTES)?;
                validate_text(&activity.session, PROVIDER_SESSION_ID_MAX_BYTES)?;
                if let Some(item) = &activity.item {
                    validate_text(item, SHORT_TEXT_MAX_BYTES)?;
                }
                validate_text(&activity.logical_key, SHORT_TEXT_MAX_BYTES)?;
                validate_text(&activity.runtime, SHORT_TEXT_MAX_BYTES)?;
                if let ActivityStatusDto::Failed { reason } = &activity.status {
                    validate_text(reason, ERROR_CODE_MAX_BYTES)?;
                }
                if (activity.activity_kind == ConversationActivityKindDto::CompletedItem)
                    != activity.completed.is_some()
                {
                    return Err(ValueError::InvalidValueCombination);
                }
                if let Some(completed) = &activity.completed {
                    validate_completed_item(completed)?;
                }
            }
        }
        if matches!(item, ConversationEntryDto::Activity(activity) if activity.sequence == 0) {
            return Err(ValueError::InvalidSequence);
        }
    }
    Ok(())
}

fn validate_authoritative_conversation_view(
    view: &AuthoritativeConversationViewDto,
) -> Result<(), ValueError> {
    validate_snapshot(&view.snapshot)?;
    if let Some(conversation) = &view.conversation {
        validate_conversation_key(&conversation.key)?;
        validate_page(&conversation.page)?;
        if conversation.page.items.len() > usize::from(MAX_MATERIALIZED_CONVERSATION_PAGE_ITEMS) {
            return Err(ValueError::TooManyItems);
        }
    }
    Ok(())
}

fn validate_completed_item(value: &CompletedItemPresentationDto) -> Result<(), ValueError> {
    match value {
        CompletedItemPresentationDto::Command {
            command, output, ..
        } => {
            validate_text(command, CONTENT_MAX_BYTES)?;
            if let Some(output) = output {
                validate_text(output, CONTENT_MAX_BYTES)?;
            }
        }
        CompletedItemPresentationDto::FileChange { changes, .. } => {
            if changes.len() > hq_domain::MAX_COMPLETED_FILE_CHANGES {
                return Err(ValueError::TooManyItems);
            }
            for change in changes {
                validate_text(&change.path, CONTENT_MAX_BYTES)?;
                if let Some(diff) = &change.diff {
                    validate_text(diff, CONTENT_MAX_BYTES)?;
                }
            }
        }
        CompletedItemPresentationDto::Tool { name, .. } => {
            validate_text(name, SHORT_TEXT_MAX_BYTES)?;
        }
        CompletedItemPresentationDto::WebSearch { query, .. } => {
            validate_text(query, CONTENT_MAX_BYTES)?;
        }
        CompletedItemPresentationDto::Unknown => {}
    }
    Ok(())
}

fn validate_optional_correlation(
    provider: Option<&str>,
    session: Option<&str>,
    operation: Option<&Id32>,
) -> Result<(), ValueError> {
    if provider.is_some() != session.is_some() || provider.is_some() != operation.is_some() {
        return Err(ValueError::InvalidValueCombination);
    }
    if let Some(provider) = provider {
        validate_text(provider, PROVIDER_ID_MAX_BYTES)?;
    }
    if let Some(session) = session {
        validate_text(session, PROVIDER_SESSION_ID_MAX_BYTES)?;
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
