//! Minimal single-binary lifecycle roles for the Rust node.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    ffi::OsString,
    fmt::{self, Write as _},
    io::Read,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hq_application::{
    AgentNameClaimRequest, AgentRetirementRequest, AgentSessionRenameRequest,
    AgentSessionSelectionRequest, ApplicationError, HumanDeviceGrantRequest,
    HumanDeviceRevokeRequest, LocalFactInputs, LocalInstallationAuthority, MailboxGrantRequest,
    MailboxRevokeRequest, MessageAuthoringAuthority, MessageStateRequest, NewMessageRequest,
    PeerRouteRequest, ProjectCommandAction, ProjectCommandRequest, ProjectCreationRequest,
    ThreadCancellationRequest, WorktreeProvisioningRequest, plan_agent_mailbox_creation,
    plan_agent_name_claim, plan_agent_session_rename, plan_agent_session_selection,
    plan_asynchronous_message, plan_human_account_creation, plan_human_account_selection,
    plan_human_device_acceptance, plan_human_device_grant, plan_human_device_revoke,
    plan_human_mailbox_creation, plan_mailbox_grant, plan_mailbox_revoke, plan_message_archive,
    plan_peer_route_block, plan_peer_route_set, plan_question, plan_thread_cancellation,
};
use hq_domain::{
    AccountId, AgentId, AuthorityReference, AuthorityRole, BoundedText, CommandId, ContentText,
    EncryptionPublicKey, ErrorCode, FactId, FactScope, GrantId, InstallationAddress,
    InstallationId, MailboxAddress, MailboxId, MessageId, MessagePurpose, OperationCorrelation,
    OperationId, PresentationKind, ProjectId, ProjectResource, ProviderId, ProviderSessionId,
    RESOURCE_LOCATOR_MAX_BYTES, RelayHints, ResourceHealth, ResourceLocator, ResourceScheme,
    ShortText, SigningPublicKey, ThreadId, Timestamp,
};
use hq_local_api::{
    ClientEvent, InitialView, project_command_request_to_v1,
    protocol::v1::{
        AgentLaunchContextDto, AgentRetirementOutcomeDto, AgentRetirementRequestDto,
        AgentSessionRequestDto, AgentSessionResultDto, AuthoritativeSnapshotDto, BuildMetadata,
        CanonicalEvidenceDto, CanonicalEvidenceRequestDto, ConversationEntryDto,
        ConversationMessageDto, ConversationPageRequest, DeviceGrantDto, EffectOutcomeDto,
        EffectRequestDto, HealthDomainDto, Id32, LaunchEnvironmentDto, LifecycleRequest,
        LifecycleState, MailboxCommandActionDto, MailboxCommandRequestDto, MessagePurposeDto,
        MutationAttemptDto, MutationOutcomeDto, MutationRequest, PeerRouteBlockDto,
        PeerRouteCandidateDto, PresentationKindDto, ProjectCommandOutcomeDto,
        ProjectCommandRequestDto, ProjectCommandStageDto, ProjectExternalStateWarningDto,
        RelayAccessDto, RelayAuthenticationDto, RelayConfigurationDto, RelayStatusDto, Request,
        ResourceHealthDto, ResourceInspectionRequestDto, ResourceInspectionResultDto,
        ResourceLocatorDto, ResourceReleaseStateDto, ResourceSchemeDto, ResponseResult,
        RuntimeObservationDto, SessionControlDto, SnapshotItem, StateHealthDto,
        SynchronizationRequestDto, agent_session_request_digest,
        resource_inspection_request_digest,
    },
};
use hq_projects::{
    ProjectResourceRelationship, agent_retirement_request_digest, desired_resource_conflict,
    project_command_request_digest,
};
use hq_protocol::VerifiedPairingInvitation;
use hq_reducer::{
    AuthorityPolicy, AuthorityProjectionKey, AuthorityReducer, DecisionStatus, reduce_complete,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::local_client::installed_local_client_config;
use crate::pairing_file::{read_pairing_file, write_new_pairing_file};
use crate::{
    BackupPassword, ForegroundNodeConfig, ForegroundNodeError, IdentityError, LifecycleClient,
    LifecycleClientConfig, LifecycleClientError, LifecycleObservation, LocalConfiguration,
    LocalNodeClient, LocalNodeClientError, NodeClientCoordinator, NodeCoordinatorConfig,
    NodeCoordinatorError, ProcessNodeLauncher, PublicIdentity, RelayEndpoint, RuntimePathError,
    RuntimePaths, StateDirectoryOwner, StatePaths, ThemeSelection, TuiThemeCatalogEntry,
    TuiThemeEnvironment, agent_guidance::AgentGuidanceTopic, list_tui_themes, resolve_tui_theme,
    run_foreground,
};

mod grammar;

/// Stable output representation selected for one invocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CliOutputFormat {
    /// Concise records intended for a person.
    #[default]
    Human,
    /// Versioned machine-readable JSON records.
    Json,
}

/// Closed daemon lifecycle behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonCommand {
    /// Own node generations in the foreground until explicit stop or signal.
    Run,
    /// Probe current node state without starting a child.
    Status,
    /// Return a ready owner, autostarting one candidate when absent.
    Readiness,
    /// Converge any current owner to absence.
    Stop,
    /// Converge on a distinct ready generation, starting when absent.
    Restart,
}

/// Closed installation-identity administration behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityCommand {
    /// Create one new installation identity without overwrite.
    Init,
    /// Inspect safe public identity metadata.
    Show,
    /// Export encrypted authority to one new absolute path using an explicit stdin password.
    Export {
        /// Absolute new backup destination.
        destination: PathBuf,
    },
    /// Import encrypted authority from one absolute path using an explicit stdin password.
    Import {
        /// Absolute existing backup source.
        source: PathBuf,
    },
}

/// Closed unsigned installation-local configuration behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigurationCommand {
    /// Inspect all typed local defaults.
    Get,
    /// Replace the optional default provider.
    SetDefaultProvider {
        /// Replacement provider, or `None` to clear the default.
        provider: Option<ProviderId>,
    },
    /// Replace the complete canonical relay-default set.
    SetRelays {
        /// Complete replacement relay set.
        relays: Vec<RelayEndpoint>,
    },
    /// Discover bundled and user-defined TUI themes.
    Themes,
    /// Replace or clear the startup TUI theme.
    SetTheme {
        /// Replacement named or absolute-file selection, or `None` for automatic selection.
        theme: Option<ThemeSelection>,
    },
}

/// Closed local human-account administration behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HumanCommand {
    /// Create or reconcile the one local creator account and select it.
    Create {
        /// Optional immutable account label.
        label: Option<ShortText>,
    },
    /// Inspect local account and selection state.
    Show,
    /// Select one account for which this installation has active authority.
    Select {
        /// Exact account identity to select.
        account_id: AccountId,
    },
    /// Create one signed, offline-verifiable invitation for an exact installation address.
    Invite {
        /// Exact invited installation.
        installation_id: InstallationId,
        /// Exact invited signing key.
        signing_key: SigningPublicKey,
        /// New absolute invitation destination.
        destination: PathBuf,
        /// Optional signed device label.
        label: Option<ShortText>,
        /// Signed non-authority relay hints.
        relay_hints: RelayHints,
    },
    /// Verify and join one existing invitation addressed to this installation.
    Join {
        /// Existing absolute invitation source.
        source: PathBuf,
    },
    /// Inspect complete device membership for the selected human account.
    Devices,
    /// Revoke one exact non-creator device from the selected human account.
    Revoke {
        /// Device installation to revoke.
        installation_id: InstallationId,
    },
}

/// Closed directional peer-route administration behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerCommand {
    /// Set or recover one exact directional route.
    Add {
        /// Peer installation.
        installation_id: InstallationId,
        /// Peer signing key.
        signing_key: SigningPublicKey,
        /// Peer transport encryption key.
        encryption_key: EncryptionPublicKey,
        /// Optional signed display label.
        label: Option<ShortText>,
        /// Signed non-authority relay hints.
        relay_hints: RelayHints,
    },
    /// Inspect complete directional route history.
    List,
    /// Revoke every local mailbox capability before blocking the route.
    Distrust {
        /// Peer installation to block.
        installation_id: InstallationId,
    },
}

/// Closed directional mailbox-capability administration behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailboxCommand {
    /// Inspect complete locally owned mailbox capability history.
    List,
    /// Grant one locally owned mailbox to one uniquely routable peer.
    Grant {
        /// Locally owned mailbox.
        mailbox_id: MailboxId,
        /// Exact peer installation.
        peer_id: InstallationId,
    },
    /// Revoke one locally owned mailbox grant for one exact peer.
    Revoke {
        /// Locally owned mailbox.
        mailbox_id: MailboxId,
        /// Exact peer installation.
        peer_id: InstallationId,
    },
}

/// Closed relay policy, synchronization, and health administration behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelayCommand {
    /// Add or replace one enabled relay policy.
    Add {
        /// Validated WebSocket relay endpoint.
        endpoint: RelayEndpoint,
        /// Enabled synchronization direction.
        access: RelayAccessDto,
        /// Connection authentication policy.
        authentication: RelayAuthenticationDto,
    },
    /// Inspect bounded durable relay and delivery health.
    List,
    /// Disable one existing relay policy without erasing history.
    Remove {
        /// Validated WebSocket relay endpoint.
        endpoint: RelayEndpoint,
    },
    /// Prompt all relays or one exact relay to perform pending work.
    Sync {
        /// Optional exact relay; absence prompts all configured relays.
        endpoint: Option<RelayEndpoint>,
    },
    /// Inspect bounded durable relay and delivery health.
    Status,
    /// Explicitly reverify the corpus and repair every rebuildable domain index.
    Repair,
}

/// Passive current relay policy presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayPolicyView {
    /// Exact WebSocket endpoint.
    pub endpoint: String,
    /// Stable read/write access label.
    pub access: String,
    /// Stable authentication label.
    pub authentication: String,
    /// Whether a session owner should exist.
    pub enabled: bool,
    /// Positive durable policy generation.
    pub generation: u64,
}

/// Passive bounded relay and delivery administration result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayAdminView {
    /// Stable operation label.
    pub operation: &'static str,
    /// Definite or reconcilable effect outcome, when an effect was requested.
    pub outcome: Option<String>,
    /// Stable operation identity for uncertain reconciliation, when present.
    pub operation_id: Option<[u8; 32]>,
    /// Serialized local revision for the domain-health observation.
    pub revision: u64,
    /// Complete reducer-domain health in stable order.
    pub domains: Vec<DomainHealthView>,
    /// Current durable policies.
    pub policies: Vec<RelayPolicyView>,
    /// Queued canonical delivery intents in the bounded observation.
    pub queued: u64,
    /// Prepared exact delivery lineages in the bounded observation.
    pub prepared: u64,
    /// Uncertain relay attempts in the bounded observation.
    pub uncertain: u64,
    /// Explicitly rejected relay attempts in the bounded observation.
    pub rejected: u64,
    /// Positively accepted relay attempts in the bounded observation.
    pub accepted: u64,
    /// Transient inbound wrappers in the bounded observation.
    pub staged: u64,
    /// Permanently rejected evidence in the bounded observation.
    pub quarantined: u64,
    /// Whether additional rows exist beyond the bounded observation.
    pub truncated: bool,
}

/// Explicit or discoverable provider-session mailbox selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentMailboxSelection {
    /// Explicit provider namespace; paired with `session`.
    pub provider: Option<ProviderId>,
    /// Explicit provider-scoped durable session; paired with `provider`.
    pub session: Option<hq_domain::ProviderSessionId>,
    /// Optional repository-discovery directory override.
    pub directory: Option<PathBuf>,
}

/// Stable user-facing selector for one durable named agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamedAgentSelector {
    /// Exact permanent lowercase name.
    Name(ShortText),
    /// Exact stable agent identity.
    Id(AgentId),
}

/// Closed named-agent catalog and durable session-metadata behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamedAgentCommand {
    /// List every projected named agent, including conflicts and retirement history.
    List,
    /// Show one exact unambiguous named agent.
    Show {
        /// Permanent name or stable identity.
        agent: NamedAgentSelector,
    },
    /// Create a fresh agent mailbox or adopt one existing local agent mailbox.
    Create {
        /// Permanent lowercase installation-local name.
        name: ShortText,
        /// Existing mailbox to adopt; absence creates a deterministic mailbox.
        mailbox_id: Option<MailboxId>,
    },
    /// Resolve the current provider environment to one durable session binding.
    Current,
    /// Select one exact durable provider session and repository context.
    Select {
        /// Permanent name or stable identity.
        agent: NamedAgentSelector,
        /// Explicit or environment-discovered provider/session pair.
        mailbox: AgentMailboxSelection,
    },
    /// Rename or explicitly clear one exact provider-session display name.
    Rename {
        /// Permanent name or stable identity.
        agent: NamedAgentSelector,
        /// Explicit provider, paired with `session`.
        provider: Option<ProviderId>,
        /// Explicit provider session, paired with `provider`.
        session: Option<ProviderSessionId>,
        /// Replacement display name, or `None` for an explicit clear.
        display_name: Option<ShortText>,
    },
    /// Permanently retire one exact named agent after explicit human confirmation.
    Retire {
        /// Permanent name or stable identity.
        agent: NamedAgentSelector,
        /// Whether failed or uncertain runtime cessation may revoke HQ authority.
        force: bool,
    },
}

/// Provider-neutral managed runtime behavior for one named agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessCommand {
    /// Start one fresh durable provider session.
    Start {
        /// Permanent name or stable identity.
        agent: NamedAgentSelector,
        /// Explicit provider namespace.
        provider: ProviderId,
        /// Optional caller-relative launch directory override.
        directory: Option<PathBuf>,
    },
    /// Resume exactly one durable provider session.
    Resume {
        /// Permanent name or stable identity.
        agent: NamedAgentSelector,
        /// Explicit provider namespace.
        provider: ProviderId,
        /// Exact provider-scoped durable session.
        session: ProviderSessionId,
        /// Optional caller-relative launch directory override.
        directory: Option<PathBuf>,
    },
    /// Stop the current local runtime without erasing durable session history.
    Stop {
        /// Permanent name or stable identity.
        agent: NamedAgentSelector,
        /// Explicit provider namespace.
        provider: ProviderId,
    },
}

/// Passive managed-session control result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessSessionView {
    /// Stable requested operation label.
    pub operation: &'static str,
    /// Exact retry-safe operation identity.
    pub operation_id: OperationId,
    /// Resolved durable named-agent identity.
    pub agent_id: AgentId,
    /// Explicit provider namespace.
    pub provider: String,
    /// Exact requested resume session, when present.
    pub requested_session: Option<String>,
    /// Acknowledged ready session, when present.
    pub ready_session: Option<String>,
    /// Canonical absolute launch directory, when sent.
    pub directory: Option<String>,
    /// Stable ready, stopped, rejected, or uncertain status.
    pub status: &'static str,
    /// Stable rejection category, when rejected.
    pub error_category: Option<String>,
    /// Stable rejection code, when rejected.
    pub error_code: Option<String>,
    /// Reconciliation identity returned for an uncertain operation.
    pub reconciliation_id: Option<[u8; 32]>,
}

/// Closed project command behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectCliCommand {
    /// List every projected project in stable identity order.
    List,
    /// Show one exact project identity.
    Show(ProjectId),
    /// Inspect desired resource definitions without observing external state.
    Resource(ProjectResourceCliCommand),
    /// Freshly inspect all or one exact desired resource on its home installation.
    Check {
        /// Stable project identity.
        project_id: ProjectId,
        /// Optional exact stable resource identity.
        resource_id: Option<hq_domain::ResourceId>,
    },
    /// Send durable human-authored work to one project's immutable mailbox.
    Send {
        /// Stable project identity.
        project_id: ProjectId,
        /// Argument body, or bounded UTF-8 stdin when absent.
        body: Option<ContentText>,
    },
    /// Create an initially open project over one existing working tree.
    Create {
        /// Human-visible project name.
        name: ShortText,
        /// Optional bounded project brief.
        brief: Option<ContentText>,
        /// Existing working-tree path on the selected home.
        path: PathBuf,
        /// Selected immutable home, or the local installation by default.
        home: Option<InstallationId>,
    },
    /// Provision one Git worktree and create its project through one durable saga.
    Worktree(WorktreeCliRequest),
    /// Open one exact closed project.
    Open(ProjectId),
    /// Activate one exact named agent assignment.
    Activate {
        /// Stable project identity.
        project_id: ProjectId,
        /// Exact agent identity or permanent name.
        agent: NamedAgentSelector,
        /// Explicit provider namespace.
        provider: ProviderId,
        /// Exact provider session to resume, or `None` to start one.
        resume_session: Option<ProviderSessionId>,
        /// Exact historical project thread to resume, when selected.
        resume_thread: Option<ThreadId>,
        /// Optional normalized absolute launch directory override.
        directory: Option<PathBuf>,
    },
    /// Reconcile and dispatch every pending accepted input in order.
    Dispatch(ProjectId),
    /// Quiesce the current assignment and activate one exact target.
    Handoff {
        /// Stable project identity.
        project_id: ProjectId,
        /// Exact target agent identity or permanent name.
        agent: NamedAgentSelector,
        /// Explicit provider namespace.
        provider: ProviderId,
        /// Exact provider session to resume, or `None` to start one.
        resume_session: Option<ProviderSessionId>,
        /// Exact historical target project thread.
        thread_id: ThreadId,
        /// Optional normalized absolute launch directory override.
        directory: Option<PathBuf>,
        /// Whether blocked quiescence may revoke the prior assignment.
        force: bool,
    },
    /// Close one exact project after explicit confirmation.
    Close {
        /// Stable project identity.
        project_id: ProjectId,
        /// Whether dirty or uncertain effects may revoke HQ authority.
        force: bool,
    },
    /// Close and hide one exact project from ordinary active views.
    Archive(ProjectId),
    /// Restore one exact archived project to ordinary closed presentation.
    Unarchive(ProjectId),
}

/// Passive parsed input for one recoverable Git worktree project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeCliRequest {
    /// Human-visible project name.
    pub name: ShortText,
    /// Optional bounded project brief.
    pub brief: Option<ContentText>,
    /// Existing repository or worktree on the selected home.
    pub source: PathBuf,
    /// Exact normalized destination reserved on the selected home.
    pub destination: PathBuf,
    /// Exact existing or newly created branch name.
    pub branch: ShortText,
    /// Exact base revision when creating the branch; absent for an existing branch.
    pub base: Option<ShortText>,
    /// Selected immutable home, or the local installation by default.
    pub home: Option<InstallationId>,
}

/// Closed snapshot-only desired-resource inspection behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectResourceCliCommand {
    /// List every desired resource for one exact project.
    List {
        /// Stable project identity.
        project_id: ProjectId,
    },
    /// Show one exact desired resource identity.
    Show {
        /// Stable project identity.
        project_id: ProjectId,
        /// Stable resource identity.
        resource_id: hq_domain::ResourceId,
    },
    /// Add one home-identified desired path.
    Add {
        /// Stable project identity.
        project_id: ProjectId,
        /// Normalized absolute display path.
        path: PathBuf,
        /// Whether the new resource becomes primary.
        make_primary: bool,
    },
    /// Remove one desired resource without touching external state.
    Remove {
        /// Stable project identity.
        project_id: ProjectId,
        /// Stable desired resource identity.
        resource_id: hq_domain::ResourceId,
        /// Whether assigned-project removal is authorized.
        force: bool,
    },
    /// Atomically replace one desired resource with a home-identified path.
    Replace {
        /// Stable project identity.
        project_id: ProjectId,
        /// Existing desired resource identity.
        resource_id: hq_domain::ResourceId,
        /// Normalized absolute replacement display path.
        path: PathBuf,
    },
    /// Select one exact desired resource as primary.
    Primary {
        /// Stable project identity.
        project_id: ProjectId,
        /// Stable desired resource identity.
        resource_id: hq_domain::ResourceId,
    },
}

/// Passive desired-resource and advisory-claim presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceView {
    /// Stable resource identity.
    pub resource_id: hq_domain::ResourceId,
    /// Normalized caller-facing spelling.
    pub display_locator: ResourceLocatorDto,
    /// Immutable home-local claim identity.
    pub canonical_locator: ResourceLocatorDto,
    /// Stable unknown, healthy, degraded, or unavailable health.
    pub health: &'static str,
    /// Whether this is the explicit launch primary.
    pub primary: bool,
    /// Whether the advisory claim is active and conflict-free.
    pub active_claim: bool,
    /// Every overlapping project in stable order.
    pub conflicting_projects: Vec<ProjectId>,
}

/// Passive current project-assignment presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectAssignmentView {
    /// Immutable assignment epoch.
    pub assignment_id: hq_domain::AssignmentId,
    /// Assigned durable named agent.
    pub agent_id: AgentId,
    /// Selected provider namespace.
    pub provider: String,
    /// Acknowledged provider session, when present.
    pub session: Option<String>,
    /// Stable configuring, runnable, or blocked phase.
    pub phase: String,
    /// Runnable project thread, when present.
    pub thread_id: Option<ThreadId>,
    /// Runnable launch directory, when present.
    pub launch_directory: Option<ResourceLocatorDto>,
    /// Stable blocking error, when blocked.
    pub blocked: Option<String>,
    /// Whether project/agent cardinality is conflicted.
    pub cardinality_conflicted: bool,
    /// Whether the assignment is currently runnable.
    pub runnable: bool,
    /// Exact supporting facts.
    pub support: Vec<FactId>,
}

/// Passive exact historical provider-session/project-thread binding.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProjectThreadView {
    /// Durable named agent that owned the thread.
    pub agent_id: AgentId,
    /// Provider namespace.
    pub provider: String,
    /// Exact provider session.
    pub session: String,
    /// Immutable project thread.
    pub thread_id: ThreadId,
}

/// Passive accepted project-input attribution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectInputView {
    /// Stable input message identity.
    pub message_id: MessageId,
    /// Immutable causal thread containing this input.
    pub thread_id: ThreadId,
    /// Home-assigned contiguous sequence.
    pub sequence: u64,
    /// Exact input-acceptance fact.
    pub accepted_fact: FactId,
}

/// Passive at-most-once dispatch attribution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectDispatchView {
    /// Stable dispatch identity.
    pub dispatch_id: hq_domain::DispatchId,
    /// Accepted input message identity.
    pub message_id: MessageId,
    /// Home input sequence.
    pub sequence: u64,
    /// Exact dispatch fact.
    pub fact_id: FactId,
    /// Whether changed duplicates make the attribution unusable.
    pub conflicted: bool,
}

/// Passive retained project-output attribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectOutputView {
    /// Stable output message identity.
    pub output_id: MessageId,
    /// Originating dispatch identity.
    pub dispatch_id: hq_domain::DispatchId,
    /// Stable current, late, or conflicted status.
    pub status: String,
    /// Bounded canonical output content.
    pub content: String,
}

/// Passive structured remote project-command checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteProjectCommandView {
    /// Stable routed command identity.
    pub command_id: CommandId,
    /// Exact request digest.
    pub request_digest: hq_domain::CommandDigest,
    /// Authorizing account.
    pub account_id: AccountId,
    /// Immutable target home.
    pub target_home: InstallationId,
    /// Caller-observed project head, absent for creation.
    pub expected_head: Option<FactId>,
    /// Stable workflow operation identity.
    pub operation_id: OperationId,
    /// Provider namespace retained for durable routing correlation.
    pub operation_provider: String,
    /// Provider session retained for durable routing correlation.
    pub operation_session: String,
    /// Request semantic time.
    pub issued_at_unix_millis: i64,
    /// Exact request fact.
    pub request_fact: FactId,
    /// Stable queued, received, terminal, or conflicted state.
    pub progress: &'static str,
    /// Home receipt fact, when received.
    pub receipt_fact: Option<FactId>,
    /// Home-observed head, when received.
    pub received_head: Option<FactId>,
    /// Receipt semantic time, when received.
    pub received_at_unix_millis: Option<i64>,
    /// Terminal outcome fact, when terminal.
    pub outcome_fact: Option<FactId>,
    /// Stable committed or rejected result state.
    pub result_state: Option<&'static str>,
    /// Committed head or rejection code, when terminal.
    pub result_value: Option<String>,
    /// Stable succeeded, failed, or uncertain runtime state.
    pub runtime_state: Option<&'static str>,
    /// Runtime failure or uncertainty code, when present.
    pub runtime_code: Option<String>,
    /// External Git state retained by the authoritative home, when reported.
    pub external_state_warning: Option<ProjectExternalStateWarningView>,
}

/// Passive complete presentation for one authoritative project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectView {
    /// Stable project identity.
    pub project_id: ProjectId,
    /// Immutable authoritative installation.
    pub home: InstallationId,
    /// Immutable human account whose devices address the project.
    pub account_id: AccountId,
    /// Immutable durable project mailbox.
    pub mailbox: MailboxAddress,
    /// Human-visible display name.
    pub name: String,
    /// Stable open, closing, closed, or conflicted lifecycle.
    pub lifecycle: String,
    /// Whether the project is hidden from ordinary active views.
    pub archived: bool,
    /// Whether every desired advisory claim is currently available.
    pub claimable: bool,
    /// Last unique canonical project head.
    pub head: FactId,
    /// Last accepted contiguous input sequence.
    pub input_sequence: u64,
    /// Current assignment, when present.
    pub assignment: Option<ProjectAssignmentView>,
    /// Complete deduplicated historical thread bindings.
    pub threads: Vec<ProjectThreadView>,
    /// Complete desired resources.
    pub resources: Vec<ProjectResourceView>,
    /// Complete accepted input attribution.
    pub inputs: Vec<ProjectInputView>,
    /// Complete dispatch attribution joined through accepted inputs.
    pub dispatches: Vec<ProjectDispatchView>,
    /// Complete retained output attribution joined through dispatches.
    pub outputs: Vec<ProjectOutputView>,
    /// Complete remote control history for this project.
    pub remote_commands: Vec<RemoteProjectCommandView>,
}

/// Passive project catalog result with explicit unjoinable-history diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectCatalogView {
    /// Stable list or show operation label.
    pub operation: &'static str,
    /// Complete or selected projects.
    pub projects: Vec<ProjectView>,
    /// Dispatch projections whose project input is unavailable.
    pub unattributed_dispatches: usize,
    /// Output projections whose dispatch is unavailable.
    pub unattributed_outputs: usize,
}

/// Passive snapshot-only desired-resource inspection result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceCatalogView {
    /// Stable `resource_list` or `resource_show` operation label.
    pub operation: &'static str,
    /// Stable owning project identity.
    pub project_id: ProjectId,
    /// Immutable resource namespace authority.
    pub home: InstallationId,
    /// Complete or selected desired resources in stable identity order.
    pub resources: Vec<ProjectResourceView>,
}

/// Passive result for one fresh resource observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceCheckItemView {
    /// Exact read-only operation identity.
    pub operation_id: OperationId,
    /// Stable desired resource identity.
    pub resource_id: hq_domain::ResourceId,
    /// Normalized human-selected spelling.
    pub display_locator: ResourceLocatorDto,
    /// Immutable expected canonical identity.
    pub canonical_locator: ResourceLocatorDto,
    /// Whether this resource is the explicit launch primary.
    pub primary: bool,
    /// Whether its advisory claim is active and conflict-free.
    pub active_claim: bool,
    /// Every overlapping project in stable identity order.
    pub conflicting_projects: Vec<ProjectId>,
    /// Stable accepted, rejected, uncertain, or `response_lost` outcome.
    pub status: &'static str,
    /// Fresh health classification, when accepted.
    pub health: Option<&'static str>,
    /// Fresh release classification, when accepted.
    pub release: Option<&'static str>,
    /// Freshly observed canonical identity, when available.
    pub observed_canonical: Option<ResourceLocatorDto>,
    /// Bounded inert adapter detail, when present.
    pub details: Option<String>,
    /// Explicit observation time, when accepted.
    pub checked_at_unix_millis: Option<i64>,
    /// Stable rejection category, when rejected.
    pub error_category: Option<String>,
    /// Stable rejection code, when rejected.
    pub error_code: Option<String>,
    /// Reconciliation identity returned by an uncertain adapter.
    pub reconciliation_id: Option<OperationId>,
}

/// Passive complete fresh resource-check result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceCheckView {
    /// Stable owning project identity.
    pub project_id: ProjectId,
    /// Immutable local resource namespace authority.
    pub home: InstallationId,
    /// Selected fresh observations in stable resource identity order.
    pub checks: Vec<ProjectResourceCheckItemView>,
}

/// Passive project workflow submission or checkpoint result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectOperationView {
    /// Stable requested operation label.
    pub operation: &'static str,
    /// Exact retry-safe command identity.
    pub command_id: CommandId,
    /// Exact durable workflow identity.
    pub operation_id: OperationId,
    /// Target or newly created project identity.
    pub project_id: ProjectId,
    /// Immutable selected home.
    pub home: InstallationId,
    /// Stable accepted, running, completed, rejected, or reconcilable state.
    pub status: &'static str,
    /// Durable workflow checkpoint, when nonterminal.
    pub stage: Option<&'static str>,
    /// Resulting canonical project head, when complete.
    pub project_head: Option<FactId>,
    /// Stable rejection or reconciliation category.
    pub error_category: Option<String>,
    /// Stable rejection or reconciliation code.
    pub error_code: Option<String>,
    /// Stable succeeded, failed, or uncertain runtime state.
    pub runtime_state: Option<&'static str>,
    /// Runtime failure or uncertainty code.
    pub runtime_code: Option<String>,
    /// External Git state deliberately retained for operator inspection.
    pub external_state_warning: Option<ProjectExternalStateWarningView>,
}

/// Passive actionable warning for external state HQ never removes automatically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectExternalStateWarningView {
    /// Stable warning kind.
    pub kind: &'static str,
    /// Exact requested worktree destination.
    pub destination: String,
    /// Exact requested branch.
    pub branch: String,
}

/// Passive exact provider-session identity shown by catalog commands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedAgentSessionView {
    /// Neutral provider namespace.
    pub provider: String,
    /// Exact provider-scoped session.
    pub session: String,
    /// Unique bound mailbox, when unconflicted.
    pub mailbox: Option<MailboxAddress>,
    /// Whether incompatible immutable bindings exist.
    pub conflicted: bool,
    /// Whether this is the agent's resolved durable selection.
    pub selected: bool,
    /// Whether the display-name register is resolved.
    pub name_resolved: bool,
    /// Resolved display name or explicit clear.
    pub display_name: Option<String>,
}

/// Passive named-agent catalog record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedAgentView {
    /// Stable agent identity.
    pub agent_id: AgentId,
    /// Candidate permanent names in stable order.
    pub names: Vec<String>,
    /// Candidate local mailboxes in stable order.
    pub mailboxes: Vec<MailboxAddress>,
    /// Stable active, conflicted, or retired lifecycle.
    pub lifecycle: String,
    /// Whether one durable session is selected without conflict.
    pub runnable: bool,
    /// Durable provider sessions compatible with this agent.
    pub sessions: Vec<NamedAgentSessionView>,
}

/// Passive named-agent catalog command result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedAgentCatalogView {
    /// Stable operation label.
    pub operation: &'static str,
    /// Complete or selected agent records.
    pub agents: Vec<NamedAgentView>,
    /// Current provider-session identity when requested and resolved.
    pub current: Option<(String, String, MailboxAddress, Option<AgentId>)>,
}

/// Passive completed named-agent retirement result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedAgentRetirementView {
    /// Retired durable named-agent identity.
    pub agent_id: AgentId,
    /// Whether the caller explicitly authorized forced authority revocation.
    pub force: bool,
    /// Project whose assignment was quiesced, absent for an idle agent.
    pub project_id: Option<ProjectId>,
    /// Stable observed runtime state, absent for an idle agent.
    pub runtime: Option<String>,
    /// Stable runtime diagnostic code for failed or uncertain cessation.
    pub runtime_code: Option<String>,
}

/// Closed agent-side mailbox messaging behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentMessageCommand {
    /// Author a question and wait for one ready answer.
    Ask {
        /// Mailbox selection inputs.
        mailbox: AgentMailboxSelection,
        /// Argument body, or stdin when absent.
        body: Option<hq_domain::ContentText>,
        /// Optional overall wait bound; absence intentionally waits indefinitely.
        timeout: Option<Duration>,
        /// Bounded request retry interval.
        interval: Duration,
    },
    /// Author an asynchronous message and return immediately.
    Send {
        /// Mailbox selection inputs.
        mailbox: AgentMailboxSelection,
        /// Argument body, or stdin when absent.
        body: Option<hq_domain::ContentText>,
    },
    /// Wait for one ready answer to a question sent by the selected mailbox.
    Wait {
        /// Mailbox selection inputs.
        mailbox: AgentMailboxSelection,
        /// Stable root public message identity.
        message_id: hq_domain::MessageId,
        /// Optional overall wait bound; absence intentionally waits indefinitely.
        timeout: Option<Duration>,
        /// Bounded request retry interval.
        interval: Duration,
    },
    /// Deliver currently ready addressed content without blocking.
    Poll {
        /// Mailbox selection inputs.
        mailbox: AgentMailboxSelection,
    },
}

/// Passive human mailbox query filters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanMessageFilters {
    /// Optional sender mailbox identity.
    pub sender: Option<MailboxId>,
    /// Optional recipient mailbox identity.
    pub recipient: Option<MailboxId>,
    /// Include archived messages only.
    pub archived: bool,
    /// Include both open and archived messages.
    pub all: bool,
    /// Inclusive bounded result limit.
    pub limit: u16,
}

/// Closed human-side mailbox messaging behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HumanMessageCommand {
    /// List messages with typed filters.
    List(HumanMessageFilters),
    /// Answer one exact question root.
    Answer {
        /// Stable root public message identity.
        message_id: hq_domain::MessageId,
        /// Argument response, or stdin when absent.
        body: Option<hq_domain::ContentText>,
    },
    /// Cancel one question authored by the local human mailbox.
    Cancel {
        /// Stable root public message identity.
        message_id: hq_domain::MessageId,
    },
    /// Archive one exact message.
    Archive {
        /// Stable public message identity.
        message_id: hq_domain::MessageId,
    },
    /// Restore one exact archived message.
    Restore {
        /// Stable public message identity.
        message_id: hq_domain::MessageId,
    },
}

/// Passive message presentation used by both human and machine renderers.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct CliMessageView {
    /// Canonical message-bearing fact.
    pub fact_id: FactId,
    /// Stable public message identity.
    pub message_id: MessageId,
    /// Stable causal thread identity.
    pub thread_id: ThreadId,
    /// Exact sender mailbox.
    pub sender: MailboxAddress,
    /// Optional direct recipient.
    pub recipient: Option<MailboxAddress>,
    /// Bounded message body.
    pub content: String,
    /// Typed purpose.
    pub purpose: MessagePurposeDto,
    /// Typed presentation behavior.
    pub presentation: PresentationKindDto,
    /// Optional provider/session/operation correlation.
    pub correlation: Option<(String, String, [u8; 32])>,
    /// Optional project association.
    pub project_id: Option<ProjectId>,
    /// Whether the message remains open.
    pub open: bool,
    /// Whether the message is absorbing-rejected.
    pub rejected: bool,
    /// Exact reversible-state frontier.
    pub state_frontier: BTreeSet<FactId>,
    /// Question root fact when normalized thread state exists.
    pub root_fact: Option<FactId>,
    /// Stable root message identity when normalized thread state exists.
    pub root_message: Option<MessageId>,
    /// Whether this fact is a currently ready answer.
    pub ready_answer: bool,
    /// Whether the normalized thread is cancelled.
    pub thread_cancelled: bool,
    /// Whether the record is inert because required causal history is incomplete.
    pub incomplete: bool,
    /// Required causal identities that are absent.
    pub missing_dependencies: BTreeSet<FactId>,
    /// Present causal identities that are unusable.
    pub unusable_dependencies: BTreeSet<FactId>,
}

/// Passive result of one mailbox message command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageCommandView {
    /// Stable operation name.
    pub operation: &'static str,
    /// Selected mailbox for agent-side actions.
    pub mailbox: Option<MailboxAddress>,
    /// Stable root identity authored or selected by the operation.
    pub root_message: Option<MessageId>,
    /// Project associated with this operation, when project-addressed.
    pub project_id: Option<ProjectId>,
    /// Canonically ordered message records.
    pub messages: Vec<CliMessageView>,
    /// Whether additional incomplete-history diagnostics exist beyond the bounded snapshot.
    pub incomplete_truncated: bool,
}

/// One repository-aware provider-session mailbox candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxDiscoveryCandidate {
    /// Provider namespace.
    pub provider: String,
    /// Provider-scoped durable session.
    pub session: String,
    /// Exact bound mailbox.
    pub mailbox: MailboxAddress,
    /// Unique compatible named agent when present.
    pub named_agent: Option<AgentId>,
    /// Whether incompatible history blocks selection.
    pub conflicted: bool,
    /// Whether at least one recorded directory matches the requested directory.
    pub directory_match: bool,
    /// Recorded canonical directory spellings in fact order.
    pub directories: Vec<String>,
    /// Recorded canonical repository identities in fact order.
    pub repositories: Vec<String>,
    /// Recorded canonical worktree identities in fact order.
    pub worktrees: Vec<String>,
    /// Recorded display branches in fact order.
    pub branches: Vec<String>,
}

/// Passive repository-aware mailbox discovery result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxDiscoveryView {
    /// Exact requested discovery directory.
    pub directory: PathBuf,
    /// Stable candidate order by provider, session, and mailbox.
    pub candidates: Vec<MailboxDiscoveryCandidate>,
}

/// Passive decision counts for one reducer domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainHealthView {
    /// Stable reducer domain name.
    pub domain: String,
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

/// Passive pairing operation result safe for human and machine output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HumanPairingView {
    /// Completed operation name.
    pub operation: &'static str,
    /// Joined or inviting account.
    pub account_id: AccountId,
    /// Exact creator-issued grant identity.
    pub grant_id: GrantId,
    /// Invited installation.
    pub device: InstallationId,
}

/// Passive human-account presentation data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanAccountView {
    /// Stable account identity.
    pub account_id: AccountId,
    /// Permanent creator installation.
    pub creator_installation: InstallationId,
    /// Optional immutable account label.
    pub label: Option<String>,
    /// Whether this account is the unique active local selection.
    pub selected: bool,
}

/// Passive local account-selection presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanView {
    /// Installation whose local view is represented.
    pub installation_id: InstallationId,
    /// Accounts visible in the authoritative snapshot.
    pub accounts: Vec<HumanAccountView>,
    /// Causal-maximal local selection candidates.
    pub selection_candidates: Vec<AccountId>,
    /// Unique active local account, when resolved.
    pub active_account: Option<AccountId>,
}

/// Closed presentation state for one human-account device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HumanDeviceState {
    /// Permanent account creator authority.
    Creator,
    /// One or more grants await an exact device acceptance.
    Pending,
    /// Exactly one current grant lineage has active acceptance authority.
    Active,
    /// A creator revoke removes all known acceptance authority.
    Revoked,
    /// Multiple current grant lineages remain without a safe historical winner.
    Conflicted,
    /// The retained projection cannot support one complete device interpretation.
    Incomplete,
}

impl HumanDeviceState {
    const fn label(self) -> &'static str {
        match self {
            Self::Creator => "creator",
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Revoked => "revoked",
            Self::Conflicted => "conflicted",
            Self::Incomplete => "incomplete",
        }
    }
}

/// Passive exact creator-grant presentation for one human device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanDeviceGrantView {
    /// Stable grant identity.
    pub grant_id: GrantId,
    /// Exact supporting canonical fact.
    pub grant_fact: FactId,
    /// Exact invited signing key.
    pub signing_key: SigningPublicKey,
    /// Optional signed display label.
    pub label: Option<String>,
    /// Signed non-authority relay hints in canonical order.
    pub relay_hints: Vec<HumanRelayHintView>,
    /// Whether the grant is a current causal maximum.
    pub frontier_member: bool,
    /// Whether a current active acceptance cites this grant.
    pub active: bool,
}

/// Passive typed relay hint retained from one signed device grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanRelayHintView {
    /// Closed resource-locator scheme name.
    pub scheme: &'static str,
    /// Bounded canonical locator value.
    pub value: String,
}

/// Passive complete presentation of one account device and its retained history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanDeviceView {
    /// Member installation.
    pub installation_id: InstallationId,
    /// Every exact signing key retained for the installation without choosing a winner.
    pub signing_keys: Vec<SigningPublicKey>,
    /// Derived closed presentation state.
    pub state: HumanDeviceState,
    /// Complete creator-issued grant history.
    pub grants: Vec<HumanDeviceGrantView>,
    /// Complete causal-maximal membership frontier.
    pub frontier: Vec<FactId>,
    /// Every usable exact acceptance fact.
    pub acceptances: Vec<FactId>,
    /// Every usable exact revoke fact.
    pub revokes: Vec<FactId>,
}

/// Passive deterministic device-list presentation for one selected account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanDevicesView {
    /// Selected account being inspected.
    pub account_id: AccountId,
    /// Permanent creator installation.
    pub creator_installation: InstallationId,
    /// Devices in installation-ID order, including the creator.
    pub devices: Vec<HumanDeviceView>,
}

/// Passive exact peer-route candidate presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRouteCandidateView {
    /// Exact signed route-set fact.
    pub fact_id: FactId,
    /// Exact peer signing key.
    pub signing_key: SigningPublicKey,
    /// Exact peer transport encryption key.
    pub encryption_key: EncryptionPublicKey,
    /// Optional signed display label.
    pub label: Option<String>,
    /// Signed non-authority relay hints.
    pub relay_hints: Vec<HumanRelayHintView>,
    /// Whether this route set is a causal maximum.
    pub frontier_member: bool,
}

/// Passive exact peer-route block presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRouteBlockView {
    /// Exact signed route-block fact.
    pub fact_id: FactId,
    /// Stable signed block reason.
    pub reason: String,
    /// Whether this route block is a causal maximum.
    pub frontier_member: bool,
}

/// Passive complete directional peer-route presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRouteView {
    /// Installation that owns this directional route.
    pub owner: InstallationId,
    /// Remote installation named by the route.
    pub peer: InstallationId,
    /// Stable derived route state.
    pub state: String,
    /// Complete causal-maximal route frontier.
    pub frontier: Vec<FactId>,
    /// Complete retained signed route-set history.
    pub routes: Vec<PeerRouteCandidateView>,
    /// Complete retained signed route-block history.
    pub blocks: Vec<PeerRouteBlockView>,
}

/// Passive complete directional mailbox-capability presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxCapabilityView {
    /// Stable capability identity.
    pub grant_id: GrantId,
    /// Exact capability-grant fact.
    pub grant_fact: FactId,
    /// Installation-qualified mailbox address.
    pub mailbox: MailboxAddress,
    /// Installation-qualified grantee address.
    pub grantee: InstallationAddress,
    /// Whether no retained revoke causally dominates the grant.
    pub active: bool,
    /// Complete causal-maximal revoke frontier.
    pub revoke_frontier: Vec<FactId>,
    /// Complete retained owner-observed action identities.
    pub observed_actions: Vec<FactId>,
    /// Complete transitive projection support.
    pub support: Vec<FactId>,
}

/// Passive locally owned mailbox presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxView {
    /// Installation-qualified mailbox address.
    pub address: MailboxAddress,
    /// Exact mailbox creation fact.
    pub create_fact: FactId,
    /// Stable mailbox kind.
    pub kind: String,
    /// Optional signed display label.
    pub label: Option<String>,
}

/// Passive administrative projection result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityAdminView {
    /// Stable operation label used by deterministic renderers.
    pub operation: &'static str,
    /// Complete local directional peer-route projections.
    pub peers: Vec<PeerRouteView>,
    /// Complete locally owned mailbox projections.
    pub mailboxes: Vec<MailboxView>,
    /// Complete locally owned mailbox-capability projections.
    pub capabilities: Vec<MailboxCapabilityView>,
}

struct LocalSelection {
    candidates: Vec<AccountId>,
    active: Option<AccountId>,
    frontier: BTreeSet<FactId>,
}

/// Closed command tree shared by the installed executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliCommand {
    /// Render complete help or help for one command path.
    Help {
        /// Human-selected command path segments.
        topic: Vec<String>,
    },
    /// Print executable and protocol build metadata.
    Version,
    /// Run the installed interactive terminal user interface.
    Tui {
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Render concise installed guidance for agents.
    AgentGuidance {
        /// Requested guidance topic.
        topic: AgentGuidanceTopic,
    },
    /// Execute one offline installation-identity operation under exclusive state ownership.
    Identity {
        /// Requested identity behavior.
        action: IdentityCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one offline typed local-configuration operation.
    Configuration {
        /// Requested configuration behavior.
        action: ConfigurationCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one human-account operation through the authenticated local API.
    Human {
        /// Requested human-account behavior.
        action: HumanCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one directional peer-route operation through the authenticated local API.
    Peer {
        /// Requested peer-route behavior.
        action: PeerCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one mailbox-capability operation through the authenticated local API.
    Mailbox {
        /// Requested mailbox-capability behavior.
        action: MailboxCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one relay administration operation through the authenticated local API.
    Relay {
        /// Requested relay behavior.
        action: RelayCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one named-agent catalog or durable session-metadata operation.
    NamedAgent {
        /// Requested catalog behavior.
        action: NamedAgentCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Control one daemon-owned managed provider session through the local API.
    Harness {
        /// Requested provider-neutral runtime behavior.
        action: HarnessCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Inspect or mutate project state through the local API.
    Project {
        /// Requested project behavior.
        action: ProjectCliCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one agent-side ask, send, wait, or poll operation.
    AgentMessage {
        /// Requested agent mailbox behavior.
        action: AgentMessageCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Inspect one exact message without consuming it.
    GetMessage {
        /// Stable public message identity.
        message_id: hq_domain::MessageId,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Discover repository-aware provider-session mailboxes.
    DiscoverMailboxes {
        /// Optional discovery-directory override.
        directory: Option<PathBuf>,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one human-side message query or mutation.
    HumanMessage {
        /// Requested human mailbox behavior.
        action: HumanMessageCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
    /// Execute one daemon lifecycle command against an explicit installation layout.
    Daemon {
        /// Requested lifecycle behavior.
        action: DaemonCommand,
        /// Validated installation state layout.
        state: StatePaths,
    },
}

/// Plain parsed invocation options and command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliInvocation {
    /// Selected output representation.
    pub output: CliOutputFormat,
    /// Requested behavior.
    pub command: CliCommand,
}

/// Stable process result consumed by the tiny installed-binary adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliExecution {
    /// Complete stdout bytes represented as UTF-8 text.
    pub stdout: String,
    /// Complete redacted stderr bytes represented as UTF-8 text.
    pub stderr: String,
    /// Portable process exit status: zero, failure, usage, or unavailable.
    pub exit_code: u8,
    /// Post-stdout delivery completion; absent for non-consuming commands.
    pub completion: Option<CliCompletion>,
}

/// Recoverable post-stdout completion for at-least-once ready-message delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliCompletion {
    /// Installation state needed to reconnect after stdout succeeds.
    pub state: StatePaths,
    /// Mailbox completing receipt of the delivered records.
    pub mailbox: MailboxAddress,
    /// Stable delivered message identities.
    pub messages: Vec<MessageId>,
}

/// Stable broad exit classification for scripts and human callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliExitClass {
    /// Command execution failed after valid invocation parsing.
    Failure,
    /// Invocation syntax or a supplied path was invalid.
    Usage,
    /// The requested local service could not become available.
    Unavailable,
}

impl CliExitClass {
    const fn status(self) -> u8 {
        match self {
            Self::Failure => 1,
            Self::Usage => 2,
            Self::Unavailable => 3,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Failure => "failure",
            Self::Usage => "usage",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Stable CLI parsing, setup, or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliError {
    /// Arguments did not match one explicit supported role.
    Arguments,
    /// The interactive terminal command was invoked without terminal input and output.
    TerminalRequired,
    /// State paths could not be derived or validated.
    StatePath,
    /// Runtime paths could not be derived or validated.
    RuntimePath,
    /// Build metadata violated protocol bounds.
    Build,
    /// A direct lifecycle request failed.
    Lifecycle(LifecycleClientError),
    /// Autostart, stop, or restart did not converge.
    Coordinator(NodeCoordinatorError),
    /// Foreground generation setup or execution failed.
    Foreground(ForegroundNodeError),
    /// The async runtime or current executable was unavailable.
    Runtime,
    /// Secure identity ownership, persistence, backup, or configuration failed.
    Identity(IdentityError),
    /// Authenticated local command execution failed.
    LocalNode(LocalNodeClientError),
    /// Pure application planning rejected incomplete or invalid authority inputs.
    Application(ApplicationError),
    /// Authoritative human-account state was absent, ambiguous, stale, or inconsistent.
    HumanState,
    /// Directional route or mailbox authority was absent, ambiguous, stale, or inconsistent.
    AuthorityState,
    /// Relay policy, synchronization, or health state was unavailable or inconsistent.
    RelayState,
    /// Mailbox selection, message state, or causal authority was unavailable or inconsistent.
    MessagingState,
    /// Named-agent catalog or session metadata was absent, stale, ambiguous, or inconsistent.
    AgentState,
    /// Managed-session launch inputs or response state were invalid or inconsistent.
    HarnessState,
    /// Project state was absent, duplicated, or internally inconsistent.
    ProjectState,
    /// Resource inspection was non-local, unavailable, or internally inconsistent.
    ResourceState,
    /// Pairing evidence or its filesystem location failed strict validation.
    PairingArtifact,
    /// Backup password input was absent, oversized, malformed, or unreadable.
    SecretInput,
    /// Theme discovery or validation failed.
    Theme,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments => formatter.write_str(
                "usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] \
                 <help|version|tui|agents|agent|harness|project|ask|send|wait|poll|get|list|answer|cancel|archive|restore|mailboxes|identity|config|human|peer|mailbox|relay|daemon>",
            ),
            Self::TerminalRequired => {
                formatter.write_str("the TUI requires terminal input and output")
            }
            Self::StatePath => formatter.write_str("node state path is unavailable or invalid"),
            Self::RuntimePath => formatter.write_str("node runtime path is unavailable or invalid"),
            Self::Build => formatter.write_str("node build metadata is invalid"),
            Self::Lifecycle(error) => error.fmt(formatter),
            Self::Coordinator(error) => error.fmt(formatter),
            Self::Foreground(error) => error.fmt(formatter),
            Self::Runtime => formatter.write_str("node process runtime is unavailable"),
            Self::Identity(error) => error.fmt(formatter),
            Self::LocalNode(error) => error.fmt(formatter),
            Self::Application(error) => error.fmt(formatter),
            Self::HumanState => {
                formatter.write_str("human account state is unavailable or ambiguous")
            }
            Self::AuthorityState => {
                formatter.write_str("peer or mailbox authority is unavailable or ambiguous")
            }
            Self::RelayState => {
                formatter.write_str("relay policy or delivery state is unavailable or inconsistent")
            }
            Self::MessagingState => {
                formatter.write_str("mailbox or message state is unavailable or ambiguous")
            }
            Self::AgentState => {
                formatter.write_str("named-agent or session state is unavailable or ambiguous")
            }
            Self::HarnessState => {
                formatter.write_str("managed harness request or response state is invalid")
            }
            Self::ProjectState => {
                formatter.write_str("project state is unavailable or inconsistent")
            }
            Self::ResourceState => {
                formatter.write_str("resource inspection is unavailable or inconsistent")
            }
            Self::PairingArtifact => formatter.write_str("human pairing invitation is invalid"),
            Self::SecretInput => formatter.write_str("backup password input is invalid"),
            Self::Theme => formatter.write_str("TUI theme discovery or validation failed"),
        }
    }
}

impl Error for CliError {}

impl CliError {
    #[allow(clippy::too_many_lines, reason = "closed public diagnostic mapping")]
    const fn diagnostic(&self) -> (&'static str, &'static str, CliExitClass) {
        match self {
            Self::Arguments => (
                "cli.arguments",
                "the command arguments are invalid; run `hq help`",
                CliExitClass::Usage,
            ),
            Self::TerminalRequired => (
                "tui.terminal_required",
                "run `hq tui` with both stdin and stdout attached to a terminal",
                CliExitClass::Usage,
            ),
            Self::StatePath => (
                "cli.state_path",
                "the state root must be a valid absolute path",
                CliExitClass::Usage,
            ),
            Self::RuntimePath => (
                "cli.runtime_path",
                "the local runtime path is invalid or unavailable",
                CliExitClass::Usage,
            ),
            Self::Build => (
                "cli.build",
                "the installed build metadata is invalid",
                CliExitClass::Failure,
            ),
            Self::Lifecycle(LifecycleClientError::Absent) => (
                "node.absent",
                "no local node is running",
                CliExitClass::Unavailable,
            ),
            Self::Lifecycle(LifecycleClientError::Incompatible)
            | Self::Coordinator(NodeCoordinatorError::Probe(LifecycleClientError::Incompatible)) => {
                (
                    "node.incompatible",
                    "the local node uses an incompatible protocol version",
                    CliExitClass::Unavailable,
                )
            }
            Self::Lifecycle(_) => (
                "node.request_failed",
                "the local node request failed",
                CliExitClass::Failure,
            ),
            Self::Coordinator(NodeCoordinatorError::ReadinessTimeout { .. }) => (
                "node.readiness_timeout",
                "the local node did not become ready before the deadline",
                CliExitClass::Unavailable,
            ),
            Self::Coordinator(_) => (
                "node.coordination_failed",
                "local node coordination failed",
                CliExitClass::Failure,
            ),
            Self::Foreground(_) => (
                "node.foreground_failed",
                "the foreground node failed",
                CliExitClass::Failure,
            ),
            Self::Runtime => (
                "cli.runtime",
                "the command runtime is unavailable",
                CliExitClass::Failure,
            ),
            Self::Identity(_) => (
                "identity.operation_failed",
                "the identity or local configuration operation failed",
                CliExitClass::Failure,
            ),
            Self::LocalNode(_) => (
                "node.command_failed",
                "the authenticated local node command failed",
                CliExitClass::Failure,
            ),
            Self::Application(_) | Self::HumanState => (
                "human.state_unavailable",
                "human account authority is absent, stale, ambiguous, or inconsistent",
                CliExitClass::Failure,
            ),
            Self::AuthorityState => (
                "authority.state_unavailable",
                "peer or mailbox authority is absent, stale, ambiguous, or inconsistent",
                CliExitClass::Failure,
            ),
            Self::RelayState => (
                "relay.state_unavailable",
                "relay policy or delivery state is unavailable or inconsistent",
                CliExitClass::Failure,
            ),
            Self::MessagingState => (
                "message.state_unavailable",
                "mailbox selection or causal message state is absent, stale, ambiguous, or inconsistent",
                CliExitClass::Failure,
            ),
            Self::AgentState => agent_state_diagnostic(),
            Self::HarnessState => (
                "harness.state_invalid",
                "managed harness launch inputs or response state are invalid or inconsistent",
                CliExitClass::Failure,
            ),
            Self::ProjectState => (
                "project.state_unavailable",
                "project state is absent, duplicated, conflicted, or inconsistent",
                CliExitClass::Failure,
            ),
            Self::ResourceState => (
                "resource.inspection_unavailable",
                "resource inspection is unavailable, non-local, or inconsistent",
                CliExitClass::Failure,
            ),
            Self::PairingArtifact | Self::SecretInput => input_diagnostic(self),
            Self::Theme => (
                "theme.invalid",
                "inspect available themes with `hq config themes`, or restore automatic selection with `hq config set theme none`",
                CliExitClass::Failure,
            ),
        }
    }
}

const fn agent_state_diagnostic() -> (&'static str, &'static str, CliExitClass) {
    (
        "agent.state_unavailable",
        "named-agent or session state is absent, stale, ambiguous, conflicted, or retired",
        CliExitClass::Failure,
    )
}

const fn input_diagnostic(error: &CliError) -> (&'static str, &'static str, CliExitClass) {
    match error {
        CliError::PairingArtifact => (
            "human.pairing_invalid",
            "the pairing invitation or its file location is invalid",
            CliExitClass::Failure,
        ),
        CliError::SecretInput => (
            "identity.secret_input",
            "provide exactly one bounded UTF-8 backup password on stdin",
            CliExitClass::Usage,
        ),
        _ => unreachable!(),
    }
}

impl From<LifecycleClientError> for CliError {
    fn from(error: LifecycleClientError) -> Self {
        Self::Lifecycle(error)
    }
}

impl From<NodeCoordinatorError> for CliError {
    fn from(error: NodeCoordinatorError) -> Self {
        Self::Coordinator(error)
    }
}

impl From<ForegroundNodeError> for CliError {
    fn from(error: ForegroundNodeError) -> Self {
        Self::Foreground(error)
    }
}

impl From<IdentityError> for CliError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(error)
    }
}

impl From<LocalNodeClientError> for CliError {
    fn from(error: LocalNodeClientError) -> Self {
        Self::LocalNode(error)
    }
}

impl From<ApplicationError> for CliError {
    fn from(error: ApplicationError) -> Self {
        Self::Application(error)
    }
}

/// Parses process arguments without consulting node state or opening runtime artifacts.
pub fn parse_cli(arguments: impl IntoIterator<Item = OsString>) -> Result<CliInvocation, CliError> {
    grammar::parse(arguments)
}

fn parsed_state(state_root: Option<&PathBuf>) -> Result<StatePaths, CliError> {
    state_root
        .cloned()
        .map_or_else(StatePaths::from_environment, StatePaths::new)
        .map_err(|_| CliError::StatePath)
}

/// Parses and executes one complete invocation with deterministic stream and exit selection.
pub fn execute_cli(arguments: impl IntoIterator<Item = OsString>) -> CliExecution {
    execute_cli_with_input(arguments, &mut std::io::empty())
}

/// Executes one complete invocation with an explicit bounded secret-input source.
pub fn execute_cli_with_input(
    arguments: impl IntoIterator<Item = OsString>,
    input: &mut dyn Read,
) -> CliExecution {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let format = grammar::output_hint(&arguments);
    match parse_cli(arguments).and_then(|invocation| {
        let result = run_cli_result(&invocation, input)?;
        let empty_poll = matches!(
            &result,
            CliResult::Messages(view) if view.operation == "poll" && view.messages.is_empty()
        );
        let exit_code = successful_result_exit_code(&result, empty_poll);
        let completion = completion_for(&invocation, &result);
        let stdout = render_result(invocation.output, &result)?;
        Ok((stdout, completion, empty_poll, exit_code))
    }) {
        Ok((stdout, completion, empty_poll, exit_code)) => CliExecution {
            stdout: if empty_poll { String::new() } else { stdout },
            stderr: String::new(),
            exit_code,
            completion: if empty_poll { None } else { completion },
        },
        Err(error) => {
            let (code, message, class) = error.diagnostic();
            CliExecution {
                stdout: String::new(),
                stderr: render_error(format, code, message, class),
                exit_code: class.status(),
                completion: None,
            }
        }
    }
}

fn successful_result_exit_code(result: &CliResult, empty_poll: bool) -> u8 {
    match result {
        CliResult::HarnessSession(HarnessSessionView {
            status: "rejected", ..
        })
        | CliResult::ProjectOperation(ProjectOperationView {
            status: "rejected", ..
        }) => 1,
        CliResult::ProjectResourceCheck(view)
            if view.checks.iter().any(|check| check.status == "rejected") =>
        {
            1
        }
        CliResult::HarnessSession(HarnessSessionView {
            status: "uncertain",
            ..
        })
        | CliResult::ProjectOperation(ProjectOperationView {
            status: "reconcilable",
            ..
        }) => 3,
        CliResult::ProjectResourceCheck(view)
            if view
                .checks
                .iter()
                .any(|check| matches!(check.status, "uncertain" | "response_lost")) =>
        {
            3
        }
        _ if empty_poll => 3,
        _ => 0,
    }
}

/// Executes one parsed invocation and returns its complete stdout record.
pub fn run_cli(invocation: &CliInvocation) -> Result<String, CliError> {
    run_cli_with_input(invocation, &mut std::io::empty())
}

/// Executes one parsed invocation with an explicit bounded secret-input source.
pub fn run_cli_with_input(
    invocation: &CliInvocation,
    input: &mut dyn Read,
) -> Result<String, CliError> {
    let result = run_cli_result(invocation, input)?;
    render_result(invocation.output, &result)
}

#[allow(clippy::too_many_lines, reason = "closed installed command dispatch")]
fn run_cli_result(invocation: &CliInvocation, input: &mut dyn Read) -> Result<CliResult, CliError> {
    match &invocation.command {
        CliCommand::Identity { action, state } => return run_identity(action, state, input),
        CliCommand::Configuration { action, state } => return run_configuration(action, state),
        CliCommand::Human { action, state } => return run_human(action, state),
        CliCommand::Peer { action, state } => return run_peer(action, state),
        CliCommand::Mailbox { action, state } => return run_mailbox(action, state),
        CliCommand::Relay { action, state } => return run_relay(action, state),
        CliCommand::NamedAgent { action, state } => return run_named_agent(action, state),
        CliCommand::Harness { action, state } => return run_harness(action, state),
        CliCommand::Project { action, state } => {
            return run_project(action, state, input);
        }
        CliCommand::AgentMessage { action, state } => {
            return run_agent_message(action, state, input);
        }
        CliCommand::GetMessage { message_id, state } => return run_get_message(*message_id, state),
        CliCommand::DiscoverMailboxes { directory, state } => {
            return run_mailbox_discovery(directory.as_deref(), state);
        }
        CliCommand::HumanMessage { action, state } => {
            return run_human_message(action, state, input);
        }
        CliCommand::Help { .. }
        | CliCommand::Version
        | CliCommand::Tui { .. }
        | CliCommand::AgentGuidance { .. }
        | CliCommand::Daemon { .. } => {}
    }
    let CliCommand::Daemon { action, state } = &invocation.command else {
        return match &invocation.command {
            CliCommand::Help { topic } => {
                render_help(invocation.output, topic).map(CliResult::Rendered)
            }
            CliCommand::Version => render_version(invocation.output).map(CliResult::Rendered),
            CliCommand::Tui { .. } => Err(CliError::TerminalRequired),
            CliCommand::AgentGuidance { topic } => Ok(CliResult::AgentGuidance(*topic)),
            CliCommand::Daemon { .. }
            | CliCommand::Identity { .. }
            | CliCommand::Configuration { .. }
            | CliCommand::Human { .. }
            | CliCommand::Peer { .. }
            | CliCommand::Mailbox { .. }
            | CliCommand::Relay { .. }
            | CliCommand::NamedAgent { .. }
            | CliCommand::Harness { .. }
            | CliCommand::Project { .. }
            | CliCommand::AgentMessage { .. }
            | CliCommand::GetMessage { .. }
            | CliCommand::DiscoverMailboxes { .. }
            | CliCommand::HumanMessage { .. } => unreachable!(),
        };
    };
    let runtime = RuntimePaths::new(state.root().join("runtime"))
        .map_err(|_error: RuntimePathError| CliError::RuntimePath)?;
    let build = build()?;
    let output = match action {
        DaemonCommand::Run => {
            let async_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| CliError::Runtime)?;
            let report = async_runtime.block_on(run_foreground(foreground_config(
                state.clone(),
                runtime,
                build,
            )))?;
            CliResult::Stopped {
                intent: format!("{:?}", report.intent).to_lowercase(),
            }
        }
        DaemonCommand::Status => {
            let mut client = lifecycle_client(runtime, build)?;
            CliResult::Lifecycle {
                label: "status",
                observation: Box::new(client.request(LifecycleRequest::Status)?),
            }
        }
        DaemonCommand::Readiness => {
            let mut coordinator = coordinator(state, runtime, build)?;
            let ready = coordinator.ensure_ready()?;
            CliResult::Lifecycle {
                label: "readiness",
                observation: Box::new(ready.observation),
            }
        }
        DaemonCommand::Stop => {
            let mut coordinator = coordinator(state, runtime, build)?;
            let stopped = coordinator.stop()?;
            CliResult::Stopped {
                intent: format!("{stopped:?}").to_lowercase(),
            }
        }
        DaemonCommand::Restart => {
            let mut coordinator = coordinator(state, runtime, build)?;
            let ready = coordinator.restart()?;
            CliResult::Lifecycle {
                label: "restart",
                observation: Box::new(ready.observation),
            }
        }
    };
    Ok(output)
}

fn run_project(
    action: &ProjectCliCommand,
    state: &StatePaths,
    input: &mut dyn Read,
) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let snapshot = client.snapshot()?;
    match action {
        ProjectCliCommand::List | ProjectCliCommand::Show(_) => {
            project_catalog_view(&snapshot, action)
                .map(|view| CliResult::ProjectCatalog(Box::new(view)))
        }
        ProjectCliCommand::Resource(resource) => {
            run_project_resource(&mut client, &snapshot, resource)
        }
        ProjectCliCommand::Check {
            project_id,
            resource_id,
        } => check_project_resources(&mut client, &snapshot, *project_id, *resource_id)
            .map(|view| CliResult::ProjectResourceCheck(Box::new(view))),
        ProjectCliCommand::Create {
            name,
            brief,
            path,
            home,
        } => create_project(&mut client, &snapshot, name, brief.as_ref(), path, *home),
        ProjectCliCommand::Worktree(request) => worktree_project(&mut client, &snapshot, request),
        ProjectCliCommand::Send { project_id, body } => {
            send_project_message(&mut client, &snapshot, *project_id, body.as_ref(), input)
        }
        ProjectCliCommand::Open(project_id) => control_project(
            &mut client,
            &snapshot,
            "open",
            *project_id,
            ProjectCommandAction::Open,
        ),
        ProjectCliCommand::Activate {
            project_id,
            agent,
            provider,
            resume_session,
            resume_thread,
            directory,
        } => activate_project(
            &mut client,
            &snapshot,
            *project_id,
            agent,
            provider,
            resume_session.as_ref(),
            *resume_thread,
            directory.as_deref(),
        ),
        ProjectCliCommand::Dispatch(project_id) => control_project(
            &mut client,
            &snapshot,
            "dispatch",
            *project_id,
            ProjectCommandAction::DispatchPending,
        ),
        ProjectCliCommand::Handoff {
            project_id,
            agent,
            provider,
            resume_session,
            thread_id,
            directory,
            force,
        } => {
            let action = project_handoff_action(
                &snapshot,
                *project_id,
                agent,
                provider,
                resume_session.as_ref(),
                *thread_id,
                directory.as_deref(),
                *force,
            )?;
            control_project(&mut client, &snapshot, "handoff", *project_id, action)
        }
        ProjectCliCommand::Close { project_id, force } => control_project(
            &mut client,
            &snapshot,
            "close",
            *project_id,
            ProjectCommandAction::Close { force: *force },
        ),
        ProjectCliCommand::Archive(project_id) => control_project(
            &mut client,
            &snapshot,
            "archive",
            *project_id,
            ProjectCommandAction::SetArchived { archived: true },
        ),
        ProjectCliCommand::Unarchive(project_id) => control_project(
            &mut client,
            &snapshot,
            "unarchive",
            *project_id,
            ProjectCommandAction::SetArchived { archived: false },
        ),
    }
}

/// Passive result subset needed by the ordinary local TUI client.
pub(crate) enum ProjectTuiResult {
    Operation(Box<ProjectOperationView>),
    ResourceChecks(Box<ProjectResourceCheckView>),
    InputSent {
        project_id: ProjectId,
        message_id: MessageId,
    },
}

pub(crate) struct ProjectResourceConflictPreviewView {
    pub project_id: ProjectId,
    pub resource_id: hq_domain::ResourceId,
    pub display_path: String,
    pub canonical_path: String,
    pub relationship: &'static str,
}

pub(crate) struct ProjectResourcePreviewView {
    pub project_id: ProjectId,
    pub operation_id: OperationId,
    pub display_path: String,
    pub canonical_path: String,
    pub conflicts: Vec<ProjectResourceConflictPreviewView>,
}

pub(crate) fn project_catalog_for_tui(
    snapshot: &AuthoritativeSnapshotDto,
) -> Result<ProjectCatalogView, CliError> {
    project_catalog_view(snapshot, &ProjectCliCommand::List)
}

pub(crate) fn run_project_for_tui(
    action: &ProjectCliCommand,
    state: &StatePaths,
) -> Result<ProjectTuiResult, CliError> {
    match run_project(action, state, &mut std::io::empty())? {
        CliResult::ProjectOperation(view) => Ok(ProjectTuiResult::Operation(Box::new(view))),
        CliResult::ProjectResourceCheck(view) => Ok(ProjectTuiResult::ResourceChecks(view)),
        CliResult::Messages(view) if view.operation == "project_send" => {
            Ok(ProjectTuiResult::InputSent {
                project_id: view.project_id.ok_or(CliError::ProjectState)?,
                message_id: view.root_message.ok_or(CliError::ProjectState)?,
            })
        }
        _ => Err(CliError::ProjectState),
    }
}

pub(crate) fn preview_project_resource_for_tui(
    state: &StatePaths,
    project_id: ProjectId,
    path: &Path,
) -> Result<ProjectResourcePreviewView, CliError> {
    let mut client = command_client(state)?;
    let snapshot = client.snapshot()?;
    let catalog = project_catalog_view(&snapshot, &ProjectCliCommand::List)?;
    let project = catalog
        .projects
        .iter()
        .find(|project| project.project_id == project_id)
        .ok_or(CliError::ProjectState)?;
    ensure_resource_check_home(client.installation_id(), project.home)?;
    preview_project_path_for_tui(&mut client, &catalog, Some(project_id), project.home, path)
}

pub(crate) fn preview_project_creation_resource_for_tui(
    state: &StatePaths,
    path: &Path,
) -> Result<ProjectResourcePreviewView, CliError> {
    let mut client = command_client(state)?;
    let snapshot = client.snapshot()?;
    let catalog = project_catalog_view(&snapshot, &ProjectCliCommand::List)?;
    let home = client.installation_id();
    preview_project_path_for_tui(&mut client, &catalog, None, home, path)
}

fn preview_project_path_for_tui(
    client: &mut LocalNodeClient,
    catalog: &ProjectCatalogView,
    project_id: Option<ProjectId>,
    home: InstallationId,
    path: &Path,
) -> Result<ProjectResourcePreviewView, CliError> {
    let display = normalized_existing_resource(path)?;
    let operation_id = OperationId::from_bytes(*random_command_id()?.as_bytes());
    let project_id = project_id.unwrap_or_else(|| ProjectId::from_bytes(*operation_id.as_bytes()));
    let resource_id = project_resource_operation_identity(operation_id);
    let display_dto =
        ResourceLocatorDto::new(ResourceSchemeDto::WorkingTree, display.value().to_owned())
            .map_err(|_| CliError::Arguments)?;
    let request_resource = ProjectResourceView {
        resource_id,
        display_locator: display_dto.clone(),
        canonical_locator: display_dto,
        health: "unknown",
        primary: false,
        active_claim: false,
        conflicting_projects: Vec::new(),
    };
    let request = resource_inspection_request(
        project_id,
        &request_resource,
        operation_id,
        current_unix_millis()?,
    )?;
    let ClientEvent::Response {
        result: ResponseResult::ResourceInspection(EffectOutcomeDto::Accepted(observed)),
        ..
    } = client.request(Request::InspectResource(request))?
    else {
        return Err(CliError::ResourceState);
    };
    let canonical_dto = observed.observed_canonical.ok_or(CliError::ResourceState)?;
    let canonical = locator_from_v1(&canonical_dto)?;
    let requested_resource = ProjectResource {
        resource_id,
        display_locator: display,
        canonical_locator: canonical,
        health: ResourceHealth::Unknown,
    };
    let mut conflicts = Vec::new();
    for candidate in &catalog.projects {
        for resource in &candidate.resources {
            if !resource.active_claim {
                continue;
            }
            let candidate_resource = ProjectResource {
                resource_id: resource.resource_id,
                display_locator: locator_from_v1(&resource.display_locator)?,
                canonical_locator: locator_from_v1(&resource.canonical_locator)?,
                health: ResourceHealth::Unknown,
            };
            if let Some(conflict) = desired_resource_conflict(
                project_id,
                home,
                &requested_resource,
                candidate.project_id,
                candidate.home,
                &candidate_resource,
            ) {
                conflicts.push(ProjectResourceConflictPreviewView {
                    project_id: conflict.project_id,
                    resource_id: conflict.resource_id,
                    display_path: conflict.display_locator.value().to_owned(),
                    canonical_path: conflict.canonical_locator.value().to_owned(),
                    relationship: match conflict.relationship {
                        ProjectResourceRelationship::Equal => "equal",
                        ProjectResourceRelationship::Ancestor => "ancestor",
                        ProjectResourceRelationship::Descendant => "descendant",
                    },
                });
            }
        }
    }
    conflicts.sort_by_key(|conflict| (conflict.project_id, conflict.resource_id));
    Ok(ProjectResourcePreviewView {
        project_id,
        operation_id,
        display_path: requested_resource.display_locator.value().to_owned(),
        canonical_path: requested_resource.canonical_locator.value().to_owned(),
        conflicts,
    })
}

fn run_project_resource(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    action: &ProjectResourceCliCommand,
) -> Result<CliResult, CliError> {
    match action {
        resource @ (ProjectResourceCliCommand::List { .. }
        | ProjectResourceCliCommand::Show { .. }) => {
            project_resource_catalog_view(snapshot, resource)
                .map(|view| CliResult::ProjectResourceCatalog(Box::new(view)))
        }
        ProjectResourceCliCommand::Add {
            project_id,
            path,
            make_primary,
        } => {
            let locator = normalized_existing_resource(path)?;
            control_project_with_action(
                client,
                snapshot,
                "resource_add",
                *project_id,
                |operation_id| {
                    Ok(ProjectCommandAction::AddResource {
                        resource_id: project_resource_operation_identity(operation_id),
                        resource: locator,
                        make_primary: *make_primary,
                    })
                },
            )
        }
        ProjectResourceCliCommand::Remove {
            project_id,
            resource_id,
            force,
        } => control_project(
            client,
            snapshot,
            "resource_remove",
            *project_id,
            ProjectCommandAction::RemoveResource {
                resource_id: *resource_id,
                force: *force,
            },
        ),
        ProjectResourceCliCommand::Replace {
            project_id,
            resource_id,
            path,
        } => {
            let locator = normalized_existing_resource(path)?;
            control_project_with_action(
                client,
                snapshot,
                "resource_replace",
                *project_id,
                |operation_id| {
                    Ok(ProjectCommandAction::ReplaceResource {
                        old_resource_id: *resource_id,
                        new_resource_id: project_resource_operation_identity(operation_id),
                        resource: locator,
                    })
                },
            )
        }
        ProjectResourceCliCommand::Primary {
            project_id,
            resource_id,
        } => control_project(
            client,
            snapshot,
            "resource_primary",
            *project_id,
            ProjectCommandAction::SetPrimaryResource {
                resource_id: *resource_id,
            },
        ),
    }
}

fn project_resource_catalog_view(
    snapshot: &AuthoritativeSnapshotDto,
    action: &ProjectResourceCliCommand,
) -> Result<ProjectResourceCatalogView, CliError> {
    let (operation, project_id, resource_id) = match action {
        ProjectResourceCliCommand::List { project_id } => ("resource_list", *project_id, None),
        ProjectResourceCliCommand::Show {
            project_id,
            resource_id,
        } => ("resource_show", *project_id, Some(*resource_id)),
        ProjectResourceCliCommand::Add { .. }
        | ProjectResourceCliCommand::Remove { .. }
        | ProjectResourceCliCommand::Replace { .. }
        | ProjectResourceCliCommand::Primary { .. } => return Err(CliError::ProjectState),
    };
    let mut catalog = project_catalog_view(snapshot, &ProjectCliCommand::Show(project_id))?;
    let project = catalog.projects.pop().ok_or(CliError::ProjectState)?;
    let resources = match resource_id {
        None => project.resources,
        Some(resource_id) => vec![
            project
                .resources
                .into_iter()
                .find(|resource| resource.resource_id == resource_id)
                .ok_or(CliError::ProjectState)?,
        ],
    };
    Ok(ProjectResourceCatalogView {
        operation,
        project_id,
        home: project.home,
        resources,
    })
}

fn check_project_resources(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    project_id: ProjectId,
    resource_id: Option<hq_domain::ResourceId>,
) -> Result<ProjectResourceCheckView, CliError> {
    let action = resource_id.map_or(
        ProjectResourceCliCommand::List { project_id },
        |resource_id| ProjectResourceCliCommand::Show {
            project_id,
            resource_id,
        },
    );
    let catalog = project_resource_catalog_view(snapshot, &action)?;
    ensure_resource_check_home(client.installation_id(), catalog.home)?;
    let mut checks = Vec::with_capacity(catalog.resources.len());
    for resource in catalog.resources {
        let operation_id = OperationId::from_bytes(*random_command_id()?.as_bytes());
        let issued_at_unix_millis = current_unix_millis()?;
        let request = resource_inspection_request(
            project_id,
            &resource,
            operation_id,
            issued_at_unix_millis,
        )?;
        let mut view = ProjectResourceCheckItemView {
            operation_id,
            resource_id: resource.resource_id,
            display_locator: resource.display_locator,
            canonical_locator: resource.canonical_locator,
            primary: resource.primary,
            active_claim: resource.active_claim,
            conflicting_projects: resource.conflicting_projects,
            status: "response_lost",
            health: None,
            release: None,
            observed_canonical: None,
            details: None,
            checked_at_unix_millis: None,
            error_category: None,
            error_code: None,
            reconciliation_id: None,
        };
        match client.request(Request::InspectResource(request))? {
            ClientEvent::Response {
                result: ResponseResult::ResourceInspection(outcome),
                ..
            } => apply_resource_inspection_outcome(&mut view, outcome),
            ClientEvent::RequestLost(_) => {}
            _ => return Err(CliError::ResourceState),
        }
        checks.push(view);
    }
    Ok(ProjectResourceCheckView {
        project_id,
        home: catalog.home,
        checks,
    })
}

fn ensure_resource_check_home(
    connected_installation: InstallationId,
    project_home: InstallationId,
) -> Result<(), CliError> {
    if connected_installation == project_home {
        Ok(())
    } else {
        Err(CliError::ResourceState)
    }
}

fn resource_inspection_request(
    project_id: ProjectId,
    resource: &ProjectResourceView,
    operation_id: OperationId,
    issued_at_unix_millis: i64,
) -> Result<EffectRequestDto<ResourceInspectionRequestDto>, CliError> {
    let body = ResourceInspectionRequestDto {
        project_id: Id32::new(*project_id.as_bytes()),
        resource_id: Id32::new(*resource.resource_id.as_bytes()),
        display_locator: resource.display_locator.clone(),
        canonical_locator: resource.canonical_locator.clone(),
    };
    let mut request = EffectRequestDto::new(
        Id32::new(*operation_id.as_bytes()),
        Id32::new([0; 32]),
        issued_at_unix_millis,
        body,
    );
    request.request_digest = Id32::new(
        *resource_inspection_request_digest(&request)
            .map_err(|_| CliError::ResourceState)?
            .as_bytes(),
    );
    Ok(request)
}

fn apply_resource_inspection_outcome(
    view: &mut ProjectResourceCheckItemView,
    outcome: EffectOutcomeDto<ResourceInspectionResultDto>,
) {
    match outcome {
        EffectOutcomeDto::Accepted(result) => {
            view.status = "accepted";
            view.health = Some(resource_health_label(result.health));
            view.release = Some(resource_release_label(result.release));
            view.observed_canonical = result.observed_canonical;
            view.details = result.details;
            view.checked_at_unix_millis = Some(result.checked_at_unix_millis);
        }
        EffectOutcomeDto::Rejected(error) => {
            view.status = "rejected";
            view.error_category = Some(error.category);
            view.error_code = Some(error.code);
        }
        EffectOutcomeDto::Uncertain(reconciliation_id) => {
            view.status = "uncertain";
            view.reconciliation_id = Some(OperationId::from_bytes(reconciliation_id.bytes()));
        }
    }
}

#[allow(clippy::too_many_arguments, reason = "exact handoff selection")]
fn project_handoff_action(
    snapshot: &AuthoritativeSnapshotDto,
    project_id: ProjectId,
    agent: &NamedAgentSelector,
    provider: &ProviderId,
    resume_session: Option<&ProviderSessionId>,
    thread_id: ThreadId,
    directory: Option<&Path>,
    force: bool,
) -> Result<ProjectCommandAction, CliError> {
    let assignments = snapshot
        .items
        .iter()
        .filter(|item| {
            matches!(item, SnapshotItem::ProjectAssignment { project_id: candidate, .. }
                if candidate.bytes() == *project_id.as_bytes())
        })
        .count();
    if assignments != 1 {
        return Err(CliError::ProjectState);
    }
    let ProjectCommandAction::Activate {
        agent_id,
        provider,
        resume_session,
        resume_thread: Some(resolved_thread),
        launch_directory,
    } = project_activation_action(
        snapshot,
        project_id,
        agent,
        provider,
        resume_session,
        Some(thread_id),
        directory,
    )?
    else {
        return Err(CliError::ProjectState);
    };
    Ok(ProjectCommandAction::Handoff {
        agent_id,
        provider,
        resume_session,
        thread_id: resolved_thread,
        launch_directory,
        force_takeover: force,
    })
}

#[allow(clippy::too_many_arguments, reason = "exact activation selection")]
fn activate_project(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    project_id: ProjectId,
    agent: &NamedAgentSelector,
    provider: &ProviderId,
    resume_session: Option<&ProviderSessionId>,
    resume_thread: Option<ThreadId>,
    directory: Option<&Path>,
) -> Result<CliResult, CliError> {
    let action = project_activation_action(
        snapshot,
        project_id,
        agent,
        provider,
        resume_session,
        resume_thread,
        directory,
    )?;
    control_project(client, snapshot, "activate", project_id, action)
}

fn project_activation_action(
    snapshot: &AuthoritativeSnapshotDto,
    project_id: ProjectId,
    agent: &NamedAgentSelector,
    provider: &ProviderId,
    resume_session: Option<&ProviderSessionId>,
    resume_thread: Option<ThreadId>,
    directory: Option<&Path>,
) -> Result<ProjectCommandAction, CliError> {
    let project = project_rows(snapshot)?
        .remove(&project_id)
        .ok_or(CliError::ProjectState)?;
    let agent = resolve_named_agent(snapshot, agent)?;
    if agent.mailbox.installation_id() != project.home {
        return Err(CliError::AgentState);
    }

    validate_project_activation_resume(
        snapshot,
        project_id,
        &agent,
        provider,
        resume_session,
        resume_thread,
    )?;

    let launch_directory = if let Some(directory) = directory {
        normalized_existing_resource(directory)?
    } else {
        let primary = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::ProjectResource {
                    project_id: candidate,
                    canonical_locator,
                    primary: true,
                    ..
                } if candidate.bytes() == *project_id.as_bytes() => Some(canonical_locator),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [primary] = primary.as_slice() else {
            return Err(CliError::ProjectState);
        };
        locator_from_v1(primary)?
    };

    Ok(ProjectCommandAction::Activate {
        agent_id: agent.agent_id,
        provider: provider.clone(),
        resume_session: resume_session.cloned(),
        resume_thread,
        launch_directory,
    })
}

fn validate_project_activation_resume(
    snapshot: &AuthoritativeSnapshotDto,
    project_id: ProjectId,
    agent: &NamedAgentEvidence,
    provider: &ProviderId,
    resume_session: Option<&ProviderSessionId>,
    resume_thread: Option<ThreadId>,
) -> Result<(), CliError> {
    if let Some(session) = resume_session {
        let matching_bindings = snapshot
            .items
            .iter()
            .filter(|item| {
                matches!(item, SnapshotItem::AgentSession {
                    provider: candidate_provider,
                    session: candidate_session,
                    mailbox_installation: Some(installation),
                    mailbox_id: Some(mailbox),
                    conflicted: false,
                    ..
                } if candidate_provider == provider.as_str()
                    && candidate_session == session.as_str()
                    && installation.bytes() == *agent.mailbox.installation_id().as_bytes()
                    && mailbox.bytes() == *agent.mailbox.mailbox_id().as_bytes())
            })
            .count();
        if matching_bindings != 1 {
            return Err(CliError::AgentState);
        }
        let thread = resume_thread.ok_or(CliError::Arguments)?;
        let exact_history = snapshot.items.iter().any(|item| {
            matches!(item, SnapshotItem::ProjectThread {
                project_id: candidate_project,
                agent_id: candidate_agent,
                provider: candidate_provider,
                session: candidate_session,
                thread_id,
            } if candidate_project.bytes() == *project_id.as_bytes()
                && candidate_agent.bytes() == *agent.agent_id.as_bytes()
                && candidate_provider == provider.as_str()
                && candidate_session == session.as_str()
                && thread_id.bytes() == *thread.as_bytes())
        });
        if !exact_history {
            return Err(CliError::ProjectState);
        }
    } else if let Some(thread) = resume_thread {
        let historical = snapshot.items.iter().any(|item| {
            matches!(item, SnapshotItem::ProjectThread {
                project_id: candidate_project,
                agent_id,
                thread_id,
                ..
            } if candidate_project.bytes() == *project_id.as_bytes()
                && agent_id.bytes() == *agent.agent_id.as_bytes()
                && thread_id.bytes() == *thread.as_bytes())
        });
        let pending = snapshot.items.iter().any(|item| {
            let SnapshotItem::ProjectInput {
                project_id: candidate_project,
                message_id,
                thread_id,
                ..
            } = item
            else {
                return false;
            };
            candidate_project.bytes() == *project_id.as_bytes()
                && thread_id.bytes() == *thread.as_bytes()
                && !snapshot.items.iter().any(|candidate| {
                    matches!(candidate, SnapshotItem::ProjectDispatch {
                        message_id: dispatched,
                        ..
                    } if dispatched == message_id)
                })
        });
        if !historical && !pending {
            return Err(CliError::ProjectState);
        }
    }
    Ok(())
}

fn control_project(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    operation: &'static str,
    project_id: ProjectId,
    action: ProjectCommandAction,
) -> Result<CliResult, CliError> {
    control_project_with_action(client, snapshot, operation, project_id, |_| Ok(action))
}

fn control_project_with_action(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    operation: &'static str,
    project_id: ProjectId,
    action: impl FnOnce(OperationId) -> Result<ProjectCommandAction, CliError>,
) -> Result<CliResult, CliError> {
    let project = project_rows(snapshot)?
        .remove(&project_id)
        .ok_or(CliError::ProjectState)?;
    let local = client.installation_id();
    let account_id = local_selection(snapshot, local)
        .map_err(|_| CliError::ProjectState)?
        .active
        .filter(|account_id| *account_id == project.account_id)
        .ok_or(CliError::ProjectState)?;
    require_active_project_home(snapshot, account_id, project.home)?;
    let command_id = random_command_id()?;
    let operation_id = project_operation_id(command_id);
    let action = action(operation_id)?;
    let wire = project_command_request(
        command_id,
        operation_id,
        account_id,
        project_id,
        project.home,
        Some(project.head),
        current_unix_millis()?,
        action,
    )?;
    let ClientEvent::ProjectCommand {
        command_id: completed,
        outcome,
    } = client.project(wire)?
    else {
        return Err(CliError::ProjectState);
    };
    if completed != command_id {
        return Err(CliError::ProjectState);
    }
    project_operation_view(
        operation,
        command_id,
        project_id,
        project.home,
        operation_id,
        outcome,
    )
    .map(CliResult::ProjectOperation)
}

fn project_resource_operation_identity(operation_id: OperationId) -> hq_domain::ResourceId {
    let mut digest = Sha256::new();
    digest.update(b"hq-project-resource-operation-identity-v1\0");
    digest.update(operation_id.as_bytes());
    hq_domain::ResourceId::from_bytes(digest.finalize().into())
}

fn send_project_message(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    project_id: ProjectId,
    body: Option<&ContentText>,
    input: &mut dyn Read,
) -> Result<CliResult, CliError> {
    let project = project_rows(snapshot)?
        .remove(&project_id)
        .ok_or(CliError::ProjectState)?;
    let local = client.installation_id();
    let (human, _) = human_mailbox(snapshot, local)?;
    let message_id = random_message_id()?;
    let plan = plan_asynchronous_message(
        account_message_authority(snapshot, local, project.account_id, human)?,
        stable_inputs(),
        NewMessageRequest {
            message_id,
            recipient: Some(project.mailbox),
            body: message_body(body, input)?,
            presentation: PresentationKind::Message,
            project_id: Some(project_id),
        },
    )?;
    submit_message_plan(client, plan)?;
    Ok(CliResult::Messages(Box::new(MessageCommandView {
        operation: "project_send",
        mailbox: Some(human),
        root_message: Some(message_id),
        project_id: Some(project_id),
        messages: Vec::new(),
        incomplete_truncated: snapshot_has_incomplete_truncation(snapshot),
    })))
}

fn create_project(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    name: &ShortText,
    brief: Option<&ContentText>,
    path: &Path,
    requested_home: Option<InstallationId>,
) -> Result<CliResult, CliError> {
    let local = client.installation_id();
    let account_id = local_selection(snapshot, local)
        .map_err(|_| CliError::ProjectState)?
        .active
        .ok_or(CliError::ProjectState)?;
    let home = requested_home.unwrap_or(local);
    require_active_project_home(snapshot, account_id, home)?;
    let resource = normalized_existing_resource(path)?;
    let command_id = random_command_id()?;
    let (wire, project_id, operation_id) = project_creation_request(
        command_id,
        account_id,
        home,
        name,
        brief,
        &resource,
        current_unix_millis()?,
    )?;
    let ClientEvent::ProjectCommand {
        command_id: completed,
        outcome,
    } = client.project(wire)?
    else {
        return Err(CliError::ProjectState);
    };
    if completed != command_id {
        return Err(CliError::ProjectState);
    }
    project_operation_view(
        "create",
        command_id,
        project_id,
        home,
        operation_id,
        outcome,
    )
    .map(CliResult::ProjectOperation)
}

fn worktree_project(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    request: &WorktreeCliRequest,
) -> Result<CliResult, CliError> {
    let local = client.installation_id();
    let account_id = local_selection(snapshot, local)
        .map_err(|_| CliError::ProjectState)?
        .active
        .ok_or(CliError::ProjectState)?;
    let home = request.home.unwrap_or(local);
    require_active_project_home(snapshot, account_id, home)?;
    let source = normalized_existing_resource(&request.source)?;
    let destination = normalized_existing_resource(&request.destination)?;
    let command_id = random_command_id()?;
    let operation_id = project_creation_operation_id(command_id);
    let project_id = ProjectId::from_bytes(project_creation_identity(operation_id, b"project"));
    let mailbox_id = MailboxId::from_bytes(project_creation_identity(operation_id, b"mailbox"));
    let wire = project_command_request(
        command_id,
        operation_id,
        account_id,
        project_id,
        home,
        None,
        current_unix_millis()?,
        ProjectCommandAction::ProvisionWorktree(WorktreeProvisioningRequest {
            mailbox_id,
            project_name: request.name.clone(),
            brief: request.brief.clone(),
            source,
            destination,
            branch: request.branch.clone(),
            base: request.base.clone(),
            create_branch: request.base.is_some(),
        }),
    )?;
    let ClientEvent::ProjectCommand {
        command_id: completed,
        outcome,
    } = client.project(wire)?
    else {
        return Err(CliError::ProjectState);
    };
    if completed != command_id {
        return Err(CliError::ProjectState);
    }
    project_operation_view(
        "worktree",
        command_id,
        project_id,
        home,
        operation_id,
        outcome,
    )
    .map(CliResult::ProjectOperation)
}

fn project_creation_request(
    command_id: CommandId,
    account_id: AccountId,
    home: InstallationId,
    name: &ShortText,
    brief: Option<&ContentText>,
    resource: &ResourceLocator,
    issued_at_unix_millis: i64,
) -> Result<(ProjectCommandRequestDto, ProjectId, OperationId), CliError> {
    let operation_id = project_creation_operation_id(command_id);
    let project_id = ProjectId::from_bytes(project_creation_identity(operation_id, b"project"));
    let mailbox_id = MailboxId::from_bytes(project_creation_identity(operation_id, b"mailbox"));
    let resource_id =
        hq_domain::ResourceId::from_bytes(project_creation_identity(operation_id, b"resource"));
    let issued_at = Timestamp::from_unix_millis(issued_at_unix_millis);
    let wire = project_command_request(
        command_id,
        operation_id,
        account_id,
        project_id,
        home,
        None,
        issued_at.as_unix_millis(),
        ProjectCommandAction::Create(ProjectCreationRequest {
            mailbox_id,
            project_name: name.clone(),
            brief: brief.cloned(),
            resource_id,
            resource: resource.clone(),
        }),
    )?;
    Ok((wire, project_id, operation_id))
}

#[allow(clippy::too_many_arguments, reason = "exact project-command envelope")]
fn project_command_request(
    command_id: CommandId,
    operation_id: OperationId,
    account_id: AccountId,
    project_id: ProjectId,
    home: InstallationId,
    expected_head: Option<FactId>,
    issued_at_unix_millis: i64,
    action: ProjectCommandAction,
) -> Result<ProjectCommandRequestDto, CliError> {
    let mut request = ProjectCommandRequest {
        command_id,
        operation_id,
        request_digest: hq_domain::CommandDigest::from_bytes([0; 32]),
        account_id,
        project_id,
        home,
        expected_head,
        issued_at: Timestamp::from_unix_millis(issued_at_unix_millis),
        action,
    };
    request.request_digest =
        project_command_request_digest(&request).map_err(|_| CliError::ProjectState)?;
    Ok(project_command_request_to_v1(&request))
}

fn require_active_project_home(
    snapshot: &AuthoritativeSnapshotDto,
    account_id: AccountId,
    home: InstallationId,
) -> Result<(), CliError> {
    let (_, creator, _, _) = account_item(snapshot, account_id).ok_or(CliError::ProjectState)?;
    if home == creator {
        return Ok(());
    }
    let matches = snapshot.items.iter().filter(|item| {
        matches!(item, SnapshotItem::Membership {
            account_id: candidate_account,
            device,
            state,
            ..
        } if candidate_account.bytes() == *account_id.as_bytes()
            && device.bytes() == *home.as_bytes()
            && state == "active")
    });
    if matches.count() == 1 {
        Ok(())
    } else {
        Err(CliError::ProjectState)
    }
}

fn normalized_existing_resource(path: &Path) -> Result<ResourceLocator, CliError> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(CliError::Arguments);
    }
    let normalized = path.components().collect::<PathBuf>();
    let value = normalized.to_str().ok_or(CliError::Arguments)?.to_owned();
    Ok(ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::<RESOURCE_LOCATOR_MAX_BYTES>::new(value).map_err(|_| CliError::Arguments)?,
    ))
}

fn current_unix_millis() -> Result<i64, CliError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .ok_or(CliError::Runtime)
}

fn project_creation_operation_id(command_id: CommandId) -> OperationId {
    OperationId::from_bytes(project_creation_identity(
        OperationId::from_bytes(*command_id.as_bytes()),
        b"operation",
    ))
}

fn project_operation_id(command_id: CommandId) -> OperationId {
    let mut digest = Sha256::new();
    digest.update(b"hq-project-operation-v1\0");
    digest.update(command_id.as_bytes());
    OperationId::from_bytes(digest.finalize().into())
}

fn project_creation_identity(operation_id: OperationId, label: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"hq-project-create-identity-v1\0");
    digest.update(operation_id.as_bytes());
    digest.update(label);
    digest.finalize().into()
}

fn project_operation_view(
    operation: &'static str,
    command_id: CommandId,
    project_id: ProjectId,
    home: InstallationId,
    expected_operation: OperationId,
    outcome: ProjectCommandOutcomeDto,
) -> Result<ProjectOperationView, CliError> {
    let (operation_id, status, stage, project_head, error, runtime, external_state_warning) =
        match outcome {
            ProjectCommandOutcomeDto::Accepted {
                operation_id,
                stage,
            } => (
                operation_id,
                "accepted",
                Some(stage),
                None,
                None,
                None,
                None,
            ),
            ProjectCommandOutcomeDto::Running {
                operation_id,
                stage,
            } => (operation_id, "running", Some(stage), None, None, None, None),
            ProjectCommandOutcomeDto::Completed {
                operation_id,
                project_head,
                runtime,
            } => (
                operation_id,
                "completed",
                None,
                Some(FactId::from_bytes(project_head.bytes())),
                None,
                runtime,
                None,
            ),
            ProjectCommandOutcomeDto::Rejected {
                operation_id,
                error,
                runtime,
                external_state_warning,
            } => (
                operation_id,
                "rejected",
                None,
                None,
                Some(error),
                runtime,
                external_state_warning.map(project_external_state_warning_view),
            ),
            ProjectCommandOutcomeDto::Reconcilable {
                operation_id,
                stage,
                error,
                external_state_warning,
            } => (
                operation_id,
                "reconcilable",
                Some(stage),
                None,
                Some(error),
                None,
                external_state_warning.map(project_external_state_warning_view),
            ),
        };
    let operation_id = OperationId::from_bytes(operation_id.bytes());
    if operation_id != expected_operation {
        return Err(CliError::ProjectState);
    }
    let (runtime_state, runtime_code) = retirement_runtime(runtime.as_ref());
    Ok(ProjectOperationView {
        operation,
        command_id,
        operation_id,
        project_id,
        home,
        status,
        stage: stage.map(project_stage_label),
        project_head,
        error_category: error.as_ref().map(|error| error.category.clone()),
        error_code: error.map(|error| error.code),
        runtime_state,
        runtime_code,
        external_state_warning,
    })
}

fn project_external_state_warning_view(
    warning: ProjectExternalStateWarningDto,
) -> ProjectExternalStateWarningView {
    match warning {
        ProjectExternalStateWarningDto::WorktreeMayExist {
            destination,
            branch,
        } => ProjectExternalStateWarningView {
            kind: "worktree_may_exist",
            destination: destination.value,
            branch,
        },
    }
}

const fn project_stage_label(stage: ProjectCommandStageDto) -> &'static str {
    match stage {
        ProjectCommandStageDto::Accepted => "accepted",
        ProjectCommandStageDto::AwaitingHome => "awaiting_home",
        ProjectCommandStageDto::ReceivedAtHome => "received_at_home",
        ProjectCommandStageDto::ValidatingResources => "validating_resources",
        ProjectCommandStageDto::Opening => "opening",
        ProjectCommandStageDto::ConfiguringAssignment => "configuring_assignment",
        ProjectCommandStageDto::StartingRuntime => "starting_runtime",
        ProjectCommandStageDto::ValidatingLaunchDirectory => "validating_launch_directory",
        ProjectCommandStageDto::MakingRunnable => "making_runnable",
        ProjectCommandStageDto::DispatchingInputs => "dispatching_inputs",
        ProjectCommandStageDto::AssessingRelease => "assessing_release",
        ProjectCommandStageDto::QuiescingRuntime => "quiescing_runtime",
        ProjectCommandStageDto::EndingAssignment => "ending_assignment",
        ProjectCommandStageDto::Closing => "closing",
        ProjectCommandStageDto::UpdatingProject => "updating_project",
        ProjectCommandStageDto::ReservingDestination => "reserving_destination",
        ProjectCommandStageDto::ReconcilingGit => "reconciling_git",
        ProjectCommandStageDto::CreatingWorktree => "creating_worktree",
        ProjectCommandStageDto::IdentifyingResource => "identifying_resource",
        ProjectCommandStageDto::CreatingProject => "creating_project",
        ProjectCommandStageDto::Compensating => "compensating",
        ProjectCommandStageDto::ReconciliationRequired => "reconciliation_required",
        ProjectCommandStageDto::Complete => "complete",
    }
}

fn project_catalog_view(
    snapshot: &AuthoritativeSnapshotDto,
    action: &ProjectCliCommand,
) -> Result<ProjectCatalogView, CliError> {
    let mut projects = project_rows(snapshot)?;
    add_project_assignments(snapshot, &mut projects)?;
    add_project_threads(snapshot, &mut projects)?;
    add_project_resources(snapshot, &mut projects)?;
    let message_projects = add_project_inputs(snapshot, &mut projects)?;
    add_remote_project_commands(snapshot, &mut projects)?;
    let (dispatch_projects, unattributed_dispatches) =
        add_project_dispatches(snapshot, &message_projects, &mut projects)?;
    let unattributed_outputs = add_project_outputs(snapshot, &dispatch_projects, &mut projects)?;
    sort_project_rows(&mut projects);

    let projects = match action {
        ProjectCliCommand::List => projects.into_values().collect(),
        ProjectCliCommand::Show(project_id) => {
            vec![projects.remove(project_id).ok_or(CliError::ProjectState)?]
        }
        ProjectCliCommand::Create { .. }
        | ProjectCliCommand::Worktree(_)
        | ProjectCliCommand::Resource(_)
        | ProjectCliCommand::Check { .. }
        | ProjectCliCommand::Send { .. }
        | ProjectCliCommand::Open(_)
        | ProjectCliCommand::Activate { .. }
        | ProjectCliCommand::Dispatch(_)
        | ProjectCliCommand::Handoff { .. }
        | ProjectCliCommand::Close { .. }
        | ProjectCliCommand::Archive(_)
        | ProjectCliCommand::Unarchive(_) => {
            return Err(CliError::ProjectState);
        }
    };
    Ok(ProjectCatalogView {
        operation: match action {
            ProjectCliCommand::List => "list",
            ProjectCliCommand::Show(_) => "show",
            ProjectCliCommand::Create { .. }
            | ProjectCliCommand::Worktree(_)
            | ProjectCliCommand::Resource(_)
            | ProjectCliCommand::Check { .. }
            | ProjectCliCommand::Send { .. }
            | ProjectCliCommand::Open(_)
            | ProjectCliCommand::Activate { .. }
            | ProjectCliCommand::Dispatch(_)
            | ProjectCliCommand::Handoff { .. }
            | ProjectCliCommand::Close { .. }
            | ProjectCliCommand::Archive(_)
            | ProjectCliCommand::Unarchive(_) => {
                return Err(CliError::ProjectState);
            }
        },
        projects,
        unattributed_dispatches,
        unattributed_outputs,
    })
}

fn project_rows(
    snapshot: &AuthoritativeSnapshotDto,
) -> Result<BTreeMap<ProjectId, ProjectView>, CliError> {
    let mut projects = BTreeMap::new();
    for item in &snapshot.items {
        let SnapshotItem::Project {
            project_id,
            home,
            account_id,
            mailbox_id,
            name,
            lifecycle,
            archived,
            claimable,
            head,
            input_sequence,
        } = item
        else {
            continue;
        };
        let project_id = ProjectId::from_bytes(project_id.bytes());
        let replaced = projects.insert(
            project_id,
            ProjectView {
                project_id,
                home: InstallationId::from_bytes(home.bytes()),
                account_id: AccountId::from_bytes(account_id.bytes()),
                mailbox: MailboxAddress::new(
                    InstallationId::from_bytes(home.bytes()),
                    MailboxId::from_bytes(mailbox_id.bytes()),
                ),
                name: name.clone(),
                lifecycle: lifecycle.clone(),
                archived: *archived,
                claimable: *claimable,
                head: FactId::from_bytes(head.bytes()),
                input_sequence: *input_sequence,
                assignment: None,
                threads: Vec::new(),
                resources: Vec::new(),
                inputs: Vec::new(),
                dispatches: Vec::new(),
                outputs: Vec::new(),
                remote_commands: Vec::new(),
            },
        );
        if replaced.is_some() {
            return Err(CliError::ProjectState);
        }
    }
    Ok(projects)
}

fn add_project_assignments(
    snapshot: &AuthoritativeSnapshotDto,
    projects: &mut BTreeMap<ProjectId, ProjectView>,
) -> Result<(), CliError> {
    for item in &snapshot.items {
        let SnapshotItem::ProjectAssignment {
            project_id,
            assignment_id,
            agent_id,
            provider,
            session,
            phase,
            thread_id,
            launch_directory,
            blocked,
            cardinality_conflicted,
            runnable,
            support,
        } = item
        else {
            continue;
        };
        let project = projects
            .get_mut(&ProjectId::from_bytes(project_id.bytes()))
            .ok_or(CliError::ProjectState)?;
        if project.assignment.is_some() {
            return Err(CliError::ProjectState);
        }
        project.assignment = Some(ProjectAssignmentView {
            assignment_id: hq_domain::AssignmentId::from_bytes(assignment_id.bytes()),
            agent_id: AgentId::from_bytes(agent_id.bytes()),
            provider: provider.clone(),
            session: session.clone(),
            phase: phase.clone(),
            thread_id: thread_id.map(|thread| ThreadId::from_bytes(thread.bytes())),
            launch_directory: launch_directory.clone(),
            blocked: blocked.clone(),
            cardinality_conflicted: *cardinality_conflicted,
            runnable: *runnable,
            support: support
                .iter()
                .map(|fact| FactId::from_bytes(fact.bytes()))
                .collect(),
        });
    }
    Ok(())
}

fn add_project_threads(
    snapshot: &AuthoritativeSnapshotDto,
    projects: &mut BTreeMap<ProjectId, ProjectView>,
) -> Result<(), CliError> {
    for item in &snapshot.items {
        let SnapshotItem::ProjectThread {
            project_id,
            agent_id,
            provider,
            session,
            thread_id,
        } = item
        else {
            continue;
        };
        let project = projects
            .get_mut(&ProjectId::from_bytes(project_id.bytes()))
            .ok_or(CliError::ProjectState)?;
        project.threads.push(ProjectThreadView {
            agent_id: AgentId::from_bytes(agent_id.bytes()),
            provider: provider.clone(),
            session: session.clone(),
            thread_id: ThreadId::from_bytes(thread_id.bytes()),
        });
    }
    Ok(())
}

fn add_project_resources(
    snapshot: &AuthoritativeSnapshotDto,
    projects: &mut BTreeMap<ProjectId, ProjectView>,
) -> Result<(), CliError> {
    for item in &snapshot.items {
        let SnapshotItem::ProjectResource {
            project_id,
            resource_id,
            display_locator,
            canonical_locator,
            health,
            primary,
            active_claim,
            conflicting_projects,
        } = item
        else {
            continue;
        };
        let project = projects
            .get_mut(&ProjectId::from_bytes(project_id.bytes()))
            .ok_or(CliError::ProjectState)?;
        let resource_id = hq_domain::ResourceId::from_bytes(resource_id.bytes());
        if project
            .resources
            .iter()
            .any(|resource| resource.resource_id == resource_id)
        {
            return Err(CliError::ProjectState);
        }
        let mut conflicts = conflicting_projects
            .iter()
            .map(|id| ProjectId::from_bytes(id.bytes()))
            .collect::<Vec<_>>();
        conflicts.sort_unstable();
        project.resources.push(ProjectResourceView {
            resource_id,
            display_locator: display_locator.clone(),
            canonical_locator: canonical_locator.clone(),
            health: resource_health_label(*health),
            primary: *primary,
            active_claim: *active_claim,
            conflicting_projects: conflicts,
        });
    }
    Ok(())
}

fn add_project_inputs(
    snapshot: &AuthoritativeSnapshotDto,
    projects: &mut BTreeMap<ProjectId, ProjectView>,
) -> Result<BTreeMap<MessageId, ProjectId>, CliError> {
    let mut message_projects = BTreeMap::new();
    for item in &snapshot.items {
        let SnapshotItem::ProjectInput {
            project_id,
            message_id,
            thread_id,
            sequence,
            accepted_fact,
        } = item
        else {
            continue;
        };
        let project_id = ProjectId::from_bytes(project_id.bytes());
        let project = projects
            .get_mut(&project_id)
            .ok_or(CliError::ProjectState)?;
        let message_id = MessageId::from_bytes(message_id.bytes());
        if message_projects.insert(message_id, project_id).is_some() {
            return Err(CliError::ProjectState);
        }
        project.inputs.push(ProjectInputView {
            message_id,
            thread_id: ThreadId::from_bytes(thread_id.bytes()),
            sequence: *sequence,
            accepted_fact: FactId::from_bytes(accepted_fact.bytes()),
        });
    }
    Ok(message_projects)
}

fn add_remote_project_commands(
    snapshot: &AuthoritativeSnapshotDto,
    projects: &mut BTreeMap<ProjectId, ProjectView>,
) -> Result<(), CliError> {
    for item in &snapshot.items {
        let SnapshotItem::RemoteCommand {
            command_id,
            request_digest,
            account_id,
            project_id,
            target_home,
            expected_head,
            operation_provider,
            operation_session,
            operation_id,
            issued_at_unix_millis,
            request_fact,
            progress,
            ..
        } = item
        else {
            continue;
        };
        projects
            .get_mut(&ProjectId::from_bytes(project_id.bytes()))
            .ok_or(CliError::ProjectState)?
            .remote_commands
            .push(remote_project_command_view(
                command_id,
                request_digest,
                account_id,
                target_home,
                expected_head.as_ref(),
                operation_provider,
                operation_session,
                operation_id,
                *issued_at_unix_millis,
                request_fact,
                progress,
            ));
    }
    Ok(())
}

fn add_project_dispatches(
    snapshot: &AuthoritativeSnapshotDto,
    message_projects: &BTreeMap<MessageId, ProjectId>,
    projects: &mut BTreeMap<ProjectId, ProjectView>,
) -> Result<(BTreeMap<hq_domain::DispatchId, ProjectId>, usize), CliError> {
    let mut dispatch_projects = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut unattributed = 0;
    for item in &snapshot.items {
        let SnapshotItem::ProjectDispatch {
            dispatch_id,
            message_id,
            sequence,
            fact_id,
            conflicted,
        } = item
        else {
            continue;
        };
        let dispatch_id = hq_domain::DispatchId::from_bytes(dispatch_id.bytes());
        if !seen.insert(dispatch_id) {
            return Err(CliError::ProjectState);
        }
        let message_id = MessageId::from_bytes(message_id.bytes());
        let Some(project_id) = message_projects.get(&message_id).copied() else {
            unattributed += 1;
            continue;
        };
        dispatch_projects.insert(dispatch_id, project_id);
        projects
            .get_mut(&project_id)
            .ok_or(CliError::ProjectState)?
            .dispatches
            .push(ProjectDispatchView {
                dispatch_id,
                message_id,
                sequence: *sequence,
                fact_id: FactId::from_bytes(fact_id.bytes()),
                conflicted: *conflicted,
            });
    }
    Ok((dispatch_projects, unattributed))
}

fn add_project_outputs(
    snapshot: &AuthoritativeSnapshotDto,
    dispatch_projects: &BTreeMap<hq_domain::DispatchId, ProjectId>,
    projects: &mut BTreeMap<ProjectId, ProjectView>,
) -> Result<usize, CliError> {
    let mut seen = BTreeSet::new();
    let mut unattributed = 0;
    for item in &snapshot.items {
        let SnapshotItem::ProjectOutput {
            output_id,
            dispatch_id,
            status,
            content,
        } = item
        else {
            continue;
        };
        let output_id = MessageId::from_bytes(output_id.bytes());
        if !seen.insert(output_id) {
            return Err(CliError::ProjectState);
        }
        let dispatch_id = hq_domain::DispatchId::from_bytes(dispatch_id.bytes());
        let Some(project_id) = dispatch_projects.get(&dispatch_id).copied() else {
            unattributed += 1;
            continue;
        };
        projects
            .get_mut(&project_id)
            .ok_or(CliError::ProjectState)?
            .outputs
            .push(ProjectOutputView {
                output_id,
                dispatch_id,
                status: status.clone(),
                content: content.clone(),
            });
    }
    Ok(unattributed)
}

fn sort_project_rows(projects: &mut BTreeMap<ProjectId, ProjectView>) {
    for project in projects.values_mut() {
        project.threads.sort();
        project
            .resources
            .sort_by_key(|resource| resource.resource_id);
        project
            .inputs
            .sort_by_key(|input| (input.sequence, input.message_id));
        project
            .dispatches
            .sort_by_key(|dispatch| (dispatch.sequence, dispatch.dispatch_id));
        project.outputs.sort_by_key(|output| output.output_id);
        project
            .remote_commands
            .sort_by_key(|command| command.command_id);
    }
}

const fn resource_health_label(health: ResourceHealthDto) -> &'static str {
    match health {
        ResourceHealthDto::Unknown => "unknown",
        ResourceHealthDto::Healthy => "healthy",
        ResourceHealthDto::Degraded => "degraded",
        ResourceHealthDto::Unavailable => "unavailable",
    }
}

const fn resource_release_label(release: ResourceReleaseStateDto) -> &'static str {
    match release {
        ResourceReleaseStateDto::Clean => "clean",
        ResourceReleaseStateDto::Dirty => "dirty",
        ResourceReleaseStateDto::Unknown => "unknown",
        ResourceReleaseStateDto::NotApplicable => "not_applicable",
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "mirrors one closed snapshot record"
)]
fn remote_project_command_view(
    command_id: &Id32,
    request_digest: &Id32,
    account_id: &Id32,
    target_home: &Id32,
    expected_head: Option<&Id32>,
    operation_provider: &str,
    operation_session: &str,
    operation_id: &Id32,
    issued_at_unix_millis: i64,
    request_fact: &Id32,
    progress: &hq_local_api::protocol::v1::RemoteCommandProgressDto,
) -> RemoteProjectCommandView {
    use hq_local_api::protocol::v1::{RemoteCommandProgressDto, RemoteCommandResultDto};

    let mut view = RemoteProjectCommandView {
        command_id: CommandId::from_bytes(command_id.bytes()),
        request_digest: hq_domain::CommandDigest::from_bytes(request_digest.bytes()),
        account_id: AccountId::from_bytes(account_id.bytes()),
        target_home: InstallationId::from_bytes(target_home.bytes()),
        expected_head: expected_head.map(|head| FactId::from_bytes(head.bytes())),
        operation_id: OperationId::from_bytes(operation_id.bytes()),
        operation_provider: operation_provider.to_owned(),
        operation_session: operation_session.to_owned(),
        issued_at_unix_millis,
        request_fact: FactId::from_bytes(request_fact.bytes()),
        progress: "queued",
        receipt_fact: None,
        received_head: None,
        received_at_unix_millis: None,
        outcome_fact: None,
        result_state: None,
        result_value: None,
        runtime_state: None,
        runtime_code: None,
        external_state_warning: None,
    };
    match progress {
        RemoteCommandProgressDto::Queued => {}
        RemoteCommandProgressDto::Received {
            receipt_fact,
            received_head,
            received_at_unix_millis,
        } => {
            view.progress = "received";
            view.receipt_fact = Some(FactId::from_bytes(receipt_fact.bytes()));
            view.received_head = received_head.map(|head| FactId::from_bytes(head.bytes()));
            view.received_at_unix_millis = Some(*received_at_unix_millis);
        }
        RemoteCommandProgressDto::Terminal {
            receipt_fact,
            received_head,
            received_at_unix_millis,
            outcome_fact,
            result,
            runtime,
        } => {
            view.progress = "terminal";
            view.receipt_fact = Some(FactId::from_bytes(receipt_fact.bytes()));
            view.received_head = received_head.map(|head| FactId::from_bytes(head.bytes()));
            view.received_at_unix_millis = Some(*received_at_unix_millis);
            view.outcome_fact = Some(FactId::from_bytes(outcome_fact.bytes()));
            match result {
                RemoteCommandResultDto::Committed(head) => {
                    view.result_state = Some("committed");
                    view.result_value = Some(encode_id(&head.bytes()));
                }
                RemoteCommandResultDto::Rejected {
                    error,
                    external_state_warning,
                } => {
                    view.result_state = Some("rejected");
                    view.result_value = Some(error.clone());
                    view.external_state_warning = external_state_warning
                        .clone()
                        .map(project_external_state_warning_view);
                }
            }
            if let Some(runtime) = runtime {
                let (state, code) = match runtime {
                    RuntimeObservationDto::Succeeded => ("succeeded", None),
                    RuntimeObservationDto::Failed(code) => ("failed", Some(code.clone())),
                    RuntimeObservationDto::Uncertain(code) => ("uncertain", Some(code.clone())),
                };
                view.runtime_state = Some(state);
                view.runtime_code = code;
            }
        }
        RemoteCommandProgressDto::Conflicted => view.progress = "conflicted",
    }
    view
}

fn run_identity(
    action: &IdentityCommand,
    state: &StatePaths,
    input: &mut dyn Read,
) -> Result<CliResult, CliError> {
    let owner = StateDirectoryOwner::acquire(state.clone())?;
    match action {
        IdentityCommand::Init => Ok(CliResult::Identity(Box::new(
            owner.initialize()?.public_identity(),
        ))),
        IdentityCommand::Show => Ok(CliResult::Identity(Box::new(
            owner.load_identity()?.public_identity(),
        ))),
        IdentityCommand::Export { destination } => {
            let password = read_password(input)?;
            let identity = owner.load_identity()?;
            owner.export_identity(&identity, &password, destination)?;
            Ok(CliResult::Completed {
                operation: "identity_export",
            })
        }
        IdentityCommand::Import { source } => {
            let password = read_password(input)?;
            Ok(CliResult::Identity(Box::new(
                owner.import_identity(source, &password)?.public_identity(),
            )))
        }
    }
}

fn run_harness(action: &HarnessCommand, state: &StatePaths) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let snapshot = client.snapshot()?;
    let selector = match action {
        HarnessCommand::Start { agent, .. }
        | HarnessCommand::Resume { agent, .. }
        | HarnessCommand::Stop { agent, .. } => agent,
    };
    let evidence = resolve_named_agent(&snapshot, selector)?;
    let operation_id = OperationId::from_bytes(*random_command_id()?.as_bytes());
    let issued_at_unix_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .ok_or(CliError::Runtime)?;
    let launch = match action {
        HarnessCommand::Start { directory, .. } | HarnessCommand::Resume { directory, .. } => {
            Some(capture_launch_context(directory.as_deref())?)
        }
        HarnessCommand::Stop { .. } => None,
    };
    let directory = launch.as_ref().map(|launch| launch.directory.value.clone());
    let request = harness_request(
        action,
        evidence.agent_id,
        operation_id,
        issued_at_unix_millis,
        launch,
    )?;
    let event = client.agent_session(request)?;
    let ClientEvent::AgentSession {
        operation_id: completed,
        outcome,
    } = event
    else {
        return Err(CliError::HarnessState);
    };
    if completed != operation_id {
        return Err(CliError::HarnessState);
    }
    let (operation, provider, requested_session) = match action {
        HarnessCommand::Start { provider, .. } => ("start", provider, None),
        HarnessCommand::Resume {
            provider, session, ..
        } => ("resume", provider, Some(session.as_str().to_owned())),
        HarnessCommand::Stop { provider, .. } => ("stop", provider, None),
    };
    let mut view = HarnessSessionView {
        operation,
        operation_id,
        agent_id: evidence.agent_id,
        provider: provider.as_str().to_owned(),
        requested_session,
        ready_session: None,
        directory,
        status: "uncertain",
        error_category: None,
        error_code: None,
        reconciliation_id: None,
    };
    match outcome {
        EffectOutcomeDto::Accepted(AgentSessionResultDto::Ready(session)) => {
            view.status = "ready";
            view.ready_session = Some(session);
        }
        EffectOutcomeDto::Accepted(AgentSessionResultDto::Stopped) => {
            view.status = "stopped";
        }
        EffectOutcomeDto::Rejected(error) => {
            view.status = "rejected";
            view.error_category = Some(error.category);
            view.error_code = Some(error.code);
        }
        EffectOutcomeDto::Uncertain(reconciliation_id) => {
            view.reconciliation_id = Some(reconciliation_id.bytes());
        }
    }
    Ok(CliResult::HarnessSession(view))
}

pub(crate) fn run_harness_for_tui(
    action: &HarnessCommand,
    state: &StatePaths,
) -> Result<HarnessSessionView, CliError> {
    match run_harness(action, state)? {
        CliResult::HarnessSession(view) => Ok(view),
        _ => Err(CliError::HarnessState),
    }
}

fn harness_request(
    action: &HarnessCommand,
    agent_id: AgentId,
    operation_id: OperationId,
    issued_at_unix_millis: i64,
    launch: Option<AgentLaunchContextDto>,
) -> Result<EffectRequestDto<AgentSessionRequestDto>, CliError> {
    let (provider, control) = match action {
        HarnessCommand::Start { provider, .. } => (provider, SessionControlDto::Start),
        HarnessCommand::Resume {
            provider, session, ..
        } => (
            provider,
            SessionControlDto::Resume(session.as_str().to_owned()),
        ),
        HarnessCommand::Stop { provider, .. } => (provider, SessionControlDto::Stop),
    };
    let body = AgentSessionRequestDto::new(
        Id32::new(*agent_id.as_bytes()),
        provider.as_str().to_owned(),
        control,
        launch,
    )
    .map_err(|_| CliError::HarnessState)?;
    let mut request = EffectRequestDto::new(
        Id32::new(*operation_id.as_bytes()),
        Id32::new([0; 32]),
        issued_at_unix_millis,
        body,
    );
    request.request_digest = Id32::new(
        *agent_session_request_digest(&request)
            .map_err(|_| CliError::HarnessState)?
            .as_bytes(),
    );
    Ok(request)
}

fn capture_launch_context(directory: Option<&Path>) -> Result<AgentLaunchContextDto, CliError> {
    let directory = directory
        .map_or_else(std::env::current_dir, |path| Ok(path.to_path_buf()))
        .map_err(|_| CliError::HarnessState)?;
    let directory = directory
        .canonicalize()
        .map_err(|_| CliError::HarnessState)?;
    if !directory.is_absolute() || !directory.is_dir() {
        return Err(CliError::HarnessState);
    }
    let directory = directory.to_str().ok_or(CliError::HarnessState)?;
    let directory = ResourceLocatorDto::new(ResourceSchemeDto::WorkingTree, directory.to_owned())
        .map_err(|_| CliError::HarnessState)?;
    Ok(AgentLaunchContextDto {
        directory,
        environment: copy_launch_environment(std::env::vars_os())?,
    })
}

fn copy_launch_environment(
    entries: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<LaunchEnvironmentDto, CliError> {
    let entries = entries
        .into_iter()
        .map(|(name, value)| {
            let name = name.into_string().map_err(|_| CliError::HarnessState)?;
            Ok((name, value.as_encoded_bytes().to_vec()))
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    LaunchEnvironmentDto::copy_from(
        entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_slice())),
    )
    .map_err(|_| CliError::HarnessState)
}

fn run_configuration(
    action: &ConfigurationCommand,
    state: &StatePaths,
) -> Result<CliResult, CliError> {
    let owner = StateDirectoryOwner::acquire(state.clone())?;
    let mut configuration = owner.load_configuration()?;
    match action {
        ConfigurationCommand::Get => Ok(CliResult::Configuration(Box::new(configuration))),
        ConfigurationCommand::SetDefaultProvider { provider } => {
            configuration.default_provider.clone_from(provider);
            let configuration = LocalConfiguration::from_parts(
                configuration.relays,
                configuration.default_provider,
                configuration.theme,
            )?;
            owner.store_configuration(&configuration)?;
            Ok(CliResult::Configuration(Box::new(configuration)))
        }
        ConfigurationCommand::SetRelays { relays } => {
            configuration.relays.clone_from(relays);
            let configuration = LocalConfiguration::from_parts(
                configuration.relays,
                configuration.default_provider,
                configuration.theme,
            )?;
            owner.store_configuration(&configuration)?;
            Ok(CliResult::Configuration(Box::new(configuration)))
        }
        ConfigurationCommand::Themes => {
            let entries = list_tui_themes(
                configuration.theme.as_ref(),
                &TuiThemeEnvironment::from_environment(),
            )
            .map_err(|_| CliError::Theme)?;
            Ok(CliResult::ThemeCatalog(entries))
        }
        ConfigurationCommand::SetTheme { theme } => {
            if let Some(selection) = theme {
                resolve_tui_theme(Some(selection), &TuiThemeEnvironment::from_environment())
                    .map_err(|_| CliError::Theme)?;
            }
            let configuration = LocalConfiguration::from_parts(
                configuration.relays,
                configuration.default_provider,
                theme.clone(),
            )?;
            owner.store_configuration(&configuration)?;
            Ok(CliResult::Configuration(Box::new(configuration)))
        }
    }
}

fn run_human(action: &HumanCommand, state: &StatePaths) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let local = client.installation_id();
    match action {
        HumanCommand::Show => {
            let snapshot = client.snapshot()?;
            Ok(CliResult::Human(Box::new(human_view(&snapshot, local)?)))
        }
        HumanCommand::Create { label } => {
            reconcile_human_mailbox(&mut client, local)?;
            let snapshot = client.snapshot()?;
            let authority = local_authority(&snapshot, local)?;
            let account_id = creator_account_id(local);
            match account_item(&snapshot, account_id) {
                Some((_, creator, existing_label, _))
                    if creator == local
                        && existing_label.as_deref() == label.as_ref().map(ShortText::as_str) => {}
                Some(_) => return Err(CliError::HumanState),
                None => {
                    let plan = plan_human_account_creation(
                        authority,
                        stable_inputs(),
                        account_id,
                        label.clone(),
                    )?;
                    submit_human_plan(&mut client, plan)?;
                }
            }
            select_human_account(&mut client, local, account_id)?;
            let snapshot = client.snapshot()?;
            Ok(CliResult::Human(Box::new(human_view(&snapshot, local)?)))
        }
        HumanCommand::Select { account_id } => {
            select_human_account(&mut client, local, *account_id)?;
            let snapshot = client.snapshot()?;
            Ok(CliResult::Human(Box::new(human_view(&snapshot, local)?)))
        }
        HumanCommand::Invite {
            installation_id,
            signing_key,
            destination,
            label,
            relay_hints,
        } => create_pairing_invitation(
            &mut client,
            local,
            InstallationAddress::new(*installation_id, *signing_key),
            destination,
            label.as_ref(),
            relay_hints,
        ),
        HumanCommand::Join { source } => join_pairing_invitation(&mut client, local, source),
        HumanCommand::Devices => {
            let snapshot = client.snapshot()?;
            Ok(CliResult::HumanDevices(Box::new(human_devices_view(
                &snapshot, local,
            )?)))
        }
        HumanCommand::Revoke { installation_id } => {
            revoke_human_device(&mut client, local, *installation_id)?;
            let snapshot = client.snapshot()?;
            Ok(CliResult::HumanDevices(Box::new(human_devices_view(
                &snapshot, local,
            )?)))
        }
    }
}

fn run_peer(action: &PeerCommand, state: &StatePaths) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let local = client.installation_id();
    match action {
        PeerCommand::List => {}
        PeerCommand::Add {
            installation_id,
            signing_key,
            encryption_key,
            label,
            relay_hints,
        } => add_peer_route(
            &mut client,
            local,
            InstallationAddress::new(*installation_id, *signing_key),
            *encryption_key,
            label.as_ref(),
            relay_hints,
        )?,
        PeerCommand::Distrust { installation_id } => {
            distrust_peer(&mut client, local, *installation_id)?;
        }
    }
    let snapshot = client.snapshot()?;
    Ok(CliResult::AuthorityAdmin(Box::new(authority_admin_view(
        &snapshot,
        local,
        match action {
            PeerCommand::List => "peer_list",
            PeerCommand::Add { .. } => "peer_add",
            PeerCommand::Distrust { .. } => "peer_distrust",
        },
    ))))
}

fn run_mailbox(action: &MailboxCommand, state: &StatePaths) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let local = client.installation_id();
    match action {
        MailboxCommand::List => {}
        MailboxCommand::Grant {
            mailbox_id,
            peer_id,
        } => grant_mailbox(&mut client, local, *mailbox_id, *peer_id)?,
        MailboxCommand::Revoke {
            mailbox_id,
            peer_id,
        } => revoke_mailbox(&mut client, local, *mailbox_id, *peer_id)?,
    }
    let snapshot = client.snapshot()?;
    Ok(CliResult::AuthorityAdmin(Box::new(authority_admin_view(
        &snapshot,
        local,
        match action {
            MailboxCommand::List => "mailbox_list",
            MailboxCommand::Grant { .. } => "mailbox_grant",
            MailboxCommand::Revoke { .. } => "mailbox_revoke",
        },
    ))))
}

fn run_relay(action: &RelayCommand, state: &StatePaths) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let (operation, outcome, operation_id) = match action {
        RelayCommand::List => ("relay_list", None, None),
        RelayCommand::Status => ("relay_status", None, None),
        RelayCommand::Add {
            endpoint,
            access,
            authentication,
        } => {
            let body = relay_configuration(endpoint, *access, *authentication, true)?;
            let (outcome, operation_id) = configure_relay(&mut client, body)?;
            ("relay_add", Some(outcome), operation_id)
        }
        RelayCommand::Remove { endpoint } => {
            let status = relay_status(&mut client)?;
            let Some(policy) = status
                .policies
                .iter()
                .find(|policy| policy.endpoint.value == endpoint.as_str())
            else {
                return Ok(CliResult::RelayAdmin(Box::new(relay_admin_view(
                    "relay_remove",
                    Some("unchanged".to_owned()),
                    None,
                    status,
                    state_health(&mut client)?,
                ))));
            };
            if !policy.enabled {
                return Ok(CliResult::RelayAdmin(Box::new(relay_admin_view(
                    "relay_remove",
                    Some("unchanged".to_owned()),
                    None,
                    status,
                    state_health(&mut client)?,
                ))));
            }
            let body = RelayConfigurationDto::new(
                policy.endpoint.clone(),
                policy.access,
                policy.authentication,
                false,
            );
            let (outcome, operation_id) = configure_relay(&mut client, body)?;
            ("relay_remove", Some(outcome), operation_id)
        }
        RelayCommand::Sync { endpoint } => {
            let body = endpoint
                .as_ref()
                .map_or(Ok(SynchronizationRequestDto::All), |endpoint| {
                    relay_locator(endpoint).map(SynchronizationRequestDto::Relay)
                })?;
            let (outcome, operation_id) = synchronize_relay(&mut client, body)?;
            ("relay_sync", Some(outcome), operation_id)
        }
        RelayCommand::Repair => {
            let health = state_health(&mut client)?;
            let operation_id = stable_repair_operation(health.revision);
            repair_state(&mut client, operation_id)?;
            (
                "relay_repair",
                Some("repaired".to_owned()),
                Some(operation_id),
            )
        }
    };
    let status = relay_status(&mut client)?;
    let health = state_health(&mut client)?;
    Ok(CliResult::RelayAdmin(Box::new(relay_admin_view(
        operation,
        outcome,
        operation_id,
        status,
        health,
    ))))
}

fn run_named_agent(action: &NamedAgentCommand, state: &StatePaths) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let local = client.installation_id();
    let mut snapshot = client.snapshot()?;
    let operation = match action {
        NamedAgentCommand::List => "agent_list",
        NamedAgentCommand::Show { agent } => {
            let selected = resolve_named_agent_id(&snapshot, agent)?;
            return Ok(CliResult::NamedAgentCatalog(Box::new(
                named_agent_catalog_view(&snapshot, "agent_show", Some(selected), None),
            )));
        }
        NamedAgentCommand::Current => {
            let current = resolve_current_session(&snapshot, local)?;
            return Ok(CliResult::NamedAgentCatalog(Box::new(
                named_agent_catalog_view(&snapshot, "agent_current", current.3, Some(current)),
            )));
        }
        NamedAgentCommand::Create { name, mailbox_id } => {
            reconcile_named_agent(&mut client, &mut snapshot, local, name, *mailbox_id)?;
            "agent_create"
        }
        NamedAgentCommand::Select { agent, mailbox } => {
            select_named_agent_session(&mut client, &snapshot, local, agent, mailbox)?;
            "agent_select"
        }
        NamedAgentCommand::Rename {
            agent,
            provider,
            session,
            display_name,
        } => {
            rename_named_agent_session(
                &mut client,
                &snapshot,
                local,
                agent,
                provider.as_ref(),
                session.as_ref(),
                display_name.clone(),
            )?;
            "agent_rename"
        }
        NamedAgentCommand::Retire { agent, force } => {
            return retire_named_agent(&mut client, &snapshot, local, agent, *force);
        }
    };
    snapshot = client.snapshot()?;
    let selected = match action {
        NamedAgentCommand::Create { name, .. } => {
            Some(resolve_named_agent(&snapshot, &NamedAgentSelector::Name(name.clone()))?.agent_id)
        }
        NamedAgentCommand::Select { agent, .. } | NamedAgentCommand::Rename { agent, .. } => {
            Some(resolve_named_agent(&snapshot, agent)?.agent_id)
        }
        NamedAgentCommand::List
        | NamedAgentCommand::Show { .. }
        | NamedAgentCommand::Current
        | NamedAgentCommand::Retire { .. } => None,
    };
    Ok(CliResult::NamedAgentCatalog(Box::new(
        named_agent_catalog_view(&snapshot, operation, selected, None),
    )))
}

pub(crate) fn run_named_agent_for_tui(
    action: &NamedAgentCommand,
    state: &StatePaths,
) -> Result<u64, CliError> {
    let _ = run_named_agent(action, state)?;
    Ok(command_client(state)?.snapshot()?.revision)
}

fn retire_named_agent(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    selector: &NamedAgentSelector,
    force: bool,
) -> Result<CliResult, CliError> {
    let agent = resolve_named_agent(snapshot, selector)?;
    let account_id = local_selection(snapshot, local)
        .map_err(|_| CliError::AgentState)?
        .active
        .ok_or(CliError::AgentState)?;
    let command_id = random_command_id()?;
    let operation_id = agent_retirement_operation_id(command_id);
    let mut request = AgentRetirementRequest {
        command_id,
        operation_id,
        request_digest: hq_domain::CommandDigest::from_bytes([0; 32]),
        account_id,
        agent_id: agent.agent_id,
        expected_claim: agent.claim_fact,
        home: local,
        issued_at: Timestamp::from_unix_millis(0),
        force,
    };
    request.request_digest = agent_retirement_request_digest(&request);
    let wire = AgentRetirementRequestDto {
        command_id: Id32::new(*request.command_id.as_bytes()),
        operation_id: Id32::new(*request.operation_id.as_bytes()),
        request_digest: Id32::new(*request.request_digest.as_bytes()),
        account_id: Id32::new(*request.account_id.as_bytes()),
        agent_id: Id32::new(*request.agent_id.as_bytes()),
        expected_claim: Id32::new(*request.expected_claim.as_bytes()),
        home: Id32::new(*request.home.as_bytes()),
        issued_at_unix_millis: request.issued_at.as_unix_millis(),
        force: request.force,
    };
    for _ in 0..16 {
        match client.agent_retirement(wire)? {
            ClientEvent::AgentRetirement {
                outcome:
                    AgentRetirementOutcomeDto::Completed {
                        project_id,
                        runtime,
                        ..
                    },
                ..
            } => {
                verify_agent_retired(client, request.agent_id, request.expected_claim)?;
                let (runtime, runtime_code) = retirement_runtime(runtime.as_ref());
                return Ok(CliResult::NamedAgentRetirement(NamedAgentRetirementView {
                    agent_id: request.agent_id,
                    force,
                    project_id: project_id.map(|value| ProjectId::from_bytes(value.bytes())),
                    runtime: runtime.map(str::to_owned),
                    runtime_code,
                }));
            }
            ClientEvent::AgentRetirement {
                outcome:
                    AgentRetirementOutcomeDto::Running { .. }
                    | AgentRetirementOutcomeDto::Reconcilable { .. },
                ..
            } => {}
            _ => return Err(CliError::AgentState),
        }
    }
    Err(CliError::AgentState)
}

fn verify_agent_retired(
    client: &mut LocalNodeClient,
    agent_id: AgentId,
    claim_fact: FactId,
) -> Result<(), CliError> {
    let snapshot = client.snapshot()?;
    snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Agent {
                agent_id: candidate,
                claims,
                retirements,
                lifecycle,
                ..
            } if candidate.bytes() == *agent_id.as_bytes()
                && lifecycle == "retired"
                && claims
                    .iter()
                    .any(|claim| claim.bytes() == *claim_fact.as_bytes())
                && !retirements.is_empty() =>
            {
                Some(())
            }
            _ => None,
        })
        .ok_or(CliError::AgentState)
}

fn retirement_runtime(
    runtime: Option<&RuntimeObservationDto>,
) -> (Option<&'static str>, Option<String>) {
    match runtime {
        None => (None, None),
        Some(RuntimeObservationDto::Succeeded) => (Some("succeeded"), None),
        Some(RuntimeObservationDto::Failed(code)) => (Some("failed"), Some(code.clone())),
        Some(RuntimeObservationDto::Uncertain(code)) => (Some("uncertain"), Some(code.clone())),
    }
}

fn agent_retirement_operation_id(command_id: CommandId) -> OperationId {
    let mut digest = Sha256::new();
    digest.update(b"hq-agent-retirement-operation-v1\0");
    digest.update(command_id.as_bytes());
    OperationId::from_bytes(digest.finalize().into())
}

#[derive(Clone, Copy)]
struct NamedAgentEvidence {
    agent_id: AgentId,
    claim_fact: FactId,
    mailbox: MailboxAddress,
}

fn resolve_named_agent_id(
    snapshot: &AuthoritativeSnapshotDto,
    selector: &NamedAgentSelector,
) -> Result<AgentId, CliError> {
    let matches = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::Agent {
            agent_id, names, ..
        } if match selector {
            NamedAgentSelector::Name(name) => names.iter().any(|value| value == name.as_str()),
            NamedAgentSelector::Id(expected) => agent_id.bytes() == *expected.as_bytes(),
        } =>
        {
            Some(AgentId::from_bytes(agent_id.bytes()))
        }
        _ => None,
    });
    let values = matches.collect::<Vec<_>>();
    match values.as_slice() {
        [agent_id] => Ok(*agent_id),
        [] | [_, _, ..] => Err(CliError::AgentState),
    }
}

fn resolve_named_agent(
    snapshot: &AuthoritativeSnapshotDto,
    selector: &NamedAgentSelector,
) -> Result<NamedAgentEvidence, CliError> {
    let matches = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Agent {
                agent_id,
                claims,
                names,
                mailboxes,
                lifecycle,
                ..
            } if match selector {
                NamedAgentSelector::Name(name) => names.iter().any(|value| value == name.as_str()),
                NamedAgentSelector::Id(expected) => agent_id.bytes() == *expected.as_bytes(),
            } =>
            {
                Some((agent_id, claims, mailboxes, lifecycle))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(agent_id, claims, mailboxes, lifecycle)] = matches.as_slice() else {
        return Err(CliError::AgentState);
    };
    let ([claim_fact], [mailbox]) = (claims.as_slice(), mailboxes.as_slice()) else {
        return Err(CliError::AgentState);
    };
    if *lifecycle != "active" {
        return Err(CliError::AgentState);
    }
    Ok(NamedAgentEvidence {
        agent_id: AgentId::from_bytes(agent_id.bytes()),
        claim_fact: FactId::from_bytes(claim_fact.bytes()),
        mailbox: MailboxAddress::new(
            InstallationId::from_bytes(mailbox.installation_id.bytes()),
            MailboxId::from_bytes(mailbox.mailbox_id.bytes()),
        ),
    })
}

fn reconcile_named_agent(
    client: &mut LocalNodeClient,
    snapshot: &mut AuthoritativeSnapshotDto,
    local: InstallationId,
    name: &ShortText,
    requested_mailbox: Option<MailboxId>,
) -> Result<(), CliError> {
    let agent_id = stable_named_agent_id(local, name);
    let mailbox_id = requested_mailbox.unwrap_or_else(|| stable_named_agent_mailbox(local, name));
    let selector = NamedAgentSelector::Name(name.clone());
    let matching = snapshot.items.iter().any(|item| match item {
        SnapshotItem::Agent { names, .. } => names.iter().any(|value| value == name.as_str()),
        _ => false,
    });
    if matching {
        let existing = resolve_named_agent(snapshot, &selector)?;
        if existing.agent_id == agent_id && existing.mailbox.mailbox_id() == mailbox_id {
            return Ok(());
        }
        return Err(CliError::AgentState);
    }

    let mailbox_root = agent_mailbox_root(snapshot, local, mailbox_id)?;
    let mailbox_root = match mailbox_root {
        Some(root) => root,
        None if requested_mailbox.is_some() => return Err(CliError::AgentState),
        None => {
            let plan = plan_agent_mailbox_creation(
                local_authority(snapshot, local).map_err(|_| CliError::AgentState)?,
                stable_inputs(),
                mailbox_id,
                Some(name.clone()),
            )?;
            submit_agent_plan(client, plan)?;
            *snapshot = client.snapshot()?;
            agent_mailbox_root(snapshot, local, mailbox_id)?.ok_or(CliError::AgentState)?
        }
    };
    let plan = plan_agent_name_claim(
        local_authority(snapshot, local).map_err(|_| CliError::AgentState)?,
        stable_inputs(),
        AgentNameClaimRequest {
            agent_id,
            mailbox: MailboxAddress::new(local, mailbox_id),
            mailbox_root,
            name: name.clone(),
        },
    )?;
    submit_agent_plan(client, plan)?;
    let reconciled = client.snapshot()?;
    let created = resolve_named_agent(&reconciled, &selector)?;
    if created.agent_id != agent_id || created.mailbox.mailbox_id() != mailbox_id {
        return Err(CliError::AgentState);
    }
    *snapshot = reconciled;
    Ok(())
}

fn agent_mailbox_root(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    mailbox_id: MailboxId,
) -> Result<Option<FactId>, CliError> {
    let matches = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Mailbox {
                installation_id,
                mailbox_id: candidate,
                create_fact,
                mailbox_kind,
                ..
            } if installation_id.bytes() == *local.as_bytes()
                && candidate.bytes() == *mailbox_id.as_bytes() =>
            {
                Some((create_fact, mailbox_kind))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [(fact, kind)] if *kind == "agent" => Ok(Some(FactId::from_bytes(fact.bytes()))),
        [_] | [_, _, ..] => Err(CliError::AgentState),
    }
}

fn stable_named_agent_id(local: InstallationId, name: &ShortText) -> AgentId {
    AgentId::from_bytes(stable_named_agent_value(
        b"hq-named-agent-id-v1\0",
        local,
        name,
    ))
}

fn stable_named_agent_mailbox(local: InstallationId, name: &ShortText) -> MailboxId {
    MailboxId::from_bytes(stable_named_agent_value(
        b"hq-named-agent-mailbox-v1\0",
        local,
        name,
    ))
}

fn stable_named_agent_value(domain: &[u8], local: InstallationId, name: &ShortText) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(local.as_bytes());
    digest.update(name.as_str().as_bytes());
    digest.finalize().into()
}

fn select_named_agent_session(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    selector: &NamedAgentSelector,
    requested: &AgentMailboxSelection,
) -> Result<(), CliError> {
    let agent = resolve_named_agent(snapshot, selector)?;
    if agent.mailbox.installation_id() != local {
        return Err(CliError::AgentState);
    }
    let (provider, session) = requested_session_identity(requested)?;
    let binding_fact = session_binding_fact(snapshot, agent.mailbox, &provider, &session)?;
    let directory = discovery_directory(requested.directory.as_deref())?;
    let (context_fact, context) = session_context(snapshot, agent.mailbox, &directory)?;
    let selection_frontier = agent_selection_frontier(snapshot, agent.agent_id)?;
    let plan = plan_agent_session_selection(
        local_authority(snapshot, local).map_err(|_| CliError::AgentState)?,
        stable_inputs(),
        AgentSessionSelectionRequest {
            agent_id: agent.agent_id,
            mailbox: agent.mailbox,
            claim_fact: agent.claim_fact,
            provider,
            session,
            binding_fact,
            context_fact,
            context,
            selection_frontier,
        },
    )?;
    submit_agent_plan(client, plan)
}

#[allow(clippy::too_many_arguments)]
fn rename_named_agent_session(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    selector: &NamedAgentSelector,
    explicit_provider: Option<&ProviderId>,
    explicit_session: Option<&ProviderSessionId>,
    display_name: Option<ShortText>,
) -> Result<(), CliError> {
    let agent = resolve_named_agent(snapshot, selector)?;
    if agent.mailbox.installation_id() != local {
        return Err(CliError::AgentState);
    }
    let (provider, session) = rename_session_identity(
        snapshot,
        agent.agent_id,
        explicit_provider,
        explicit_session,
    )?;
    let binding_fact = session_binding_fact(snapshot, agent.mailbox, &provider, &session)?;
    let rename_frontier = agent_rename_frontier(snapshot, agent.agent_id, &provider, &session)?;
    let plan = plan_agent_session_rename(
        local_authority(snapshot, local).map_err(|_| CliError::AgentState)?,
        stable_inputs(),
        AgentSessionRenameRequest {
            agent_id: agent.agent_id,
            mailbox: agent.mailbox,
            claim_fact: agent.claim_fact,
            provider,
            session,
            binding_fact,
            display_name,
            rename_frontier,
        },
    )?;
    submit_agent_plan(client, plan)
}

fn requested_session_identity(
    requested: &AgentMailboxSelection,
) -> Result<(ProviderId, ProviderSessionId), CliError> {
    match (&requested.provider, &requested.session) {
        (Some(provider), Some(session)) => Ok((provider.clone(), session.clone())),
        (None, None) => {
            let (provider, session) =
                environment_session_identity()?.ok_or(CliError::AgentState)?;
            Ok((
                ProviderId::new(provider).map_err(|_| CliError::AgentState)?,
                ProviderSessionId::new(session).map_err(|_| CliError::AgentState)?,
            ))
        }
        _ => Err(CliError::AgentState),
    }
}

fn rename_session_identity(
    snapshot: &AuthoritativeSnapshotDto,
    agent_id: AgentId,
    explicit_provider: Option<&ProviderId>,
    explicit_session: Option<&ProviderSessionId>,
) -> Result<(ProviderId, ProviderSessionId), CliError> {
    match (explicit_provider, explicit_session) {
        (Some(provider), Some(session)) => return Ok((provider.clone(), session.clone())),
        (None, None) => {}
        _ => return Err(CliError::AgentState),
    }
    if let Some((provider, session)) = environment_session_identity()? {
        return Ok((
            ProviderId::new(provider).map_err(|_| CliError::AgentState)?,
            ProviderSessionId::new(session).map_err(|_| CliError::AgentState)?,
        ));
    }
    let (provider, session) = snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::AgentSelection {
                agent_id: candidate,
                provider: Some(provider),
                session: Some(session),
                conflicted: false,
                ..
            } if candidate.bytes() == *agent_id.as_bytes() => Some((provider, session)),
            _ => None,
        })
        .ok_or(CliError::AgentState)?;
    Ok((
        ProviderId::new(provider.clone()).map_err(|_| CliError::AgentState)?,
        ProviderSessionId::new(session.clone()).map_err(|_| CliError::AgentState)?,
    ))
}

fn session_binding_fact(
    snapshot: &AuthoritativeSnapshotDto,
    mailbox: MailboxAddress,
    provider: &ProviderId,
    session: &ProviderSessionId,
) -> Result<FactId, CliError> {
    let matches = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::AgentSession {
            provider: candidate_provider,
            session: candidate_session,
            bindings,
            mailbox_installation: Some(installation),
            mailbox_id: Some(mailbox_id),
            conflicted: false,
        } if candidate_provider == provider.as_str()
            && candidate_session == session.as_str()
            && installation.bytes() == *mailbox.installation_id().as_bytes()
            && mailbox_id.bytes() == *mailbox.mailbox_id().as_bytes() =>
        {
            bindings
                .iter()
                .find(|binding| {
                    binding.mailbox.installation_id.bytes() == *mailbox.installation_id().as_bytes()
                        && binding.mailbox.mailbox_id.bytes() == *mailbox.mailbox_id().as_bytes()
                })
                .map(|binding| &binding.fact_id)
        }
        _ => None,
    });
    let values = matches.collect::<Vec<_>>();
    match values.as_slice() {
        [fact] => Ok(FactId::from_bytes(fact.bytes())),
        [] | [_, _, ..] => Err(CliError::AgentState),
    }
}

fn session_context(
    snapshot: &AuthoritativeSnapshotDto,
    mailbox: MailboxAddress,
    directory: &Path,
) -> Result<(FactId, hq_domain::RepositoryContext), CliError> {
    let requested = directory.to_string_lossy();
    let matches = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::AgentContext {
                mailbox_installation,
                mailbox_id,
                history,
                ..
            } if mailbox_installation.bytes() == *mailbox.installation_id().as_bytes()
                && mailbox_id.bytes() == *mailbox.mailbox_id().as_bytes() =>
            {
                Some(history)
            }
            _ => None,
        })
        .flatten()
        .filter(|context| context.directory.value == requested.as_ref())
        .map(|context| {
            Ok((
                FactId::from_bytes(context.fact_id.bytes()),
                repository_context_from_v1(context)?,
            ))
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    let Some((first_fact, first_context)) = matches.first() else {
        return Err(CliError::AgentState);
    };
    if matches.iter().any(|(_, context)| context != first_context) {
        return Err(CliError::AgentState);
    }
    Ok((*first_fact, first_context.clone()))
}

fn repository_context_from_v1(
    context: &hq_local_api::protocol::v1::RepositoryContextDto,
) -> Result<hq_domain::RepositoryContext, CliError> {
    Ok(hq_domain::RepositoryContext {
        directory: locator_from_v1(&context.directory)?,
        repository: context
            .repository
            .as_ref()
            .map(locator_from_v1)
            .transpose()?,
        worktree: context.worktree.as_ref().map(locator_from_v1).transpose()?,
        branch: context
            .branch
            .as_ref()
            .map(|branch| ShortText::new(branch.clone()).map_err(|_| CliError::AgentState))
            .transpose()?,
    })
}

fn locator_from_v1(locator: &ResourceLocatorDto) -> Result<ResourceLocator, CliError> {
    Ok(ResourceLocator::new(
        match locator.scheme {
            ResourceSchemeDto::GitRepository => ResourceScheme::GitRepository,
            ResourceSchemeDto::WorkingTree => ResourceScheme::WorkingTree,
            ResourceSchemeDto::Container => ResourceScheme::Container,
            ResourceSchemeDto::Opaque => ResourceScheme::Opaque,
        },
        BoundedText::new(locator.value.clone()).map_err(|_| CliError::AgentState)?,
    ))
}

fn agent_selection_frontier(
    snapshot: &AuthoritativeSnapshotDto,
    agent_id: AgentId,
) -> Result<BTreeSet<FactId>, CliError> {
    let matches = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::AgentSelection {
            agent_id: candidate,
            frontier,
            ..
        } if candidate.bytes() == *agent_id.as_bytes() => Some(frontier),
        _ => None,
    });
    let values = matches.collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(BTreeSet::new()),
        [frontier] => Ok(frontier
            .iter()
            .map(|fact| FactId::from_bytes(fact.bytes()))
            .collect()),
        [_, _, ..] => Err(CliError::AgentState),
    }
}

fn agent_rename_frontier(
    snapshot: &AuthoritativeSnapshotDto,
    agent_id: AgentId,
    provider: &ProviderId,
    session: &ProviderSessionId,
) -> Result<BTreeSet<FactId>, CliError> {
    let matches = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::AgentSessionName {
            agent_id: candidate,
            provider: candidate_provider,
            session: candidate_session,
            frontier,
            ..
        } if candidate.bytes() == *agent_id.as_bytes()
            && candidate_provider == provider.as_str()
            && candidate_session == session.as_str() =>
        {
            Some(frontier)
        }
        _ => None,
    });
    let values = matches.collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(BTreeSet::new()),
        [frontier] => Ok(frontier
            .iter()
            .map(|fact| FactId::from_bytes(fact.bytes()))
            .collect()),
        [_, _, ..] => Err(CliError::AgentState),
    }
}

fn submit_agent_plan(
    client: &mut LocalNodeClient,
    plan: hq_application::FactPlan,
) -> Result<(), CliError> {
    let request =
        MutationRequest::from_plan(random_command_id()?, plan).map_err(|_| CliError::AgentState)?;
    match client.mutation(request)? {
        ClientEvent::Mutation(MutationAttemptDto::Completed {
            outcome: MutationOutcomeDto::Committed,
            ..
        }) => Ok(()),
        _ => Err(CliError::AgentState),
    }
}

fn resolve_current_session(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Result<(String, String, MailboxAddress, Option<AgentId>), CliError> {
    let (provider, session) = environment_session_identity()?.ok_or(CliError::AgentState)?;
    let matches = direct_session_candidates(snapshot, local)
        .into_iter()
        .filter(|candidate| {
            candidate.provider == provider && candidate.session == session && !candidate.conflicted
        })
        .collect::<Vec<_>>();
    let [candidate] = matches.as_slice() else {
        return Err(CliError::AgentState);
    };
    Ok((provider, session, candidate.mailbox, candidate.named_agent))
}

pub(crate) fn named_agent_catalog_view(
    snapshot: &AuthoritativeSnapshotDto,
    operation: &'static str,
    selected_agent: Option<AgentId>,
    current: Option<(String, String, MailboxAddress, Option<AgentId>)>,
) -> NamedAgentCatalogView {
    let mut agents = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Agent {
                agent_id,
                names,
                mailboxes,
                lifecycle,
                runnable,
                ..
            } => {
                let agent_id = AgentId::from_bytes(agent_id.bytes());
                selected_agent
                    .is_none_or(|selected| selected == agent_id)
                    .then(|| NamedAgentView {
                        agent_id,
                        names: names.clone(),
                        mailboxes: mailboxes
                            .iter()
                            .map(|mailbox| {
                                MailboxAddress::new(
                                    InstallationId::from_bytes(mailbox.installation_id.bytes()),
                                    MailboxId::from_bytes(mailbox.mailbox_id.bytes()),
                                )
                            })
                            .collect(),
                        lifecycle: lifecycle.clone(),
                        runnable: *runnable,
                        sessions: named_agent_session_views(snapshot, agent_id, mailboxes),
                    })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    agents.sort_by(|left, right| (&left.names, left.agent_id).cmp(&(&right.names, right.agent_id)));
    NamedAgentCatalogView {
        operation,
        agents,
        current,
    }
}

fn named_agent_session_views(
    snapshot: &AuthoritativeSnapshotDto,
    agent_id: AgentId,
    mailboxes: &[hq_local_api::protocol::v1::MailboxAddressDto],
) -> Vec<NamedAgentSessionView> {
    let selected = snapshot.items.iter().find_map(|item| match item {
        SnapshotItem::AgentSelection {
            agent_id: candidate,
            provider,
            session,
            conflicted: false,
            ..
        } if candidate.bytes() == *agent_id.as_bytes() => provider.as_ref().zip(session.as_ref()),
        _ => None,
    });
    let mut sessions = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::AgentSession {
                provider,
                session,
                bindings,
                mailbox_installation,
                mailbox_id,
                conflicted,
            } => {
                let mailbox =
                    mailbox_installation
                        .zip(*mailbox_id)
                        .map(|(installation, mailbox)| {
                            MailboxAddress::new(
                                InstallationId::from_bytes(installation.bytes()),
                                MailboxId::from_bytes(mailbox.bytes()),
                            )
                        });
                let compatible = bindings.iter().any(|binding| {
                    mailboxes.iter().any(|candidate| {
                        candidate.installation_id == binding.mailbox.installation_id
                            && candidate.mailbox_id == binding.mailbox.mailbox_id
                    })
                });
                compatible.then(|| {
                    let display = snapshot.items.iter().find_map(|item| match item {
                        SnapshotItem::AgentSessionName {
                            agent_id: candidate,
                            provider: candidate_provider,
                            session: candidate_session,
                            resolved,
                            display_name,
                            ..
                        } if candidate.bytes() == *agent_id.as_bytes()
                            && candidate_provider == provider
                            && candidate_session == session =>
                        {
                            Some((*resolved, display_name.clone()))
                        }
                        _ => None,
                    });
                    NamedAgentSessionView {
                        provider: provider.clone(),
                        session: session.clone(),
                        mailbox,
                        conflicted: *conflicted,
                        selected: selected.is_some_and(|(selected_provider, selected_session)| {
                            selected_provider == provider && selected_session == session
                        }),
                        name_resolved: display.as_ref().is_none_or(|(resolved, _)| *resolved),
                        display_name: display.and_then(|(_, name)| name),
                    }
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        (&left.provider, &left.session).cmp(&(&right.provider, &right.session))
    });
    sessions
}

fn run_agent_message(
    action: &AgentMessageCommand,
    state: &StatePaths,
    input: &mut dyn Read,
) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let local = client.installation_id();
    let snapshot = messaging_snapshot(&mut client)?;
    let selection = match action {
        AgentMessageCommand::Ask { mailbox, .. }
        | AgentMessageCommand::Send { mailbox, .. }
        | AgentMessageCommand::Wait { mailbox, .. }
        | AgentMessageCommand::Poll { mailbox } => mailbox,
    };
    let mailbox = resolve_agent_mailbox(&snapshot, local, selection)?;
    match action {
        AgentMessageCommand::Ask {
            body,
            timeout,
            interval,
            ..
        } => {
            let body = message_body(body.as_ref(), input)?;
            let message_id = random_message_id()?;
            let plan = plan_question(
                message_authority(&snapshot, local, mailbox)?,
                stable_inputs(),
                NewMessageRequest {
                    message_id,
                    recipient: Some(human_mailbox(&snapshot, local)?.0),
                    body,
                    presentation: PresentationKind::Message,
                    project_id: None,
                },
            )?;
            submit_message_plan(&mut client, plan)?;
            let answer = wait_for_answer(&mut client, mailbox, message_id, *timeout, *interval)?;
            Ok(agent_message_result(
                "ask",
                mailbox,
                Some(message_id),
                vec![answer],
                &snapshot,
            ))
        }
        AgentMessageCommand::Send { body, .. } => {
            let body = message_body(body.as_ref(), input)?;
            let message_id = random_message_id()?;
            let plan = plan_asynchronous_message(
                message_authority(&snapshot, local, mailbox)?,
                stable_inputs(),
                NewMessageRequest {
                    message_id,
                    recipient: Some(human_mailbox(&snapshot, local)?.0),
                    body,
                    presentation: PresentationKind::Message,
                    project_id: None,
                },
            )?;
            submit_message_plan(&mut client, plan)?;
            Ok(agent_message_result(
                "send",
                mailbox,
                Some(message_id),
                Vec::new(),
                &snapshot,
            ))
        }
        AgentMessageCommand::Wait {
            message_id,
            timeout,
            interval,
            ..
        } => {
            let answer = wait_for_answer(&mut client, mailbox, *message_id, *timeout, *interval)?;
            Ok(agent_message_result(
                "wait",
                mailbox,
                Some(*message_id),
                vec![answer],
                &snapshot,
            ))
        }
        AgentMessageCommand::Poll { .. } => {
            let snapshot = messaging_snapshot(&mut client)?;
            let messages = load_all_messages(&mut client, &snapshot)?
                .into_iter()
                .filter(|message| {
                    (message.open || message.incomplete)
                        && message.recipient == Some(mailbox)
                        && (message.incomplete
                            || message.ready_answer
                            || message.purpose == MessagePurposeDto::Asynchronous)
                })
                .collect();
            Ok(agent_message_result(
                "poll", mailbox, None, messages, &snapshot,
            ))
        }
    }
}

fn agent_message_result(
    operation: &'static str,
    mailbox: MailboxAddress,
    root_message: Option<MessageId>,
    messages: Vec<CliMessageView>,
    snapshot: &AuthoritativeSnapshotDto,
) -> CliResult {
    CliResult::Messages(Box::new(MessageCommandView {
        operation,
        mailbox: Some(mailbox),
        root_message,
        project_id: None,
        messages,
        incomplete_truncated: snapshot_has_incomplete_truncation(snapshot),
    }))
}

fn run_get_message(message_id: MessageId, state: &StatePaths) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let snapshot = messaging_snapshot(&mut client)?;
    let message = exact_message(load_all_messages(&mut client, &snapshot)?, message_id)?;
    Ok(CliResult::Messages(Box::new(MessageCommandView {
        operation: "get",
        mailbox: None,
        root_message: Some(message_id),
        project_id: None,
        messages: vec![message],
        incomplete_truncated: snapshot_has_incomplete_truncation(&snapshot),
    })))
}

fn run_mailbox_discovery(
    directory: Option<&Path>,
    state: &StatePaths,
) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let local = client.installation_id();
    let snapshot = messaging_snapshot(&mut client)?;
    let directory = discovery_directory(directory)?;
    Ok(CliResult::MailboxDiscovery(Box::new(
        mailbox_discovery_view(&snapshot, local, directory)?,
    )))
}

fn run_human_message(
    action: &HumanMessageCommand,
    state: &StatePaths,
    input: &mut dyn Read,
) -> Result<CliResult, CliError> {
    let mut client = command_client(state)?;
    let local = client.installation_id();
    let snapshot = messaging_snapshot(&mut client)?;
    let (human, _) = human_mailbox(&snapshot, local)?;
    let all = load_all_messages(&mut client, &snapshot)?;
    match action {
        HumanMessageCommand::List(filters) => {
            let messages = filter_human_messages(all, filters);
            Ok(human_message_result(
                "list", human, None, messages, &snapshot,
            ))
        }
        HumanMessageCommand::Answer { message_id, body } => {
            let _root = answerable_human_question(all, *message_id, human)?;
            let answer_id = random_message_id()?;
            let content = message_body(body.as_ref(), input)?.into_string();
            submit_mailbox_command(
                &mut client,
                MailboxCommandActionDto::Reply {
                    target_message: Id32::new(*message_id.as_bytes()),
                    message_id: Id32::new(*answer_id.as_bytes()),
                },
                Some(content),
            )?;
            Ok(human_message_result(
                "answer",
                human,
                Some(*message_id),
                Vec::new(),
                &snapshot,
            ))
        }
        HumanMessageCommand::Cancel { message_id } => {
            let root = exact_message(all, *message_id)?;
            if root.incomplete {
                return Err(CliError::MessagingState);
            }
            let plan = plan_thread_cancellation(
                message_authority(&snapshot, local, human)?,
                stable_inputs(),
                ThreadCancellationRequest {
                    thread_id: root.thread_id,
                    root_fact: root.fact_id,
                    root: message_content(&root)?,
                    root_scope: FactScope::InstallationPrivate(local),
                    reason: None,
                },
            )?;
            submit_message_plan(&mut client, plan)?;
            Ok(human_message_result(
                "cancel",
                human,
                Some(*message_id),
                Vec::new(),
                &snapshot,
            ))
        }
        HumanMessageCommand::Archive { message_id }
        | HumanMessageCommand::Restore { message_id } => {
            let target = exact_message(all, *message_id)?;
            if target.incomplete {
                return Err(CliError::MessagingState);
            }
            let (operation, command) = if matches!(action, HumanMessageCommand::Archive { .. }) {
                (
                    "archive",
                    MailboxCommandActionDto::Archive {
                        target_message: Id32::new(*message_id.as_bytes()),
                    },
                )
            } else {
                (
                    "restore",
                    MailboxCommandActionDto::Restore {
                        target_message: Id32::new(*message_id.as_bytes()),
                    },
                )
            };
            submit_mailbox_command(&mut client, command, None)?;
            Ok(human_message_result(
                operation,
                human,
                Some(*message_id),
                Vec::new(),
                &snapshot,
            ))
        }
    }
}

fn answerable_human_question(
    messages: Vec<CliMessageView>,
    message_id: MessageId,
    human: MailboxAddress,
) -> Result<CliMessageView, CliError> {
    let root = exact_message(messages, message_id)?;
    if root.incomplete
        || root.fact_id != root.root_fact.unwrap_or(root.fact_id)
        || root.purpose != MessagePurposeDto::Question
        || root.recipient != Some(human)
        || root.thread_cancelled
    {
        return Err(CliError::MessagingState);
    }
    Ok(root)
}

fn filter_human_messages(
    messages: Vec<CliMessageView>,
    filters: &HumanMessageFilters,
) -> Vec<CliMessageView> {
    messages
        .into_iter()
        .filter(|message| {
            filters
                .sender
                .is_none_or(|sender| message.sender.mailbox_id() == sender)
                && filters.recipient.is_none_or(|recipient| {
                    message
                        .recipient
                        .is_some_and(|address| address.mailbox_id() == recipient)
                })
                && (message.incomplete
                    || filters.all
                    || if filters.archived {
                        !message.open
                    } else {
                        message.open
                    })
        })
        .take(usize::from(filters.limit))
        .collect()
}

fn human_message_result(
    operation: &'static str,
    human: MailboxAddress,
    root_message: Option<MessageId>,
    messages: Vec<CliMessageView>,
    snapshot: &AuthoritativeSnapshotDto,
) -> CliResult {
    CliResult::Messages(Box::new(MessageCommandView {
        operation,
        mailbox: Some(human),
        root_message,
        project_id: None,
        messages,
        incomplete_truncated: snapshot_has_incomplete_truncation(snapshot),
    }))
}

fn resolve_agent_mailbox(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    selection: &AgentMailboxSelection,
) -> Result<MailboxAddress, CliError> {
    let explicit = match (&selection.provider, &selection.session) {
        (Some(provider), Some(session)) => {
            Some((provider.as_str().to_owned(), session.as_str().to_owned()))
        }
        (None, None) => None,
        _ => return Err(CliError::MessagingState),
    };
    let identity = match explicit {
        Some(identity) => Some(identity),
        None => environment_session_identity()?,
    };
    if let Some((provider, session)) = identity {
        let matches = direct_session_candidates(snapshot, local)
            .into_iter()
            .filter(|candidate| candidate.provider == provider && candidate.session == session)
            .collect::<Vec<_>>();
        let [candidate] = matches.as_slice() else {
            return Err(CliError::MessagingState);
        };
        return (!candidate.conflicted)
            .then_some(candidate.mailbox)
            .ok_or(CliError::MessagingState);
    }
    let directory = discovery_directory(selection.directory.as_deref())?;
    let matches = mailbox_discovery_view(snapshot, local, directory)?
        .candidates
        .into_iter()
        .filter(|candidate| candidate.directory_match && !candidate.conflicted)
        .collect::<Vec<_>>();
    let [candidate] = matches.as_slice() else {
        return Err(CliError::MessagingState);
    };
    Ok(candidate.mailbox)
}

fn environment_session_identity() -> Result<Option<(String, String)>, CliError> {
    let builtins = [
        ("codex", "CODEX_THREAD_ID"),
        ("claude", "CLAUDE_CODE_SESSION_ID"),
        ("pi", "PI_SESSION_ID"),
    ]
    .into_iter()
    .filter_map(|(provider, variable)| {
        std::env::var(variable)
            .ok()
            .filter(|session| !session.is_empty())
            .map(|session| (provider.to_owned(), session))
    })
    .collect::<Vec<_>>();
    resolve_environment_session(
        builtins,
        std::env::var("HQ_PROVIDER").ok(),
        std::env::var("HQ_SESSION").ok(),
    )
}

fn resolve_environment_session(
    mut builtins: Vec<(String, String)>,
    custom_provider: Option<String>,
    custom_session: Option<String>,
) -> Result<Option<(String, String)>, CliError> {
    match (custom_provider, custom_session) {
        (None, None) => {}
        (Some(provider), Some(session)) if !provider.is_empty() && !session.is_empty() => {
            ProviderId::new(provider.clone()).map_err(|_| CliError::MessagingState)?;
            ProviderSessionId::new(session.clone()).map_err(|_| CliError::MessagingState)?;
            builtins.push((provider, session));
        }
        _ => return Err(CliError::MessagingState),
    }
    match builtins.as_slice() {
        [] => Ok(None),
        [identity] => Ok(Some(identity.clone())),
        [_, _, ..] => Err(CliError::MessagingState),
    }
}

fn direct_session_candidates(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Vec<MailboxDiscoveryCandidate> {
    let contexts = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::AgentContext {
                mailbox_installation,
                mailbox_id,
                history,
                ..
            } if mailbox_installation.bytes() == *local.as_bytes() => Some((
                mailbox_id.bytes(),
                (
                    history
                        .iter()
                        .map(|context| context.directory.value.clone())
                        .collect::<Vec<_>>(),
                    history
                        .iter()
                        .filter_map(|context| {
                            context.repository.as_ref().map(|value| value.value.clone())
                        })
                        .collect::<Vec<_>>(),
                    history
                        .iter()
                        .filter_map(|context| {
                            context.worktree.as_ref().map(|value| value.value.clone())
                        })
                        .collect::<Vec<_>>(),
                    history
                        .iter()
                        .filter_map(|context| context.branch.clone())
                        .collect::<Vec<_>>(),
                ),
            )),
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut candidates = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::AgentDirectSession {
                provider,
                session,
                mailbox_installation,
                mailbox_id,
                named_agent,
                conflicted,
            } if mailbox_installation.bytes() == *local.as_bytes() => {
                let mailbox = MailboxAddress::new(local, MailboxId::from_bytes(mailbox_id.bytes()));
                Some(MailboxDiscoveryCandidate {
                    provider: provider.clone(),
                    session: session.clone(),
                    mailbox,
                    named_agent: named_agent.map(|agent| AgentId::from_bytes(agent.bytes())),
                    conflicted: *conflicted,
                    directory_match: false,
                    directories: contexts
                        .get(&mailbox_id.bytes())
                        .map(|context| context.0.clone())
                        .unwrap_or_default(),
                    repositories: contexts
                        .get(&mailbox_id.bytes())
                        .map(|context| context.1.clone())
                        .unwrap_or_default(),
                    worktrees: contexts
                        .get(&mailbox_id.bytes())
                        .map(|context| context.2.clone())
                        .unwrap_or_default(),
                    branches: contexts
                        .get(&mailbox_id.bytes())
                        .map(|context| context.3.clone())
                        .unwrap_or_default(),
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (&left.provider, &left.session, left.mailbox).cmp(&(
            &right.provider,
            &right.session,
            right.mailbox,
        ))
    });
    candidates
}

fn mailbox_discovery_view(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    directory: PathBuf,
) -> Result<MailboxDiscoveryView, CliError> {
    let requested = crate::ProjectResourceAdapter::system(local)
        .repository_context(directory)
        .map_err(|_| CliError::MessagingState)?;
    let mut candidates = direct_session_candidates(snapshot, local);
    for candidate in &mut candidates {
        candidate.directory_match = candidate
            .directories
            .iter()
            .any(|value| value == requested.directory.value())
            || requested.repository.as_ref().is_some_and(|repository| {
                candidate
                    .repositories
                    .iter()
                    .any(|value| value == repository.value())
            })
            || requested.worktree.as_ref().is_some_and(|worktree| {
                candidate
                    .worktrees
                    .iter()
                    .any(|value| value == worktree.value())
            });
    }
    Ok(MailboxDiscoveryView {
        directory: PathBuf::from(requested.directory.value()),
        candidates,
    })
}

fn discovery_directory(directory: Option<&Path>) -> Result<PathBuf, CliError> {
    let directory = directory
        .map_or_else(std::env::current_dir, |path| Ok(path.to_path_buf()))
        .map_err(|_| CliError::MessagingState)?;
    Ok(directory)
}

fn human_mailbox(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Result<(MailboxAddress, FactId), CliError> {
    local_mailbox(snapshot, local, crate::foreground::reserved_human_mailbox())
        .map_err(|_| CliError::MessagingState)
}

fn message_authority(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    sender: MailboxAddress,
) -> Result<MessageAuthoringAuthority, CliError> {
    let installation = local_authority(snapshot, local).map_err(|_| CliError::MessagingState)?;
    let (_, mailbox_fact) = local_mailbox(snapshot, local, sender.mailbox_id())
        .map_err(|_| CliError::MessagingState)?;
    Ok(MessageAuthoringAuthority {
        author: local,
        sender,
        scope: FactScope::InstallationPrivate(local),
        authority: AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation.root_fact,
        ),
        support: [installation.root_fact, mailbox_fact].into_iter().collect(),
    })
}

fn account_message_authority(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    account_id: AccountId,
    sender: MailboxAddress,
) -> Result<MessageAuthoringAuthority, CliError> {
    if local_selection(snapshot, local)
        .map_err(|_| CliError::ProjectState)?
        .active
        != Some(account_id)
    {
        return Err(CliError::ProjectState);
    }
    let (account_root, creator, _, _) =
        account_item(snapshot, account_id).ok_or(CliError::ProjectState)?;
    let membership_fact = if creator == local {
        account_root
    } else {
        let membership = membership_record(snapshot, account_id, local)?
            .filter(|membership| membership.state == "active")
            .ok_or(CliError::ProjectState)?;
        match membership
            .active_acceptances
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .as_slice()
        {
            [fact] => *fact,
            _ => return Err(CliError::ProjectState),
        }
    };
    let (_, mailbox_fact) =
        local_mailbox(snapshot, local, sender.mailbox_id()).map_err(|_| CliError::ProjectState)?;
    Ok(MessageAuthoringAuthority {
        author: local,
        sender,
        scope: FactScope::AccountAddressed(account_id),
        authority: AuthorityReference::new(AuthorityRole::AccountMembership, membership_fact),
        support: [membership_fact, mailbox_fact].into_iter().collect(),
    })
}

fn message_body(
    argument: Option<&hq_domain::ContentText>,
    input: &mut dyn Read,
) -> Result<hq_domain::ContentText, CliError> {
    if let Some(argument) = argument {
        return Ok(argument.clone());
    }
    let mut bytes = Vec::new();
    input
        .take(u64::try_from(hq_domain::CONTENT_MAX_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::Arguments)?;
    if bytes.len() > hq_domain::CONTENT_MAX_BYTES {
        return Err(CliError::Arguments);
    }
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        let _ = bytes.pop();
    }
    let content = String::from_utf8(bytes).map_err(|_| CliError::Arguments)?;
    if content.is_empty() {
        return Err(CliError::Arguments);
    }
    hq_domain::ContentText::new(content).map_err(|_| CliError::Arguments)
}

fn submit_message_plan(
    client: &mut LocalNodeClient,
    plan: hq_application::FactPlan,
) -> Result<(), CliError> {
    let request = MutationRequest::from_plan(random_command_id()?, plan)
        .map_err(|_| CliError::MessagingState)?;
    match client.mutation(request)? {
        ClientEvent::Mutation(MutationAttemptDto::Completed {
            outcome: MutationOutcomeDto::Committed,
            ..
        }) => Ok(()),
        _ => Err(CliError::MessagingState),
    }
}

fn submit_mailbox_command(
    client: &mut LocalNodeClient,
    action: MailboxCommandActionDto,
    content: Option<String>,
) -> Result<(), CliError> {
    let inputs = stable_inputs();
    let command_id = random_command_id()?;
    let request = MailboxCommandRequestDto::new(
        Id32::new(*command_id.as_bytes()),
        None,
        action,
        content,
        inputs.authored_at.as_unix_millis(),
        inputs.auxiliary_randomness,
    );
    match client.mailbox_command(request)? {
        ClientEvent::Mutation(MutationAttemptDto::Completed {
            outcome: MutationOutcomeDto::Committed,
            ..
        }) => Ok(()),
        _ => Err(CliError::MessagingState),
    }
}

fn random_message_id() -> Result<MessageId, CliError> {
    random_command_id().map(|identity| MessageId::from_bytes(*identity.as_bytes()))
}

fn wait_for_answer(
    client: &mut LocalNodeClient,
    sender: MailboxAddress,
    root_message: MessageId,
    timeout: Option<Duration>,
    interval: Duration,
) -> Result<CliMessageView, CliError> {
    let started = Instant::now();
    loop {
        let snapshot = messaging_snapshot(client)?;
        let messages = load_all_messages(client, &snapshot)?;
        let root = exact_message(messages.clone(), root_message)?;
        if root.incomplete || root.sender != sender || root.purpose != MessagePurposeDto::Question {
            return Err(CliError::MessagingState);
        }
        if let Some(answer) = messages
            .into_iter()
            .find(|message| message.root_message == Some(root_message) && message.ready_answer)
        {
            return Ok(answer);
        }
        if root.thread_cancelled || timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
            return Err(CliError::MessagingState);
        }
        std::thread::sleep(interval);
    }
}

fn load_all_messages(
    client: &mut LocalNodeClient,
    snapshot: &AuthoritativeSnapshotDto,
) -> Result<Vec<CliMessageView>, CliError> {
    let keys = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Conversation { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut messages = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::IncompleteMessage { .. } => Some(incomplete_message_from_snapshot(item)),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    for key in keys {
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        loop {
            let request = ConversationPageRequest::new(key.clone(), 200, cursor.clone())
                .map_err(|_| CliError::MessagingState)?;
            let ClientEvent::Response {
                result: ResponseResult::ConversationPage(page),
                ..
            } = messaging_request(client, &Request::ConversationPage(request))?
            else {
                return Err(CliError::MessagingState);
            };
            for item in page.items {
                if let ConversationEntryDto::Message(_) = item {
                    messages.push(message_from_dto(item)?);
                }
            }
            let Some(next) = page.next_cursor else {
                break;
            };
            if !seen.insert(next.clone()) {
                return Err(CliError::MessagingState);
            }
            cursor = Some(next);
        }
    }
    Ok(messages)
}

fn snapshot_has_incomplete_truncation(snapshot: &AuthoritativeSnapshotDto) -> bool {
    snapshot
        .items
        .iter()
        .any(|item| matches!(item, SnapshotItem::IncompleteMessagesTruncated))
}

fn messaging_snapshot(client: &mut LocalNodeClient) -> Result<AuthoritativeSnapshotDto, CliError> {
    for _ in 0..3 {
        if let Ok(snapshot) = client.snapshot() {
            return Ok(snapshot);
        }
    }
    Err(CliError::MessagingState)
}

fn messaging_request(
    client: &mut LocalNodeClient,
    request: &Request,
) -> Result<ClientEvent, CliError> {
    for _ in 0..3 {
        if let Ok(event) = client.request(request.clone()) {
            return Ok(event);
        }
    }
    Err(CliError::MessagingState)
}

fn message_from_dto(item: ConversationEntryDto) -> Result<CliMessageView, CliError> {
    let ConversationEntryDto::Message(message) = item else {
        return Err(CliError::MessagingState);
    };
    let ConversationMessageDto {
        fact_id,
        message_id,
        thread_id,
        content,
        sender_installation,
        sender_mailbox,
        recipient_installation,
        recipient_mailbox,
        purpose,
        presentation,
        correlation_provider,
        correlation_session,
        correlation_operation,
        project_id,
        open,
        rejected,
        state_frontier,
        root_fact,
        root_message,
        ready_answer,
        thread_cancelled,
        ..
    } = *message;
    if recipient_installation.is_some() != recipient_mailbox.is_some() {
        return Err(CliError::MessagingState);
    }
    Ok(CliMessageView {
        fact_id: FactId::from_bytes(fact_id.bytes()),
        message_id: MessageId::from_bytes(message_id.bytes()),
        thread_id: ThreadId::from_bytes(thread_id.bytes()),
        sender: MailboxAddress::new(
            InstallationId::from_bytes(sender_installation.bytes()),
            MailboxId::from_bytes(sender_mailbox.bytes()),
        ),
        recipient: recipient_installation
            .zip(recipient_mailbox)
            .map(|(installation, mailbox)| {
                MailboxAddress::new(
                    InstallationId::from_bytes(installation.bytes()),
                    MailboxId::from_bytes(mailbox.bytes()),
                )
            }),
        content,
        purpose,
        presentation,
        correlation: correlation_from_dto(
            correlation_provider,
            correlation_session,
            correlation_operation,
        )?,
        project_id: project_id.map(|project| ProjectId::from_bytes(project.bytes())),
        open,
        rejected,
        state_frontier: state_frontier
            .into_iter()
            .map(|fact_id| FactId::from_bytes(fact_id.bytes()))
            .collect(),
        root_fact: root_fact.map(|fact_id| FactId::from_bytes(fact_id.bytes())),
        root_message: root_message.map(|message_id| MessageId::from_bytes(message_id.bytes())),
        ready_answer,
        thread_cancelled,
        incomplete: false,
        missing_dependencies: BTreeSet::new(),
        unusable_dependencies: BTreeSet::new(),
    })
}

fn incomplete_message_from_snapshot(item: &SnapshotItem) -> Result<CliMessageView, CliError> {
    let SnapshotItem::IncompleteMessage {
        fact_id,
        message_id,
        thread_id,
        sender_installation,
        sender_mailbox,
        recipient_installation,
        recipient_mailbox,
        content,
        purpose,
        presentation,
        correlation_provider,
        correlation_session,
        correlation_operation,
        project_id,
        missing_dependencies,
        unusable_dependencies,
        ..
    } = item
    else {
        return Err(CliError::MessagingState);
    };
    if recipient_installation.is_some() != recipient_mailbox.is_some() {
        return Err(CliError::MessagingState);
    }
    Ok(CliMessageView {
        fact_id: FactId::from_bytes(fact_id.bytes()),
        message_id: MessageId::from_bytes(message_id.bytes()),
        thread_id: ThreadId::from_bytes(thread_id.bytes()),
        sender: MailboxAddress::new(
            InstallationId::from_bytes(sender_installation.bytes()),
            MailboxId::from_bytes(sender_mailbox.bytes()),
        ),
        recipient: recipient_installation
            .zip(*recipient_mailbox)
            .map(|(installation, mailbox)| {
                MailboxAddress::new(
                    InstallationId::from_bytes(installation.bytes()),
                    MailboxId::from_bytes(mailbox.bytes()),
                )
            }),
        content: content.clone(),
        purpose: *purpose,
        presentation: *presentation,
        correlation: correlation_from_dto(
            correlation_provider.clone(),
            correlation_session.clone(),
            *correlation_operation,
        )?,
        project_id: project_id.map(|project| ProjectId::from_bytes(project.bytes())),
        open: false,
        rejected: false,
        state_frontier: BTreeSet::new(),
        root_fact: None,
        root_message: None,
        ready_answer: false,
        thread_cancelled: false,
        incomplete: true,
        missing_dependencies: missing_dependencies
            .iter()
            .map(|dependency| FactId::from_bytes(dependency.bytes()))
            .collect(),
        unusable_dependencies: unusable_dependencies
            .iter()
            .map(|dependency| FactId::from_bytes(dependency.bytes()))
            .collect(),
    })
}

fn exact_message(
    messages: Vec<CliMessageView>,
    message_id: MessageId,
) -> Result<CliMessageView, CliError> {
    let matches = messages
        .into_iter()
        .filter(|message| message.message_id == message_id)
        .collect::<Vec<_>>();
    let [message] = matches.as_slice() else {
        return Err(CliError::MessagingState);
    };
    Ok(message.clone())
}

fn message_content(message: &CliMessageView) -> Result<hq_domain::MessageContent, CliError> {
    let correlation = match &message.correlation {
        None => None,
        Some((provider, session, operation)) => Some(OperationCorrelation::new(
            ProviderId::new(provider.clone()).map_err(|_| CliError::MessagingState)?,
            ProviderSessionId::new(session.clone()).map_err(|_| CliError::MessagingState)?,
            OperationId::from_bytes(*operation),
        )),
    };
    Ok(hq_domain::MessageContent {
        message_id: message.message_id,
        sender: message.sender,
        recipient: message.recipient,
        body: hq_domain::ContentText::new(message.content.clone())
            .map_err(|_| CliError::MessagingState)?,
        purpose: match message.purpose {
            MessagePurposeDto::Question => MessagePurpose::Question,
            MessagePurposeDto::Asynchronous => MessagePurpose::Asynchronous,
            MessagePurposeDto::ProjectOutput => MessagePurpose::ProjectOutput,
        },
        presentation: match message.presentation {
            PresentationKindDto::Message => PresentationKind::Message,
            PresentationKindDto::FinalAnswer => PresentationKind::FinalAnswer,
            PresentationKindDto::Status => PresentationKind::Status,
        },
        correlation,
        project_id: message.project_id,
    })
}

fn correlation_from_dto(
    provider: Option<String>,
    session: Option<String>,
    operation: Option<Id32>,
) -> Result<Option<(String, String, [u8; 32])>, CliError> {
    match (provider, session, operation) {
        (None, None, None) => Ok(None),
        (Some(provider), Some(session), Some(operation)) => {
            Ok(Some((provider, session, operation.bytes())))
        }
        _ => Err(CliError::MessagingState),
    }
}

fn relay_configuration(
    endpoint: &RelayEndpoint,
    access: RelayAccessDto,
    authentication: RelayAuthenticationDto,
    enabled: bool,
) -> Result<RelayConfigurationDto, CliError> {
    Ok(RelayConfigurationDto::new(
        relay_locator(endpoint)?,
        access,
        authentication,
        enabled,
    ))
}

fn relay_locator(endpoint: &RelayEndpoint) -> Result<ResourceLocatorDto, CliError> {
    ResourceLocatorDto::new(ResourceSchemeDto::Opaque, endpoint.as_str().to_owned())
        .map_err(|_| CliError::Arguments)
}

fn relay_status(client: &mut LocalNodeClient) -> Result<RelayStatusDto, CliError> {
    for _ in 0..2 {
        match client.request(Request::RelayStatus)? {
            ClientEvent::Response {
                result: ResponseResult::RelayStatus(status),
                ..
            } => return Ok(status),
            ClientEvent::RequestLost(_) => {}
            _ => return Err(CliError::RelayState),
        }
    }
    Err(CliError::RelayState)
}

fn state_health(client: &mut LocalNodeClient) -> Result<StateHealthDto, CliError> {
    for _ in 0..2 {
        match client.request(Request::StateHealth)? {
            ClientEvent::Response {
                result: ResponseResult::StateHealth(status),
                ..
            } => return Ok(status),
            ClientEvent::RequestLost(_) => {}
            _ => return Err(CliError::RelayState),
        }
    }
    Err(CliError::RelayState)
}

fn stable_repair_operation(revision: u64) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"hq-cli-repair-operation-v1\0");
    digest.update(revision.to_be_bytes());
    digest.finalize().into()
}

fn repair_state(client: &mut LocalNodeClient, operation_id: [u8; 32]) -> Result<(), CliError> {
    for _ in 0..2 {
        match client.request(Request::RepairState {
            operation_id: Id32::new(operation_id),
        })? {
            ClientEvent::Response {
                result: ResponseResult::StateRepair(report),
                ..
            } if report.operation_id.bytes() == operation_id => return Ok(()),
            ClientEvent::RequestLost(_) => {}
            _ => return Err(CliError::RelayState),
        }
    }
    Err(CliError::RelayState)
}

fn configure_relay(
    client: &mut LocalNodeClient,
    body: RelayConfigurationDto,
) -> Result<(String, Option<[u8; 32]>), CliError> {
    let prior = relay_status(client)?;
    if relay_policy_matches(&prior, &body) {
        return Ok(("unchanged".to_owned(), None));
    }
    let generation = prior
        .policies
        .iter()
        .find(|policy| policy.endpoint == body.endpoint)
        .map_or(0, |policy| policy.generation);
    let request = stable_relay_effect(b"configure", generation, body)?;
    for _ in 0..2 {
        match client.request(Request::ConfigureRelay(request.clone()))? {
            ClientEvent::Response {
                result: ResponseResult::EmptyEffect(outcome),
                ..
            } => return effect_outcome(&outcome, request.operation_id.bytes()),
            ClientEvent::RequestLost(_) => {
                if relay_policy_matches(&relay_status(client)?, &request.body) {
                    return Ok(("reconciled".to_owned(), Some(request.operation_id.bytes())));
                }
            }
            _ => return Err(CliError::RelayState),
        }
    }
    Err(CliError::RelayState)
}

fn synchronize_relay(
    client: &mut LocalNodeClient,
    body: SynchronizationRequestDto,
) -> Result<(String, Option<[u8; 32]>), CliError> {
    let request = stable_relay_effect(b"synchronize", 0, body)?;
    for _ in 0..2 {
        match client.request(Request::Synchronize(request.clone()))? {
            ClientEvent::Response {
                result: ResponseResult::EmptyEffect(outcome),
                ..
            } => return effect_outcome(&outcome, request.operation_id.bytes()),
            ClientEvent::RequestLost(_) => {}
            _ => return Err(CliError::RelayState),
        }
    }
    Err(CliError::RelayState)
}

fn stable_relay_effect<T>(
    domain: &[u8],
    generation: u64,
    body: T,
) -> Result<EffectRequestDto<T>, CliError>
where
    T: serde::Serialize,
{
    let body_bytes = serde_json::to_vec(&body).map_err(|_| CliError::Runtime)?;
    let mut operation = Sha256::new();
    operation.update(b"hq-cli-relay-operation-v1\0");
    operation.update(domain);
    operation.update(generation.to_be_bytes());
    operation.update(&body_bytes);
    let operation_id = Id32::new(operation.finalize().into());
    let mut request = Sha256::new();
    request.update(b"hq-cli-relay-request-v1\0");
    request.update(operation_id.bytes());
    request.update(0_i64.to_be_bytes());
    request.update(&body_bytes);
    Ok(EffectRequestDto::new(
        operation_id,
        Id32::new(request.finalize().into()),
        0,
        body,
    ))
}

fn relay_policy_matches(status: &RelayStatusDto, desired: &RelayConfigurationDto) -> bool {
    status.policies.iter().any(|policy| {
        policy.endpoint == desired.endpoint
            && policy.access == desired.access
            && policy.authentication == desired.authentication
            && policy.enabled == desired.enabled
    })
}

fn effect_outcome(
    outcome: &EffectOutcomeDto<()>,
    expected_operation_id: [u8; 32],
) -> Result<(String, Option<[u8; 32]>), CliError> {
    match outcome {
        EffectOutcomeDto::Accepted(()) => Ok(("accepted".to_owned(), Some(expected_operation_id))),
        EffectOutcomeDto::Rejected(_) => Ok(("rejected".to_owned(), Some(expected_operation_id))),
        EffectOutcomeDto::Uncertain(operation_id)
            if operation_id.bytes() == expected_operation_id =>
        {
            Ok(("uncertain".to_owned(), Some(expected_operation_id)))
        }
        EffectOutcomeDto::Uncertain(_) => Err(CliError::RelayState),
    }
}

fn relay_admin_view(
    operation: &'static str,
    outcome: Option<String>,
    operation_id: Option<[u8; 32]>,
    status: RelayStatusDto,
    health: StateHealthDto,
) -> RelayAdminView {
    RelayAdminView {
        operation,
        outcome,
        operation_id,
        revision: health.revision,
        domains: health
            .domains
            .into_iter()
            .map(|domain| DomainHealthView {
                domain: match domain.domain {
                    HealthDomainDto::Authority => "authority",
                    HealthDomainDto::Conversation => "conversation",
                    HealthDomainDto::Agent => "agent",
                    HealthDomainDto::Project => "project",
                }
                .to_owned(),
                projected: domain.projected,
                unresolved: domain.unresolved,
                unauthorized: domain.unauthorized,
                conflicted: domain.conflicted,
                invalid: domain.invalid,
                unsupported: domain.unsupported,
                conflicts: domain.conflicts,
            })
            .collect(),
        policies: status
            .policies
            .into_iter()
            .map(|policy| RelayPolicyView {
                endpoint: policy.endpoint.value,
                access: relay_access_label(policy.access).to_owned(),
                authentication: relay_authentication_label(policy.authentication).to_owned(),
                enabled: policy.enabled,
                generation: policy.generation,
            })
            .collect(),
        queued: status.queued,
        prepared: status.prepared,
        uncertain: status.uncertain,
        rejected: status.rejected,
        accepted: status.accepted,
        staged: status.staged,
        quarantined: status.quarantined,
        truncated: status.truncated,
    }
}

const fn relay_access_label(access: RelayAccessDto) -> &'static str {
    match access {
        RelayAccessDto::Read => "read",
        RelayAccessDto::Write => "write",
        RelayAccessDto::ReadWrite => "read-write",
    }
}

const fn relay_authentication_label(authentication: RelayAuthenticationDto) -> &'static str {
    match authentication {
        RelayAuthenticationDto::Disabled => "disabled",
        RelayAuthenticationDto::OnChallenge => "on-challenge",
        RelayAuthenticationDto::Required => "required",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MembershipRecord {
    state: String,
    frontier: BTreeSet<FactId>,
    grants: Vec<DeviceGrantDto>,
    acceptances: BTreeSet<FactId>,
    revokes: BTreeSet<FactId>,
    active_acceptances: BTreeSet<FactId>,
}

fn create_pairing_invitation(
    client: &mut LocalNodeClient,
    local: InstallationId,
    device: InstallationAddress,
    destination: &Path,
    label: Option<&ShortText>,
    relay_hints: &RelayHints,
) -> Result<CliResult, CliError> {
    if device.installation_id() == local {
        return Err(CliError::HumanState);
    }
    let snapshot = client.snapshot()?;
    let authority = local_authority(&snapshot, local)?;
    let selection = local_selection(&snapshot, local)?;
    let account_id = selection.active.ok_or(CliError::HumanState)?;
    let (account_root, creator, _, _) =
        account_item(&snapshot, account_id).ok_or(CliError::HumanState)?;
    if creator != local {
        return Err(CliError::HumanState);
    }
    let history = membership_record(&snapshot, account_id, device.installation_id())?;
    let frontier = history
        .as_ref()
        .map_or_else(BTreeSet::new, |history| history.frontier.clone());
    let reusable = reusable_pairing_grant(history.as_ref(), device, label, relay_hints)?;
    let (grant_id, grant_fact) = if let Some(reusable) = reusable {
        reusable
    } else {
        let grant_id = pairing_grant_id(account_id, device, label, relay_hints, &frontier);
        if history.as_ref().is_some_and(|history| {
            history.grants.iter().any(|grant| {
                grant.grant_id.bytes() == *grant_id.as_bytes()
                    && !device_grant_matches(grant, device, label, relay_hints)
            })
        }) {
            return Err(CliError::HumanState);
        }
        let request = HumanDeviceGrantRequest {
            account_id,
            account_root,
            grant_id,
            device,
            label: label.cloned(),
            relay_hints: relay_hints.clone(),
            membership_frontier: frontier,
        };
        let grant_fact = author_pairing_grant(client, authority, request)?;
        (grant_id, grant_fact)
    };
    let evidence = load_pairing_evidence(client, grant_fact)?;
    let invitation = VerifiedPairingInvitation::from_evidence(
        grant_fact,
        evidence
            .iter()
            .map(|item| item.exact_event.as_bytes().to_vec()),
    )
    .map_err(|_| CliError::PairingArtifact)?;
    verify_pairing_authority(&invitation, local)?;
    write_new_pairing_file(destination, invitation.canonical_bytes())
        .map_err(|_| CliError::PairingArtifact)?;
    Ok(CliResult::HumanPairing(HumanPairingView {
        operation: "invite",
        account_id,
        grant_id,
        device: device.installation_id(),
    }))
}

fn reusable_pairing_grant(
    history: Option<&MembershipRecord>,
    device: InstallationAddress,
    label: Option<&ShortText>,
    relay_hints: &RelayHints,
) -> Result<Option<(GrantId, FactId)>, CliError> {
    let Some(history) = history else {
        return Ok(None);
    };
    let candidates = history
        .grants
        .iter()
        .filter(|grant| {
            device_grant_matches(grant, device, label, relay_hints)
                && (grant.active || (history.state == "pending" && grant.frontier_member))
        })
        .map(|grant| {
            (
                GrantId::from_bytes(grant.grant_id.bytes()),
                FactId::from_bytes(grant.grant_fact.bytes()),
            )
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [] => Ok(None),
        [candidate] => Ok(Some(*candidate)),
        [_, _, ..] => Err(CliError::HumanState),
    }
}

fn author_pairing_grant(
    client: &mut LocalNodeClient,
    authority: LocalInstallationAuthority,
    request: HumanDeviceGrantRequest,
) -> Result<FactId, CliError> {
    let account_id = request.account_id;
    let grant_id = request.grant_id;
    let device = request.device;
    let label = request.label.clone();
    let relay_hints = request.relay_hints.clone();
    let plan = plan_human_device_grant(authority, stable_inputs(), request)?;
    submit_human_plan(client, plan)?;
    let refreshed = client.snapshot()?;
    let refreshed = membership_record(&refreshed, account_id, device.installation_id())?
        .ok_or(CliError::HumanState)?;
    let grant = refreshed
        .grants
        .iter()
        .find(|grant| grant.grant_id.bytes() == *grant_id.as_bytes())
        .ok_or(CliError::HumanState)?;
    if !device_grant_matches(grant, device, label.as_ref(), &relay_hints) {
        return Err(CliError::HumanState);
    }
    Ok(FactId::from_bytes(grant.grant_fact.bytes()))
}

fn join_pairing_invitation(
    client: &mut LocalNodeClient,
    local: InstallationId,
    source: &Path,
) -> Result<CliResult, CliError> {
    let bytes = read_pairing_file(source).map_err(|_| CliError::PairingArtifact)?;
    let invitation =
        VerifiedPairingInvitation::decode(&bytes).map_err(|_| CliError::PairingArtifact)?;
    let snapshot = client.snapshot()?;
    let authority = local_authority(&snapshot, local)?;
    let grant = invitation.grant();
    if grant.device != InstallationAddress::new(authority.installation_id, authority.signing_key) {
        return Err(CliError::PairingArtifact);
    }
    verify_pairing_authority(&invitation, local)?;
    reconcile_human_mailbox(client, local)?;
    ingest_pairing_evidence(client, &invitation)?;

    let snapshot = client.snapshot()?;
    account_item(&snapshot, grant.account_id).ok_or(CliError::HumanState)?;
    let membership =
        membership_record(&snapshot, grant.account_id, local)?.ok_or(CliError::HumanState)?;
    let projected_grant = membership
        .grants
        .iter()
        .find(|candidate| candidate.grant_fact.bytes() == *grant.fact_id.as_bytes())
        .ok_or(CliError::HumanState)?;
    if !device_grant_matches(
        projected_grant,
        grant.device,
        grant.label.as_ref(),
        &grant.relay_hints,
    ) {
        return Err(CliError::HumanState);
    }
    if membership.state != "active" || membership.active_acceptances.is_empty() {
        let plan = plan_human_device_acceptance(
            authority,
            stable_inputs(),
            grant.account_id,
            grant.grant_id,
            grant.fact_id,
        )?;
        submit_human_plan(client, plan)?;
    }
    select_human_account(client, local, grant.account_id)?;
    Ok(CliResult::HumanPairing(HumanPairingView {
        operation: "join",
        account_id: grant.account_id,
        grant_id: grant.grant_id,
        device: local,
    }))
}

fn membership_record(
    snapshot: &AuthoritativeSnapshotDto,
    account: AccountId,
    device: InstallationId,
) -> Result<Option<MembershipRecord>, CliError> {
    let matches = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::Membership {
            account_id,
            device: candidate,
            state,
            frontier,
            grants,
            acceptances,
            revokes,
            active_acceptances,
        } if account_id.bytes() == *account.as_bytes()
            && candidate.bytes() == *device.as_bytes() =>
        {
            Some(MembershipRecord {
                state: state.clone(),
                frontier: frontier
                    .iter()
                    .map(|fact| FactId::from_bytes(fact.bytes()))
                    .collect(),
                grants: grants.clone(),
                acceptances: acceptances
                    .iter()
                    .map(|fact| FactId::from_bytes(fact.bytes()))
                    .collect(),
                revokes: revokes
                    .iter()
                    .map(|fact| FactId::from_bytes(fact.bytes()))
                    .collect(),
                active_acceptances: active_acceptances
                    .iter()
                    .map(|fact| FactId::from_bytes(fact.bytes()))
                    .collect(),
            })
        }
        _ => None,
    });
    let matches = matches.collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(CliError::HumanState),
    }
}

fn human_devices_view(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Result<HumanDevicesView, CliError> {
    let selection = local_selection(snapshot, local)?;
    let account_id = selection.active.ok_or(CliError::HumanState)?;
    let (_, creator_installation, _, _) =
        account_item(snapshot, account_id).ok_or(CliError::HumanState)?;
    let creator_keys = installation_signing_keys(snapshot, creator_installation);
    let creator_state = match creator_keys.len() {
        1 => HumanDeviceState::Creator,
        0 => HumanDeviceState::Incomplete,
        _ => HumanDeviceState::Conflicted,
    };
    let mut devices = vec![HumanDeviceView {
        installation_id: creator_installation,
        signing_keys: creator_keys,
        state: creator_state,
        grants: Vec::new(),
        frontier: Vec::new(),
        acceptances: Vec::new(),
        revokes: Vec::new(),
    }];
    for item in &snapshot.items {
        if let Some(device) = membership_device_view(item, account_id, creator_installation)? {
            devices.push(device);
        }
    }
    devices.sort_by_key(|device| device.installation_id);
    if devices
        .windows(2)
        .any(|pair| pair[0].installation_id == pair[1].installation_id)
    {
        return Err(CliError::HumanState);
    }
    Ok(HumanDevicesView {
        account_id,
        creator_installation,
        devices,
    })
}

fn membership_device_view(
    item: &SnapshotItem,
    account_id: AccountId,
    creator_installation: InstallationId,
) -> Result<Option<HumanDeviceView>, CliError> {
    let SnapshotItem::Membership {
        account_id: candidate_account,
        device,
        state,
        frontier,
        grants,
        acceptances,
        revokes,
        active_acceptances,
    } = item
    else {
        return Ok(None);
    };
    if candidate_account.bytes() != *account_id.as_bytes() {
        return Ok(None);
    }
    let installation_id = InstallationId::from_bytes(device.bytes());
    if installation_id == creator_installation {
        return Err(CliError::HumanState);
    }
    let grant_subjects_match = grants
        .iter()
        .all(|grant| grant.device.bytes() == *installation_id.as_bytes());
    let mut grant_views = grants
        .iter()
        .map(|grant| HumanDeviceGrantView {
            grant_id: GrantId::from_bytes(grant.grant_id.bytes()),
            grant_fact: FactId::from_bytes(grant.grant_fact.bytes()),
            signing_key: SigningPublicKey::from_bytes(grant.signing_key.bytes()),
            label: grant.label.clone(),
            relay_hints: grant
                .relay_hints
                .iter()
                .map(|hint| HumanRelayHintView {
                    scheme: resource_scheme_label(hint.scheme),
                    value: hint.value.clone(),
                })
                .collect(),
            frontier_member: grant.frontier_member,
            active: grant.active,
        })
        .collect::<Vec<_>>();
    grant_views.sort_by_key(|grant| grant.grant_id);
    let signing_keys = grant_views
        .iter()
        .map(|grant| grant.signing_key)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let frontier = decode_fact_ids(frontier);
    let acceptances = decode_fact_ids(acceptances);
    let revokes = decode_fact_ids(revokes);
    let active_acceptances = decode_fact_ids(active_acceptances)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let state = classify_device_state(
        state,
        &grant_views,
        &frontier,
        &acceptances,
        &revokes,
        &active_acceptances,
        grant_subjects_match,
    );
    Ok(Some(HumanDeviceView {
        installation_id,
        signing_keys,
        state,
        grants: grant_views,
        frontier,
        acceptances,
        revokes,
    }))
}

fn decode_fact_ids(ids: &[Id32]) -> Vec<FactId> {
    ids.iter()
        .map(|fact| FactId::from_bytes(fact.bytes()))
        .collect()
}

const fn resource_scheme_label(scheme: ResourceSchemeDto) -> &'static str {
    match scheme {
        ResourceSchemeDto::GitRepository => "git_repository",
        ResourceSchemeDto::WorkingTree => "working_tree",
        ResourceSchemeDto::Container => "container",
        ResourceSchemeDto::Opaque => "opaque",
    }
}

fn installation_signing_keys(
    snapshot: &AuthoritativeSnapshotDto,
    installation: InstallationId,
) -> Vec<SigningPublicKey> {
    snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Installation {
                installation_id,
                signing_key,
                ..
            } if installation_id.bytes() == *installation.as_bytes() => {
                Some(SigningPublicKey::from_bytes(signing_key.bytes()))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn authority_admin_view(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    operation: &'static str,
) -> AuthorityAdminView {
    let mut peers = Vec::new();
    let mut mailboxes = Vec::new();
    let mut capabilities = Vec::new();
    for item in &snapshot.items {
        match item {
            SnapshotItem::PeerRoute {
                owner,
                peer,
                state,
                frontier,
                routes,
                blocks,
            } if owner.bytes() == *local.as_bytes() => peers.push(PeerRouteView {
                owner: local,
                peer: InstallationId::from_bytes(peer.bytes()),
                state: state.clone(),
                frontier: decode_fact_ids(frontier),
                routes: routes.iter().map(peer_candidate_view).collect(),
                blocks: blocks.iter().map(peer_block_view).collect(),
            }),
            SnapshotItem::Mailbox {
                installation_id,
                mailbox_id,
                create_fact,
                mailbox_kind,
                label,
            } if installation_id.bytes() == *local.as_bytes() => mailboxes.push(MailboxView {
                address: MailboxAddress::new(local, MailboxId::from_bytes(mailbox_id.bytes())),
                create_fact: FactId::from_bytes(create_fact.bytes()),
                kind: mailbox_kind.clone(),
                label: label.clone(),
            }),
            SnapshotItem::MailboxCapability {
                grant_id,
                grant_fact,
                mailbox_installation,
                mailbox_id,
                grantee_installation,
                grantee_signing_key,
                active,
                revoke_frontier,
                observed_actions,
                support,
            } if mailbox_installation.bytes() == *local.as_bytes() => {
                capabilities.push(MailboxCapabilityView {
                    grant_id: GrantId::from_bytes(grant_id.bytes()),
                    grant_fact: FactId::from_bytes(grant_fact.bytes()),
                    mailbox: MailboxAddress::new(local, MailboxId::from_bytes(mailbox_id.bytes())),
                    grantee: InstallationAddress::new(
                        InstallationId::from_bytes(grantee_installation.bytes()),
                        SigningPublicKey::from_bytes(grantee_signing_key.bytes()),
                    ),
                    active: *active,
                    revoke_frontier: decode_fact_ids(revoke_frontier),
                    observed_actions: decode_fact_ids(observed_actions),
                    support: decode_fact_ids(support),
                });
            }
            _ => {}
        }
    }
    peers.sort_by_key(|peer| peer.peer);
    mailboxes.sort_by_key(|mailbox| mailbox.address);
    capabilities.sort_by_key(|capability| capability.grant_id);
    AuthorityAdminView {
        operation,
        peers,
        mailboxes,
        capabilities,
    }
}

fn add_peer_route(
    client: &mut LocalNodeClient,
    local: InstallationId,
    peer: InstallationAddress,
    encryption_key: EncryptionPublicKey,
    label: Option<&ShortText>,
    relay_hints: &RelayHints,
) -> Result<(), CliError> {
    if peer.installation_id() == local {
        return Err(CliError::AuthorityState);
    }
    let snapshot = client.snapshot()?;
    let authority = local_authority(&snapshot, local)?;
    let current = peer_route(&snapshot, local, peer.installation_id())?;
    if current.as_ref().is_some_and(|route| {
        route.state == "routable"
            && route
                .routes
                .iter()
                .filter(|candidate| candidate.frontier_member)
                .count()
                == 1
            && route.routes.iter().any(|candidate| {
                candidate.frontier_member
                    && candidate.signing_key == peer.signing_key()
                    && candidate.encryption_key == encryption_key
                    && candidate.label.as_deref() == label.map(ShortText::as_str)
                    && relay_views_match(&candidate.relay_hints, relay_hints)
            })
    }) {
        return Ok(());
    }
    let plan = plan_peer_route_set(
        authority,
        stable_inputs(),
        PeerRouteRequest {
            peer,
            encryption_key,
            label: label.cloned(),
            relay_hints: relay_hints.clone(),
            route_frontier: current
                .map_or_else(BTreeSet::new, |route| route.frontier.into_iter().collect()),
        },
    )?;
    submit_human_plan(client, plan)
}

fn distrust_peer(
    client: &mut LocalNodeClient,
    local: InstallationId,
    peer: InstallationId,
) -> Result<(), CliError> {
    if peer == local {
        return Err(CliError::AuthorityState);
    }
    let snapshot = client.snapshot()?;
    let active = mailbox_capabilities(&snapshot, local)
        .into_iter()
        .filter(|capability| capability.active && capability.grantee.installation_id() == peer)
        .collect::<Vec<_>>();
    for capability in active {
        revoke_exact_capability(client, local, &capability)?;
    }
    let snapshot = client.snapshot()?;
    let route = peer_route(&snapshot, local, peer)?.ok_or(CliError::AuthorityState)?;
    if route.state == "blocked" {
        return Ok(());
    }
    let plan = plan_peer_route_block(
        local_authority(&snapshot, local)?,
        stable_inputs(),
        peer,
        ErrorCode::new("operator-distrust").map_err(|_| CliError::AuthorityState)?,
        route.frontier.into_iter().collect(),
    )?;
    submit_human_plan(client, plan)
}

fn grant_mailbox(
    client: &mut LocalNodeClient,
    local: InstallationId,
    mailbox_id: MailboxId,
    peer: InstallationId,
) -> Result<(), CliError> {
    let snapshot = client.snapshot()?;
    let authority = local_authority(&snapshot, local)?;
    let route = peer_route(&snapshot, local, peer)?.ok_or(CliError::AuthorityState)?;
    let candidates = route
        .routes
        .iter()
        .filter(|candidate| candidate.frontier_member)
        .collect::<Vec<_>>();
    let [candidate] = candidates.as_slice() else {
        return Err(CliError::AuthorityState);
    };
    if route.state != "routable" {
        return Err(CliError::AuthorityState);
    }
    let grantee = InstallationAddress::new(peer, candidate.signing_key);
    let (mailbox, mailbox_fact) = local_mailbox(&snapshot, local, mailbox_id)?;
    let history = mailbox_capabilities(&snapshot, local)
        .into_iter()
        .filter(|capability| {
            capability.mailbox == mailbox && capability.grantee.installation_id() == peer
        })
        .collect::<Vec<_>>();
    let active = history
        .iter()
        .filter(|capability| capability.active)
        .collect::<Vec<_>>();
    match active.as_slice() {
        [] => {}
        [capability] if capability.grantee == grantee => return Ok(()),
        [_] | [_, _, ..] => return Err(CliError::AuthorityState),
    }
    let lineage_frontier = history
        .iter()
        .flat_map(|capability| capability.revoke_frontier.iter().copied())
        .collect::<BTreeSet<_>>();
    let grant_id = mailbox_grant_id(mailbox, grantee, &lineage_frontier);
    let plan = plan_mailbox_grant(
        authority,
        stable_inputs(),
        MailboxGrantRequest {
            grant_id,
            mailbox,
            mailbox_fact,
            grantee,
            lineage_frontier,
        },
    )?;
    submit_human_plan(client, plan)
}

fn revoke_mailbox(
    client: &mut LocalNodeClient,
    local: InstallationId,
    mailbox_id: MailboxId,
    peer: InstallationId,
) -> Result<(), CliError> {
    let snapshot = client.snapshot()?;
    let (mailbox, _) = local_mailbox(&snapshot, local, mailbox_id)?;
    let history = mailbox_capabilities(&snapshot, local)
        .into_iter()
        .filter(|capability| {
            capability.mailbox == mailbox && capability.grantee.installation_id() == peer
        })
        .collect::<Vec<_>>();
    let active = history
        .iter()
        .filter(|capability| capability.active)
        .collect::<Vec<_>>();
    match active.as_slice() {
        [] if !history.is_empty() => Ok(()),
        [capability] => revoke_exact_capability(client, local, capability),
        [] | [_, _, ..] => Err(CliError::AuthorityState),
    }
}

fn revoke_exact_capability(
    client: &mut LocalNodeClient,
    local: InstallationId,
    capability: &MailboxCapabilityView,
) -> Result<(), CliError> {
    let snapshot = client.snapshot()?;
    let authority = local_authority(&snapshot, local)?;
    let current = mailbox_capabilities(&snapshot, local)
        .into_iter()
        .find(|candidate| candidate.grant_id == capability.grant_id)
        .ok_or(CliError::AuthorityState)?;
    if !current.active {
        return Ok(());
    }
    let plan = plan_mailbox_revoke(
        authority,
        stable_inputs(),
        MailboxRevokeRequest {
            grant_id: current.grant_id,
            grant_fact: current.grant_fact,
            mailbox: current.mailbox,
            grantee_id: current.grantee.installation_id(),
            capability_frontier: current.support.into_iter().collect(),
        },
    )?;
    submit_human_plan(client, plan)
}

fn peer_candidate_view(candidate: &PeerRouteCandidateDto) -> PeerRouteCandidateView {
    PeerRouteCandidateView {
        fact_id: FactId::from_bytes(candidate.fact_id.bytes()),
        signing_key: SigningPublicKey::from_bytes(candidate.signing_key.bytes()),
        encryption_key: EncryptionPublicKey::from_bytes(candidate.encryption_key.bytes()),
        label: candidate.label.clone(),
        relay_hints: candidate
            .relay_hints
            .iter()
            .map(|hint| HumanRelayHintView {
                scheme: resource_scheme_label(hint.scheme),
                value: hint.value.clone(),
            })
            .collect(),
        frontier_member: candidate.frontier_member,
    }
}

fn peer_block_view(block: &PeerRouteBlockDto) -> PeerRouteBlockView {
    PeerRouteBlockView {
        fact_id: FactId::from_bytes(block.fact_id.bytes()),
        reason: block.reason.clone(),
        frontier_member: block.frontier_member,
    }
}

fn peer_route(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    peer: InstallationId,
) -> Result<Option<PeerRouteView>, CliError> {
    let matches = authority_admin_view(snapshot, local, "internal")
        .peers
        .into_iter()
        .filter(|route| route.peer == peer)
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(CliError::AuthorityState),
    }
}

fn mailbox_capabilities(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Vec<MailboxCapabilityView> {
    authority_admin_view(snapshot, local, "internal").capabilities
}

fn local_mailbox(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    mailbox_id: MailboxId,
) -> Result<(MailboxAddress, FactId), CliError> {
    let address = MailboxAddress::new(local, mailbox_id);
    let matches = authority_admin_view(snapshot, local, "internal")
        .mailboxes
        .into_iter()
        .filter(|mailbox| mailbox.address == address)
        .map(|mailbox| (mailbox.address, mailbox.create_fact))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [mailbox] => Ok(*mailbox),
        [] | [_, _, ..] => Err(CliError::AuthorityState),
    }
}

fn relay_views_match(actual: &[HumanRelayHintView], expected: &RelayHints) -> bool {
    actual.len() == expected.as_slice().len()
        && actual.iter().zip(expected.as_slice()).all(|(left, right)| {
            left.value == right.value()
                && left.scheme
                    == resource_scheme_label(match right.scheme() {
                        ResourceScheme::GitRepository => ResourceSchemeDto::GitRepository,
                        ResourceScheme::WorkingTree => ResourceSchemeDto::WorkingTree,
                        ResourceScheme::Container => ResourceSchemeDto::Container,
                        ResourceScheme::Opaque => ResourceSchemeDto::Opaque,
                    })
        })
}

fn mailbox_grant_id(
    mailbox: MailboxAddress,
    grantee: InstallationAddress,
    frontier: &BTreeSet<FactId>,
) -> GrantId {
    let mut digest = Sha256::new();
    digest.update(b"hq-mailbox-capability-grant-v1\0");
    digest.update(mailbox.installation_id().as_bytes());
    digest.update(mailbox.mailbox_id().as_bytes());
    digest.update(grantee.installation_id().as_bytes());
    digest.update(grantee.signing_key().as_bytes());
    for fact in frontier {
        digest.update(fact.as_bytes());
    }
    GrantId::from_bytes(digest.finalize().into())
}

fn classify_device_state(
    projected: &str,
    grants: &[HumanDeviceGrantView],
    frontier: &[FactId],
    acceptances: &[FactId],
    revokes: &[FactId],
    active_acceptances: &BTreeSet<FactId>,
    grant_subjects_match: bool,
) -> HumanDeviceState {
    let retained = grants
        .iter()
        .map(|grant| grant.grant_fact)
        .chain(acceptances.iter().copied())
        .chain(revokes.iter().copied())
        .collect::<BTreeSet<_>>();
    let active_grants = grants.iter().filter(|grant| grant.active).count();
    let frontier_grants = grants.iter().filter(|grant| grant.frontier_member).count();
    let incomplete = grants.is_empty()
        || !grant_subjects_match
        || frontier.iter().any(|fact| !retained.contains(fact))
        || active_acceptances
            .iter()
            .any(|fact| !acceptances.contains(fact))
        || (projected == "active" && (active_grants == 0 || active_acceptances.is_empty()))
        || (projected == "pending" && frontier_grants == 0)
        || (projected == "revoked" && revokes.is_empty());
    if incomplete {
        HumanDeviceState::Incomplete
    } else if (projected == "active" && active_grants > 1)
        || (projected == "pending" && frontier_grants > 1)
    {
        HumanDeviceState::Conflicted
    } else {
        match projected {
            "pending" => HumanDeviceState::Pending,
            "active" => HumanDeviceState::Active,
            "revoked" => HumanDeviceState::Revoked,
            _ => HumanDeviceState::Incomplete,
        }
    }
}

fn revoke_human_device(
    client: &mut LocalNodeClient,
    local: InstallationId,
    device: InstallationId,
) -> Result<(), CliError> {
    if device == local {
        return Err(CliError::HumanState);
    }
    let snapshot = client.snapshot()?;
    let authority = local_authority(&snapshot, local)?;
    let selection = local_selection(&snapshot, local)?;
    let account_id = selection.active.ok_or(CliError::HumanState)?;
    let (account_root, creator_installation, _, _) =
        account_item(&snapshot, account_id).ok_or(CliError::HumanState)?;
    if creator_installation != local {
        return Err(CliError::HumanState);
    }
    let membership =
        membership_record(&snapshot, account_id, device)?.ok_or(CliError::HumanState)?;
    let devices = human_devices_view(&snapshot, local)?;
    let presented = devices
        .devices
        .iter()
        .find(|candidate| candidate.installation_id == device)
        .ok_or(CliError::HumanState)?;
    if presented.state == HumanDeviceState::Revoked {
        return Ok(());
    }
    if matches!(
        presented.state,
        HumanDeviceState::Creator | HumanDeviceState::Conflicted | HumanDeviceState::Incomplete
    ) {
        return Err(CliError::HumanState);
    }
    let candidates = membership
        .grants
        .iter()
        .filter(|grant| match presented.state {
            HumanDeviceState::Active => grant.active,
            HumanDeviceState::Pending => grant.frontier_member,
            _ => false,
        })
        .collect::<Vec<_>>();
    let [grant] = candidates.as_slice() else {
        return Err(CliError::HumanState);
    };
    let request = HumanDeviceRevokeRequest {
        account_id,
        account_root,
        creator: InstallationAddress::new(authority.installation_id, authority.signing_key),
        grant_id: GrantId::from_bytes(grant.grant_id.bytes()),
        grant_fact: FactId::from_bytes(grant.grant_fact.bytes()),
        device_id: device,
        membership_frontier: membership.frontier,
    };
    let plan = plan_human_device_revoke(authority, stable_inputs(), request)?;
    if let Err(error) = submit_human_plan(client, plan) {
        let reconciled = client.snapshot()?;
        if membership_record(&reconciled, account_id, device)?
            .is_some_and(|membership| membership.state == "revoked")
        {
            return Ok(());
        }
        return Err(error);
    }
    let refreshed = client.snapshot()?;
    membership_record(&refreshed, account_id, device)?
        .filter(|membership| membership.state == "revoked")
        .map(|_| ())
        .ok_or(CliError::HumanState)
}

fn device_grant_matches(
    grant: &DeviceGrantDto,
    device: InstallationAddress,
    label: Option<&ShortText>,
    relay_hints: &RelayHints,
) -> bool {
    grant.device.bytes() == *device.installation_id().as_bytes()
        && grant.signing_key.bytes() == *device.signing_key().as_bytes()
        && grant.label.as_deref() == label.map(ShortText::as_str)
        && grant.relay_hints.len() == relay_hints.as_slice().len()
        && grant
            .relay_hints
            .iter()
            .zip(relay_hints.as_slice())
            .all(|(actual, expected)| {
                actual.value == expected.value()
                    && actual.scheme
                        == match expected.scheme() {
                            ResourceScheme::GitRepository => ResourceSchemeDto::GitRepository,
                            ResourceScheme::WorkingTree => ResourceSchemeDto::WorkingTree,
                            ResourceScheme::Container => ResourceSchemeDto::Container,
                            ResourceScheme::Opaque => ResourceSchemeDto::Opaque,
                        }
            })
}

fn pairing_grant_id(
    account_id: AccountId,
    device: InstallationAddress,
    label: Option<&ShortText>,
    relay_hints: &RelayHints,
    frontier: &BTreeSet<FactId>,
) -> GrantId {
    let mut digest = Sha256::new();
    digest.update(b"hq-human-device-grant-v1\0");
    digest.update(account_id.as_bytes());
    digest.update(device.installation_id().as_bytes());
    digest.update(device.signing_key().as_bytes());
    match label {
        Some(label) => {
            digest.update([1]);
            update_digest_text(&mut digest, label.as_str());
        }
        None => digest.update([0]),
    }
    digest.update(
        u64::try_from(relay_hints.as_slice().len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for locator in relay_hints.as_slice() {
        digest.update([match locator.scheme() {
            ResourceScheme::GitRepository => 1,
            ResourceScheme::WorkingTree => 2,
            ResourceScheme::Container => 3,
            ResourceScheme::Opaque => 4,
        }]);
        update_digest_text(&mut digest, locator.value());
    }
    digest.update(
        u64::try_from(frontier.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for fact_id in frontier {
        digest.update(fact_id.as_bytes());
    }
    GrantId::from_bytes(digest.finalize().into())
}

fn update_digest_text(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value.as_bytes());
}

fn load_pairing_evidence(
    client: &mut LocalNodeClient,
    grant_fact: FactId,
) -> Result<Vec<CanonicalEvidenceDto>, CliError> {
    let request = || {
        Request::CanonicalEvidence(CanonicalEvidenceRequestDto {
            roots: vec![Id32::new(*grant_fact.as_bytes())],
        })
    };
    for _ in 0..2 {
        match client.request(request())? {
            ClientEvent::Response {
                result: ResponseResult::CanonicalEvidence(evidence),
                ..
            } if evidence
                .iter()
                .any(|item| item.fact_id.bytes() == *grant_fact.as_bytes()) =>
            {
                return Ok(evidence);
            }
            ClientEvent::RequestLost(_) => {}
            _ => return Err(CliError::HumanState),
        }
    }
    Err(CliError::HumanState)
}

fn ingest_pairing_evidence(
    client: &mut LocalNodeClient,
    invitation: &VerifiedPairingInvitation,
) -> Result<(), CliError> {
    let evidence = invitation
        .facts()
        .map(|fact| {
            std::str::from_utf8(fact.verified_event().exact_event_bytes())
                .map(|exact_event| CanonicalEvidenceDto {
                    fact_id: Id32::new(*fact.fact().id().as_bytes()),
                    exact_event: exact_event.to_owned(),
                })
                .map_err(|_| CliError::PairingArtifact)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = evidence
        .iter()
        .map(|item| item.fact_id)
        .collect::<BTreeSet<_>>();
    for _ in 0..2 {
        match client.request(Request::IngestCanonicalEvidence(evidence.clone()))? {
            ClientEvent::Response {
                result: ResponseResult::EvidenceIngest(outcomes),
                ..
            } if outcomes
                .iter()
                .map(|outcome| outcome.fact_id)
                .collect::<BTreeSet<_>>()
                == expected
                && outcomes.len() == evidence.len() =>
            {
                return Ok(());
            }
            ClientEvent::RequestLost(_) => {}
            _ => return Err(CliError::HumanState),
        }
    }
    Err(CliError::HumanState)
}

fn verify_pairing_authority(
    invitation: &VerifiedPairingInvitation,
    local: InstallationId,
) -> Result<(), CliError> {
    let grant = invitation.grant();
    let report = reduce_complete(
        invitation.facts().map(|fact| fact.fact().clone()),
        &AuthorityReducer::new(AuthorityPolicy::new(
            local,
            crate::foreground::reserved_human_mailbox(),
        )),
    )
    .map_err(|_| CliError::PairingArtifact)?;
    let projected_grant = report
        .decisions()
        .get(&grant.fact_id)
        .is_some_and(|decision| decision.status() == DecisionStatus::Projected);
    let projected_account = report
        .projections()
        .contains_key(&AuthorityProjectionKey::Account(grant.account_id));
    let projected_membership =
        report
            .projections()
            .contains_key(&AuthorityProjectionKey::Membership {
                account: grant.account_id,
                device: grant.device.installation_id(),
            });
    if projected_grant && projected_account && projected_membership {
        Ok(())
    } else {
        Err(CliError::PairingArtifact)
    }
}

fn reconcile_human_mailbox(
    client: &mut LocalNodeClient,
    local: InstallationId,
) -> Result<(), CliError> {
    let snapshot = client.snapshot()?;
    let authority = local_authority(&snapshot, local)?;
    let mailbox = crate::foreground::reserved_human_mailbox();
    let matching = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::Mailbox {
            installation_id,
            mailbox_id,
            mailbox_kind,
            ..
        } if installation_id.bytes() == *local.as_bytes()
            && mailbox_id.bytes() == *mailbox.as_bytes() =>
        {
            Some(mailbox_kind.as_str())
        }
        _ => None,
    });
    let kinds = matching.collect::<Vec<_>>();
    match kinds.as_slice() {
        ["human"] => Ok(()),
        [] => {
            let plan = plan_human_mailbox_creation(authority, stable_inputs(), mailbox, None)?;
            submit_human_plan(client, plan)
        }
        [_] | [_, ..] => Err(CliError::HumanState),
    }
}

fn select_human_account(
    client: &mut LocalNodeClient,
    local: InstallationId,
    account_id: AccountId,
) -> Result<(), CliError> {
    let snapshot = client.snapshot()?;
    let view = human_view(&snapshot, local)?;
    if view.active_account == Some(account_id) {
        return Ok(());
    }
    let authority = local_authority(&snapshot, local)?;
    let (root_fact, creator, _, _) =
        account_item(&snapshot, account_id).ok_or(CliError::HumanState)?;
    let membership_fact = if creator == local {
        root_fact
    } else {
        active_membership_fact(&snapshot, local, account_id)?
    };
    let frontier = local_selection(&snapshot, local)?.frontier;
    let plan = plan_human_account_selection(
        authority,
        stable_inputs(),
        account_id,
        membership_fact,
        frontier,
    )?;
    submit_human_plan(client, plan)
}

fn local_authority(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Result<LocalInstallationAuthority, CliError> {
    let matches = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::Installation {
            installation_id,
            root_fact,
            signing_key,
            ..
        } if installation_id.bytes() == *local.as_bytes() => Some(LocalInstallationAuthority {
            installation_id: local,
            signing_key: SigningPublicKey::from_bytes(signing_key.bytes()),
            root_fact: FactId::from_bytes(root_fact.bytes()),
        }),
        _ => None,
    });
    let values = matches.collect::<Vec<_>>();
    match values.as_slice() {
        [authority] => Ok(*authority),
        [] | [_, _, ..] => Err(CliError::HumanState),
    }
}

fn account_item(
    snapshot: &AuthoritativeSnapshotDto,
    target: AccountId,
) -> Option<(FactId, InstallationId, Option<String>, bool)> {
    snapshot.items.iter().find_map(|item| match item {
        SnapshotItem::Account {
            account_id,
            root_fact,
            creator_installation,
            label,
            selected,
        } if account_id.bytes() == *target.as_bytes() => Some((
            FactId::from_bytes(root_fact.bytes()),
            InstallationId::from_bytes(creator_installation.bytes()),
            label.clone(),
            *selected,
        )),
        _ => None,
    })
}

fn active_membership_fact(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
    account: AccountId,
) -> Result<FactId, CliError> {
    snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Membership {
                account_id,
                device,
                state,
                active_acceptances,
                ..
            } if account_id.bytes() == *account.as_bytes()
                && device.bytes() == *local.as_bytes()
                && state == "active" =>
            {
                active_acceptances
                    .first()
                    .map(|fact| FactId::from_bytes(fact.bytes()))
            }
            _ => None,
        })
        .ok_or(CliError::HumanState)
}

fn local_selection(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Result<LocalSelection, CliError> {
    let matches = snapshot.items.iter().filter_map(|item| match item {
        SnapshotItem::AccountSelection {
            installation_id,
            candidates,
            active,
            frontier,
        } if installation_id.bytes() == *local.as_bytes() => Some(LocalSelection {
            candidates: candidates
                .iter()
                .map(|account| AccountId::from_bytes(account.bytes()))
                .collect(),
            active: active.map(|account| AccountId::from_bytes(account.bytes())),
            frontier: frontier
                .iter()
                .map(|fact| FactId::from_bytes(fact.bytes()))
                .collect(),
        }),
        _ => None,
    });
    let values = matches.collect::<Vec<_>>();
    match values.len() {
        0 => Ok(LocalSelection {
            candidates: Vec::new(),
            active: None,
            frontier: BTreeSet::new(),
        }),
        1 => values.into_iter().next().ok_or(CliError::HumanState),
        _ => Err(CliError::HumanState),
    }
}

fn human_view(
    snapshot: &AuthoritativeSnapshotDto,
    local: InstallationId,
) -> Result<HumanView, CliError> {
    let selection = local_selection(snapshot, local)?;
    let mut accounts = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Account {
                account_id,
                creator_installation,
                label,
                ..
            } => {
                let account_id = AccountId::from_bytes(account_id.bytes());
                Some(HumanAccountView {
                    selected: selection.active == Some(account_id),
                    account_id,
                    creator_installation: InstallationId::from_bytes(creator_installation.bytes()),
                    label: label.clone(),
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    accounts.sort_by_key(|account| account.account_id);
    Ok(HumanView {
        installation_id: local,
        accounts,
        selection_candidates: selection.candidates,
        active_account: selection.active,
    })
}

fn creator_account_id(local: InstallationId) -> AccountId {
    let mut digest = Sha256::new();
    digest.update(b"hq-human-creator-account-v1\0");
    digest.update(local.as_bytes());
    AccountId::from_bytes(digest.finalize().into())
}

fn stable_inputs() -> LocalFactInputs {
    LocalFactInputs {
        authored_at: Timestamp::from_unix_millis(0),
        auxiliary_randomness: [0; 32],
    }
}

fn submit_human_plan(
    client: &mut LocalNodeClient,
    plan: hq_application::FactPlan,
) -> Result<(), CliError> {
    let request =
        MutationRequest::from_plan(random_command_id()?, plan).map_err(|_| CliError::HumanState)?;
    match client.mutation(request)? {
        ClientEvent::Mutation(MutationAttemptDto::Completed {
            outcome: MutationOutcomeDto::Committed,
            ..
        }) => Ok(()),
        _ => Err(CliError::HumanState),
    }
}

fn random_command_id() -> Result<CommandId, CliError> {
    for _ in 0..16 {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| CliError::Runtime)?;
        if bytes != [0; 32] {
            return Ok(CommandId::from_bytes(bytes));
        }
    }
    Err(CliError::Runtime)
}

fn command_client(state: &StatePaths) -> Result<LocalNodeClient, CliError> {
    LocalNodeClient::connect(installed_local_client_config(
        state.clone(),
        build()?,
        InitialView::OnDemand,
    ))
    .map_err(Into::into)
}

fn read_password(input: &mut dyn Read) -> Result<BackupPassword, CliError> {
    const MAX_INPUT_BYTES: u64 = 1_027;
    let mut bytes = Zeroizing::new(Vec::new());
    input
        .take(MAX_INPUT_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::SecretInput)?;
    if bytes.len() >= usize::try_from(MAX_INPUT_BYTES).unwrap_or(usize::MAX) {
        return Err(CliError::SecretInput);
    }
    if bytes.last() == Some(&b'\n') {
        let _ = bytes.pop();
        if bytes.last() == Some(&b'\r') {
            let _ = bytes.pop();
        }
    }
    if bytes.contains(&b'\n') || bytes.contains(&b'\r') {
        return Err(CliError::SecretInput);
    }
    if bytes.is_empty() {
        return Err(CliError::SecretInput);
    }
    let password = std::str::from_utf8(&bytes)
        .map(str::to_owned)
        .map_err(|_| CliError::SecretInput)?;
    BackupPassword::new(password).map_err(|_| CliError::SecretInput)
}

fn build() -> Result<BuildMetadata, CliError> {
    BuildMetadata::new(
        "hq",
        env!("CARGO_PKG_VERSION"),
        option_env!("HQ_BUILD_COMMIT"),
    )
    .map_err(|_| CliError::Build)
}

fn lifecycle_client(
    runtime: RuntimePaths,
    build: BuildMetadata,
) -> Result<LifecycleClient, CliError> {
    LifecycleClient::new(LifecycleClientConfig {
        runtime,
        build,
        io_timeout: Duration::from_secs(2),
    })
    .map_err(Into::into)
}

fn coordinator(
    state: &StatePaths,
    runtime: RuntimePaths,
    build: BuildMetadata,
) -> Result<NodeClientCoordinator<LifecycleClient, ProcessNodeLauncher>, CliError> {
    let probe = lifecycle_client(runtime, build)?;
    let launcher = ProcessNodeLauncher::current_executable().map_err(|_| CliError::Runtime)?;
    NodeClientCoordinator::new(
        probe,
        launcher,
        NodeCoordinatorConfig {
            state_root: state.root().to_path_buf(),
            readiness_timeout: Duration::from_secs(10),
            retry_interval: Duration::from_millis(25),
        },
    )
    .map_err(Into::into)
}

fn foreground_config(
    state: StatePaths,
    runtime: RuntimePaths,
    build: BuildMetadata,
) -> ForegroundNodeConfig {
    ForegroundNodeConfig {
        state,
        runtime,
        build,
        store_capacity: nonzero(64),
        task_capacity: nonzero(64),
        subscription_capacity: nonzero(256),
        session_capacity: nonzero(64),
        event_capacity: nonzero(256),
        write_capacity: nonzero(8),
        response_drain_timeout: Duration::from_secs(2),
    }
}

const fn nonzero(value: usize) -> NonZeroUsize {
    match NonZeroUsize::new(value) {
        Some(value) => value,
        None => unreachable!(),
    }
}

enum CliResult {
    Rendered(String),
    AgentGuidance(AgentGuidanceTopic),
    Lifecycle {
        label: &'static str,
        observation: Box<LifecycleObservation>,
    },
    Stopped {
        intent: String,
    },
    Identity(Box<PublicIdentity>),
    Configuration(Box<LocalConfiguration>),
    ThemeCatalog(Vec<TuiThemeCatalogEntry>),
    Human(Box<HumanView>),
    HumanPairing(HumanPairingView),
    HumanDevices(Box<HumanDevicesView>),
    AuthorityAdmin(Box<AuthorityAdminView>),
    RelayAdmin(Box<RelayAdminView>),
    Messages(Box<MessageCommandView>),
    MailboxDiscovery(Box<MailboxDiscoveryView>),
    NamedAgentCatalog(Box<NamedAgentCatalogView>),
    NamedAgentRetirement(NamedAgentRetirementView),
    HarnessSession(HarnessSessionView),
    ProjectCatalog(Box<ProjectCatalogView>),
    ProjectResourceCatalog(Box<ProjectResourceCatalogView>),
    ProjectResourceCheck(Box<ProjectResourceCheckView>),
    ProjectOperation(ProjectOperationView),
    Completed {
        operation: &'static str,
    },
}

fn render_theme_catalog(entries: &[TuiThemeCatalogEntry]) -> String {
    let mut output = String::from("TUI themes (select with `hq config set theme NAME`):\n");
    for entry in entries {
        let marker = if entry.active { '*' } else { ' ' };
        let _ = write!(
            output,
            "{marker} {} — {} [{}]",
            entry.selector, entry.name, entry.source
        );
        if let Some(author) = &entry.author {
            let _ = write!(output, " — {author}");
        }
        if let Some(error) = &entry.error {
            let _ = write!(output, " — invalid: {error}");
        }
        output.push('\n');
    }
    output
}

#[allow(clippy::too_many_lines, reason = "closed CLI result rendering matrix")]
fn render_result(format: CliOutputFormat, result: &CliResult) -> Result<String, CliError> {
    if let Some(rendered) = render_agent_result(format, result) {
        return rendered;
    }
    match (format, result) {
        (_, CliResult::Rendered(output)) => Ok(output.clone()),
        (CliOutputFormat::Human, CliResult::Lifecycle { label, observation }) => {
            Ok(format_observation(label, observation))
        }
        (CliOutputFormat::Human, CliResult::Identity(identity)) => Ok(format!(
            "installation={} public_key={} fingerprint={}\nNext: run hq\n",
            crate::identity::encode_hex(identity.installation_id.as_bytes()),
            crate::identity::encode_hex(&identity.signing_public_key),
            identity.fingerprint,
        )),
        (CliOutputFormat::Human, CliResult::Configuration(configuration)) => Ok(format!(
            "default_provider={} relays={} theme={}\n",
            configuration
                .default_provider
                .as_ref()
                .map_or("none", ProviderId::as_str),
            configuration
                .relays
                .iter()
                .map(RelayEndpoint::as_str)
                .collect::<Vec<_>>()
                .join(","),
            configuration
                .theme
                .as_ref()
                .map_or("automatic", ThemeSelection::as_str),
        )),
        (CliOutputFormat::Human, CliResult::ThemeCatalog(entries)) => {
            Ok(render_theme_catalog(entries))
        }
        (CliOutputFormat::Human, CliResult::Human(view)) => render_human_view(view),
        (CliOutputFormat::Human, CliResult::HumanDevices(view)) => render_human_devices(view),
        (CliOutputFormat::Human, CliResult::HumanPairing(view)) => Ok(render_human_pairing(view)),
        (format, CliResult::Messages(view)) => render_message_result(format, view),
        (format, CliResult::MailboxDiscovery(view)) => render_mailbox_discovery(format, view),
        (CliOutputFormat::Json, CliResult::Lifecycle { label, observation }) => machine_record(
            "lifecycle",
            &serde_json::json!({
                "command": label,
                "process_id": observation.readiness.as_ref().map(|ready| ready.process_id),
                "revision": observation.status.revision,
                "state": lifecycle_state(observation.status.state),
            }),
        ),
        (CliOutputFormat::Json, CliResult::Stopped { intent }) => {
            machine_record("stopped", &serde_json::json!({ "intent": intent }))
        }
        (CliOutputFormat::Json, CliResult::Identity(identity)) => machine_record(
            "identity",
            &serde_json::json!({
                "fingerprint": identity.fingerprint,
                "installation_id": crate::identity::encode_hex(identity.installation_id.as_bytes()),
                "signing_public_key": crate::identity::encode_hex(&identity.signing_public_key),
            }),
        ),
        (CliOutputFormat::Json, CliResult::Configuration(configuration)) => machine_record(
            "configuration",
            &serde_json::json!({
                "default_provider": configuration.default_provider.as_ref().map(ProviderId::as_str),
                "relays": configuration.relays.iter().map(RelayEndpoint::as_str).collect::<Vec<_>>(),
                "theme": configuration.theme.as_ref().map(ThemeSelection::as_str),
            }),
        ),
        (CliOutputFormat::Json, CliResult::ThemeCatalog(entries)) => machine_record(
            "themes",
            &serde_json::json!({
                "themes": entries.iter().map(|entry| serde_json::json!({
                    "active": entry.active,
                    "author": entry.author,
                    "error": entry.error,
                    "name": entry.name,
                    "selector": entry.selector,
                    "source": entry.source,
                })).collect::<Vec<_>>(),
            }),
        ),
        (CliOutputFormat::Json, CliResult::Human(view)) => machine_record(
            "human",
            &serde_json::json!({
                "accounts": view.accounts.iter().map(|account| serde_json::json!({
                    "account_id": crate::identity::encode_hex(account.account_id.as_bytes()),
                    "creator_installation": crate::identity::encode_hex(account.creator_installation.as_bytes()),
                    "label": account.label,
                    "selected": account.selected,
                })).collect::<Vec<_>>(),
                "active_account": view.active_account.map(|account| crate::identity::encode_hex(account.as_bytes())),
                "installation_id": crate::identity::encode_hex(view.installation_id.as_bytes()),
                "selection_candidates": view.selection_candidates.iter().map(|account| crate::identity::encode_hex(account.as_bytes())).collect::<Vec<_>>(),
            }),
        ),
        (CliOutputFormat::Json, CliResult::HumanPairing(view)) => machine_record(
            "human_pairing",
            &serde_json::json!({
                "account_id": crate::identity::encode_hex(view.account_id.as_bytes()),
                "device": crate::identity::encode_hex(view.device.as_bytes()),
                "grant_id": crate::identity::encode_hex(view.grant_id.as_bytes()),
                "operation": view.operation,
            }),
        ),
        (CliOutputFormat::Json, CliResult::HumanDevices(view)) => machine_record(
            "human_devices",
            &serde_json::json!({
                "account_id": encode_id(view.account_id.as_bytes()),
                "creator_installation": encode_id(view.creator_installation.as_bytes()),
                "devices": view.devices.iter().map(device_json).collect::<Vec<_>>(),
            }),
        ),
        (format, CliResult::AuthorityAdmin(view)) => render_authority_admin_result(format, view),
        (format, CliResult::RelayAdmin(view)) => render_relay_admin_result(format, view),
        (format, CliResult::HarnessSession(view)) => render_harness_session(format, view),
        (format, CliResult::ProjectCatalog(view)) => render_project_catalog(format, view),
        (format, CliResult::ProjectResourceCatalog(view)) => {
            render_project_resource_catalog(format, view)
        }
        (format, CliResult::ProjectResourceCheck(view)) => {
            render_project_resource_check(format, view)
        }
        (format, CliResult::ProjectOperation(view)) => render_project_operation(format, view),
        (
            _,
            CliResult::AgentGuidance(_)
            | CliResult::NamedAgentCatalog(_)
            | CliResult::NamedAgentRetirement(_)
            | CliResult::Completed { .. }
            | CliResult::Stopped { .. },
        ) => unreachable!(),
    }
}

fn render_project_resource_catalog(
    format: CliOutputFormat,
    view: &ProjectResourceCatalogView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => {
            let mut output = format!(
                "project_resources operation={} project={} home={} resources={}\n",
                view.operation,
                encode_id(view.project_id.as_bytes()),
                encode_id(view.home.as_bytes()),
                view.resources.len(),
            );
            for resource in &view.resources {
                write_project_resource_line(&mut output, view.project_id, resource)?;
            }
            Ok(output)
        }
        CliOutputFormat::Json => machine_record(
            "project_resources",
            &serde_json::json!({
                "home": encode_id(view.home.as_bytes()),
                "operation": view.operation,
                "project_id": encode_id(view.project_id.as_bytes()),
                "resources": view.resources.iter().map(project_resource_json).collect::<Vec<_>>(),
            }),
        ),
    }
}

fn render_project_resource_check(
    format: CliOutputFormat,
    view: &ProjectResourceCheckView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => {
            let mut output = format!(
                "project_check project={} home={} resources={}\n",
                encode_id(view.project_id.as_bytes()),
                encode_id(view.home.as_bytes()),
                view.checks.len(),
            );
            for check in &view.checks {
                writeln!(
                    output,
                    "resource_check project={} id={} operation_id={} display={}:{} canonical={}:{} observed={} health={} release={} primary={} active_claim={} conflicts={} status={} checked_at_unix_millis={} details={:?} error={}:{} reconciliation_id={}",
                    encode_id(view.project_id.as_bytes()),
                    encode_id(check.resource_id.as_bytes()),
                    encode_id(check.operation_id.as_bytes()),
                    resource_scheme_label(check.display_locator.scheme),
                    check.display_locator.value,
                    resource_scheme_label(check.canonical_locator.scheme),
                    check.canonical_locator.value,
                    check.observed_canonical.as_ref().map_or_else(
                        || "none".to_owned(),
                        |locator| format!("{}:{}", resource_scheme_label(locator.scheme), locator.value),
                    ),
                    check.health.unwrap_or("none"),
                    check.release.unwrap_or("none"),
                    check.primary,
                    check.active_claim,
                    check.conflicting_projects.iter().map(|id| encode_id(id.as_bytes())).collect::<Vec<_>>().join(","),
                    check.status,
                    check.checked_at_unix_millis.map_or_else(|| "none".to_owned(), |value| value.to_string()),
                    check.details,
                    check.error_category.as_deref().unwrap_or("none"),
                    check.error_code.as_deref().unwrap_or("none"),
                    check.reconciliation_id.as_ref().map_or_else(|| "none".to_owned(), |id| encode_id(id.as_bytes())),
                )
                .map_err(|_| CliError::Runtime)?;
            }
            Ok(output)
        }
        CliOutputFormat::Json => machine_record(
            "project_check",
            &serde_json::json!({
                "checks": view.checks.iter().map(project_resource_check_json).collect::<Vec<_>>(),
                "home": encode_id(view.home.as_bytes()),
                "project_id": encode_id(view.project_id.as_bytes()),
            }),
        ),
    }
}

fn write_project_resource_line(
    output: &mut String,
    project_id: ProjectId,
    resource: &ProjectResourceView,
) -> Result<(), CliError> {
    writeln!(
        output,
        "resource project={} id={} display={}:{} canonical={}:{} health={} primary={} active_claim={} conflicts={}",
        encode_id(project_id.as_bytes()),
        encode_id(resource.resource_id.as_bytes()),
        resource_scheme_label(resource.display_locator.scheme),
        resource.display_locator.value,
        resource_scheme_label(resource.canonical_locator.scheme),
        resource.canonical_locator.value,
        resource.health,
        resource.primary,
        resource.active_claim,
        resource.conflicting_projects.iter().map(|id| encode_id(id.as_bytes())).collect::<Vec<_>>().join(","),
    )
    .map_err(|_| CliError::Runtime)
}

fn project_resource_json(resource: &ProjectResourceView) -> serde_json::Value {
    serde_json::json!({
        "active_claim": resource.active_claim,
        "canonical_locator": resource.canonical_locator,
        "conflicting_projects": resource.conflicting_projects.iter().map(|id| encode_id(id.as_bytes())).collect::<Vec<_>>(),
        "display_locator": resource.display_locator,
        "health": resource.health,
        "primary": resource.primary,
        "resource_id": encode_id(resource.resource_id.as_bytes()),
    })
}

fn project_resource_check_json(check: &ProjectResourceCheckItemView) -> serde_json::Value {
    serde_json::json!({
        "active_claim": check.active_claim,
        "canonical_locator": check.canonical_locator,
        "checked_at_unix_millis": check.checked_at_unix_millis,
        "conflicting_projects": check.conflicting_projects.iter().map(|id| encode_id(id.as_bytes())).collect::<Vec<_>>(),
        "details": check.details,
        "display_locator": check.display_locator,
        "error_category": check.error_category,
        "error_code": check.error_code,
        "health": check.health,
        "observed_canonical": check.observed_canonical,
        "operation_id": encode_id(check.operation_id.as_bytes()),
        "primary": check.primary,
        "reconciliation_id": check.reconciliation_id.as_ref().map(|id| encode_id(id.as_bytes())),
        "release": check.release,
        "resource_id": encode_id(check.resource_id.as_bytes()),
        "status": check.status,
    })
}

fn render_project_catalog(
    format: CliOutputFormat,
    view: &ProjectCatalogView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => render_project_catalog_human(view),
        CliOutputFormat::Json => machine_record(
            "project_catalog",
            &serde_json::json!({
                "operation": view.operation,
                "projects": view.projects.iter().map(project_json).collect::<Vec<_>>(),
                "unattributed_dispatches": view.unattributed_dispatches,
                "unattributed_outputs": view.unattributed_outputs,
            }),
        ),
    }
}

fn render_project_operation(
    format: CliOutputFormat,
    view: &ProjectOperationView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => Ok(format!(
            "project_operation operation={} status={} project={} home={} command_id={} operation_id={} stage={} project_head={} error={}:{} runtime={}:{} external_state_warning={}\n",
            view.operation,
            view.status,
            encode_id(view.project_id.as_bytes()),
            encode_id(view.home.as_bytes()),
            encode_id(view.command_id.as_bytes()),
            encode_id(view.operation_id.as_bytes()),
            view.stage.unwrap_or("none"),
            optional_id(view.project_head.as_ref()),
            view.error_category.as_deref().unwrap_or("none"),
            view.error_code.as_deref().unwrap_or("none"),
            view.runtime_state.unwrap_or("none"),
            view.runtime_code.as_deref().unwrap_or("none"),
            view.external_state_warning.as_ref().map_or_else(
                || "none".to_owned(),
                |warning| format!(
                    "{}:{}:{}",
                    warning.kind, warning.destination, warning.branch
                ),
            ),
        )),
        CliOutputFormat::Json => machine_record(
            "project_operation",
            &serde_json::json!({
                "command_id": encode_id(view.command_id.as_bytes()),
                "error_category": view.error_category,
                "error_code": view.error_code,
                "external_state_warning": view.external_state_warning.as_ref().map(|warning| serde_json::json!({
                    "branch": warning.branch,
                    "destination": warning.destination,
                    "kind": warning.kind,
                })),
                "home": encode_id(view.home.as_bytes()),
                "operation": view.operation,
                "operation_id": encode_id(view.operation_id.as_bytes()),
                "project_head": view.project_head.map(|head| encode_id(head.as_bytes())),
                "project_id": encode_id(view.project_id.as_bytes()),
                "runtime_code": view.runtime_code,
                "runtime_state": view.runtime_state,
                "stage": view.stage,
                "status": view.status,
            }),
        ),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "complete line-oriented catalog presentation"
)]
fn render_project_catalog_human(view: &ProjectCatalogView) -> Result<String, CliError> {
    let mut output = format!(
        "project_catalog operation={} projects={} unattributed_dispatches={} unattributed_outputs={}\n",
        view.operation,
        view.projects.len(),
        view.unattributed_dispatches,
        view.unattributed_outputs,
    );
    for project in &view.projects {
        let project_id = encode_id(project.project_id.as_bytes());
        writeln!(
            output,
            "project id={} home={} account={} mailbox={} name={:?} lifecycle={} archived={} claimable={} head={} input_sequence={}",
            project_id,
            encode_id(project.home.as_bytes()),
            encode_id(project.account_id.as_bytes()),
            encode_id(project.mailbox.mailbox_id().as_bytes()),
            project.name,
            project.lifecycle,
            project.archived,
            project.claimable,
            encode_id(project.head.as_bytes()),
            project.input_sequence,
        )
        .map_err(|_| CliError::Runtime)?;
        if let Some(assignment) = &project.assignment {
            writeln!(
                output,
                "assignment project={} id={} agent={} provider={} session={} phase={} thread={} directory={} blocked={} cardinality_conflicted={} runnable={} support={}",
                project_id,
                encode_id(assignment.assignment_id.as_bytes()),
                encode_id(assignment.agent_id.as_bytes()),
                assignment.provider,
                assignment.session.as_deref().unwrap_or("none"),
                assignment.phase,
                assignment.thread_id.map_or_else(|| "none".to_owned(), |id| encode_id(id.as_bytes())),
                assignment.launch_directory.as_ref().map_or("none", |locator| locator.value.as_str()),
                assignment.blocked.as_deref().unwrap_or("none"),
                assignment.cardinality_conflicted,
                assignment.runnable,
                assignment.support.len(),
            )
            .map_err(|_| CliError::Runtime)?;
        }
        for thread in &project.threads {
            writeln!(
                output,
                "thread project={} agent={} provider={} session={} thread={}",
                project_id,
                encode_id(thread.agent_id.as_bytes()),
                thread.provider,
                thread.session,
                encode_id(thread.thread_id.as_bytes()),
            )
            .map_err(|_| CliError::Runtime)?;
        }
        for resource in &project.resources {
            writeln!(
                output,
                "resource project={} id={} display={}:{} canonical={}:{} health={} primary={} active_claim={} conflicts={}",
                project_id,
                encode_id(resource.resource_id.as_bytes()),
                resource_scheme_label(resource.display_locator.scheme),
                resource.display_locator.value,
                resource_scheme_label(resource.canonical_locator.scheme),
                resource.canonical_locator.value,
                resource.health,
                resource.primary,
                resource.active_claim,
                resource
                    .conflicting_projects
                    .iter()
                    .map(|id| encode_id(id.as_bytes()))
                    .collect::<Vec<_>>()
                    .join(","),
            )
            .map_err(|_| CliError::Runtime)?;
        }
        for input in &project.inputs {
            writeln!(
                output,
                "input project={} sequence={} message={} thread={} accepted_fact={}",
                project_id,
                input.sequence,
                encode_id(input.message_id.as_bytes()),
                encode_id(input.thread_id.as_bytes()),
                encode_id(input.accepted_fact.as_bytes()),
            )
            .map_err(|_| CliError::Runtime)?;
        }
        for dispatch in &project.dispatches {
            writeln!(
                output,
                "dispatch project={} sequence={} id={} message={} fact={} conflicted={}",
                project_id,
                dispatch.sequence,
                encode_id(dispatch.dispatch_id.as_bytes()),
                encode_id(dispatch.message_id.as_bytes()),
                encode_id(dispatch.fact_id.as_bytes()),
                dispatch.conflicted,
            )
            .map_err(|_| CliError::Runtime)?;
        }
        for item in &project.outputs {
            writeln!(
                output,
                "output project={} id={} dispatch={} status={} content={:?}",
                project_id,
                encode_id(item.output_id.as_bytes()),
                encode_id(item.dispatch_id.as_bytes()),
                item.status,
                item.content,
            )
            .map_err(|_| CliError::Runtime)?;
        }
        for command in &project.remote_commands {
            writeln!(
                output,
                "remote_command project={} id={} progress={} target_home={} expected_head={} operation_id={} provider={} session={} issued_at_unix_millis={} request_fact={} receipt_fact={} received_head={} received_at_unix_millis={} outcome_fact={} result_state={} result_value={} runtime_state={} runtime_code={} external_state_warning={}",
                project_id,
                encode_id(command.command_id.as_bytes()),
                command.progress,
                encode_id(command.target_home.as_bytes()),
                optional_id(command.expected_head.as_ref()),
                encode_id(command.operation_id.as_bytes()),
                command.operation_provider,
                command.operation_session,
                command.issued_at_unix_millis,
                encode_id(command.request_fact.as_bytes()),
                optional_id(command.receipt_fact.as_ref()),
                optional_id(command.received_head.as_ref()),
                command.received_at_unix_millis.map_or_else(|| "none".to_owned(), |value| value.to_string()),
                optional_id(command.outcome_fact.as_ref()),
                command.result_state.unwrap_or("none"),
                command.result_value.as_deref().unwrap_or("none"),
                command.runtime_state.unwrap_or("none"),
                command.runtime_code.as_deref().unwrap_or("none"),
                command.external_state_warning.as_ref().map_or_else(
                    || "none".to_owned(),
                    |warning| format!(
                        "{}:{}:{}",
                        warning.kind, warning.destination, warning.branch
                    ),
                ),
            )
            .map_err(|_| CliError::Runtime)?;
        }
    }
    Ok(output)
}

fn optional_id(id: Option<&FactId>) -> String {
    id.map_or_else(|| "none".to_owned(), |id| encode_id(id.as_bytes()))
}

fn project_json(project: &ProjectView) -> serde_json::Value {
    serde_json::json!({
        "assignment": project.assignment.as_ref().map(|assignment| serde_json::json!({
            "agent_id": encode_id(assignment.agent_id.as_bytes()),
            "assignment_id": encode_id(assignment.assignment_id.as_bytes()),
            "blocked": assignment.blocked,
            "cardinality_conflicted": assignment.cardinality_conflicted,
            "launch_directory": assignment.launch_directory,
            "phase": assignment.phase,
            "provider": assignment.provider,
            "runnable": assignment.runnable,
            "session": assignment.session,
            "support": assignment.support.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
            "thread_id": assignment.thread_id.map(|id| encode_id(id.as_bytes())),
        })),
        "archived": project.archived,
        "claimable": project.claimable,
        "dispatches": project.dispatches.iter().map(|item| serde_json::json!({
            "conflicted": item.conflicted,
            "dispatch_id": encode_id(item.dispatch_id.as_bytes()),
            "fact_id": encode_id(item.fact_id.as_bytes()),
            "message_id": encode_id(item.message_id.as_bytes()),
            "sequence": item.sequence,
        })).collect::<Vec<_>>(),
        "head": encode_id(project.head.as_bytes()),
        "home": encode_id(project.home.as_bytes()),
        "account_id": encode_id(project.account_id.as_bytes()),
        "mailbox_id": encode_id(project.mailbox.mailbox_id().as_bytes()),
        "input_sequence": project.input_sequence,
        "inputs": project.inputs.iter().map(|item| serde_json::json!({
            "accepted_fact": encode_id(item.accepted_fact.as_bytes()),
            "message_id": encode_id(item.message_id.as_bytes()),
            "sequence": item.sequence,
            "thread_id": encode_id(item.thread_id.as_bytes()),
        })).collect::<Vec<_>>(),
        "lifecycle": project.lifecycle,
        "name": project.name,
        "outputs": project.outputs.iter().map(|item| serde_json::json!({
            "content": item.content,
            "dispatch_id": encode_id(item.dispatch_id.as_bytes()),
            "output_id": encode_id(item.output_id.as_bytes()),
            "status": item.status,
        })).collect::<Vec<_>>(),
        "project_id": encode_id(project.project_id.as_bytes()),
        "remote_commands": project.remote_commands.iter().map(remote_project_command_json).collect::<Vec<_>>(),
        "resources": project.resources.iter().map(|resource| serde_json::json!({
            "active_claim": resource.active_claim,
            "canonical_locator": resource.canonical_locator,
            "conflicting_projects": resource.conflicting_projects.iter().map(|id| encode_id(id.as_bytes())).collect::<Vec<_>>(),
            "display_locator": resource.display_locator,
            "health": resource.health,
            "primary": resource.primary,
            "resource_id": encode_id(resource.resource_id.as_bytes()),
        })).collect::<Vec<_>>(),
        "threads": project.threads.iter().map(|thread| serde_json::json!({
            "agent_id": encode_id(thread.agent_id.as_bytes()),
            "provider": thread.provider,
            "session": thread.session,
            "thread_id": encode_id(thread.thread_id.as_bytes()),
        })).collect::<Vec<_>>(),
    })
}

fn remote_project_command_json(command: &RemoteProjectCommandView) -> serde_json::Value {
    serde_json::json!({
        "account_id": encode_id(command.account_id.as_bytes()),
        "command_id": encode_id(command.command_id.as_bytes()),
        "expected_head": command.expected_head.map(|id| encode_id(id.as_bytes())),
        "issued_at_unix_millis": command.issued_at_unix_millis,
        "operation_id": encode_id(command.operation_id.as_bytes()),
        "operation_provider": command.operation_provider,
        "operation_session": command.operation_session,
        "outcome_fact": command.outcome_fact.map(|id| encode_id(id.as_bytes())),
        "progress": command.progress,
        "receipt_fact": command.receipt_fact.map(|id| encode_id(id.as_bytes())),
        "received_at_unix_millis": command.received_at_unix_millis,
        "received_head": command.received_head.map(|id| encode_id(id.as_bytes())),
        "request_digest": encode_id(command.request_digest.as_bytes()),
        "request_fact": encode_id(command.request_fact.as_bytes()),
        "result_state": command.result_state,
        "result_value": command.result_value,
        "runtime_code": command.runtime_code,
        "runtime_state": command.runtime_state,
        "external_state_warning": command.external_state_warning.as_ref().map(|warning| serde_json::json!({
            "branch": warning.branch,
            "destination": warning.destination,
            "kind": warning.kind,
        })),
        "target_home": encode_id(command.target_home.as_bytes()),
    })
}

fn render_harness_session(
    format: CliOutputFormat,
    view: &HarnessSessionView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => Ok(format!(
            "harness={} agent={} provider={} requested_session={} session={} directory={} operation_id={} reconciliation_id={} error={}:{}\n",
            view.status,
            encode_id(view.agent_id.as_bytes()),
            view.provider,
            view.requested_session.as_deref().unwrap_or("none"),
            view.ready_session.as_deref().unwrap_or("none"),
            view.directory.as_deref().unwrap_or("none"),
            encode_id(view.operation_id.as_bytes()),
            view.reconciliation_id
                .as_ref()
                .map_or_else(|| "none".to_owned(), encode_id),
            view.error_category.as_deref().unwrap_or("none"),
            view.error_code.as_deref().unwrap_or("none"),
        )),
        CliOutputFormat::Json => machine_record(
            "harness_session",
            &serde_json::json!({
                "agent_id": encode_id(view.agent_id.as_bytes()),
                "directory": view.directory,
                "error_category": view.error_category,
                "error_code": view.error_code,
                "operation": view.operation,
                "operation_id": encode_id(view.operation_id.as_bytes()),
                "provider": view.provider,
                "ready_session": view.ready_session,
                "reconciliation_id": view.reconciliation_id.as_ref().map(encode_id),
                "requested_session": view.requested_session,
                "status": view.status,
            }),
        ),
    }
}

fn render_agent_result(
    format: CliOutputFormat,
    result: &CliResult,
) -> Option<Result<String, CliError>> {
    match (format, result) {
        (CliOutputFormat::Human, CliResult::AgentGuidance(topic)) => {
            Some(Ok(topic.text().to_owned()))
        }
        (CliOutputFormat::Json, CliResult::AgentGuidance(topic)) => Some(machine_record(
            "agent_guidance",
            &serde_json::json!({ "text": topic.text().trim_end(), "topic": topic.label() }),
        )),
        (format, CliResult::NamedAgentCatalog(view)) => {
            Some(render_named_agent_catalog(format, view))
        }
        (CliOutputFormat::Human, CliResult::NamedAgentRetirement(view)) => Some(Ok(format!(
            "retired agent={} force={} project={} runtime={} runtime_code={}\n",
            encode_id(view.agent_id.as_bytes()),
            view.force,
            view.project_id.map_or_else(
                || "none".to_owned(),
                |project| encode_id(project.as_bytes())
            ),
            view.runtime.as_deref().unwrap_or("none"),
            view.runtime_code.as_deref().unwrap_or("none"),
        ))),
        (CliOutputFormat::Json, CliResult::NamedAgentRetirement(view)) => Some(machine_record(
            "named_agent_retirement",
            &serde_json::json!({
                "agent_id": encode_id(view.agent_id.as_bytes()),
                "force": view.force,
                "project_id": view.project_id.map(|project| encode_id(project.as_bytes())),
                "runtime": view.runtime,
                "runtime_code": view.runtime_code,
            }),
        )),
        (CliOutputFormat::Human, CliResult::Completed { operation }) => {
            Some(Ok(format!("completed operation={operation}\n")))
        }
        (CliOutputFormat::Json, CliResult::Completed { operation }) => Some(machine_record(
            "completed",
            &serde_json::json!({ "operation": operation }),
        )),
        (CliOutputFormat::Human, CliResult::Stopped { intent }) => {
            Some(Ok(format!("stopped intent={intent}\n")))
        }
        _ => None,
    }
}

fn completion_for(invocation: &CliInvocation, result: &CliResult) -> Option<CliCompletion> {
    let CliCommand::AgentMessage { state, .. } = &invocation.command else {
        return None;
    };
    let CliResult::Messages(view) = result else {
        return None;
    };
    if !matches!(view.operation, "ask" | "wait" | "poll") || view.messages.is_empty() {
        return None;
    }
    let messages = view
        .messages
        .iter()
        .filter(|message| !message.incomplete)
        .map(|message| message.message_id)
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return None;
    }
    Some(CliCompletion {
        state: state.clone(),
        mailbox: view.mailbox?,
        messages,
    })
}

/// Completes ready-message delivery only after the caller has successfully written stdout.
pub fn complete_cli_delivery(completion: &CliCompletion) -> Result<(), CliError> {
    let mut client = command_client(&completion.state)?;
    let local = client.installation_id();
    if completion.mailbox.installation_id() != local {
        return Err(CliError::MessagingState);
    }
    for message_id in &completion.messages {
        let snapshot = messaging_snapshot(&mut client)?;
        let target = exact_message(load_all_messages(&mut client, &snapshot)?, *message_id)?;
        if !target.open {
            continue;
        }
        let plan = plan_message_archive(
            message_authority(&snapshot, local, completion.mailbox)?,
            stable_inputs(),
            MessageStateRequest {
                message_id: *message_id,
                target_fact: target.fact_id,
                state_frontier: target.state_frontier,
            },
        )?;
        submit_message_plan(&mut client, plan)?;
    }
    Ok(())
}

fn render_message_result(
    format: CliOutputFormat,
    view: &MessageCommandView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human if matches!(view.operation, "ask" | "wait") => {
            let mut output = String::new();
            for message in &view.messages {
                writeln!(output, "{}", message.content).map_err(|_| CliError::Runtime)?;
            }
            Ok(output)
        }
        CliOutputFormat::Human if view.operation == "send" => Ok(format!(
            "message={}\n",
            view.root_message.map_or_else(
                || "none".to_owned(),
                |message| encode_id(message.as_bytes())
            )
        )),
        CliOutputFormat::Human if view.operation == "project_send" => Ok(format!(
            "project={} message={}\n",
            view.project_id.map_or_else(
                || "none".to_owned(),
                |project| encode_id(project.as_bytes())
            ),
            view.root_message.map_or_else(
                || "none".to_owned(),
                |message| encode_id(message.as_bytes())
            )
        )),
        CliOutputFormat::Human => {
            let mut output = String::new();
            for message in &view.messages {
                writeln!(
                    output,
                    "message={} fact={} thread={} sender={}:{} recipient={} purpose={} open={} rejected={} ready={} cancelled={} incomplete={} missing={} unusable={} content={}",
                    encode_id(message.message_id.as_bytes()),
                    encode_id(message.fact_id.as_bytes()),
                    encode_id(message.thread_id.as_bytes()),
                    encode_id(message.sender.installation_id().as_bytes()),
                    encode_id(message.sender.mailbox_id().as_bytes()),
                    message.recipient.map_or_else(
                        || "none".to_owned(),
                        |recipient| format!(
                            "{}:{}",
                            encode_id(recipient.installation_id().as_bytes()),
                            encode_id(recipient.mailbox_id().as_bytes())
                        )
                    ),
                    message_purpose_label(message.purpose),
                    message.open,
                    message.rejected,
                    message.ready_answer,
                    message.thread_cancelled,
                    message.incomplete,
                    message.missing_dependencies.len(),
                    message.unusable_dependencies.len(),
                    serde_json::to_string(&message.content).map_err(|_| CliError::Runtime)?,
                )
                .map_err(|_| CliError::Runtime)?;
            }
            if view.incomplete_truncated {
                output.push_str("incomplete_history=truncated\n");
            }
            Ok(output)
        }
        CliOutputFormat::Json => machine_record(
            "messages",
            &serde_json::json!({
                "mailbox": view.mailbox.map(mailbox_json),
                "incomplete_truncated": view.incomplete_truncated,
                "messages": view.messages.iter().map(message_json).collect::<Vec<_>>(),
                "operation": view.operation,
                "project_id": view.project_id.map(|project| encode_id(project.as_bytes())),
                "root_message": view.root_message.map(|message| encode_id(message.as_bytes())),
            }),
        ),
    }
}

fn render_mailbox_discovery(
    format: CliOutputFormat,
    view: &MailboxDiscoveryView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => {
            let mut output = String::new();
            for candidate in &view.candidates {
                writeln!(
                    output,
                    "provider={} session={} mailbox={}:{} conflicted={} directory_match={} directories={}",
                    candidate.provider,
                    candidate.session,
                    encode_id(candidate.mailbox.installation_id().as_bytes()),
                    encode_id(candidate.mailbox.mailbox_id().as_bytes()),
                    candidate.conflicted,
                    candidate.directory_match,
                    candidate.directories.join(","),
                )
                .map_err(|_| CliError::Runtime)?;
            }
            Ok(output)
        }
        CliOutputFormat::Json => machine_record(
            "mailboxes",
            &serde_json::json!({
                "candidates": view.candidates.iter().map(|candidate| serde_json::json!({
                    "conflicted": candidate.conflicted,
                    "branches": candidate.branches,
                    "directories": candidate.directories,
                    "directory_match": candidate.directory_match,
                    "mailbox": mailbox_json(candidate.mailbox),
                    "provider": candidate.provider,
                    "repositories": candidate.repositories,
                    "session": candidate.session,
                    "worktrees": candidate.worktrees,
                })).collect::<Vec<_>>(),
                "directory": view.directory,
            }),
        ),
    }
}

fn render_named_agent_catalog(
    format: CliOutputFormat,
    view: &NamedAgentCatalogView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => {
            let mut output = String::new();
            if let Some((provider, session, mailbox, agent_id)) = &view.current {
                writeln!(
                    output,
                    "current provider={} session={} mailbox={}:{} agent={}",
                    provider,
                    session,
                    encode_id(mailbox.installation_id().as_bytes()),
                    encode_id(mailbox.mailbox_id().as_bytes()),
                    agent_id.map_or_else(|| "none".to_owned(), |agent| encode_id(agent.as_bytes())),
                )
                .map_err(|_| CliError::Runtime)?;
            }
            for agent in &view.agents {
                writeln!(
                    output,
                    "agent={} names={} mailboxes={} lifecycle={} runnable={}",
                    encode_id(agent.agent_id.as_bytes()),
                    agent.names.join(","),
                    agent
                        .mailboxes
                        .iter()
                        .map(|mailbox| format!(
                            "{}:{}",
                            encode_id(mailbox.installation_id().as_bytes()),
                            encode_id(mailbox.mailbox_id().as_bytes())
                        ))
                        .collect::<Vec<_>>()
                        .join(","),
                    agent.lifecycle,
                    agent.runnable,
                )
                .map_err(|_| CliError::Runtime)?;
                for session in &agent.sessions {
                    writeln!(
                        output,
                        "  session provider={} id={} mailbox={} selected={} conflicted={} name_resolved={} display_name={}",
                        session.provider,
                        session.session,
                        session.mailbox.map_or_else(
                            || "none".to_owned(),
                            |mailbox| format!(
                                "{}:{}",
                                encode_id(mailbox.installation_id().as_bytes()),
                                encode_id(mailbox.mailbox_id().as_bytes())
                            )
                        ),
                        session.selected,
                        session.conflicted,
                        session.name_resolved,
                        session.display_name.as_deref().unwrap_or("none"),
                    )
                    .map_err(|_| CliError::Runtime)?;
                }
            }
            Ok(output)
        }
        CliOutputFormat::Json => machine_record(
            "named_agents",
            &serde_json::json!({
                "agents": view.agents.iter().map(|agent| serde_json::json!({
                    "agent_id": encode_id(agent.agent_id.as_bytes()),
                    "lifecycle": agent.lifecycle,
                    "mailboxes": agent.mailboxes.iter().copied().map(mailbox_json).collect::<Vec<_>>(),
                    "names": agent.names,
                    "runnable": agent.runnable,
                    "sessions": agent.sessions.iter().map(|session| serde_json::json!({
                        "conflicted": session.conflicted,
                        "display_name": session.display_name,
                        "mailbox": session.mailbox.map(mailbox_json),
                        "name_resolved": session.name_resolved,
                        "provider": session.provider,
                        "selected": session.selected,
                        "session": session.session,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "current": view.current.as_ref().map(|(provider, session, mailbox, agent)| serde_json::json!({
                    "agent_id": agent.map(|agent| encode_id(agent.as_bytes())),
                    "mailbox": mailbox_json(*mailbox),
                    "provider": provider,
                    "session": session,
                })),
                "operation": view.operation,
            }),
        ),
    }
}

fn message_json(message: &CliMessageView) -> serde_json::Value {
    serde_json::json!({
        "cancelled": message.thread_cancelled,
        "content": message.content,
        "correlation": message.correlation.as_ref().map(|(provider, session, operation)| serde_json::json!({
            "operation_id": encode_id(operation),
            "provider": provider,
            "session": session,
        })),
        "fact_id": encode_id(message.fact_id.as_bytes()),
        "incomplete": message.incomplete,
        "message_id": encode_id(message.message_id.as_bytes()),
        "open": message.open,
        "purpose": message_purpose_label(message.purpose),
        "presentation": presentation_label(message.presentation),
        "project_id": message.project_id.map(|project| encode_id(project.as_bytes())),
        "ready_answer": message.ready_answer,
        "recipient": message.recipient.map(mailbox_json),
        "rejected": message.rejected,
        "root_fact": message.root_fact.map(|fact| encode_id(fact.as_bytes())),
        "root_message": message.root_message.map(|message| encode_id(message.as_bytes())),
        "sender": mailbox_json(message.sender),
        "state_frontier": message.state_frontier.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
        "missing_dependencies": message.missing_dependencies.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
        "thread_id": encode_id(message.thread_id.as_bytes()),
        "unusable_dependencies": message.unusable_dependencies.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
    })
}

fn mailbox_json(mailbox: MailboxAddress) -> serde_json::Value {
    serde_json::json!({
        "installation_id": encode_id(mailbox.installation_id().as_bytes()),
        "mailbox_id": encode_id(mailbox.mailbox_id().as_bytes()),
    })
}

const fn message_purpose_label(purpose: MessagePurposeDto) -> &'static str {
    match purpose {
        MessagePurposeDto::Question => "question",
        MessagePurposeDto::Asynchronous => "asynchronous",
        MessagePurposeDto::ProjectOutput => "project_output",
    }
}

const fn presentation_label(presentation: PresentationKindDto) -> &'static str {
    match presentation {
        PresentationKindDto::Message => "message",
        PresentationKindDto::FinalAnswer => "final_answer",
        PresentationKindDto::Status => "status",
    }
}

fn render_human_pairing(view: &HumanPairingView) -> String {
    format!(
        "completed operation={} account={} grant={} device={}\n",
        view.operation,
        encode_id(view.account_id.as_bytes()),
        encode_id(view.grant_id.as_bytes()),
        encode_id(view.device.as_bytes()),
    )
}

fn render_authority_admin_result(
    format: CliOutputFormat,
    view: &AuthorityAdminView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => render_authority_admin(view),
        CliOutputFormat::Json => machine_record("authority_admin", &authority_admin_json(view)),
    }
}

fn render_relay_admin_result(
    format: CliOutputFormat,
    view: &RelayAdminView,
) -> Result<String, CliError> {
    match format {
        CliOutputFormat::Human => render_relay_admin(view),
        CliOutputFormat::Json => machine_record("relay_admin", &relay_admin_json(view)),
    }
}

fn relay_admin_json(view: &RelayAdminView) -> serde_json::Value {
    serde_json::json!({
        "accepted": view.accepted,
        "domains": view.domains.iter().map(|domain| serde_json::json!({
            "conflicted": domain.conflicted,
            "conflicts": domain.conflicts,
            "domain": domain.domain,
            "invalid": domain.invalid,
            "projected": domain.projected,
            "unauthorized": domain.unauthorized,
            "unresolved": domain.unresolved,
            "unsupported": domain.unsupported,
        })).collect::<Vec<_>>(),
        "operation": view.operation,
        "operation_id": view.operation_id.map(|identity| encode_id(&identity)),
        "outcome": view.outcome,
        "policies": view.policies.iter().map(|policy| serde_json::json!({
            "access": policy.access,
            "authentication": policy.authentication,
            "enabled": policy.enabled,
            "endpoint": policy.endpoint,
            "generation": policy.generation,
        })).collect::<Vec<_>>(),
        "prepared": view.prepared,
        "quarantined": view.quarantined,
        "queued": view.queued,
        "rejected": view.rejected,
        "revision": view.revision,
        "staged": view.staged,
        "truncated": view.truncated,
        "uncertain": view.uncertain,
    })
}

fn render_relay_admin(view: &RelayAdminView) -> Result<String, CliError> {
    let operation_id = view
        .operation_id
        .map_or_else(|| "none".to_owned(), |identity| encode_id(&identity));
    let mut output = format!(
        "operation={} outcome={} operation_id={} revision={} policies={} queued={} prepared={} uncertain={} rejected={} accepted={} staged={} quarantined={} truncated={}\n",
        view.operation,
        view.outcome.as_deref().unwrap_or("none"),
        operation_id,
        view.revision,
        view.policies.len(),
        view.queued,
        view.prepared,
        view.uncertain,
        view.rejected,
        view.accepted,
        view.staged,
        view.quarantined,
        view.truncated,
    );
    for domain in &view.domains {
        writeln!(
            output,
            "domain={} projected={} unresolved={} unauthorized={} conflicted={} invalid={} unsupported={} conflicts={}",
            domain.domain,
            domain.projected,
            domain.unresolved,
            domain.unauthorized,
            domain.conflicted,
            domain.invalid,
            domain.unsupported,
            domain.conflicts,
        )
        .map_err(|_| CliError::Runtime)?;
    }
    for policy in &view.policies {
        writeln!(
            output,
            "relay={} access={} authentication={} enabled={} generation={}",
            policy.endpoint,
            policy.access,
            policy.authentication,
            policy.enabled,
            policy.generation,
        )
        .map_err(|_| CliError::Runtime)?;
    }
    Ok(output)
}

fn authority_admin_json(view: &AuthorityAdminView) -> serde_json::Value {
    serde_json::json!({
        "capabilities": view.capabilities.iter().map(|capability| serde_json::json!({
            "active": capability.active,
            "grant_fact": encode_id(capability.grant_fact.as_bytes()),
            "grant_id": encode_id(capability.grant_id.as_bytes()),
            "grantee_installation": encode_id(capability.grantee.installation_id().as_bytes()),
            "grantee_signing_key": encode_id(capability.grantee.signing_key().as_bytes()),
            "mailbox_id": encode_id(capability.mailbox.mailbox_id().as_bytes()),
            "mailbox_installation": encode_id(capability.mailbox.installation_id().as_bytes()),
            "observed_actions": capability.observed_actions.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
            "revoke_frontier": capability.revoke_frontier.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
            "support": capability.support.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "mailboxes": view.mailboxes.iter().map(|mailbox| serde_json::json!({
            "create_fact": encode_id(mailbox.create_fact.as_bytes()),
            "kind": mailbox.kind,
            "label": mailbox.label,
            "mailbox_id": encode_id(mailbox.address.mailbox_id().as_bytes()),
            "owner": encode_id(mailbox.address.installation_id().as_bytes()),
        })).collect::<Vec<_>>(),
        "operation": view.operation,
        "peers": view.peers.iter().map(peer_json).collect::<Vec<_>>(),
    })
}

fn peer_json(peer: &PeerRouteView) -> serde_json::Value {
    serde_json::json!({
        "blocks": peer.blocks.iter().map(|block| serde_json::json!({
            "fact_id": encode_id(block.fact_id.as_bytes()),
            "frontier_member": block.frontier_member,
            "reason": block.reason,
        })).collect::<Vec<_>>(),
        "frontier": peer.frontier.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
        "owner": encode_id(peer.owner.as_bytes()),
        "peer": encode_id(peer.peer.as_bytes()),
        "routes": peer.routes.iter().map(|route| serde_json::json!({
            "encryption_key": encode_id(route.encryption_key.as_bytes()),
            "fact_id": encode_id(route.fact_id.as_bytes()),
            "frontier_member": route.frontier_member,
            "label": route.label,
            "relay_hints": route.relay_hints.iter().map(|hint| serde_json::json!({"scheme": hint.scheme, "value": hint.value})).collect::<Vec<_>>(),
            "signing_key": encode_id(route.signing_key.as_bytes()),
        })).collect::<Vec<_>>(),
        "state": peer.state,
    })
}

fn device_json(device: &HumanDeviceView) -> serde_json::Value {
    serde_json::json!({
        "acceptances": device.acceptances.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
        "frontier": device.frontier.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
        "grants": device.grants.iter().map(|grant| serde_json::json!({
            "active": grant.active,
            "frontier_member": grant.frontier_member,
            "grant_fact": encode_id(grant.grant_fact.as_bytes()),
            "grant_id": encode_id(grant.grant_id.as_bytes()),
            "label": grant.label,
            "relay_hints": grant.relay_hints.iter().map(|hint| serde_json::json!({
                "scheme": hint.scheme,
                "value": hint.value,
            })).collect::<Vec<_>>(),
            "signing_key": encode_id(grant.signing_key.as_bytes()),
        })).collect::<Vec<_>>(),
        "installation_id": encode_id(device.installation_id.as_bytes()),
        "revokes": device.revokes.iter().map(|fact| encode_id(fact.as_bytes())).collect::<Vec<_>>(),
        "signing_keys": device.signing_keys.iter().map(|key| encode_id(key.as_bytes())).collect::<Vec<_>>(),
        "state": device.state.label(),
    })
}

fn encode_id(bytes: &[u8; 32]) -> String {
    crate::identity::encode_hex(bytes)
}

fn render_human_view(view: &HumanView) -> Result<String, CliError> {
    let active = view.active_account.map_or_else(
        || "none".to_owned(),
        |account| crate::identity::encode_hex(account.as_bytes()),
    );
    let candidates = view
        .selection_candidates
        .iter()
        .map(|account| crate::identity::encode_hex(account.as_bytes()))
        .collect::<Vec<_>>()
        .join(",");
    let mut output = format!(
        "installation={} active_account={} selection_candidates={} accounts={}\n",
        crate::identity::encode_hex(view.installation_id.as_bytes()),
        active,
        candidates,
        view.accounts.len(),
    );
    for account in &view.accounts {
        let label = serde_json::to_string(&account.label).map_err(|_| CliError::Runtime)?;
        writeln!(
            output,
            "account={} creator={} selected={} label={}",
            crate::identity::encode_hex(account.account_id.as_bytes()),
            crate::identity::encode_hex(account.creator_installation.as_bytes()),
            account.selected,
            label,
        )
        .map_err(|_| CliError::Runtime)?;
    }
    if view.active_account.is_some() {
        output.push_str("Next: run hq\n");
    }
    Ok(output)
}

fn render_human_devices(view: &HumanDevicesView) -> Result<String, CliError> {
    let mut output = format!(
        "account={} creator={} devices={}\n",
        encode_id(view.account_id.as_bytes()),
        encode_id(view.creator_installation.as_bytes()),
        view.devices.len(),
    );
    for device in &view.devices {
        writeln!(
            output,
            "device={} state={} keys={} frontier={} acceptances={} revokes={} grants={}",
            encode_id(device.installation_id.as_bytes()),
            device.state.label(),
            device
                .signing_keys
                .iter()
                .map(|key| encode_id(key.as_bytes()))
                .collect::<Vec<_>>()
                .join(","),
            device
                .frontier
                .iter()
                .map(|fact| encode_id(fact.as_bytes()))
                .collect::<Vec<_>>()
                .join(","),
            device
                .acceptances
                .iter()
                .map(|fact| encode_id(fact.as_bytes()))
                .collect::<Vec<_>>()
                .join(","),
            device
                .revokes
                .iter()
                .map(|fact| encode_id(fact.as_bytes()))
                .collect::<Vec<_>>()
                .join(","),
            device.grants.len(),
        )
        .map_err(|_| CliError::Runtime)?;
        for grant in &device.grants {
            let label = serde_json::to_string(&grant.label).map_err(|_| CliError::Runtime)?;
            writeln!(
                output,
                "grant={} fact={} key={} active={} frontier_member={} label={} relays={}",
                encode_id(grant.grant_id.as_bytes()),
                encode_id(grant.grant_fact.as_bytes()),
                encode_id(grant.signing_key.as_bytes()),
                grant.active,
                grant.frontier_member,
                label,
                grant
                    .relay_hints
                    .iter()
                    .map(|hint| format!("{}:{}", hint.scheme, hint.value))
                    .collect::<Vec<_>>()
                    .join(","),
            )
            .map_err(|_| CliError::Runtime)?;
        }
    }
    Ok(output)
}

fn render_authority_admin(view: &AuthorityAdminView) -> Result<String, CliError> {
    let mut output = format!(
        "operation={} peers={} mailboxes={} capabilities={}\n",
        view.operation,
        view.peers.len(),
        view.mailboxes.len(),
        view.capabilities.len(),
    );
    for peer in &view.peers {
        writeln!(
            output,
            "peer={} state={} frontier={} routes={} blocks={}",
            encode_id(peer.peer.as_bytes()),
            peer.state,
            peer.frontier
                .iter()
                .map(|fact| encode_id(fact.as_bytes()))
                .collect::<Vec<_>>()
                .join(","),
            peer.routes.len(),
            peer.blocks.len(),
        )
        .map_err(|_| CliError::Runtime)?;
    }
    for mailbox in &view.mailboxes {
        writeln!(
            output,
            "mailbox={}:{} fact={} kind={} label={}",
            encode_id(mailbox.address.installation_id().as_bytes()),
            encode_id(mailbox.address.mailbox_id().as_bytes()),
            encode_id(mailbox.create_fact.as_bytes()),
            mailbox.kind,
            serde_json::to_string(&mailbox.label).map_err(|_| CliError::Runtime)?,
        )
        .map_err(|_| CliError::Runtime)?;
    }
    for capability in &view.capabilities {
        writeln!(
            output,
            "capability={} fact={} mailbox={}:{} grantee={} active={} revokes={} observations={}",
            encode_id(capability.grant_id.as_bytes()),
            encode_id(capability.grant_fact.as_bytes()),
            encode_id(capability.mailbox.installation_id().as_bytes()),
            encode_id(capability.mailbox.mailbox_id().as_bytes()),
            encode_id(capability.grantee.installation_id().as_bytes()),
            capability.active,
            capability.revoke_frontier.len(),
            capability.observed_actions.len(),
        )
        .map_err(|_| CliError::Runtime)?;
    }
    Ok(output)
}

fn render_version(format: CliOutputFormat) -> Result<String, CliError> {
    let build = build()?;
    match format {
        CliOutputFormat::Human => Ok(format!(
            "{} {} protocol={} commit={}\n",
            build.name(),
            build.version(),
            hq_local_api::protocol::v1::V1,
            build.commit().unwrap_or("none"),
        )),
        CliOutputFormat::Json => machine_record(
            "version",
            &serde_json::json!({
                "commit": build.commit(),
                "name": build.name(),
                "protocol": hq_local_api::protocol::v1::V1,
                "version": build.version(),
            }),
        ),
    }
}

fn render_help(format: CliOutputFormat, topic: &[String]) -> Result<String, CliError> {
    let text = grammar::help(topic)?;
    match format {
        CliOutputFormat::Human => Ok(text),
        CliOutputFormat::Json => machine_record(
            "help",
            &serde_json::json!({ "text": text.trim_end(), "topic": topic }),
        ),
    }
}

fn render_error(format: CliOutputFormat, code: &str, message: &str, class: CliExitClass) -> String {
    match format {
        CliOutputFormat::Human => format!("hq: {code}: {message}\n"),
        CliOutputFormat::Json => serde_json::to_string(&serde_json::json!({
            "data": {
                "class": class.label(),
                "code": code,
                "message": message,
            },
            "kind": "error",
            "ok": false,
            "schema": "hq-cli-output-v1",
        }))
        .map_or_else(
            |_| {
                "{\"data\":{\"class\":\"failure\",\"code\":\"cli.runtime\",\"message\":\"the command runtime is unavailable\"},\"kind\":\"error\",\"ok\":false,\"schema\":\"hq-cli-output-v1\"}\n".to_owned()
            },
            |mut record| {
                record.push('\n');
                record
            },
        ),
    }
}

fn machine_record(kind: &str, data: &serde_json::Value) -> Result<String, CliError> {
    serde_json::to_string(&serde_json::json!({
        "data": data,
        "kind": kind,
        "ok": true,
        "schema": "hq-cli-output-v1",
    }))
    .map(|mut record| {
        record.push('\n');
        record
    })
    .map_err(|_| CliError::Runtime)
}

const fn lifecycle_state(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Starting => "starting",
        LifecycleState::Ready => "ready",
        LifecycleState::Draining => "draining",
        LifecycleState::Stopped => "stopped",
        LifecycleState::Failed => "failed",
    }
}

fn format_observation(label: &str, observation: &LifecycleObservation) -> String {
    let state = lifecycle_state(observation.status.state);
    let revision = observation
        .status
        .revision
        .map_or_else(|| "none".to_owned(), |revision| revision.to_string());
    let process = observation
        .readiness
        .as_ref()
        .map_or_else(|| "none".to_owned(), |ready| ready.process_id.to_string());
    format!("{label}={state} revision={revision} process={process}\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{collections::BTreeSet, ffi::OsString, path::Path, time::Duration};

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt as _;

    use super::{
        AgentMailboxSelection, AgentMessageCommand, CliCommand, CliError, CliMessageView,
        CliOutputFormat, ConfigurationCommand, DaemonCommand, HarnessCommand, HarnessSessionView,
        HumanCommand, HumanDeviceState, HumanMessageCommand, HumanMessageFilters, IdentityCommand,
        MailboxCommand, MessageCommandView, NamedAgentCommand, NamedAgentSelector, PeerCommand,
        ProjectCliCommand, ProjectResourceCliCommand, RelayCommand, WorktreeCliRequest,
        completion_for, copy_launch_environment, effect_outcome, ensure_resource_check_home,
        execute_cli, harness_request, human_devices_view, human_view, mailbox_discovery_view,
        message_body, named_agent_catalog_view, normalized_existing_resource, pairing_grant_id,
        parse_cli, project_activation_action, project_catalog_view, project_command_request,
        project_creation_request, project_handoff_action, project_operation_view,
        project_resource_catalog_view, project_resource_operation_identity, read_password,
        render_project_catalog, render_project_resource_catalog, render_project_resource_check,
        render_result, resolve_environment_session, resolve_named_agent_id,
        resource_inspection_request, run_cli, session_binding_fact, session_context,
        stable_relay_effect, stable_repair_operation, successful_result_exit_code,
    };
    use hq_application::ProjectCommandAction;
    use hq_domain::{
        AccountId, AgentId, FactId, InstallationAddress, InstallationId, MailboxAddress, MailboxId,
        MessageId, OperationId, ProjectId, ProviderId, ProviderSessionId, RelayHints, ResourceId,
        SigningPublicKey, ThreadId,
    };
    use hq_local_api::protocol::v1::{
        AgentLaunchContextDto, AgentSessionBindingDto, AuthoritativeSnapshotDto, DeviceGrantDto,
        DomainErrorDto, EffectOutcomeDto, Id32, MailboxAddressDto, MessagePurposeDto,
        PresentationKindDto, ProjectCommandActionDto, ProjectCommandOutcomeDto,
        ProjectCommandStageDto, ProjectExternalStateWarningDto, RelayAccessDto,
        RelayAuthenticationDto, RemoteCommandProgressDto, RemoteCommandResultDto,
        RepositoryContextDto, ResourceHealthDto, ResourceLocatorDto, ResourceSchemeDto,
        RuntimeObservationDto, SnapshotItem, SynchronizationRequestDto,
    };

    #[test]
    fn parser_accepts_explicit_managed_harness_operations() {
        let start = parse_cli([
            OsString::from("harness"),
            OsString::from("start"),
            OsString::from("--agent"),
            OsString::from("fred"),
            OsString::from("--provider"),
            OsString::from("codex"),
            OsString::from("--dir"),
            OsString::from("."),
        ])
        .expect("start parses");
        assert!(matches!(
            start.command,
            CliCommand::Harness {
                action: HarnessCommand::Start { agent: NamedAgentSelector::Name(name), provider, directory: Some(directory) },
                ..
            } if name.as_str() == "fred" && provider.as_str() == "codex" && directory == Path::new(".")
        ));

        let session = "session-exact";
        let resume = parse_cli([
            OsString::from("harness"),
            OsString::from("resume"),
            OsString::from("--provider"),
            OsString::from("fake"),
            OsString::from("--session"),
            OsString::from(session),
            OsString::from("--agent"),
            OsString::from("11".repeat(32)),
        ])
        .expect("resume parses");
        assert!(matches!(
            resume.command,
            CliCommand::Harness {
                action: HarnessCommand::Resume {
                    agent: NamedAgentSelector::Id(agent),
                    provider,
                    session: parsed_session,
                    directory: None,
                },
                ..
            } if agent.as_bytes() == &[0x11; 32]
                && provider.as_str() == "fake"
                && parsed_session.as_str() == session
        ));

        let stop = parse_cli([
            OsString::from("harness"),
            OsString::from("stop"),
            OsString::from("--agent"),
            OsString::from("fred"),
            OsString::from("--provider"),
            OsString::from("codex"),
        ])
        .expect("stop parses");
        assert!(matches!(
            stop.command,
            CliCommand::Harness {
                action: HarnessCommand::Stop { .. },
                ..
            }
        ));
    }

    #[test]
    fn parser_accepts_project_catalog_and_existing_resource_creation() {
        let listed = parse_cli([OsString::from("project"), OsString::from("list")])
            .expect("project list parses");
        assert!(matches!(
            listed.command,
            CliCommand::Project {
                action: ProjectCliCommand::List,
                ..
            }
        ));
        let shown = parse_cli([
            OsString::from("project"),
            OsString::from("show"),
            OsString::from("22".repeat(32)),
        ])
        .expect("project show parses");
        assert!(matches!(
            shown.command,
            CliCommand::Project {
                action: ProjectCliCommand::Show(project_id),
                ..
            } if project_id.as_bytes() == &[0x22; 32]
        ));
        let sent = parse_cli([
            OsString::from("project"),
            OsString::from("send"),
            OsString::from("22".repeat(32)),
            OsString::from("queued work"),
        ])
        .expect("project send parses");
        assert!(matches!(
            sent.command,
            CliCommand::Project {
                action: ProjectCliCommand::Send { project_id, body: Some(body) },
                ..
            } if project_id.as_bytes() == &[0x22; 32] && body.as_str() == "queued work"
        ));
        let created = parse_cli([
            OsString::from("project"),
            OsString::from("create"),
            OsString::from("catalog"),
            OsString::from("--path"),
            OsString::from("/work/catalog"),
            OsString::from("--brief"),
            OsString::from("exact work"),
            OsString::from("--home"),
            OsString::from("33".repeat(32)),
        ])
        .expect("project create parses");
        assert!(matches!(
            created.command,
            CliCommand::Project {
                action: ProjectCliCommand::Create { name, brief: Some(brief), path, home: Some(home) },
                ..
            } if name.as_str() == "catalog"
                && brief.as_str() == "exact work"
                && path == Path::new("/work/catalog")
                && home.as_bytes() == &[0x33; 32]
        ));
        for arguments in [
            vec!["project"],
            vec!["project", "show"],
            vec!["project", "list", "extra"],
            vec!["project", "show", "not-an-id"],
            vec!["project", "send"],
            vec!["project", "send", "not-an-id"],
            vec!["project", "send", &"22".repeat(32), "one", "two"],
            vec!["project", "create", "later"],
            vec!["project", "create", "name", "--path", "relative"],
        ] {
            assert_eq!(
                parse_cli(arguments.into_iter().map(OsString::from)),
                Err(CliError::Arguments)
            );
        }
    }

    #[test]
    fn parser_accepts_exact_worktree_provisioning_modes() {
        let created = parse_cli([
            OsString::from("project"),
            OsString::from("worktree"),
            OsString::from("feature-work"),
            OsString::from("--source"),
            OsString::from("/repo/source"),
            OsString::from("--destination"),
            OsString::from("/repo/worktrees/feature"),
            OsString::from("--branch"),
            OsString::from("feature/exact"),
            OsString::from("--create-branch"),
            OsString::from("refs/heads/main"),
            OsString::from("--brief"),
            OsString::from("bounded work"),
            OsString::from("--home"),
            OsString::from("33".repeat(32)),
        ])
        .expect("new-branch worktree parses");
        assert!(matches!(
            created.command,
            CliCommand::Project {
                action: ProjectCliCommand::Worktree(WorktreeCliRequest {
                    name,
                    brief: Some(brief),
                    source,
                    destination,
                    branch,
                    base: Some(base),
                    home: Some(home),
                }),
                ..
            } if name.as_str() == "feature-work"
                && brief.as_str() == "bounded work"
                && source == Path::new("/repo/source")
                && destination == Path::new("/repo/worktrees/feature")
                && branch.as_str() == "feature/exact"
                && base.as_str() == "refs/heads/main"
                && home.as_bytes() == &[0x33; 32]
        ));

        let existing = parse_cli([
            OsString::from("project"),
            OsString::from("worktree"),
            OsString::from("existing"),
            OsString::from("--source"),
            OsString::from("/repo/source"),
            OsString::from("--destination"),
            OsString::from("/repo/worktrees/existing"),
            OsString::from("--branch"),
            OsString::from("feature/existing"),
        ])
        .expect("existing-branch worktree parses");
        assert!(matches!(
            existing.command,
            CliCommand::Project {
                action: ProjectCliCommand::Worktree(WorktreeCliRequest { base: None, .. }),
                ..
            }
        ));

        for arguments in [
            vec!["project", "worktree", "name"],
            vec![
                "project",
                "worktree",
                "name",
                "--source",
                "relative",
                "--destination",
                "/worktree",
                "--branch",
                "feature",
            ],
            vec![
                "project",
                "worktree",
                "name",
                "--source",
                "/repo",
                "--destination",
                "/worktree",
                "--branch",
                "feature",
                "--create-branch",
            ],
        ] {
            assert_eq!(
                parse_cli(arguments.into_iter().map(OsString::from)),
                Err(CliError::Arguments)
            );
        }
    }

    #[test]
    fn parser_accepts_desired_resource_inspection_and_fresh_checks() {
        let project_id = ProjectId::from_bytes([0x22; 32]);
        let resource_id = hq_domain::ResourceId::from_bytes([0x33; 32]);

        for (arguments, expected) in [
            (
                vec!["project", "resource", "list", &"22".repeat(32)],
                ProjectCliCommand::Resource(ProjectResourceCliCommand::List { project_id }),
            ),
            (
                vec![
                    "project",
                    "resource",
                    "show",
                    &"22".repeat(32),
                    &"33".repeat(32),
                ],
                ProjectCliCommand::Resource(ProjectResourceCliCommand::Show {
                    project_id,
                    resource_id,
                }),
            ),
            (
                vec!["project", "check", &"22".repeat(32)],
                ProjectCliCommand::Check {
                    project_id,
                    resource_id: None,
                },
            ),
            (
                vec!["project", "check", &"22".repeat(32), &"33".repeat(32)],
                ProjectCliCommand::Check {
                    project_id,
                    resource_id: Some(resource_id),
                },
            ),
        ] {
            let parsed = parse_cli(arguments.into_iter().map(OsString::from))
                .expect("resource inspection parses");
            assert!(matches!(
                parsed.command,
                CliCommand::Project { action, .. } if action == expected
            ));
        }
    }

    #[test]
    fn parser_accepts_desired_resource_mutations() {
        let project_id = ProjectId::from_bytes([0x22; 32]);
        let resource_id = hq_domain::ResourceId::from_bytes([0x33; 32]);

        let added = parse_cli([
            OsString::from("project"),
            OsString::from("resource"),
            OsString::from("add"),
            OsString::from("22".repeat(32)),
            OsString::from("--path"),
            OsString::from("/work/added"),
            OsString::from("--primary"),
        ])
        .expect("resource add parses");
        assert!(matches!(
            added.command,
            CliCommand::Project {
                action: ProjectCliCommand::Resource(ProjectResourceCliCommand::Add {
                    project_id: candidate,
                    path,
                    make_primary: true,
                }),
                ..
            } if candidate == project_id && path == Path::new("/work/added")
        ));

        let removed = parse_cli([
            OsString::from("project"),
            OsString::from("resource"),
            OsString::from("remove"),
            OsString::from("22".repeat(32)),
            OsString::from("33".repeat(32)),
            OsString::from("--force"),
        ])
        .expect("resource remove parses");
        assert!(matches!(
            removed.command,
            CliCommand::Project {
                action: ProjectCliCommand::Resource(ProjectResourceCliCommand::Remove {
                    project_id: candidate_project,
                    resource_id: candidate_resource,
                    force: true,
                }),
                ..
            } if candidate_project == project_id && candidate_resource == resource_id
        ));

        let replaced = parse_cli([
            OsString::from("project"),
            OsString::from("resource"),
            OsString::from("replace"),
            OsString::from("22".repeat(32)),
            OsString::from("33".repeat(32)),
            OsString::from("--path"),
            OsString::from("/work/replaced"),
        ])
        .expect("resource replace parses");
        assert!(matches!(
            replaced.command,
            CliCommand::Project {
                action: ProjectCliCommand::Resource(ProjectResourceCliCommand::Replace {
                    project_id: candidate_project,
                    resource_id: candidate_resource,
                    path,
                }),
                ..
            } if candidate_project == project_id
                && candidate_resource == resource_id
                && path == Path::new("/work/replaced")
        ));

        let primary = parse_cli([
            OsString::from("project"),
            OsString::from("resource"),
            OsString::from("primary"),
            OsString::from("22".repeat(32)),
            OsString::from("33".repeat(32)),
        ])
        .expect("resource primary parses");
        assert!(matches!(
            primary.command,
            CliCommand::Project {
                action: ProjectCliCommand::Resource(ProjectResourceCliCommand::Primary {
                    project_id: candidate_project,
                    resource_id: candidate_resource,
                }),
                ..
            } if candidate_project == project_id && candidate_resource == resource_id
        ));

        let operation = OperationId::from_bytes([0x77; 32]);
        assert_eq!(
            project_resource_operation_identity(operation),
            project_resource_operation_identity(operation)
        );
        assert_ne!(
            project_resource_operation_identity(operation),
            project_resource_operation_identity(OperationId::from_bytes([0x78; 32]))
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "complete desired and observed resource fixture"
    )]
    fn desired_resource_views_preserve_identity_and_fresh_release_truth() {
        let project_id = ProjectId::from_bytes([0x22; 32]);
        let home = InstallationId::from_bytes([0x44; 32]);
        let first_id = ResourceId::from_bytes([0x31; 32]);
        let second_id = ResourceId::from_bytes([0x32; 32]);
        let locator = |value: &str| {
            ResourceLocatorDto::new(ResourceSchemeDto::WorkingTree, value.to_owned())
                .expect("locator")
        };
        let snapshot = AuthoritativeSnapshotDto {
            revision: 7,
            items: vec![
                SnapshotItem::Project {
                    project_id: Id32::new(*project_id.as_bytes()),
                    home: Id32::new(*home.as_bytes()),
                    account_id: Id32::new([0x10; 32]),
                    mailbox_id: Id32::new([0x11; 32]),
                    name: "resources".to_owned(),
                    lifecycle: "open".to_owned(),
                    archived: false,
                    claimable: false,
                    head: Id32::new([0x12; 32]),
                    input_sequence: 0,
                },
                SnapshotItem::ProjectResource {
                    project_id: Id32::new(*project_id.as_bytes()),
                    resource_id: Id32::new(*second_id.as_bytes()),
                    display_locator: locator("/display/second"),
                    canonical_locator: locator("/canonical/second"),
                    health: ResourceHealthDto::Unknown,
                    primary: false,
                    active_claim: true,
                    conflicting_projects: Vec::new(),
                },
                SnapshotItem::ProjectResource {
                    project_id: Id32::new(*project_id.as_bytes()),
                    resource_id: Id32::new(*first_id.as_bytes()),
                    display_locator: locator("/display/first"),
                    canonical_locator: locator("/canonical/first"),
                    health: ResourceHealthDto::Degraded,
                    primary: true,
                    active_claim: false,
                    conflicting_projects: vec![Id32::new([0x55; 32])],
                },
            ],
        };

        let listed = project_resource_catalog_view(
            &snapshot,
            &ProjectResourceCliCommand::List { project_id },
        )
        .expect("resource list");
        assert_eq!(listed.home, home);
        assert_eq!(
            listed
                .resources
                .iter()
                .map(|resource| resource.resource_id)
                .collect::<Vec<_>>(),
            [first_id, second_id]
        );
        assert!(listed.resources[0].primary);
        assert_eq!(
            listed.resources[0].conflicting_projects,
            [ProjectId::from_bytes([0x55; 32])]
        );
        let shown = project_resource_catalog_view(
            &snapshot,
            &ProjectResourceCliCommand::Show {
                project_id,
                resource_id: second_id,
            },
        )
        .expect("resource show");
        assert_eq!(shown.resources.len(), 1);
        assert_eq!(shown.resources[0].resource_id, second_id);

        let operation_id = OperationId::from_bytes([0x66; 32]);
        let request = resource_inspection_request(
            project_id,
            &listed.resources[0],
            operation_id,
            1_700_000_000_000,
        )
        .expect("inspection request");
        assert_eq!(request.body.resource_id.bytes(), *first_id.as_bytes());
        assert_ne!(request.request_digest.bytes(), [0; 32]);
        let repeated = resource_inspection_request(
            project_id,
            &listed.resources[0],
            operation_id,
            1_700_000_000_000,
        )
        .expect("repeat request");
        assert_eq!(request, repeated);

        let check = super::ProjectResourceCheckItemView {
            operation_id,
            resource_id: first_id,
            display_locator: locator("/display/first"),
            canonical_locator: locator("/canonical/first"),
            primary: true,
            active_claim: false,
            conflicting_projects: vec![ProjectId::from_bytes([0x55; 32])],
            status: "accepted",
            health: Some("degraded"),
            release: Some("dirty"),
            observed_canonical: Some(locator("/canonical/moved")),
            details: Some("canonical identity changed".to_owned()),
            checked_at_unix_millis: Some(1_700_000_000_001),
            error_category: None,
            error_code: None,
            reconciliation_id: None,
        };
        let checked = super::ProjectResourceCheckView {
            project_id,
            home,
            checks: vec![check],
        };
        let human =
            render_project_resource_check(CliOutputFormat::Human, &checked).expect("human check");
        assert!(human.contains("health=degraded release=dirty"));
        assert!(human.contains("observed=working_tree:/canonical/moved"));
        let json =
            render_project_resource_check(CliOutputFormat::Json, &checked).expect("JSON check");
        let record: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(record["kind"], "project_check");
        assert_eq!(record["data"]["checks"][0]["release"], "dirty");

        let static_human = render_project_resource_catalog(CliOutputFormat::Human, &listed)
            .expect("human resources");
        assert!(static_human.contains("primary=true"));
        assert!(static_human.contains(&"55".repeat(32)));
        assert_eq!(ensure_resource_check_home(home, home), Ok(()));
        assert_eq!(
            ensure_resource_check_home(InstallationId::from_bytes([0x45; 32]), home),
            Err(CliError::ResourceState)
        );

        let mut duplicated = snapshot.clone();
        duplicated.items.push(
            snapshot
                .items
                .iter()
                .find(|item| matches!(item, SnapshotItem::ProjectResource { .. }))
                .expect("resource item")
                .clone(),
        );
        assert_eq!(
            project_resource_catalog_view(
                &duplicated,
                &ProjectResourceCliCommand::List { project_id },
            ),
            Err(CliError::ProjectState)
        );
    }

    #[test]
    fn parser_accepts_project_lifecycle_commands_and_requires_close_confirmation() {
        let project = "22".repeat(32);
        for (arguments, expected) in [
            (
                vec!["project", "open", &project],
                ProjectCliCommand::Open(ProjectId::from_bytes([0x22; 32])),
            ),
            (
                vec!["project", "archive", &project],
                ProjectCliCommand::Archive(ProjectId::from_bytes([0x22; 32])),
            ),
            (
                vec!["project", "unarchive", &project],
                ProjectCliCommand::Unarchive(ProjectId::from_bytes([0x22; 32])),
            ),
            (
                vec!["project", "close", &project, "--yes"],
                ProjectCliCommand::Close {
                    project_id: ProjectId::from_bytes([0x22; 32]),
                    force: false,
                },
            ),
            (
                vec!["project", "close", &project, "--force", "--yes"],
                ProjectCliCommand::Close {
                    project_id: ProjectId::from_bytes([0x22; 32]),
                    force: true,
                },
            ),
        ] {
            let parsed = parse_cli(arguments.into_iter().map(OsString::from))
                .expect("lifecycle command parses");
            assert!(
                matches!(parsed.command, CliCommand::Project { action, .. } if action == expected)
            );
        }
        for arguments in [
            vec!["project", "close", &project],
            vec!["project", "close", &project, "--force"],
            vec!["project", "open", &project, "--yes"],
            vec!["project", "archive", &project, "--force"],
        ] {
            assert_eq!(
                parse_cli(arguments.into_iter().map(OsString::from)),
                Err(CliError::Arguments)
            );
        }
    }

    #[test]
    fn parser_accepts_project_activation_and_pending_dispatch() {
        let project = "22".repeat(32);
        let thread = "33".repeat(32);
        let fresh = parse_cli(
            [
                "project",
                "activate",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--new-session",
                "--dir",
                "/work/project",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .expect("fresh activation parses");
        assert!(matches!(fresh.command, CliCommand::Project {
            action: ProjectCliCommand::Activate {
                project_id,
                agent: NamedAgentSelector::Name(name),
                provider,
                resume_session: None,
                resume_thread: None,
                directory: Some(directory),
            }, ..
        } if project_id.as_bytes() == &[0x22; 32]
            && name.as_str() == "fred"
            && provider.as_str() == "codex"
            && directory == Path::new("/work/project")));

        let resumed = parse_cli(
            [
                "project",
                "activate",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--session",
                "session-1",
                "--thread",
                &thread,
            ]
            .into_iter()
            .map(OsString::from),
        )
        .expect("exact resume parses");
        assert!(matches!(resumed.command, CliCommand::Project {
            action: ProjectCliCommand::Activate {
                resume_session: Some(session),
                resume_thread: Some(thread),
                directory: None,
                ..
            }, ..
        } if session.as_str() == "session-1" && thread.as_bytes() == &[0x33; 32]));

        let dispatch = parse_cli([
            OsString::from("project"),
            OsString::from("dispatch"),
            OsString::from(&project),
        ])
        .expect("pending dispatch parses");
        assert!(matches!(dispatch.command, CliCommand::Project {
            action: ProjectCliCommand::Dispatch(project_id), ..
        } if project_id.as_bytes() == &[0x22; 32]));
    }

    #[test]
    fn parser_rejects_ambiguous_project_activation_sessions() {
        let project = "22".repeat(32);
        let thread = "33".repeat(32);
        for arguments in [
            vec![
                "project",
                "activate",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
            ],
            vec![
                "project",
                "activate",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--session",
                "session-1",
            ],
            vec![
                "project",
                "activate",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--new-session",
                "--session",
                "session-1",
                "--thread",
                &thread,
            ],
            vec![
                "project",
                "activate",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--new-session",
                "--dir",
                "relative",
            ],
            vec!["project", "dispatch", &project, "extra"],
        ] {
            assert_eq!(
                parse_cli(arguments.into_iter().map(OsString::from)),
                Err(CliError::Arguments)
            );
        }
    }

    #[test]
    fn parser_requires_confirmed_project_handoff_and_separate_force() {
        let project = "22".repeat(32);
        let thread = "33".repeat(32);
        let parsed = parse_cli(
            [
                "project",
                "handoff",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--new-session",
                "--thread",
                &thread,
                "--yes",
                "--force",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .expect("forced handoff parses");
        assert!(matches!(parsed.command, CliCommand::Project {
            action: ProjectCliCommand::Handoff {
                project_id,
                agent: NamedAgentSelector::Name(name),
                provider,
                resume_session: None,
                thread_id,
                directory: None,
                force: true,
            }, ..
        } if project_id.as_bytes() == &[0x22; 32]
            && name.as_str() == "fred"
            && provider.as_str() == "codex"
            && thread_id.as_bytes() == &[0x33; 32]));

        for arguments in [
            vec![
                "project",
                "handoff",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--new-session",
                "--thread",
                &thread,
            ],
            vec![
                "project",
                "handoff",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--new-session",
                "--yes",
            ],
            vec![
                "project",
                "handoff",
                &project,
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--session",
                "session-1",
                "--thread",
                &thread,
                "--force",
            ],
        ] {
            assert_eq!(
                parse_cli(arguments.into_iter().map(OsString::from)),
                Err(CliError::Arguments)
            );
        }
    }

    #[test]
    fn project_command_request_binds_exact_snapshot_authority_and_head() {
        let command_id = hq_domain::CommandId::from_bytes([1; 32]);
        let operation_id = OperationId::from_bytes([2; 32]);
        let account_id = AccountId::from_bytes([3; 32]);
        let project_id = ProjectId::from_bytes([4; 32]);
        let home = InstallationId::from_bytes([5; 32]);
        let head = FactId::from_bytes([6; 32]);
        let first = project_command_request(
            command_id,
            operation_id,
            account_id,
            project_id,
            home,
            Some(head),
            1_700_000_000_000,
            ProjectCommandAction::Close { force: true },
        )
        .expect("project command request");
        let repeated = project_command_request(
            command_id,
            operation_id,
            account_id,
            project_id,
            home,
            Some(head),
            1_700_000_000_000,
            ProjectCommandAction::Close { force: true },
        )
        .expect("repeated request");
        assert_eq!(first, repeated);
        assert_eq!(first.account_id.bytes(), *account_id.as_bytes());
        assert_eq!(first.project_id.bytes(), *project_id.as_bytes());
        assert_eq!(first.home.bytes(), *home.as_bytes());
        assert_eq!(first.expected_head.map(Id32::bytes), Some(*head.as_bytes()));
        assert!(matches!(
            first.action,
            ProjectCommandActionDto::Close { force: true }
        ));

        let changed = project_command_request(
            command_id,
            operation_id,
            account_id,
            project_id,
            home,
            Some(FactId::from_bytes([7; 32])),
            1_700_000_000_000,
            ProjectCommandAction::Close { force: true },
        )
        .expect("changed-head request");
        assert_ne!(first.request_digest, changed.request_digest);
    }

    #[test]
    fn project_activation_resolves_exact_authoritative_binding_and_primary_directory() {
        let snapshot = activation_snapshot();
        let provider = ProviderId::new("fake").expect("provider");
        let session = ProviderSessionId::new("session-1").expect("session");
        let action = project_activation_action(
            &snapshot,
            ProjectId::from_bytes([1; 32]),
            &NamedAgentSelector::Name(hq_domain::ShortText::new("fred").expect("name")),
            &provider,
            Some(&session),
            Some(ThreadId::from_bytes([9; 32])),
            None,
        )
        .expect("exact activation resolves");
        assert!(matches!(action, ProjectCommandAction::Activate {
            agent_id,
            provider: selected_provider,
            resume_session: Some(selected_session),
            resume_thread: Some(thread),
            launch_directory,
        } if agent_id.as_bytes() == &[4; 32]
            && selected_provider == provider
            && selected_session == session
            && thread.as_bytes() == &[9; 32]
            && launch_directory.value() == "/work/project"));
    }

    #[test]
    fn project_activation_rejects_session_thread_mismatches() {
        let snapshot = activation_snapshot();
        let provider = ProviderId::new("fake").expect("provider");
        let session = ProviderSessionId::new("session-1").expect("session");
        for thread in [None, Some(ThreadId::from_bytes([8; 32]))] {
            assert_eq!(
                project_activation_action(
                    &snapshot,
                    ProjectId::from_bytes([1; 32]),
                    &NamedAgentSelector::Id(AgentId::from_bytes([4; 32])),
                    &provider,
                    Some(&session),
                    thread,
                    None,
                ),
                Err(if thread.is_none() {
                    CliError::Arguments
                } else {
                    CliError::ProjectState
                })
            );
        }
        let wrong_session = ProviderSessionId::new("session-2").expect("session");
        assert_eq!(
            project_activation_action(
                &snapshot,
                ProjectId::from_bytes([1; 32]),
                &NamedAgentSelector::Id(AgentId::from_bytes([4; 32])),
                &provider,
                Some(&wrong_session),
                Some(ThreadId::from_bytes([9; 32])),
                None,
            ),
            Err(CliError::AgentState)
        );
    }

    #[test]
    fn project_activation_accepts_an_exact_undispatched_input_thread() {
        let mut snapshot = activation_snapshot();
        snapshot.items.retain(|item| {
            !matches!(
                item,
                SnapshotItem::ProjectThread { .. } | SnapshotItem::ProjectAssignment { .. }
            )
        });
        if let Some(SnapshotItem::Project { input_sequence, .. }) = snapshot
            .items
            .iter_mut()
            .find(|item| matches!(item, SnapshotItem::Project { .. }))
        {
            *input_sequence = 1;
        }
        snapshot.items.push(SnapshotItem::ProjectInput {
            project_id: Id32::new([1; 32]),
            message_id: Id32::new([8; 32]),
            thread_id: Id32::new([9; 32]),
            sequence: 1,
            accepted_fact: Id32::new([10; 32]),
        });
        let provider = ProviderId::new("fake").expect("provider");

        let action = project_activation_action(
            &snapshot,
            ProjectId::from_bytes([1; 32]),
            &NamedAgentSelector::Id(AgentId::from_bytes([4; 32])),
            &provider,
            None,
            Some(ThreadId::from_bytes([9; 32])),
            None,
        )
        .expect("pending input thread resolves");
        assert!(matches!(
            action,
            ProjectCommandAction::Activate {
                resume_session: None,
                resume_thread: Some(thread),
                ..
            } if thread.as_bytes() == &[9; 32]
        ));

        snapshot.items.push(SnapshotItem::ProjectDispatch {
            dispatch_id: Id32::new([11; 32]),
            message_id: Id32::new([8; 32]),
            sequence: 1,
            fact_id: Id32::new([12; 32]),
            conflicted: false,
        });
        assert_eq!(
            project_activation_action(
                &snapshot,
                ProjectId::from_bytes([1; 32]),
                &NamedAgentSelector::Id(AgentId::from_bytes([4; 32])),
                &provider,
                None,
                Some(ThreadId::from_bytes([9; 32])),
                None,
            ),
            Err(CliError::ProjectState)
        );
    }

    #[test]
    fn project_handoff_requires_an_assignment_and_preserves_takeover_authorization() {
        let snapshot = activation_snapshot();
        let provider = ProviderId::new("fake").expect("provider");
        let action = project_handoff_action(
            &snapshot,
            ProjectId::from_bytes([1; 32]),
            &NamedAgentSelector::Id(AgentId::from_bytes([4; 32])),
            &provider,
            None,
            ThreadId::from_bytes([9; 32]),
            None,
            true,
        )
        .expect("handoff target resolves");
        assert!(matches!(action, ProjectCommandAction::Handoff {
            agent_id,
            provider: selected_provider,
            resume_session: None,
            thread_id,
            force_takeover: true,
            ..
        } if agent_id.as_bytes() == &[4; 32]
            && selected_provider == provider
            && thread_id.as_bytes() == &[9; 32]));

        let mut unassigned = snapshot;
        unassigned
            .items
            .retain(|item| !matches!(item, SnapshotItem::ProjectAssignment { .. }));
        assert_eq!(
            project_handoff_action(
                &unassigned,
                ProjectId::from_bytes([1; 32]),
                &NamedAgentSelector::Id(AgentId::from_bytes([4; 32])),
                &provider,
                None,
                ThreadId::from_bytes([9; 32]),
                None,
                false,
            ),
            Err(CliError::ProjectState)
        );
    }

    fn activation_snapshot() -> AuthoritativeSnapshotDto {
        AuthoritativeSnapshotDto {
            revision: 1,
            items: vec![
                SnapshotItem::Project {
                    project_id: Id32::new([1; 32]),
                    home: Id32::new([2; 32]),
                    account_id: Id32::new([3; 32]),
                    mailbox_id: Id32::new([30; 32]),
                    name: "project".to_owned(),
                    lifecycle: "open".to_owned(),
                    archived: false,
                    claimable: true,
                    head: Id32::new([31; 32]),
                    input_sequence: 0,
                },
                SnapshotItem::ProjectResource {
                    project_id: Id32::new([1; 32]),
                    resource_id: Id32::new([32; 32]),
                    display_locator: ResourceLocatorDto::new(
                        ResourceSchemeDto::WorkingTree,
                        "/work/project".to_owned(),
                    )
                    .expect("display locator"),
                    canonical_locator: ResourceLocatorDto::new(
                        ResourceSchemeDto::WorkingTree,
                        "/work/project".to_owned(),
                    )
                    .expect("canonical locator"),
                    health: ResourceHealthDto::Healthy,
                    primary: true,
                    active_claim: true,
                    conflicting_projects: vec![],
                },
                SnapshotItem::ProjectAssignment {
                    project_id: Id32::new([1; 32]),
                    assignment_id: Id32::new([40; 32]),
                    agent_id: Id32::new([41; 32]),
                    provider: "fake".to_owned(),
                    session: Some("current-session".to_owned()),
                    phase: "runnable".to_owned(),
                    thread_id: Some(Id32::new([42; 32])),
                    launch_directory: Some(
                        ResourceLocatorDto::new(
                            ResourceSchemeDto::WorkingTree,
                            "/work/project".to_owned(),
                        )
                        .expect("launch directory"),
                    ),
                    blocked: None,
                    cardinality_conflicted: false,
                    runnable: true,
                    support: vec![Id32::new([43; 32])],
                },
                SnapshotItem::Agent {
                    agent_id: Id32::new([4; 32]),
                    claims: vec![Id32::new([5; 32])],
                    names: vec!["fred".to_owned()],
                    mailboxes: vec![MailboxAddressDto {
                        installation_id: Id32::new([2; 32]),
                        mailbox_id: Id32::new([6; 32]),
                    }],
                    retirements: vec![],
                    lifecycle: "active".to_owned(),
                    runnable: true,
                },
                SnapshotItem::AgentSession {
                    provider: "fake".to_owned(),
                    session: "session-1".to_owned(),
                    bindings: vec![AgentSessionBindingDto {
                        fact_id: Id32::new([7; 32]),
                        mailbox: MailboxAddressDto {
                            installation_id: Id32::new([2; 32]),
                            mailbox_id: Id32::new([6; 32]),
                        },
                    }],
                    mailbox_installation: Some(Id32::new([2; 32])),
                    mailbox_id: Some(Id32::new([6; 32])),
                    conflicted: false,
                },
                SnapshotItem::ProjectThread {
                    project_id: Id32::new([1; 32]),
                    agent_id: Id32::new([4; 32]),
                    provider: "fake".to_owned(),
                    session: "session-1".to_owned(),
                    thread_id: Id32::new([9; 32]),
                },
            ],
        }
    }

    #[test]
    fn project_creation_request_has_stable_identity_and_exact_content_digest() {
        let command_id = hq_domain::CommandId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([2; 32]);
        let home = InstallationId::from_bytes([3; 32]);
        let name = hq_domain::ShortText::new("catalog").expect("name");
        let brief = hq_domain::ContentText::new("exact work").expect("brief");
        let resource =
            normalized_existing_resource(Path::new("/work/catalog")).expect("normalized resource");
        let first = project_creation_request(
            command_id,
            account_id,
            home,
            &name,
            Some(&brief),
            &resource,
            1_700_000_000_000,
        )
        .expect("creation request");
        let repeated = project_creation_request(
            command_id,
            account_id,
            home,
            &name,
            Some(&brief),
            &resource,
            1_700_000_000_000,
        )
        .expect("repeated request");
        assert_eq!(first, repeated);
        assert_eq!(first.0.expected_head, None);
        assert!(matches!(
            &first.0.action,
            ProjectCommandActionDto::Create(request)
                if request.project_name == "catalog"
                    && request.brief.as_deref() == Some("exact work")
                    && request.resource.value == "/work/catalog"
        ));

        let changed_name = hq_domain::ShortText::new("changed").expect("changed name");
        let changed = project_creation_request(
            command_id,
            account_id,
            home,
            &changed_name,
            Some(&brief),
            &resource,
            1_700_000_000_000,
        )
        .expect("changed request");
        assert_eq!(first.1, changed.1);
        assert_eq!(first.2, changed.2);
        assert_ne!(first.0.request_digest, changed.0.request_digest);
    }

    #[test]
    fn project_operation_result_preserves_terminal_and_reconcilable_semantics() {
        let command_id = hq_domain::CommandId::from_bytes([1; 32]);
        let operation_id = OperationId::from_bytes([2; 32]);
        let project_id = ProjectId::from_bytes([3; 32]);
        let home = InstallationId::from_bytes([4; 32]);
        let rejected = project_operation_view(
            "close",
            command_id,
            project_id,
            home,
            operation_id,
            ProjectCommandOutcomeDto::Rejected {
                operation_id: Id32::new(*operation_id.as_bytes()),
                error: DomainErrorDto::new("project".to_owned(), "resource_conflict".to_owned())
                    .expect("domain error"),
                runtime: Some(RuntimeObservationDto::Failed("stop_failed".to_owned())),
                external_state_warning: Some(ProjectExternalStateWarningDto::WorktreeMayExist {
                    destination: ResourceLocatorDto::new(
                        ResourceSchemeDto::WorkingTree,
                        "/repo/worktree".to_owned(),
                    )
                    .expect("warning locator"),
                    branch: "feature/exact".to_owned(),
                }),
            },
        )
        .expect("rejected view");
        let rejected_result = super::CliResult::ProjectOperation(rejected);
        assert_eq!(successful_result_exit_code(&rejected_result, false), 1);
        let output = render_result(CliOutputFormat::Json, &rejected_result).expect("JSON result");
        let value: serde_json::Value = serde_json::from_str(&output).expect("JSON");
        assert_eq!(value["kind"], "project_operation");
        assert_eq!(value["data"]["status"], "rejected");
        assert_eq!(value["data"]["error_category"], "project");
        assert_eq!(value["data"]["error_code"], "resource_conflict");
        assert_eq!(value["data"]["runtime_state"], "failed");
        assert_eq!(value["data"]["runtime_code"], "stop_failed");
        assert_eq!(
            value["data"]["external_state_warning"]["kind"],
            "worktree_may_exist"
        );
        assert_eq!(
            value["data"]["external_state_warning"]["destination"],
            "/repo/worktree"
        );

        let uncertain = project_operation_view(
            "close",
            command_id,
            project_id,
            home,
            operation_id,
            ProjectCommandOutcomeDto::Completed {
                operation_id: Id32::new(*operation_id.as_bytes()),
                project_head: Id32::new([5; 32]),
                runtime: Some(RuntimeObservationDto::Uncertain("stop_unknown".to_owned())),
            },
        )
        .expect("uncertain runtime view");
        assert_eq!(uncertain.status, "completed");
        assert_eq!(uncertain.runtime_state, Some("uncertain"));
        assert_eq!(uncertain.runtime_code.as_deref(), Some("stop_unknown"));

        let reconcilable = project_operation_view(
            "create",
            command_id,
            project_id,
            home,
            operation_id,
            ProjectCommandOutcomeDto::Reconcilable {
                operation_id: Id32::new(*operation_id.as_bytes()),
                stage: ProjectCommandStageDto::IdentifyingResource,
                error: DomainErrorDto::new("effect".to_owned(), "outcome_unknown".to_owned())
                    .expect("domain error"),
                external_state_warning: None,
            },
        )
        .expect("reconcilable view");
        let reconcilable_result = super::CliResult::ProjectOperation(reconcilable);
        assert_eq!(successful_result_exit_code(&reconcilable_result, false), 3);
        let output = render_result(CliOutputFormat::Json, &reconcilable_result).expect("JSON");
        let value: serde_json::Value = serde_json::from_str(&output).expect("JSON");
        assert!(value["data"]["external_state_warning"].is_null());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "complete heterogeneous snapshot fixture"
    )]
    fn project_catalog_preserves_conflicts_attribution_and_remote_checkpoints() {
        let snapshot = AuthoritativeSnapshotDto {
            revision: 19,
            items: vec![
                SnapshotItem::Project {
                    project_id: Id32::new([2; 32]),
                    home: Id32::new([12; 32]),
                    account_id: Id32::new([10; 32]),
                    mailbox_id: Id32::new([42; 32]),
                    name: "second".to_owned(),
                    lifecycle: "closed".to_owned(),
                    archived: true,
                    claimable: true,
                    head: Id32::new([22; 32]),
                    input_sequence: 0,
                },
                SnapshotItem::Project {
                    project_id: Id32::new([1; 32]),
                    home: Id32::new([11; 32]),
                    account_id: Id32::new([10; 32]),
                    mailbox_id: Id32::new([41; 32]),
                    name: "first project".to_owned(),
                    lifecycle: "conflicted".to_owned(),
                    archived: false,
                    claimable: false,
                    head: Id32::new([21; 32]),
                    input_sequence: 7,
                },
                SnapshotItem::ProjectAssignment {
                    project_id: Id32::new([1; 32]),
                    assignment_id: Id32::new([81; 32]),
                    agent_id: Id32::new([82; 32]),
                    provider: "codex".to_owned(),
                    session: Some("session-7".to_owned()),
                    phase: "runnable".to_owned(),
                    thread_id: Some(Id32::new([83; 32])),
                    launch_directory: Some(
                        ResourceLocatorDto::new(
                            ResourceSchemeDto::WorkingTree,
                            "/repo/work".to_owned(),
                        )
                        .expect("launch directory"),
                    ),
                    blocked: None,
                    cardinality_conflicted: false,
                    runnable: true,
                    support: vec![Id32::new([84; 32])],
                },
                SnapshotItem::ProjectThread {
                    project_id: Id32::new([1; 32]),
                    agent_id: Id32::new([82; 32]),
                    provider: "codex".to_owned(),
                    session: "session-7".to_owned(),
                    thread_id: Id32::new([83; 32]),
                },
                SnapshotItem::ProjectResource {
                    project_id: Id32::new([1; 32]),
                    resource_id: Id32::new([31; 32]),
                    display_locator: ResourceLocatorDto::new(
                        ResourceSchemeDto::WorkingTree,
                        "./work".to_owned(),
                    )
                    .expect("display locator"),
                    canonical_locator: ResourceLocatorDto::new(
                        ResourceSchemeDto::WorkingTree,
                        "/repo/work".to_owned(),
                    )
                    .expect("canonical locator"),
                    health: ResourceHealthDto::Degraded,
                    primary: true,
                    active_claim: false,
                    conflicting_projects: vec![Id32::new([2; 32])],
                },
                SnapshotItem::ProjectInput {
                    project_id: Id32::new([1; 32]),
                    message_id: Id32::new([41; 32]),
                    thread_id: Id32::new([43; 32]),
                    sequence: 7,
                    accepted_fact: Id32::new([42; 32]),
                },
                SnapshotItem::ProjectDispatch {
                    dispatch_id: Id32::new([51; 32]),
                    message_id: Id32::new([41; 32]),
                    sequence: 7,
                    fact_id: Id32::new([52; 32]),
                    conflicted: true,
                },
                SnapshotItem::ProjectDispatch {
                    dispatch_id: Id32::new([53; 32]),
                    message_id: Id32::new([99; 32]),
                    sequence: 8,
                    fact_id: Id32::new([54; 32]),
                    conflicted: false,
                },
                SnapshotItem::ProjectOutput {
                    output_id: Id32::new([61; 32]),
                    dispatch_id: Id32::new([51; 32]),
                    status: "conflicted".to_owned(),
                    content: "retained output".to_owned(),
                },
                SnapshotItem::ProjectOutput {
                    output_id: Id32::new([62; 32]),
                    dispatch_id: Id32::new([98; 32]),
                    status: "late".to_owned(),
                    content: "unjoinable".to_owned(),
                },
                SnapshotItem::RemoteCommand {
                    command_id: Id32::new([71; 32]),
                    request_digest: Id32::new([72; 32]),
                    account_id: Id32::new([73; 32]),
                    project_id: Id32::new([1; 32]),
                    target_home: Id32::new([11; 32]),
                    expected_head: Some(Id32::new([21; 32])),
                    operation_provider: "codex".to_owned(),
                    operation_session: "session-7".to_owned(),
                    operation_id: Id32::new([74; 32]),
                    body: "{\"version\":1}".to_owned(),
                    issued_at_unix_millis: 1_700_000_000_000,
                    request_fact: Id32::new([75; 32]),
                    progress: Box::new(RemoteCommandProgressDto::Terminal {
                        receipt_fact: Id32::new([76; 32]),
                        received_head: Some(Id32::new([21; 32])),
                        received_at_unix_millis: 1_700_000_000_001,
                        outcome_fact: Id32::new([77; 32]),
                        result: RemoteCommandResultDto::Rejected {
                            error: "stale_head".to_owned(),
                            external_state_warning: None,
                        },
                        runtime: Some(RuntimeObservationDto::Uncertain("runtime_lost".to_owned())),
                    }),
                },
            ],
        };

        let view = project_catalog_view(&snapshot, &ProjectCliCommand::List).expect("catalog");
        assert_eq!(
            view.projects
                .iter()
                .map(|project| project.project_id)
                .collect::<Vec<_>>(),
            [
                ProjectId::from_bytes([1; 32]),
                ProjectId::from_bytes([2; 32])
            ]
        );
        assert_eq!(view.unattributed_dispatches, 1);
        assert_eq!(view.unattributed_outputs, 1);
        let first = &view.projects[0];
        assert_eq!(first.lifecycle, "conflicted");
        assert!(
            first
                .assignment
                .as_ref()
                .is_some_and(|assignment| assignment.runnable)
        );
        assert_eq!(first.threads.len(), 1);
        assert_eq!(first.resources[0].health, "degraded");
        assert_eq!(
            first.resources[0].conflicting_projects,
            [ProjectId::from_bytes([2; 32])]
        );
        assert!(first.dispatches[0].conflicted);
        assert_eq!(first.outputs[0].status, "conflicted");
        assert_eq!(first.remote_commands[0].progress, "terminal");
        assert_eq!(first.remote_commands[0].result_state, Some("rejected"));
        assert_eq!(
            first.remote_commands[0].result_value.as_deref(),
            Some("stale_head")
        );
        assert_eq!(first.remote_commands[0].runtime_state, Some("uncertain"));

        let human = render_project_catalog(CliOutputFormat::Human, &view).expect("human catalog");
        assert!(human.contains("lifecycle=conflicted"));
        assert!(human.contains("active_claim=false"));
        assert!(human.contains("runtime_state=uncertain"));
        assert!(human.contains("assignment project="));
        assert!(human.contains("thread project="));
        let json = render_project_catalog(CliOutputFormat::Json, &view).expect("JSON catalog");
        assert_eq!(
            json,
            render_project_catalog(CliOutputFormat::Json, &view).expect("stable JSON catalog")
        );
        let record: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(record["kind"], "project_catalog");
        assert_eq!(record["data"]["unattributed_dispatches"], 1);
        assert_eq!(
            record["data"]["projects"][0]["assignment"]["provider"],
            "codex"
        );
        assert_eq!(
            record["data"]["projects"][0]["threads"][0]["session"],
            "session-7"
        );
        assert_eq!(
            record["data"]["projects"][0]["remote_commands"][0]["runtime_code"],
            "runtime_lost"
        );

        let shown = project_catalog_view(
            &snapshot,
            &ProjectCliCommand::Show(ProjectId::from_bytes([2; 32])),
        )
        .expect("exact project");
        assert_eq!(shown.projects.len(), 1);
        assert_eq!(shown.projects[0].name, "second");
        assert_eq!(
            project_catalog_view(
                &snapshot,
                &ProjectCliCommand::Show(ProjectId::from_bytes([9; 32])),
            ),
            Err(CliError::ProjectState)
        );
    }

    #[test]
    fn parser_rejects_implicit_or_incoherent_harness_operations() {
        for arguments in [
            vec!["harness", "start", "--agent", "fred"],
            vec![
                "harness",
                "resume",
                "--agent",
                "fred",
                "--provider",
                "codex",
            ],
            vec![
                "harness",
                "start",
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--session",
                "old",
            ],
            vec![
                "harness",
                "stop",
                "--agent",
                "fred",
                "--provider",
                "codex",
                "--dir",
                ".",
            ],
        ] {
            assert_eq!(
                parse_cli(arguments.into_iter().map(OsString::from)),
                Err(CliError::Arguments)
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn launch_environment_preserves_non_utf8_values_and_redacts_debug() {
        let environment = copy_launch_environment([(
            OsString::from("HQ_BINARY_SECRET"),
            OsString::from_vec(vec![0xff, b'x']),
        )])
        .expect("binary value copies");
        let mut observed = Vec::new();
        environment.visit(|name, value| observed.push((name.to_owned(), value.to_vec())));
        assert_eq!(
            observed,
            [("HQ_BINARY_SECRET".to_owned(), vec![0xff, b'x'])]
        );
        let debug = format!("{environment:?}");
        assert!(debug.contains("entry_count"));
        assert!(!debug.contains("HQ_BINARY_SECRET"));
    }

    #[test]
    fn managed_harness_request_identity_is_exact_and_stable() {
        let action = HarnessCommand::Resume {
            agent: NamedAgentSelector::Name(hq_domain::ShortText::new("fred").expect("name")),
            provider: ProviderId::new("fake").expect("provider"),
            session: ProviderSessionId::new("session-1").expect("session"),
            directory: None,
        };
        let launch = AgentLaunchContextDto {
            directory: ResourceLocatorDto::new(
                ResourceSchemeDto::WorkingTree,
                "/work/hq".to_owned(),
            )
            .expect("directory"),
            environment: copy_launch_environment([(
                OsString::from("HQ_TOKEN"),
                OsString::from("secret"),
            )])
            .expect("environment"),
        };
        let first = harness_request(
            &action,
            AgentId::from_bytes([1; 32]),
            OperationId::from_bytes([2; 32]),
            1_700_000_000_000,
            Some(launch.clone()),
        )
        .expect("request");
        let replay = harness_request(
            &action,
            AgentId::from_bytes([1; 32]),
            OperationId::from_bytes([2; 32]),
            1_700_000_000_000,
            Some(launch),
        )
        .expect("replay");
        assert_eq!(first, replay);

        let changed = harness_request(
            &action,
            AgentId::from_bytes([1; 32]),
            OperationId::from_bytes([3; 32]),
            1_700_000_000_000,
            Some(AgentLaunchContextDto {
                directory: ResourceLocatorDto::new(
                    ResourceSchemeDto::WorkingTree,
                    "/work/hq".to_owned(),
                )
                .expect("directory"),
                environment: copy_launch_environment([(
                    OsString::from("HQ_TOKEN"),
                    OsString::from("secret"),
                )])
                .expect("environment"),
            }),
        )
        .expect("changed request");
        assert_ne!(first.request_digest, changed.request_digest);
    }

    #[test]
    fn harness_machine_result_exposes_reconciliation_without_secrets() {
        let result = super::CliResult::HarnessSession(HarnessSessionView {
            operation: "resume",
            operation_id: OperationId::from_bytes([1; 32]),
            agent_id: AgentId::from_bytes([2; 32]),
            provider: "fake".to_owned(),
            requested_session: Some("session-1".to_owned()),
            ready_session: None,
            directory: Some("/work/hq".to_owned()),
            status: "uncertain",
            error_category: None,
            error_code: None,
            reconciliation_id: Some([3; 32]),
        });
        let output = render_result(CliOutputFormat::Json, &result).expect("machine result");
        assert_eq!(successful_result_exit_code(&result, false), 3);
        let value: serde_json::Value = serde_json::from_str(&output).expect("JSON");
        assert_eq!(value["kind"], "harness_session");
        assert_eq!(value["data"]["status"], "uncertain");
        assert_eq!(
            value["data"]["reconciliation_id"].as_str().map(str::len),
            Some(64)
        );
        assert!(!output.contains("secret"));
    }

    #[test]
    fn parser_accepts_global_output_and_explicit_daemon_roles() {
        let root = std::env::temp_dir().join("hq-cli-parser");
        let tui = parse_cli([
            OsString::from("--state-root"),
            root.clone().into_os_string(),
            OsString::from("tui"),
        ])
        .expect("TUI parses");
        assert!(matches!(tui.command, CliCommand::Tui { state } if state.root() == root));
        assert_eq!(
            parse_cli([
                OsString::from("--output"),
                OsString::from("json"),
                OsString::from("tui"),
            ]),
            Err(CliError::Arguments)
        );
        let parsed = parse_cli([
            OsString::from("--output"),
            OsString::from("json"),
            OsString::from("--state-root"),
            root.clone().into_os_string(),
            OsString::from("daemon"),
            OsString::from("restart"),
        ])
        .expect("restart parses");
        assert_eq!(parsed.output, CliOutputFormat::Json);
        assert!(matches!(parsed.command, CliCommand::Daemon {
            action: DaemonCommand::Restart,
            state,
        } if state.root() == root));
        let help = parse_cli([OsString::from("help"), OsString::from("project")])
            .expect("topic help parses");
        assert!(matches!(help.command, CliCommand::Help { topic } if topic == ["project"]));
        assert_eq!(
            parse_cli([OsString::from("node"), OsString::from("run")]),
            Err(CliError::Arguments)
        );
        assert_eq!(
            parse_cli([
                OsString::from("--state-root"),
                OsString::from("relative"),
                OsString::from("daemon"),
                OsString::from("run"),
            ]),
            Err(CliError::StatePath)
        );
    }

    #[test]
    fn parser_accepts_typed_offline_administration_and_requires_explicit_secret_input() {
        let root = std::env::temp_dir().join("hq-cli-admin-parser");
        let backup = std::env::temp_dir().join("hq-cli-admin-backup.json");
        let identity = parse_cli([
            OsString::from("--state-root"),
            root.clone().into_os_string(),
            OsString::from("identity"),
            OsString::from("export"),
            backup.clone().into_os_string(),
            OsString::from("--password-stdin"),
        ])
        .expect("offline export parses");
        assert!(matches!(identity.command, CliCommand::Identity {
            action: IdentityCommand::Export { destination },
            state,
        } if state.root() == root && destination == backup));

        let config = parse_cli([
            OsString::from("--state-root"),
            root.clone().into_os_string(),
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("default-provider"),
            OsString::from("codex"),
        ])
        .expect("typed configuration parses");
        assert!(matches!(config.command, CliCommand::Configuration {
            action: ConfigurationCommand::SetDefaultProvider { provider: Some(provider) },
            state,
        } if state.root() == root && provider.as_str() == "codex"));

        let theme = parse_cli([
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("theme"),
            OsString::from("gruvbox-light-soft"),
        ])
        .expect("theme selection parses");
        assert!(matches!(theme.command, CliCommand::Configuration {
            action: ConfigurationCommand::SetTheme { theme: Some(theme) },
            ..
        } if theme.as_str() == "gruvbox-light-soft"));
        assert!(matches!(
            parse_cli([OsString::from("config"), OsString::from("themes")])
                .expect("theme discovery parses")
                .command,
            CliCommand::Configuration {
                action: ConfigurationCommand::Themes,
                ..
            }
        ));

        assert_eq!(
            parse_cli([
                OsString::from("--state-root"),
                root.clone().into_os_string(),
                OsString::from("identity"),
                OsString::from("export"),
                backup.into_os_string(),
            ]),
            Err(CliError::Arguments)
        );
        assert_eq!(
            parse_cli([
                OsString::from("--state-root"),
                root.into_os_string(),
                OsString::from("identity"),
                OsString::from("import"),
                OsString::from("relative.json"),
                OsString::from("--password-stdin"),
            ]),
            Err(CliError::Arguments)
        );

        let devices = parse_cli([OsString::from("human"), OsString::from("devices")])
            .expect("device inspection parses");
        assert!(matches!(
            devices.command,
            CliCommand::Human {
                action: HumanCommand::Devices,
                ..
            }
        ));
        let revoke = parse_cli([
            OsString::from("human"),
            OsString::from("revoke"),
            OsString::from("33".repeat(32)),
        ])
        .expect("device revoke parses");
        assert!(matches!(
            revoke.command,
            CliCommand::Human {
                action: HumanCommand::Revoke { installation_id },
                ..
            } if installation_id.as_bytes() == &[0x33; 32]
        ));
    }

    #[test]
    fn device_view_preserves_all_current_authorities_and_exposes_conflict() {
        let local = InstallationId::from_bytes([1; 32]);
        let account = Id32::new([2; 32]);
        let target = Id32::new([3; 32]);
        let snapshot = AuthoritativeSnapshotDto::new(
            9,
            vec![
                SnapshotItem::Installation {
                    installation_id: Id32::new([1; 32]),
                    root_fact: Id32::new([4; 32]),
                    signing_key: Id32::new([5; 32]),
                    encryption_key: Id32::new([6; 32]),
                    label: None,
                },
                SnapshotItem::Account {
                    account_id: account,
                    root_fact: Id32::new([7; 32]),
                    creator_installation: Id32::new([1; 32]),
                    label: None,
                    selected: true,
                },
                SnapshotItem::AccountSelection {
                    installation_id: Id32::new([1; 32]),
                    candidates: vec![account],
                    active: Some(account),
                    frontier: vec![Id32::new([8; 32])],
                },
                SnapshotItem::Membership {
                    account_id: account,
                    device: target,
                    state: "active".to_owned(),
                    frontier: vec![Id32::new([11; 32]), Id32::new([12; 32])],
                    grants: vec![
                        DeviceGrantDto {
                            grant_id: Id32::new([9; 32]),
                            grant_fact: Id32::new([13; 32]),
                            device: target,
                            signing_key: Id32::new([14; 32]),
                            label: Some("desktop".to_owned()),
                            relay_hints: vec![],
                            frontier_member: false,
                            active: true,
                        },
                        DeviceGrantDto {
                            grant_id: Id32::new([10; 32]),
                            grant_fact: Id32::new([15; 32]),
                            device: target,
                            signing_key: Id32::new([16; 32]),
                            label: Some("replacement".to_owned()),
                            relay_hints: vec![],
                            frontier_member: false,
                            active: true,
                        },
                    ],
                    acceptances: vec![Id32::new([11; 32]), Id32::new([12; 32])],
                    revokes: vec![],
                    active_acceptances: vec![Id32::new([11; 32]), Id32::new([12; 32])],
                },
            ],
        )
        .expect("snapshot");
        let view = human_devices_view(&snapshot, local).expect("device view");
        assert_eq!(view.devices.len(), 2);
        let member = view
            .devices
            .iter()
            .find(|device| device.installation_id.as_bytes() == &[3; 32])
            .expect("member");
        assert_eq!(member.state, HumanDeviceState::Conflicted);
        assert_eq!(member.grants.len(), 2);
        assert_eq!(member.acceptances.len(), 2);
        assert_eq!(member.signing_keys.len(), 2);
        assert_eq!(
            super::classify_device_state(
                "revoked",
                &[],
                &[FactId::from_bytes([17; 32])],
                &[],
                &[FactId::from_bytes([17; 32])],
                &BTreeSet::new(),
                true,
            ),
            HumanDeviceState::Incomplete
        );
    }

    #[test]
    fn parser_accepts_typed_human_administration_and_rejects_noncanonical_ids() {
        let root = std::env::temp_dir().join("hq-cli-human-parser");
        let account = "ab".repeat(32);
        let parsed = parse_cli([
            OsString::from("--state-root"),
            root.clone().into_os_string(),
            OsString::from("human"),
            OsString::from("select"),
            OsString::from(&account),
        ])
        .expect("human selection parses");
        assert!(matches!(parsed.command, CliCommand::Human {
            action: HumanCommand::Select { account_id },
            state,
        } if state.root() == root && account_id.as_bytes() == &[0xab; 32]));

        assert_eq!(
            parse_cli([
                OsString::from("human"),
                OsString::from("select"),
                OsString::from(account.to_uppercase()),
            ]),
            Err(CliError::Arguments)
        );

        let invitation = std::env::temp_dir().join("hq-pairing-invitation.json");
        let invite = parse_cli([
            OsString::from("human"),
            OsString::from("invite"),
            OsString::from("11".repeat(32)),
            OsString::from("22".repeat(32)),
            invitation.clone().into_os_string(),
            OsString::from("--relay"),
            OsString::from("wss://relay.example"),
            OsString::from("--label"),
            OsString::from("laptop"),
        ])
        .expect("pairing invitation parses");
        assert!(matches!(invite.command, CliCommand::Human {
            action: HumanCommand::Invite {
                installation_id,
                signing_key,
                destination,
                label: Some(label),
                relay_hints,
            },
            ..
        } if installation_id.as_bytes() == &[0x11; 32]
            && signing_key.as_bytes() == &[0x22; 32]
            && destination == invitation
            && label.as_str() == "laptop"
            && relay_hints.as_slice().len() == 1));

        assert_eq!(
            parse_cli([
                OsString::from("human"),
                OsString::from("join"),
                OsString::from("relative-invitation.json"),
            ]),
            Err(CliError::Arguments)
        );
    }

    #[test]
    fn parser_accepts_directional_peer_and_mailbox_administration() {
        let peer = parse_cli([
            OsString::from("peer"),
            OsString::from("add"),
            OsString::from("11".repeat(32)),
            OsString::from("22".repeat(32)),
            OsString::from("33".repeat(32)),
            OsString::from("--label"),
            OsString::from("desk"),
        ])
        .expect("peer add parses");
        assert!(matches!(
            peer.command,
            CliCommand::Peer {
                action: PeerCommand::Add {
                    installation_id,
                    label: Some(label),
                    ..
                },
                ..
            } if installation_id.as_bytes() == &[0x11; 32] && label.as_str() == "desk"
        ));
        let grant = parse_cli([
            OsString::from("mailbox"),
            OsString::from("grant"),
            OsString::from("44".repeat(32)),
            OsString::from("11".repeat(32)),
        ])
        .expect("mailbox grant parses");
        assert!(matches!(
            grant.command,
            CliCommand::Mailbox {
                action: MailboxCommand::Grant { mailbox_id, peer_id },
                ..
            } if mailbox_id.as_bytes() == &[0x44; 32] && peer_id.as_bytes() == &[0x11; 32]
        ));
    }

    #[test]
    fn parser_accepts_relay_policy_sync_health_and_repair_administration() {
        let add = parse_cli([
            OsString::from("relay"),
            OsString::from("add"),
            OsString::from("wss://relay.example"),
            OsString::from("--access"),
            OsString::from("read"),
            OsString::from("--auth"),
            OsString::from("required"),
        ])
        .expect("relay add parses");
        assert!(matches!(
            add.command,
            CliCommand::Relay {
                action: RelayCommand::Add {
                    access: RelayAccessDto::Read,
                    authentication: RelayAuthenticationDto::Required,
                    ..
                },
                ..
            }
        ));
        for action in ["list", "status", "repair"] {
            assert!(matches!(
                parse_cli([OsString::from("relay"), OsString::from(action)])
                    .expect("relay command parses")
                    .command,
                CliCommand::Relay { .. }
            ));
        }
        assert!(matches!(
            parse_cli([
                OsString::from("relay"),
                OsString::from("sync"),
                OsString::from("wss://relay.example"),
            ])
            .expect("relay sync parses")
            .command,
            CliCommand::Relay {
                action: RelayCommand::Sync { endpoint: Some(_) },
                ..
            }
        ));
    }

    #[test]
    fn parser_accepts_explicit_agent_mailbox_messaging_without_provider_inference() {
        let ask = parse_cli([
            OsString::from("ask"),
            OsString::from("--provider"),
            OsString::from("codex"),
            OsString::from("--session"),
            OsString::from("session-1"),
            OsString::from("--timeout"),
            OsString::from("30s"),
            OsString::from("--interval"),
            OsString::from("100ms"),
            OsString::from("What changed?"),
        ])
        .expect("typed ask parses");
        assert!(matches!(
            ask.command,
            CliCommand::AgentMessage {
                action: AgentMessageCommand::Ask {
                    mailbox: AgentMailboxSelection {
                        provider: Some(_),
                        session: Some(_),
                        ..
                    },
                    timeout: Some(timeout),
                    interval,
                    ..
                },
                ..
            } if timeout == Duration::from_secs(30) && interval == Duration::from_millis(100)
        ));

        assert!(
            parse_cli([
                OsString::from("send"),
                OsString::from("--session"),
                OsString::from("ambiguous"),
                OsString::from("hello"),
            ])
            .is_err()
        );
        assert!(
            parse_cli([
                OsString::from("poll"),
                OsString::from("--provider"),
                OsString::from("codex"),
            ])
            .is_err()
        );
    }

    #[test]
    fn parser_accepts_named_agent_catalog_and_guidance_commands() {
        let root = std::env::temp_dir().join("hq-cli-agent-catalog-parser");
        let mailbox = "11".repeat(32);
        let created = parse_cli([
            OsString::from("--state-root"),
            root.clone().into_os_string(),
            OsString::from("agent"),
            OsString::from("create"),
            OsString::from("build-agent"),
            OsString::from("--mailbox"),
            OsString::from(mailbox),
        ])
        .expect("agent adoption parses");
        assert!(matches!(
            created.command,
            CliCommand::NamedAgent {
                action: NamedAgentCommand::Create {
                    name,
                    mailbox_id: Some(_),
                },
                state,
            } if name.as_str() == "build-agent" && state.root() == root
        ));

        let selected = parse_cli([
            OsString::from("agent"),
            OsString::from("select"),
            OsString::from("build-agent"),
            OsString::from("--provider"),
            OsString::from("codex"),
            OsString::from("--session"),
            OsString::from("thread-1"),
            OsString::from("--dir"),
            OsString::from("/work/repo"),
        ])
        .expect("exact selection parses");
        assert!(matches!(
            selected.command,
            CliCommand::NamedAgent {
                action: NamedAgentCommand::Select {
                    agent: NamedAgentSelector::Name(_),
                    mailbox: AgentMailboxSelection {
                        provider: Some(_),
                        session: Some(_),
                        directory: Some(_),
                    },
                },
                ..
            }
        ));

        let renamed = parse_cli([
            OsString::from("agent"),
            OsString::from("rename"),
            OsString::from("build-agent"),
            OsString::from("--clear"),
        ])
        .expect("explicit clear parses");
        assert!(matches!(
            renamed.command,
            CliCommand::NamedAgent {
                action: NamedAgentCommand::Rename {
                    display_name: None,
                    provider: None,
                    session: None,
                    ..
                },
                ..
            }
        ));

        let guidance = parse_cli([OsString::from("agents"), OsString::from("retry")])
            .expect("guidance topic parses");
        assert!(
            matches!(guidance.command, CliCommand::AgentGuidance { topic } if topic.label() == "retry")
        );
        assert_eq!(
            parse_cli([
                OsString::from("agent"),
                OsString::from("select"),
                OsString::from("build-agent"),
                OsString::from("--provider"),
                OsString::from("codex"),
            ]),
            Err(CliError::Arguments)
        );
    }

    #[test]
    fn parser_requires_explicit_named_agent_retirement_confirmation() {
        let retired = parse_cli([
            OsString::from("agent"),
            OsString::from("retire"),
            OsString::from("build-agent"),
            OsString::from("--yes"),
            OsString::from("--force"),
        ])
        .expect("confirmed forced retirement parses");
        assert!(matches!(
            retired.command,
            CliCommand::NamedAgent {
                action: NamedAgentCommand::Retire { force: true, .. },
                ..
            }
        ));
        assert_eq!(
            parse_cli([
                OsString::from("agent"),
                OsString::from("retire"),
                OsString::from("build-agent"),
            ]),
            Err(CliError::Arguments)
        );
    }

    #[test]
    fn provider_environment_discovery_rejects_every_ambiguous_or_partial_source() {
        assert_eq!(
            resolve_environment_session(
                vec![("codex".to_owned(), "thread-1".to_owned())],
                None,
                None,
            )
            .expect("one provider"),
            Some(("codex".to_owned(), "thread-1".to_owned()))
        );
        assert_eq!(
            resolve_environment_session(
                vec![
                    ("codex".to_owned(), "thread-1".to_owned()),
                    ("pi".to_owned(), "session-2".to_owned()),
                ],
                None,
                None,
            ),
            Err(CliError::MessagingState)
        );
        assert_eq!(
            resolve_environment_session(
                vec![("codex".to_owned(), "thread-1".to_owned())],
                Some("custom".to_owned()),
                Some("session-3".to_owned()),
            ),
            Err(CliError::MessagingState)
        );
        assert_eq!(
            resolve_environment_session(vec![], Some("custom".to_owned()), None),
            Err(CliError::MessagingState)
        );
    }

    #[test]
    fn catalog_session_planning_rejects_conflicted_bindings_and_stale_context() {
        let local = InstallationId::from_bytes([1; 32]);
        let mailbox = MailboxAddress::new(local, MailboxId::from_bytes([2; 32]));
        let provider = hq_domain::ProviderId::new("codex").expect("provider");
        let session = hq_domain::ProviderSessionId::new("thread-1").expect("session");
        let conflicted = AuthoritativeSnapshotDto::new(
            1,
            vec![SnapshotItem::AgentSession {
                provider: provider.as_str().to_owned(),
                session: session.as_str().to_owned(),
                bindings: vec![AgentSessionBindingDto {
                    fact_id: Id32::new([3; 32]),
                    mailbox: MailboxAddressDto {
                        installation_id: Id32::new(*local.as_bytes()),
                        mailbox_id: Id32::new(*mailbox.mailbox_id().as_bytes()),
                    },
                }],
                mailbox_installation: None,
                mailbox_id: None,
                conflicted: true,
            }],
        )
        .expect("conflicted snapshot");
        assert_eq!(
            session_binding_fact(&conflicted, mailbox, &provider, &session),
            Err(CliError::AgentState)
        );

        let stale = AuthoritativeSnapshotDto::new(1, Vec::new()).expect("empty snapshot");
        assert_eq!(
            session_context(&stale, mailbox, Path::new("/work/repo")),
            Err(CliError::AgentState)
        );

        let mut conflict_items = [4_u8, 5_u8]
            .into_iter()
            .map(|byte| SnapshotItem::Agent {
                agent_id: Id32::new([byte; 32]),
                claims: vec![Id32::new([byte + 10; 32])],
                names: vec!["shared-name".to_owned()],
                mailboxes: vec![MailboxAddressDto {
                    installation_id: Id32::new(*local.as_bytes()),
                    mailbox_id: Id32::new([byte + 20; 32]),
                }],
                retirements: Vec::new(),
                lifecycle: "conflicted".to_owned(),
                runnable: false,
            })
            .collect::<Vec<_>>();
        conflict_items.push(SnapshotItem::AgentSession {
            provider: provider.as_str().to_owned(),
            session: session.as_str().to_owned(),
            bindings: [4_u8, 5_u8]
                .into_iter()
                .map(|byte| AgentSessionBindingDto {
                    fact_id: Id32::new([byte + 30; 32]),
                    mailbox: MailboxAddressDto {
                        installation_id: Id32::new(*local.as_bytes()),
                        mailbox_id: Id32::new([byte + 20; 32]),
                    },
                })
                .collect(),
            mailbox_installation: None,
            mailbox_id: None,
            conflicted: true,
        });
        let conflicting_name =
            AuthoritativeSnapshotDto::new(1, conflict_items).expect("name-conflict snapshot");
        assert_eq!(
            resolve_named_agent_id(
                &conflicting_name,
                &NamedAgentSelector::Name(hq_domain::ShortText::new("shared-name").expect("name"))
            ),
            Err(CliError::AgentState)
        );
        let view = named_agent_catalog_view(&conflicting_name, "agent_list", None, None);
        assert_eq!(view.agents.len(), 2);
        assert!(view.agents.iter().all(|agent| {
            agent.sessions.len() == 1
                && agent.sessions[0].conflicted
                && agent.sessions[0].mailbox.is_none()
        }));
    }

    #[test]
    fn parser_accepts_non_consuming_get_discovery_and_human_mailbox_actions() {
        let id = "11".repeat(32);
        assert!(matches!(
            parse_cli([OsString::from("get"), OsString::from(&id)])
                .expect("get parses")
                .command,
            CliCommand::GetMessage { .. }
        ));
        assert!(matches!(
            parse_cli([
                OsString::from("mailboxes"),
                OsString::from("--dir"),
                OsString::from("/tmp/repository"),
            ])
            .expect("discovery parses")
            .command,
            CliCommand::DiscoverMailboxes {
                directory: Some(_),
                ..
            }
        ));
        assert!(matches!(
            parse_cli([
                OsString::from("list"),
                OsString::from("--archived"),
                OsString::from("--limit"),
                OsString::from("25"),
            ])
            .expect("list parses")
            .command,
            CliCommand::HumanMessage {
                action: HumanMessageCommand::List(HumanMessageFilters {
                    archived: true,
                    limit: 25,
                    ..
                }),
                ..
            }
        ));
        for action in ["answer", "cancel", "archive", "restore"] {
            assert!(matches!(
                parse_cli([OsString::from(action), OsString::from(&id)])
                    .expect("human action parses")
                    .command,
                CliCommand::HumanMessage { .. }
            ));
        }
    }

    #[test]
    fn repository_discovery_joins_public_context_and_direct_session_records() {
        let local = InstallationId::from_bytes([1; 32]);
        let mailbox_id = Id32::new([2; 32]);
        let context_fact = Id32::new([3; 32]);
        let directory = std::env::current_dir().expect("current directory");
        let snapshot = AuthoritativeSnapshotDto::new(
            1,
            vec![
                SnapshotItem::AgentContext {
                    mailbox_installation: Id32::new(*local.as_bytes()),
                    mailbox_id,
                    history: vec![RepositoryContextDto {
                        fact_id: context_fact,
                        directory: ResourceLocatorDto::new(
                            ResourceSchemeDto::WorkingTree,
                            directory.to_str().expect("UTF-8 path").to_owned(),
                        )
                        .expect("locator"),
                        repository: None,
                        worktree: None,
                        branch: Some("main".to_owned()),
                    }],
                    frontier: vec![context_fact],
                },
                SnapshotItem::AgentDirectSession {
                    provider: "codex".to_owned(),
                    session: "session-1".to_owned(),
                    mailbox_installation: Id32::new(*local.as_bytes()),
                    mailbox_id,
                    named_agent: None,
                    conflicted: false,
                },
            ],
        )
        .expect("snapshot validates");

        let view = mailbox_discovery_view(&snapshot, local, directory)
            .expect("repository discovery succeeds");
        assert_eq!(view.candidates.len(), 1);
        assert!(view.candidates[0].directory_match);
        assert_eq!(view.candidates[0].provider, "codex");
        assert_eq!(view.candidates[0].branches, vec!["main"]);
    }

    #[test]
    fn message_rendering_keeps_stable_identity_in_machine_output_and_plain_wait_output() {
        let installation = InstallationId::from_bytes([1; 32]);
        let mailbox = MailboxAddress::new(installation, MailboxId::from_bytes([2; 32]));
        let message = CliMessageView {
            fact_id: FactId::from_bytes([3; 32]),
            message_id: MessageId::from_bytes([4; 32]),
            thread_id: ThreadId::from_bytes([5; 32]),
            sender: mailbox,
            recipient: Some(mailbox),
            content: "ready answer".to_owned(),
            purpose: MessagePurposeDto::Question,
            presentation: PresentationKindDto::FinalAnswer,
            correlation: None,
            project_id: None,
            open: true,
            rejected: false,
            state_frontier: BTreeSet::new(),
            root_fact: Some(FactId::from_bytes([5; 32])),
            root_message: Some(MessageId::from_bytes([6; 32])),
            ready_answer: true,
            thread_cancelled: false,
            incomplete: false,
            missing_dependencies: BTreeSet::new(),
            unusable_dependencies: BTreeSet::new(),
        };
        let result = super::CliResult::Messages(Box::new(MessageCommandView {
            operation: "wait",
            mailbox: Some(mailbox),
            root_message: message.root_message,
            project_id: None,
            messages: vec![message.clone()],
            incomplete_truncated: false,
        }));
        assert_eq!(
            render_result(CliOutputFormat::Human, &result).expect("human output"),
            "ready answer\n"
        );
        let machine = render_result(CliOutputFormat::Json, &result).expect("machine output");
        assert!(machine.contains(&crate::identity::encode_hex(message.message_id.as_bytes())));
        assert!(machine.contains("ready_answer"));

        let invocation = parse_cli([
            OsString::from("wait"),
            OsString::from("--provider"),
            OsString::from("codex"),
            OsString::from("--session"),
            OsString::from("session-1"),
            OsString::from("06".repeat(32)),
        ])
        .expect("wait invocation");
        let completion = completion_for(&invocation, &result).expect("delivery completion");
        assert_eq!(completion.mailbox, mailbox);
        assert_eq!(completion.messages, vec![message.message_id]);
    }

    #[test]
    fn project_send_rendering_is_exact_and_machine_readable() {
        let project_id = ProjectId::from_bytes([7; 32]);
        let message_id = MessageId::from_bytes([8; 32]);
        let result = super::CliResult::Messages(Box::new(MessageCommandView {
            operation: "project_send",
            mailbox: None,
            root_message: Some(message_id),
            project_id: Some(project_id),
            messages: Vec::new(),
            incomplete_truncated: false,
        }));
        assert_eq!(
            render_result(CliOutputFormat::Human, &result).expect("human output"),
            format!(
                "project={} message={}\n",
                crate::identity::encode_hex(project_id.as_bytes()),
                crate::identity::encode_hex(message_id.as_bytes())
            )
        );
        let machine = render_result(CliOutputFormat::Json, &result).expect("machine output");
        let value: serde_json::Value = serde_json::from_str(&machine).expect("JSON output");
        assert_eq!(value["kind"], "messages");
        assert_eq!(value["data"]["operation"], "project_send");
        assert_eq!(
            value["data"]["project_id"],
            crate::identity::encode_hex(project_id.as_bytes())
        );
        assert_eq!(
            value["data"]["root_message"],
            crate::identity::encode_hex(message_id.as_bytes())
        );
    }

    #[test]
    fn relay_and_repair_effect_identities_are_stable_and_revision_sensitive() {
        let first = stable_relay_effect(b"synchronize", 3, SynchronizationRequestDto::All)
            .expect("effect builds");
        let replay = stable_relay_effect(b"synchronize", 3, SynchronizationRequestDto::All)
            .expect("effect replays");
        let next_generation =
            stable_relay_effect(b"synchronize", 4, SynchronizationRequestDto::All)
                .expect("new generation builds");
        assert_eq!(first, replay);
        assert_ne!(first.operation_id, next_generation.operation_id);
        assert_eq!(stable_repair_operation(7), stable_repair_operation(7));
        assert_ne!(stable_repair_operation(7), stable_repair_operation(8));
        let expected = [0x51; 32];
        assert_eq!(
            effect_outcome(&EffectOutcomeDto::Accepted(()), expected),
            Ok(("accepted".to_owned(), Some(expected)))
        );
        assert_eq!(
            effect_outcome(&EffectOutcomeDto::Uncertain(Id32::new(expected)), expected),
            Ok(("uncertain".to_owned(), Some(expected)))
        );
        assert_eq!(
            effect_outcome(
                &EffectOutcomeDto::Uncertain(Id32::new([0x52; 32])),
                expected
            ),
            Err(CliError::RelayState)
        );
    }

    #[test]
    fn human_view_derives_selection_from_only_the_local_installation() {
        let local = InstallationId::from_bytes([1; 32]);
        let account = Id32::new([2; 32]);
        let snapshot = AuthoritativeSnapshotDto::new(
            1,
            vec![
                SnapshotItem::Account {
                    account_id: account,
                    root_fact: Id32::new([3; 32]),
                    creator_installation: Id32::new([4; 32]),
                    label: None,
                    selected: true,
                },
                SnapshotItem::AccountSelection {
                    installation_id: Id32::new(*local.as_bytes()),
                    candidates: Vec::new(),
                    active: None,
                    frontier: Vec::new(),
                },
            ],
        )
        .expect("snapshot");

        let view = human_view(&snapshot, local).expect("human view");
        assert_eq!(view.active_account, None);
        assert_eq!(view.accounts.len(), 1);
        assert!(!view.accounts[0].selected);
    }

    #[test]
    fn pairing_grant_identity_is_stable_and_frontier_sensitive() {
        let account = AccountId::from_bytes([1; 32]);
        let device = InstallationAddress::new(
            InstallationId::from_bytes([2; 32]),
            SigningPublicKey::from_bytes([3; 32]),
        );
        let relays = RelayHints::new([]).expect("empty relay hints");
        let empty = pairing_grant_id(account, device, None, &relays, &BTreeSet::new());
        let repeated = pairing_grant_id(account, device, None, &relays, &BTreeSet::new());
        let regrant = pairing_grant_id(
            account,
            device,
            None,
            &relays,
            &BTreeSet::from([FactId::from_bytes([4; 32])]),
        );
        assert_eq!(empty, repeated);
        assert_ne!(empty, regrant);
    }

    #[test]
    fn help_and_version_have_stable_human_and_machine_records() {
        let help = parse_cli([]).expect("bare invocation renders help");
        assert!(
            run_cli(&help)
                .expect("help renders")
                .starts_with("HQ local client\n")
        );

        let version = parse_cli([
            OsString::from("--output"),
            OsString::from("json"),
            OsString::from("version"),
        ])
        .expect("machine version parses");
        let rendered = run_cli(&version).expect("machine version renders");
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON record");
        assert_eq!(value["schema"], "hq-cli-output-v1");
        assert_eq!(value["ok"], true);
        assert_eq!(value["kind"], "version");
        assert_eq!(value["data"]["protocol"], 1);
        assert_eq!(value["data"]["name"], "hq");
    }

    #[test]
    fn help_is_generated_from_the_command_grammar() {
        let root = run_cli(&parse_cli([]).expect("root help parses")).expect("root help");
        assert!(root.contains("Usage: hq [OPTIONS] [COMMAND]"));
        assert!(root.contains("Options:"));

        let nested = run_cli(
            &parse_cli([
                OsString::from("help"),
                OsString::from("project"),
                OsString::from("resource"),
                OsString::from("add"),
            ])
            .expect("nested help parses"),
        )
        .expect("nested help");
        assert!(nested.contains("Usage: hq project resource add"));
        assert!(nested.contains("--primary"));
    }

    #[test]
    fn help_snapshots_cover_the_complete_foundation_tree() {
        let root = run_cli(&parse_cli([]).expect("root help parses")).expect("root help");
        assert!(root.starts_with("HQ local client\n\nUsage: hq [OPTIONS] [COMMAND]"));
        assert!(root.contains("Options:"));
        for command in [
            "help",
            "version",
            "tui",
            "agents",
            "agent",
            "harness",
            "project",
            "ask",
            "send",
            "wait",
            "poll",
            "get",
            "list",
            "answer",
            "cancel",
            "archive",
            "restore",
            "mailboxes",
            "identity",
            "config",
            "human",
            "peer",
            "mailbox",
            "relay",
            "daemon",
        ] {
            assert!(
                root.lines()
                    .any(|line| line.trim_start().starts_with(command))
            );
        }

        let ask = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("ask")]).expect("ask help parses"),
        )
        .expect("ask help");
        assert!(ask.contains("intentionally unbounded"));
        assert!(ask.contains("stable message IDs"));

        let identity_export = run_cli(
            &parse_cli([
                OsString::from("help"),
                OsString::from("identity"),
                OsString::from("export"),
            ])
            .expect("identity export help parses"),
        )
        .expect("identity export help");
        assert!(identity_export.contains("--password-stdin"));
        assert!(!identity_export.contains("<PASSWORD>"));

        let daemon = run_cli(
            &parse_cli([OsString::from("daemon"), OsString::from("--help")])
                .expect("daemon help parses"),
        )
        .expect("daemon help");
        assert!(daemon.contains("Own the node in the foreground"));
        assert!(daemon.contains("Converge on a fresh ready generation"));

        let guidance = run_cli(
            &parse_cli([OsString::from("agents"), OsString::from("delivery")])
                .expect("delivery guidance parses"),
        )
        .expect("delivery guidance");
        assert!(guidance.contains("at least once"));
        assert!(guidance.contains("stable message identity"));

        assert_eq!(
            run_cli(
                &parse_cli([OsString::from("help"), OsString::from("unknown")])
                    .expect("unknown help path parses")
            ),
            Err(CliError::Arguments)
        );
    }

    #[test]
    fn project_help_covers_catalog_creation_assignment_and_lifecycle() {
        let project = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("project")])
                .expect("project help parses"),
        )
        .expect("project help");
        for command in [
            "list",
            "show",
            "resource",
            "check",
            "send",
            "create",
            "worktree",
            "open",
            "activate",
            "dispatch",
            "handoff",
            "close",
            "archive",
            "unarchive",
        ] {
            assert!(
                project
                    .lines()
                    .any(|line| line.trim_start().starts_with(command))
            );
        }
        assert!(project.contains("never chooses a historical winner"));

        for (command, expected) in [
            ("create", "--path <ABSOLUTE_PATH>"),
            ("activate", "--new-session"),
            ("handoff", "--yes"),
            ("close", "--force"),
        ] {
            let help = run_cli(
                &parse_cli([
                    OsString::from("help"),
                    OsString::from("project"),
                    OsString::from(command),
                ])
                .expect("project command help parses"),
            )
            .expect("project command help");
            assert!(help.contains(expected));
        }
    }
    #[test]
    fn process_execution_renders_typed_machine_errors_without_echoing_inputs() {
        let execution = execute_cli([
            OsString::from("--output"),
            OsString::from("json"),
            OsString::from("--state-root"),
            OsString::from("relative-secret-path"),
            OsString::from("daemon"),
            OsString::from("status"),
        ]);
        assert_eq!(execution.exit_code, 2);
        assert!(execution.stdout.is_empty());
        assert!(!execution.stderr.contains("relative-secret-path"));
        let value: serde_json::Value =
            serde_json::from_str(&execution.stderr).expect("machine error record");
        assert_eq!(value["schema"], "hq-cli-output-v1");
        assert_eq!(value["ok"], false);
        assert_eq!(value["kind"], "error");
        assert_eq!(value["data"]["class"], "usage");
        assert_eq!(value["data"]["code"], "cli.state_path");
    }

    #[test]
    fn clap_relationship_errors_remain_redacted_and_machine_typed() {
        let execution = execute_cli([
            OsString::from("agent"),
            OsString::from("select"),
            OsString::from("private-agent-name"),
            OsString::from("--provider"),
            OsString::from("private-provider-value"),
            OsString::from("--output=json"),
        ]);
        assert_eq!(execution.exit_code, 2);
        assert!(!execution.stderr.contains("private-agent-name"));
        assert!(!execution.stderr.contains("private-provider-value"));
        let value: serde_json::Value =
            serde_json::from_str(&execution.stderr).expect("machine error record");
        assert_eq!(value["data"]["class"], "usage");
        assert_eq!(value["data"]["code"], "cli.arguments");
    }

    #[test]
    fn clap_global_options_are_accepted_after_subcommands() {
        let invocation = parse_cli([
            OsString::from("daemon"),
            OsString::from("status"),
            OsString::from("--output"),
            OsString::from("json"),
            OsString::from("--state-root"),
            OsString::from("/tmp/hq-clap-global-options"),
        ])
        .expect("global options parse after the command");
        assert_eq!(invocation.output, CliOutputFormat::Json);
        assert!(matches!(
            invocation.command,
            CliCommand::Daemon {
                action: DaemonCommand::Status,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn clap_preserves_non_utf8_path_arguments() {
        use std::os::unix::ffi::OsStringExt as _;

        let path = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]);
        let expected = std::path::PathBuf::from(path.clone());
        let invocation = parse_cli([
            OsString::from("mailboxes"),
            OsString::from("--dir"),
            path.clone(),
        ])
        .expect("raw path parses");
        assert!(matches!(
            invocation.command,
            CliCommand::DiscoverMailboxes {
                directory: Some(directory),
                ..
            } if directory == expected
        ));
    }

    #[test]
    fn secret_input_accepts_one_bounded_line_and_rejects_ambiguous_streams() {
        assert!(read_password(&mut b"correct horse battery staple\r\n".as_slice()).is_ok());
        assert!(matches!(
            read_password(&mut b"first line\nsecond line\n".as_slice()),
            Err(CliError::SecretInput)
        ));
        assert!(matches!(
            read_password(&mut vec![b'x'; 1_025].as_slice()),
            Err(CliError::SecretInput)
        ));
        assert!(matches!(
            read_password(&mut std::io::empty()),
            Err(CliError::SecretInput)
        ));
    }

    #[test]
    fn non_tty_message_input_accepts_bounded_utf8_and_rejects_empty_or_oversized_streams() {
        let mut input = std::io::Cursor::new(b"line one\nline two\n".to_vec());
        assert_eq!(
            message_body(None, &mut input)
                .expect("multiline message input")
                .as_str(),
            "line one\nline two"
        );
        assert!(message_body(None, &mut std::io::empty()).is_err());
        let mut oversized = std::io::Cursor::new(vec![b'x'; hq_domain::CONTENT_MAX_BYTES + 1]);
        assert!(message_body(None, &mut oversized).is_err());
    }
}
