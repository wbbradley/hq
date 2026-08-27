//! Verified semantic fact envelope and catalog payloads.

use std::{error::Error, fmt, num::NonZeroU64};

use crate::{
    AccountId, AgentId, AssignmentId, BoundedText, BoundedVec, CausalReferences, CommandDigest,
    CommandId, DispatchId, EncryptionPublicKey, ErrorCode, FactId, FactKind, GrantId,
    InstallationAddress, InstallationId, MailboxAddress, MailboxId, MessageId,
    OperationCorrelation, ProjectId, ProtocolClass, ProviderId, ProviderSessionId, ResourceId,
    ResourceLocator, SigningPublicKey, ThreadId, Timestamp,
};

/// Maximum parents retained by one semantic fact.
pub const MAX_FACT_PARENTS: usize = 64;
/// Maximum typed authority roles retained by one semantic fact.
pub const MAX_FACT_AUTHORITIES: usize = 8;
/// Maximum short display value length in UTF-8 bytes.
pub const SHORT_TEXT_MAX_BYTES: usize = 128;
/// Maximum message or diagnostic content length in UTF-8 bytes.
pub const CONTENT_MAX_BYTES: usize = 16_384;
/// Maximum relay hints carried by an identity or route fact.
pub const MAX_RELAY_HINTS: usize = 8;

/// Bounded short display text.
pub type ShortText = BoundedText<SHORT_TEXT_MAX_BYTES>;
/// Bounded message, brief, or diagnostic text.
pub type ContentText = BoundedText<CONTENT_MAX_BYTES>;
/// Bounded relay-routing metadata.
pub type RelayHints = BoundedVec<ResourceLocator, MAX_RELAY_HINTS>;

/// Signed semantic audience and routing scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FactScope {
    /// Private to one installation.
    InstallationPrivate(InstallationId),
    /// Addressed through one mailbox capability.
    PeerAddressed(MailboxAddress),
    /// Addressed to every active member of one human account.
    AccountAddressed(AccountId),
    /// Signed remote-control record for a project home.
    RemoteControl {
        /// Human account carrying the request or result.
        account_id: AccountId,
        /// Installation authoritative for the project.
        target_home: InstallationId,
    },
}

/// Mailbox role fixed at mailbox creation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailboxKind {
    /// Reserved human-control mailbox.
    Human,
    /// Managed or direct agent mailbox.
    Agent,
}

/// Typed repository context used for display, search, and launch selection only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryContext {
    /// Canonical working directory locator.
    pub directory: ResourceLocator,
    /// Optional repository identity.
    pub repository: Option<ResourceLocator>,
    /// Optional worktree identity.
    pub worktree: Option<ResourceLocator>,
    /// Optional display branch.
    pub branch: Option<ShortText>,
}

/// Typed message purpose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessagePurpose {
    /// A question expects an answer.
    Question,
    /// An asynchronous message does not imply a blocking wait.
    Asynchronous,
    /// Output produced for a project input.
    ProjectOutput,
}

/// Typed presentation semantics carried without parsing message text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationKind {
    /// Ordinary user or agent prose.
    Message,
    /// Final answer from a managed operation.
    FinalAnswer,
    /// Concise status or progress notice.
    Status,
}

/// Shared intrinsic message content for root, answer, and output facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageContent {
    /// Stable public message identity.
    pub message_id: MessageId,
    /// Sender mailbox.
    pub sender: MailboxAddress,
    /// Optional direct recipient; account audiences are expressed by scope.
    pub recipient: Option<MailboxAddress>,
    /// Bounded semantic body.
    pub body: ContentText,
    /// Explicit message purpose.
    pub purpose: MessagePurpose,
    /// Typed presentation behavior.
    pub presentation: PresentationKind,
    /// Optional managed-operation correlation.
    pub correlation: Option<OperationCorrelation>,
    /// Optional project scope.
    pub project_id: Option<ProjectId>,
}

/// Activity class that remains non-actionable conversation content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityKind {
    /// Operation status snapshot.
    Status,
    /// Incremental progress record.
    Progress,
    /// Plan or task state.
    Plan,
    /// Completed command, file, or tool record.
    CompletedItem,
}

/// Terminal/runtime observation without claiming external truth.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeObservation {
    /// External success was explicitly observed.
    Succeeded,
    /// External failure was explicitly observed with a stable reason.
    Failed(ErrorCode),
    /// Whether the external action happened is unknown.
    Uncertain(ErrorCode),
}

/// Desired resource and its latest typed observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResource {
    /// Stable resource identity.
    pub resource_id: ResourceId,
    /// Home-qualified canonical locator.
    pub locator: ResourceLocator,
    /// Latest health observation.
    pub health: ResourceHealth,
}

/// Resource health classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceHealth {
    /// Adapter has not yet checked the resource.
    Unknown,
    /// Resource is available for intended operations.
    Healthy,
    /// Resource exists but needs attention.
    Degraded,
    /// Resource is unavailable.
    Unavailable,
}

/// Immutable identity of a project assignment epoch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentBinding {
    /// Assignment epoch identity.
    pub assignment_id: AssignmentId,
    /// Assigned durable agent.
    pub agent_id: AgentId,
    /// Selected provider namespace.
    pub provider: ProviderId,
    /// Selected provider session.
    pub session: ProviderSessionId,
}

/// Lifecycle starting state for a new project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialProjectState {
    /// Project begins open and claims its desired resources.
    Open,
    /// Project begins closed without active claims.
    Closed,
}

/// Definite remote command result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteCommandResult {
    /// Canonical project facts committed at the returned head.
    Committed(FactId),
    /// Home rejected the request with a stable domain code.
    Rejected(ErrorCode),
}

/// Exhaustive typed semantic payload catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
// Variant fields mirror the normative catalog columns and retain descriptive semantic names; the
// per-family FCT documentation is maintained once in the catalog rather than repeated on 152 fields.
#[allow(missing_docs)]
pub enum SemanticPayload {
    /// FCT-001.
    InstallationDeclared {
        installation_id: InstallationId,
        signing_key: SigningPublicKey,
        encryption_key: EncryptionPublicKey,
        label: Option<ShortText>,
    },
    /// FCT-002.
    MailboxCreated {
        mailbox_id: MailboxId,
        kind: MailboxKind,
        label: Option<ShortText>,
    },
    /// FCT-003.
    MailboxSessionBound {
        mailbox_id: MailboxId,
        provider: ProviderId,
        session: ProviderSessionId,
    },
    /// FCT-004.
    MailboxContextRecorded {
        mailbox_id: MailboxId,
        context: RepositoryContext,
    },
    /// FCT-005.
    PeerRouteSet {
        peer: InstallationAddress,
        encryption_key: EncryptionPublicKey,
        label: Option<ShortText>,
        relay_hints: RelayHints,
    },
    /// FCT-006.
    PeerRouteBlocked {
        peer_id: InstallationId,
        reason: ErrorCode,
    },
    /// FCT-007.
    MailboxAccessGranted {
        grant_id: GrantId,
        mailbox: MailboxAddress,
        grantee: InstallationAddress,
    },
    /// FCT-008.
    MailboxAccessRevoked {
        grant_id: GrantId,
        mailbox: MailboxAddress,
        grantee_id: InstallationId,
    },
    /// FCT-009.
    MailboxActionObserved {
        grant_id: GrantId,
        action_id: FactId,
    },
    /// FCT-010.
    HumanAccountCreated {
        account_id: AccountId,
        creator: InstallationAddress,
        label: Option<ShortText>,
    },
    /// FCT-011.
    HumanAccountSelected { account_id: AccountId },
    /// FCT-012.
    HumanDeviceGranted {
        account_id: AccountId,
        grant_id: GrantId,
        device: InstallationAddress,
        label: Option<ShortText>,
        relay_hints: RelayHints,
    },
    /// FCT-013.
    HumanDeviceAccepted {
        account_id: AccountId,
        grant_id: GrantId,
        device: InstallationAddress,
    },
    /// FCT-014.
    HumanDeviceRevoked {
        account_id: AccountId,
        grant_id: GrantId,
        device_id: InstallationId,
    },
    /// FCT-015.
    QuestionAsked(MessageContent),
    /// FCT-016.
    AsynchronousMessageSent(MessageContent),
    /// FCT-017.
    AnswerGiven {
        thread_id: ThreadId,
        message: MessageContent,
    },
    /// FCT-018.
    ThreadCancelled {
        thread_id: ThreadId,
        reason: Option<ContentText>,
    },
    /// FCT-019.
    MessageArchived { message_id: MessageId },
    /// FCT-020.
    MessageRestored { message_id: MessageId },
    /// FCT-021.
    MessageRejected {
        message_id: MessageId,
        reason: ErrorCode,
    },
    /// FCT-022.
    HarnessActivityRecorded {
        source: MailboxAddress,
        correlation: OperationCorrelation,
        kind: ActivityKind,
        logical_key: ShortText,
        sequence: NonZeroU64,
        content: ContentText,
        truncated: bool,
    },
    /// FCT-023.
    AgentNameClaimed {
        agent_id: AgentId,
        mailbox_id: MailboxId,
        name: ShortText,
    },
    /// FCT-024.
    AgentRetired {
        agent_id: AgentId,
        mailbox_id: MailboxId,
    },
    /// FCT-025.
    ProviderSessionSelected {
        agent_id: AgentId,
        mailbox_id: MailboxId,
        provider: ProviderId,
        session: ProviderSessionId,
        context: RepositoryContext,
    },
    /// FCT-026.
    ProviderSessionRenamed {
        agent_id: AgentId,
        provider: ProviderId,
        session: ProviderSessionId,
        display_name: Option<ShortText>,
    },
    /// FCT-027.
    ProjectCreated {
        project_id: ProjectId,
        mailbox_id: MailboxId,
        home: InstallationId,
        name: ShortText,
        brief: Option<ContentText>,
        predecessor: Option<ProjectId>,
        resources: BoundedVec<ProjectResource, 64>,
        primary: Option<ResourceId>,
        initial_state: InitialProjectState,
    },
    /// FCT-028.
    ProjectOpened { project_id: ProjectId },
    /// FCT-029.
    ProjectClosingStarted { project_id: ProjectId },
    /// FCT-030.
    ProjectClosed {
        project_id: ProjectId,
        forced: bool,
        runtime: Option<RuntimeObservation>,
    },
    /// FCT-031.
    ProjectArchived { project_id: ProjectId },
    /// FCT-032.
    ProjectUnarchived { project_id: ProjectId },
    /// FCT-033.
    ProjectMetadataUpdated {
        project_id: ProjectId,
        name: ShortText,
        brief: Option<ContentText>,
    },
    /// FCT-034.
    ProjectResourceAdded {
        project_id: ProjectId,
        resource: ProjectResource,
        make_primary: bool,
    },
    /// FCT-035.
    ProjectResourceRemoved {
        project_id: ProjectId,
        resource_id: ResourceId,
        force: bool,
    },
    /// FCT-036.
    ProjectResourceReplaced {
        project_id: ProjectId,
        old_resource_id: ResourceId,
        new_resource: ProjectResource,
    },
    /// FCT-037.
    ProjectPrimaryResourceChanged {
        project_id: ProjectId,
        resource_id: ResourceId,
    },
    /// FCT-038.
    ProjectResourceHealthObserved {
        project_id: ProjectId,
        resource_id: ResourceId,
        health: ResourceHealth,
        details: Option<ContentText>,
        checked_at: Timestamp,
    },
    /// FCT-039.
    ProjectAssignmentConfiguring {
        project_id: ProjectId,
        binding: AssignmentBinding,
    },
    /// FCT-040.
    ProjectAssignmentRunnable {
        project_id: ProjectId,
        binding: AssignmentBinding,
        thread_id: ThreadId,
        launch_directory: ResourceLocator,
        activation: OperationCorrelation,
    },
    /// FCT-041.
    ProjectAssignmentBlocked {
        project_id: ProjectId,
        assignment_id: AssignmentId,
        cause: ErrorCode,
    },
    /// FCT-042.
    ProjectAssignmentEnded {
        project_id: ProjectId,
        assignment_id: AssignmentId,
        forced: bool,
        runtime: Option<RuntimeObservation>,
    },
    /// FCT-043.
    ProjectInputAccepted {
        project_id: ProjectId,
        message_id: MessageId,
        input_fact_id: FactId,
        sequence: NonZeroU64,
    },
    /// FCT-044.
    ProjectInputDispatched {
        project_id: ProjectId,
        message_id: MessageId,
        sequence: NonZeroU64,
        dispatch_id: DispatchId,
        binding: AssignmentBinding,
        thread_id: ThreadId,
    },
    /// FCT-045.
    ProjectOutputRecorded {
        project_id: ProjectId,
        output_id: MessageId,
        dispatch_id: DispatchId,
        binding: AssignmentBinding,
        thread_id: ThreadId,
        message: MessageContent,
    },
    /// FCT-046.
    RemoteProjectCommandRequested {
        command_id: CommandId,
        digest: CommandDigest,
        project_id: ProjectId,
        target_home: InstallationId,
        expected_head: FactId,
        operation: OperationCorrelation,
        body: ContentText,
    },
    /// FCT-047.
    RemoteProjectCommandReceipt {
        command_id: CommandId,
        digest: CommandDigest,
        project_id: ProjectId,
        received_head: FactId,
        received_at: Timestamp,
    },
    /// FCT-048.
    RemoteProjectCommandOutcome {
        command_id: CommandId,
        digest: CommandDigest,
        project_id: ProjectId,
        result: RemoteCommandResult,
        runtime: Option<RuntimeObservation>,
    },
}

impl SemanticPayload {
    /// Returns the exact catalog family represented by this payload.
    pub const fn kind(&self) -> FactKind {
        match self {
            Self::InstallationDeclared { .. } => FactKind::InstallationDeclared,
            Self::MailboxCreated { .. } => FactKind::MailboxCreated,
            Self::MailboxSessionBound { .. } => FactKind::MailboxSessionBound,
            Self::MailboxContextRecorded { .. } => FactKind::MailboxContextRecorded,
            Self::PeerRouteSet { .. } => FactKind::PeerRouteSet,
            Self::PeerRouteBlocked { .. } => FactKind::PeerRouteBlocked,
            Self::MailboxAccessGranted { .. } => FactKind::MailboxAccessGranted,
            Self::MailboxAccessRevoked { .. } => FactKind::MailboxAccessRevoked,
            Self::MailboxActionObserved { .. } => FactKind::MailboxActionObserved,
            Self::HumanAccountCreated { .. } => FactKind::HumanAccountCreated,
            Self::HumanAccountSelected { .. } => FactKind::HumanAccountSelected,
            Self::HumanDeviceGranted { .. } => FactKind::HumanDeviceGranted,
            Self::HumanDeviceAccepted { .. } => FactKind::HumanDeviceAccepted,
            Self::HumanDeviceRevoked { .. } => FactKind::HumanDeviceRevoked,
            Self::QuestionAsked(_) => FactKind::QuestionAsked,
            Self::AsynchronousMessageSent(_) => FactKind::AsynchronousMessageSent,
            Self::AnswerGiven { .. } => FactKind::AnswerGiven,
            Self::ThreadCancelled { .. } => FactKind::ThreadCancelled,
            Self::MessageArchived { .. } => FactKind::MessageArchived,
            Self::MessageRestored { .. } => FactKind::MessageRestored,
            Self::MessageRejected { .. } => FactKind::MessageRejected,
            Self::HarnessActivityRecorded { .. } => FactKind::HarnessActivityRecorded,
            Self::AgentNameClaimed { .. } => FactKind::AgentNameClaimed,
            Self::AgentRetired { .. } => FactKind::AgentRetired,
            Self::ProviderSessionSelected { .. } => FactKind::ProviderSessionSelected,
            Self::ProviderSessionRenamed { .. } => FactKind::ProviderSessionRenamed,
            Self::ProjectCreated { .. } => FactKind::ProjectCreated,
            Self::ProjectOpened { .. } => FactKind::ProjectOpened,
            Self::ProjectClosingStarted { .. } => FactKind::ProjectClosingStarted,
            Self::ProjectClosed { .. } => FactKind::ProjectClosed,
            Self::ProjectArchived { .. } => FactKind::ProjectArchived,
            Self::ProjectUnarchived { .. } => FactKind::ProjectUnarchived,
            Self::ProjectMetadataUpdated { .. } => FactKind::ProjectMetadataUpdated,
            Self::ProjectResourceAdded { .. } => FactKind::ProjectResourceAdded,
            Self::ProjectResourceRemoved { .. } => FactKind::ProjectResourceRemoved,
            Self::ProjectResourceReplaced { .. } => FactKind::ProjectResourceReplaced,
            Self::ProjectPrimaryResourceChanged { .. } => FactKind::ProjectPrimaryResourceChanged,
            Self::ProjectResourceHealthObserved { .. } => FactKind::ProjectResourceHealthObserved,
            Self::ProjectAssignmentConfiguring { .. } => FactKind::ProjectAssignmentConfiguring,
            Self::ProjectAssignmentRunnable { .. } => FactKind::ProjectAssignmentRunnable,
            Self::ProjectAssignmentBlocked { .. } => FactKind::ProjectAssignmentBlocked,
            Self::ProjectAssignmentEnded { .. } => FactKind::ProjectAssignmentEnded,
            Self::ProjectInputAccepted { .. } => FactKind::ProjectInputAccepted,
            Self::ProjectInputDispatched { .. } => FactKind::ProjectInputDispatched,
            Self::ProjectOutputRecorded { .. } => FactKind::ProjectOutputRecorded,
            Self::RemoteProjectCommandRequested { .. } => FactKind::RemoteProjectCommandRequested,
            Self::RemoteProjectCommandReceipt { .. } => FactKind::RemoteProjectCommandReceipt,
            Self::RemoteProjectCommandOutcome { .. } => FactKind::RemoteProjectCommandOutcome,
        }
    }
}

/// Verified semantic fact supplied to causal reduction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticFact {
    id: FactId,
    author: InstallationAddress,
    authored_at: Timestamp,
    scope: FactScope,
    causal: CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>,
    payload: SemanticPayload,
}

/// Intrinsic mismatch between verified envelope and payload class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticFactError {
    /// Remote-control records and canonical scopes were mixed.
    ProtocolScopeMismatch,
    /// Payload purpose or embedded subject fields contradict the fact family.
    PayloadInvariant,
    /// A root declaration did not match its verified author and installation scope.
    AuthorSubjectMismatch,
}

impl fmt::Display for SemanticFactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProtocolScopeMismatch => {
                formatter.write_str("fact protocol class does not match its scope")
            }
            Self::PayloadInvariant => {
                formatter.write_str("fact payload violates its family invariant")
            }
            Self::AuthorSubjectMismatch => {
                formatter.write_str("fact payload subject does not match its author or scope")
            }
        }
    }
}

impl Error for SemanticFactError {}

impl SemanticFact {
    /// Constructs a verified fact while enforcing canonical/control-plane isolation.
    pub fn new(
        id: FactId,
        author: InstallationAddress,
        authored_at: Timestamp,
        scope: FactScope,
        causal: CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>,
        payload: SemanticPayload,
    ) -> Result<Self, SemanticFactError> {
        let remote_scope = matches!(scope, FactScope::RemoteControl { .. });
        let remote_payload = payload.kind().protocol_class() == ProtocolClass::RemoteControl;
        if remote_scope != remote_payload {
            return Err(SemanticFactError::ProtocolScopeMismatch);
        }
        match &payload {
            SemanticPayload::InstallationDeclared {
                installation_id,
                signing_key,
                ..
            } => {
                let matching_scope = matches!(
                    scope,
                    FactScope::InstallationPrivate(scope_id) if scope_id == *installation_id
                );
                if author.installation_id() != *installation_id
                    || author.signing_key() != *signing_key
                    || !matching_scope
                {
                    return Err(SemanticFactError::AuthorSubjectMismatch);
                }
            }
            SemanticPayload::QuestionAsked(message)
                if message.purpose != MessagePurpose::Question =>
            {
                return Err(SemanticFactError::PayloadInvariant);
            }
            SemanticPayload::AsynchronousMessageSent(message)
                if message.purpose != MessagePurpose::Asynchronous =>
            {
                return Err(SemanticFactError::PayloadInvariant);
            }
            SemanticPayload::ProjectOutputRecorded {
                project_id,
                message,
                ..
            } if message.purpose != MessagePurpose::ProjectOutput
                || message.project_id != Some(*project_id) =>
            {
                return Err(SemanticFactError::PayloadInvariant);
            }
            SemanticPayload::ProjectCreated {
                resources,
                primary: Some(primary),
                ..
            } if !resources
                .as_slice()
                .iter()
                .any(|resource| resource.resource_id == *primary) =>
            {
                return Err(SemanticFactError::PayloadInvariant);
            }
            SemanticPayload::RemoteProjectCommandRequested { target_home, .. } if !matches!(scope, FactScope::RemoteControl { target_home: scope_home, .. } if scope_home == *target_home) =>
            {
                return Err(SemanticFactError::AuthorSubjectMismatch);
            }
            _ => {}
        }
        Ok(Self {
            id,
            author,
            authored_at,
            scope,
            causal,
            payload,
        })
    }

    /// Returns the content-derived fact identity.
    pub const fn id(&self) -> FactId {
        self.id
    }
    /// Returns the verified author address.
    pub const fn author(&self) -> InstallationAddress {
        self.author
    }
    /// Returns the author-supplied semantic time.
    pub const fn authored_at(&self) -> Timestamp {
        self.authored_at
    }
    /// Returns the signed scope.
    pub const fn scope(&self) -> &FactScope {
        &self.scope
    }
    /// Returns required parents and typed authorities.
    pub const fn causal(&self) -> &CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES> {
        &self.causal
    }
    /// Returns the typed payload.
    pub const fn payload(&self) -> &SemanticPayload {
        &self.payload
    }
    /// Returns the exact catalog kind.
    pub const fn kind(&self) -> FactKind {
        self.payload.kind()
    }
}

/// Compatibility name used by the workspace skeleton while callers migrate.
pub type Fact = SemanticFact;
