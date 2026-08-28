//! Minimal single-binary lifecycle roles for the Rust node.

use std::{
    collections::BTreeSet,
    error::Error,
    ffi::OsString,
    fmt::{self, Write as _},
    io::Read,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Duration,
};

use hq_application::{
    ApplicationError, HumanDeviceGrantRequest, HumanDeviceRevokeRequest, LocalFactInputs,
    LocalInstallationAuthority, MailboxGrantRequest, MailboxRevokeRequest, PeerRouteRequest,
    plan_human_account_creation, plan_human_account_selection, plan_human_device_acceptance,
    plan_human_device_grant, plan_human_device_revoke, plan_human_mailbox_creation,
    plan_mailbox_grant, plan_mailbox_revoke, plan_peer_route_block, plan_peer_route_set,
};
use hq_domain::{
    AccountId, BoundedText, CommandId, EncryptionPublicKey, ErrorCode, FactId, GrantId,
    InstallationAddress, InstallationId, MailboxAddress, MailboxId, ProviderId,
    RESOURCE_LOCATOR_MAX_BYTES, RelayHints, ResourceLocator, ResourceScheme, ShortText,
    SigningPublicKey, Timestamp,
};
use hq_local_api::{
    ClientEvent, InitialView,
    protocol::v1::{
        AuthoritativeSnapshotDto, BuildMetadata, CanonicalEvidenceDto, CanonicalEvidenceRequestDto,
        DeviceGrantDto, EffectOutcomeDto, EffectRequestDto, HealthDomainDto, Id32,
        LifecycleRequest, LifecycleState, MutationAttemptDto, MutationOutcomeDto, MutationRequest,
        PeerRouteBlockDto, PeerRouteCandidateDto, RelayAccessDto, RelayAuthenticationDto,
        RelayConfigurationDto, RelayStatusDto, Request, ResourceLocatorDto, ResourceSchemeDto,
        ResponseResult, SnapshotItem, StateHealthDto, SynchronizationRequestDto,
    },
};
use hq_protocol::VerifiedPairingInvitation;
use hq_reducer::{
    AuthorityPolicy, AuthorityProjectionKey, AuthorityReducer, DecisionStatus, reduce_complete,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::pairing_file::{read_pairing_file, write_new_pairing_file};
use crate::{
    BackupPassword, ForegroundNodeConfig, ForegroundNodeError, IdentityError, LifecycleClient,
    LifecycleClientConfig, LifecycleClientError, LifecycleObservation, LocalConfiguration,
    LocalNodeClient, LocalNodeClientConfig, LocalNodeClientError, NodeClientCoordinator,
    NodeCoordinatorConfig, NodeCoordinatorError, ProcessNodeLauncher, PublicIdentity,
    RelayEndpoint, RuntimePathError, RuntimePaths, StateDirectoryOwner, StatePaths, run_foreground,
};

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
    /// Pairing evidence or its filesystem location failed strict validation.
    PairingArtifact,
    /// Backup password input was absent, oversized, malformed, or unreadable.
    SecretInput,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments => formatter.write_str(
                "usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] \
                 <help|version|identity|config|human|peer|mailbox|daemon>",
            ),
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
            Self::PairingArtifact => formatter.write_str("human pairing invitation is invalid"),
            Self::SecretInput => formatter.write_str("backup password input is invalid"),
        }
    }
}

impl Error for CliError {}

impl CliError {
    const fn diagnostic(&self) -> (&'static str, &'static str, CliExitClass) {
        match self {
            Self::Arguments => (
                "cli.arguments",
                "the command arguments are invalid; run `hq help`",
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
            Self::PairingArtifact => (
                "human.pairing_invalid",
                "the pairing invitation or its file location is invalid",
                CliExitClass::Failure,
            ),
            Self::SecretInput => (
                "identity.secret_input",
                "provide exactly one bounded UTF-8 backup password on stdin",
                CliExitClass::Usage,
            ),
        }
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
    let mut arguments = arguments.into_iter().peekable();
    let mut output = CliOutputFormat::Human;
    let mut state_root = None;
    while let Some(argument) = arguments.peek() {
        match argument.to_str() {
            Some("--output") => {
                let _ = arguments.next();
                output = match arguments.next().as_ref().and_then(|value| value.to_str()) {
                    Some("human") => CliOutputFormat::Human,
                    Some("json") => CliOutputFormat::Json,
                    _ => return Err(CliError::Arguments),
                };
            }
            Some("--state-root") => {
                let _ = arguments.next();
                if state_root.is_some() {
                    return Err(CliError::Arguments);
                }
                state_root = Some(PathBuf::from(arguments.next().ok_or(CliError::Arguments)?));
            }
            _ => break,
        }
    }
    let command = arguments.next();
    let rest = arguments.collect::<Vec<_>>();
    let command = match command.as_ref().and_then(|value| value.to_str()) {
        None | Some("help" | "--help") => CliCommand::Help {
            topic: rest
                .iter()
                .map(|value| value.to_str().map(str::to_owned).ok_or(CliError::Arguments))
                .collect::<Result<Vec<_>, _>>()?,
        },
        Some("version" | "--version") if rest.is_empty() => CliCommand::Version,
        Some("identity") => parse_identity(&rest, state_root.as_ref())?,
        Some("config") => parse_configuration(&rest, state_root.as_ref())?,
        Some("human") => parse_human(&rest, state_root.as_ref())?,
        Some("peer") => parse_peer(&rest, state_root.as_ref())?,
        Some("mailbox") => parse_mailbox(&rest, state_root.as_ref())?,
        Some("relay") => parse_relay_command(&rest, state_root.as_ref())?,
        Some("daemon") if rest.as_slice() == [OsString::from("--help")] => CliCommand::Help {
            topic: vec!["daemon".to_owned()],
        },
        Some("daemon") => {
            let [action] = rest.as_slice() else {
                return Err(CliError::Arguments);
            };
            let action = match action.to_str() {
                Some("run") => DaemonCommand::Run,
                Some("status") => DaemonCommand::Status,
                Some("readiness") => DaemonCommand::Readiness,
                Some("stop") => DaemonCommand::Stop,
                Some("restart") => DaemonCommand::Restart,
                _ => return Err(CliError::Arguments),
            };
            let state = parsed_state(state_root.as_ref())?;
            CliCommand::Daemon { action, state }
        }
        _ => return Err(CliError::Arguments),
    };
    if state_root.is_some()
        && !matches!(
            command,
            CliCommand::Daemon { .. }
                | CliCommand::Identity { .. }
                | CliCommand::Configuration { .. }
                | CliCommand::Human { .. }
                | CliCommand::Peer { .. }
                | CliCommand::Mailbox { .. }
                | CliCommand::Relay { .. }
        )
    {
        return Err(CliError::Arguments);
    }
    Ok(CliInvocation { output, command })
}

fn parse_peer(
    arguments: &[OsString],
    state_root: Option<&PathBuf>,
) -> Result<CliCommand, CliError> {
    let action = match arguments {
        [action] if action == "list" => PeerCommand::List,
        [action, installation] if action == "distrust" => PeerCommand::Distrust {
            installation_id: InstallationId::from_bytes(parse_hex32(installation)?),
        },
        [action, installation, signing, encryption, options @ ..] if action == "add" => {
            let (label, relay_hints) = parse_pairing_options(options)?;
            PeerCommand::Add {
                installation_id: InstallationId::from_bytes(parse_hex32(installation)?),
                signing_key: SigningPublicKey::from_bytes(parse_hex32(signing)?),
                encryption_key: EncryptionPublicKey::from_bytes(parse_hex32(encryption)?),
                label,
                relay_hints,
            }
        }
        _ => return Err(CliError::Arguments),
    };
    Ok(CliCommand::Peer {
        action,
        state: parsed_state(state_root)?,
    })
}

fn parse_mailbox(
    arguments: &[OsString],
    state_root: Option<&PathBuf>,
) -> Result<CliCommand, CliError> {
    let action = match arguments {
        [action] if action == "list" => MailboxCommand::List,
        [action, mailbox, peer] if action == "grant" => MailboxCommand::Grant {
            mailbox_id: MailboxId::from_bytes(parse_hex32(mailbox)?),
            peer_id: InstallationId::from_bytes(parse_hex32(peer)?),
        },
        [action, mailbox, peer] if action == "revoke" => MailboxCommand::Revoke {
            mailbox_id: MailboxId::from_bytes(parse_hex32(mailbox)?),
            peer_id: InstallationId::from_bytes(parse_hex32(peer)?),
        },
        _ => return Err(CliError::Arguments),
    };
    Ok(CliCommand::Mailbox {
        action,
        state: parsed_state(state_root)?,
    })
}

fn parse_relay_command(
    arguments: &[OsString],
    state_root: Option<&PathBuf>,
) -> Result<CliCommand, CliError> {
    let action = match arguments {
        [action] if action == "list" => RelayCommand::List,
        [action] if action == "status" => RelayCommand::Status,
        [action] if action == "repair" => RelayCommand::Repair,
        [action] if action == "sync" => RelayCommand::Sync { endpoint: None },
        [action, endpoint] if action == "sync" => RelayCommand::Sync {
            endpoint: Some(parse_relay(endpoint)?),
        },
        [action, endpoint] if action == "remove" => RelayCommand::Remove {
            endpoint: parse_relay(endpoint)?,
        },
        [action, endpoint, options @ ..] if action == "add" => {
            let (access, authentication) = parse_relay_policy_options(options)?;
            RelayCommand::Add {
                endpoint: parse_relay(endpoint)?,
                access,
                authentication,
            }
        }
        _ => return Err(CliError::Arguments),
    };
    Ok(CliCommand::Relay {
        action,
        state: parsed_state(state_root)?,
    })
}

fn parse_relay_policy_options(
    options: &[OsString],
) -> Result<(RelayAccessDto, RelayAuthenticationDto), CliError> {
    let mut access = RelayAccessDto::ReadWrite;
    let mut authentication = RelayAuthenticationDto::OnChallenge;
    let mut saw_access = false;
    let mut saw_authentication = false;
    let mut index = 0;
    while index < options.len() {
        match options[index].to_str() {
            Some("--access") if !saw_access => {
                access = match options.get(index + 1).and_then(|value| value.to_str()) {
                    Some("read") => RelayAccessDto::Read,
                    Some("write") => RelayAccessDto::Write,
                    Some("read-write") => RelayAccessDto::ReadWrite,
                    _ => return Err(CliError::Arguments),
                };
                saw_access = true;
                index += 2;
            }
            Some("--auth") if !saw_authentication => {
                authentication = match options.get(index + 1).and_then(|value| value.to_str()) {
                    Some("disabled") => RelayAuthenticationDto::Disabled,
                    Some("on-challenge") => RelayAuthenticationDto::OnChallenge,
                    Some("required") => RelayAuthenticationDto::Required,
                    _ => return Err(CliError::Arguments),
                };
                saw_authentication = true;
                index += 2;
            }
            _ => return Err(CliError::Arguments),
        }
    }
    Ok((access, authentication))
}

fn parse_identity(
    arguments: &[OsString],
    state_root: Option<&PathBuf>,
) -> Result<CliCommand, CliError> {
    let action = match arguments {
        [action] if action == "init" => IdentityCommand::Init,
        [action] if action == "show" => IdentityCommand::Show,
        [action, path, password_source]
            if action == "export" && password_source == "--password-stdin" =>
        {
            IdentityCommand::Export {
                destination: absolute_path(path)?,
            }
        }
        [action, path, password_source]
            if action == "import" && password_source == "--password-stdin" =>
        {
            IdentityCommand::Import {
                source: absolute_path(path)?,
            }
        }
        _ => return Err(CliError::Arguments),
    };
    Ok(CliCommand::Identity {
        action,
        state: parsed_state(state_root)?,
    })
}

fn parse_configuration(
    arguments: &[OsString],
    state_root: Option<&PathBuf>,
) -> Result<CliCommand, CliError> {
    let action = match arguments {
        [action] if action == "get" => ConfigurationCommand::Get,
        [set, key, provider] if set == "set" && key == "default-provider" => {
            ConfigurationCommand::SetDefaultProvider {
                provider: match provider.to_str() {
                    Some("none") => None,
                    Some(provider) => {
                        Some(ProviderId::new(provider).map_err(|_| CliError::Arguments)?)
                    }
                    None => return Err(CliError::Arguments),
                },
            }
        }
        [set, key, values @ ..] if set == "set" && key == "relays" => {
            let relays = if values == [OsString::from("none")] {
                Vec::new()
            } else {
                values
                    .iter()
                    .map(parse_relay)
                    .collect::<Result<Vec<_>, _>>()?
            };
            ConfigurationCommand::SetRelays { relays }
        }
        _ => return Err(CliError::Arguments),
    };
    Ok(CliCommand::Configuration {
        action,
        state: parsed_state(state_root)?,
    })
}

fn parse_human(
    arguments: &[OsString],
    state_root: Option<&PathBuf>,
) -> Result<CliCommand, CliError> {
    let action = match arguments {
        [action] if action == "create" => HumanCommand::Create { label: None },
        [action, label] if action == "create" => HumanCommand::Create {
            label: Some(
                label
                    .to_str()
                    .ok_or(CliError::Arguments)
                    .and_then(|label| ShortText::new(label).map_err(|_| CliError::Arguments))?,
            ),
        },
        [action] if action == "show" => HumanCommand::Show,
        [action, account] if action == "select" => HumanCommand::Select {
            account_id: AccountId::from_bytes(parse_hex32(account)?),
        },
        [action, installation, signing_key, destination, options @ ..] if action == "invite" => {
            let (label, relay_hints) = parse_pairing_options(options)?;
            HumanCommand::Invite {
                installation_id: InstallationId::from_bytes(parse_hex32(installation)?),
                signing_key: SigningPublicKey::from_bytes(parse_hex32(signing_key)?),
                destination: absolute_path(destination)?,
                label,
                relay_hints,
            }
        }
        [action, source] if action == "join" => HumanCommand::Join {
            source: absolute_path(source)?,
        },
        [action] if action == "devices" => HumanCommand::Devices,
        [action, installation] if action == "revoke" => HumanCommand::Revoke {
            installation_id: InstallationId::from_bytes(parse_hex32(installation)?),
        },
        _ => return Err(CliError::Arguments),
    };
    Ok(CliCommand::Human {
        action,
        state: parsed_state(state_root)?,
    })
}

fn parse_pairing_options(
    options: &[OsString],
) -> Result<(Option<ShortText>, RelayHints), CliError> {
    let mut label = None;
    let mut relays = Vec::new();
    let mut index = 0;
    while index < options.len() {
        match options[index].to_str() {
            Some("--label") if label.is_none() => {
                let value = options.get(index + 1).ok_or(CliError::Arguments)?;
                label = Some(
                    value
                        .to_str()
                        .ok_or(CliError::Arguments)
                        .and_then(|value| ShortText::new(value).map_err(|_| CliError::Arguments))?,
                );
                index += 2;
            }
            Some("--relay") => {
                let value = options
                    .get(index + 1)
                    .and_then(|value| value.to_str())
                    .ok_or(CliError::Arguments)?;
                let value = BoundedText::<RESOURCE_LOCATOR_MAX_BYTES>::new(value)
                    .map_err(|_| CliError::Arguments)?;
                relays.push(ResourceLocator::new(ResourceScheme::Opaque, value));
                index += 2;
            }
            _ => return Err(CliError::Arguments),
        }
    }
    relays.sort();
    if relays.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CliError::Arguments);
    }
    let relays = RelayHints::new(relays).map_err(|_| CliError::Arguments)?;
    Ok((label, relays))
}

fn parse_relay(value: &OsString) -> Result<RelayEndpoint, CliError> {
    value
        .to_str()
        .ok_or(CliError::Arguments)
        .and_then(|value| RelayEndpoint::new(value.to_owned()).map_err(|_| CliError::Arguments))
}

fn parsed_state(state_root: Option<&PathBuf>) -> Result<StatePaths, CliError> {
    state_root
        .cloned()
        .map_or_else(StatePaths::from_environment, StatePaths::new)
        .map_err(|_| CliError::StatePath)
}

fn absolute_path(value: &OsString) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(CliError::Arguments)
    }
}

fn parse_hex32(value: &OsString) -> Result<[u8; 32], CliError> {
    let value = value.to_str().ok_or(CliError::Arguments)?.as_bytes();
    if value.len() != 64 {
        return Err(CliError::Arguments);
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_chunks::<2>().0.iter().enumerate() {
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(output)
}

const fn hex_nibble(value: u8) -> Result<u8, CliError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(CliError::Arguments),
    }
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
    let format = output_hint(&arguments);
    match parse_cli(arguments).and_then(|invocation| run_cli_with_input(&invocation, input)) {
        Ok(stdout) => CliExecution {
            stdout,
            stderr: String::new(),
            exit_code: 0,
        },
        Err(error) => {
            let (code, message, class) = error.diagnostic();
            CliExecution {
                stdout: String::new(),
                stderr: render_error(format, code, message, class),
                exit_code: class.status(),
            }
        }
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
    match &invocation.command {
        CliCommand::Identity { action, state } => {
            return render_result(invocation.output, &run_identity(action, state, input)?);
        }
        CliCommand::Configuration { action, state } => {
            return render_result(invocation.output, &run_configuration(action, state)?);
        }
        CliCommand::Human { action, state } => {
            return render_result(invocation.output, &run_human(action, state)?);
        }
        CliCommand::Peer { action, state } => {
            return render_result(invocation.output, &run_peer(action, state)?);
        }
        CliCommand::Mailbox { action, state } => {
            return render_result(invocation.output, &run_mailbox(action, state)?);
        }
        CliCommand::Relay { action, state } => {
            return render_result(invocation.output, &run_relay(action, state)?);
        }
        CliCommand::Help { .. } | CliCommand::Version | CliCommand::Daemon { .. } => {}
    }
    let CliCommand::Daemon { action, state } = &invocation.command else {
        return match &invocation.command {
            CliCommand::Help { topic } => render_help(invocation.output, topic),
            CliCommand::Version => render_version(invocation.output),
            CliCommand::Daemon { .. }
            | CliCommand::Identity { .. }
            | CliCommand::Configuration { .. }
            | CliCommand::Human { .. }
            | CliCommand::Peer { .. }
            | CliCommand::Mailbox { .. }
            | CliCommand::Relay { .. } => unreachable!(),
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
    render_result(invocation.output, &output)
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
            let configuration =
                LocalConfiguration::new(configuration.relays, configuration.default_provider)?;
            owner.store_configuration(&configuration)?;
            Ok(CliResult::Configuration(Box::new(configuration)))
        }
        ConfigurationCommand::SetRelays { relays } => {
            configuration.relays.clone_from(relays);
            let configuration =
                LocalConfiguration::new(configuration.relays, configuration.default_provider)?;
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
    LocalNodeClient::connect(LocalNodeClientConfig {
        state: state.clone(),
        build: build()?,
        initial_view: InitialView::OnDemand,
        io_timeout: Duration::from_secs(2),
        command_deadline: Duration::from_secs(10),
        max_connection_attempts: nonzero(8),
        readiness_timeout: Duration::from_secs(10),
        readiness_retry_interval: Duration::from_millis(25),
        reconnect_initial: Duration::from_millis(25),
        reconnect_maximum: Duration::from_millis(250),
        completed_identity_capacity: nonzero(64),
    })
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
    Lifecycle {
        label: &'static str,
        observation: Box<LifecycleObservation>,
    },
    Stopped {
        intent: String,
    },
    Identity(Box<PublicIdentity>),
    Configuration(Box<LocalConfiguration>),
    Human(Box<HumanView>),
    HumanPairing(HumanPairingView),
    HumanDevices(Box<HumanDevicesView>),
    AuthorityAdmin(Box<AuthorityAdminView>),
    RelayAdmin(Box<RelayAdminView>),
    Completed {
        operation: &'static str,
    },
}

fn render_result(format: CliOutputFormat, result: &CliResult) -> Result<String, CliError> {
    match (format, result) {
        (CliOutputFormat::Human, CliResult::Lifecycle { label, observation }) => {
            Ok(format_observation(label, observation))
        }
        (CliOutputFormat::Human, CliResult::Stopped { intent }) => {
            Ok(format!("stopped intent={intent}\n"))
        }
        (CliOutputFormat::Human, CliResult::Identity(identity)) => Ok(format!(
            "installation={} public_key={} fingerprint={}\n",
            crate::identity::encode_hex(identity.installation_id.as_bytes()),
            crate::identity::encode_hex(&identity.signing_public_key),
            identity.fingerprint,
        )),
        (CliOutputFormat::Human, CliResult::Configuration(configuration)) => Ok(format!(
            "default_provider={} relays={}\n",
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
        )),
        (CliOutputFormat::Human, CliResult::Human(view)) => render_human_view(view),
        (CliOutputFormat::Human, CliResult::HumanDevices(view)) => render_human_devices(view),
        (CliOutputFormat::Human, CliResult::HumanPairing(view)) => Ok(render_human_pairing(view)),
        (CliOutputFormat::Human, CliResult::Completed { operation }) => {
            Ok(format!("completed operation={operation}\n"))
        }
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
        (CliOutputFormat::Json, CliResult::Completed { operation }) => {
            machine_record("completed", &serde_json::json!({ "operation": operation }))
        }
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
    let text = help_text(topic).ok_or(CliError::Arguments)?;
    match format {
        CliOutputFormat::Human => Ok(text.to_owned()),
        CliOutputFormat::Json => machine_record(
            "help",
            &serde_json::json!({ "text": text.trim_end(), "topic": topic }),
        ),
    }
}

fn help_text(topic: &[String]) -> Option<&'static str> {
    match topic {
        [] => Some(
            "HQ local client\n\n\
             Usage: hq [--output human|json] [--state-root ABSOLUTE_PATH] <COMMAND>\n\n\
             Commands:\n  help [COMMAND]  Show complete command help\n  version         Show build and protocol metadata\n  identity        Manage installation identity offline\n  config          Manage typed local defaults offline\n  human           Manage the local human account\n  peer            Manage directional peer routes\n  mailbox         Manage directional mailbox capabilities\n  relay           Manage relay policy, synchronization, and health\n  daemon          Manage the local node lifecycle\n\n\
             Global options:\n  --output human|json          Select human or hq-cli-output-v1 JSON records\n  --state-root ABSOLUTE_PATH   Select an installation state root\n  --help                       Show this help\n  --version                    Show build and protocol metadata\n",
        ),
        [command] if command == "version" => Some(
            "Usage: hq [--output human|json] version\n\nShow executable version, local protocol version, and build commit metadata.\n",
        ),
        [command] if command == "identity" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] identity <COMMAND>\n\n\
             Commands:\n  init                                      Create identity without overwrite\n  show                                      Show safe public identity metadata\n  export ABSOLUTE_PATH --password-stdin     Export an encrypted backup without overwrite\n  import ABSOLUTE_PATH --password-stdin     Import an encrypted backup without overwrite\n\n\
             Identity commands require exclusive offline ownership. Password input is one bounded UTF-8 line on stdin and is never accepted as an argument.\n",
        ),
        [command] if command == "config" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] config <COMMAND>\n\n\
             Commands:\n  get                                      Show all local defaults\n  set default-provider PROVIDER|none       Replace the provider default\n  set relays URL...|none                   Replace the complete relay set\n\n\
             Configuration commands require exclusive offline ownership.\n",
        ),
        [command] if command == "human" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] human <COMMAND>\n\n\
             Commands:\n  create [LABEL]                         Create/reconcile and select the local creator account\n  show                                   Show authoritative account and selection state\n  select ACCOUNT_ID                      Select one actively authorized account\n\n\
             invite INSTALLATION_ID SIGNING_KEY ABSOLUTE_PATH [--label LABEL] [--relay URL]...\n                                          Export one new signed invitation\n  join ABSOLUTE_PATH                     Verify, import, accept, and select one invitation\n  devices                               Show complete selected-account device history\n  revoke INSTALLATION_ID                Revoke one device as the account creator\n\n\
             Human commands start or connect to the local node and author only through application plans.\n",
        ),
        [command] if command == "peer" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] peer <COMMAND>\n\n\
             Commands:\n  add INSTALLATION_ID SIGNING_KEY ENCRYPTION_KEY [--label LABEL] [--relay URL]...\n  list\n  distrust INSTALLATION_ID\n\nRoutes are directional metadata only and never grant mailbox authority. Distrust revokes active local capabilities before blocking the route.\n",
        ),
        [command] if command == "mailbox" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] mailbox <COMMAND>\n\n\
             Commands:\n  list\n  grant MAILBOX_ID PEER_INSTALLATION_ID\n  revoke MAILBOX_ID PEER_INSTALLATION_ID\n\nMailbox capability commands require an exact locally owned mailbox and uniquely routable peer.\n",
        ),
        [command] if command == "relay" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] relay <COMMAND>\n\n\
             Commands:\n  add URL [--access read|write|read-write] [--auth disabled|on-challenge|required]\n  list\n  remove URL\n  sync [URL]\n  status\n  repair\n\nRelay removal disables the durable policy without erasing delivery history. Status is a bounded authoritative observation. Repair explicitly reverifies the immutable corpus and atomically replaces rebuildable indexes.\n",
        ),
        [command] if command == "daemon" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] daemon <COMMAND>\n\n\
             Commands:\n  run        Own the node in the foreground\n  status     Probe without starting a node\n  readiness  Return a ready node, starting one when absent\n  stop       Converge the node to absence\n  restart    Converge on a fresh ready generation\n",
        ),
        [command, action]
            if (command == "daemon"
                && matches!(
                    action.as_str(),
                    "run" | "status" | "readiness" | "stop" | "restart"
                ))
                || (command == "identity" && matches!(action.as_str(), "init" | "show"))
                || (command == "config" && action == "get")
                || (command == "human"
                    && matches!(
                        action.as_str(),
                        "show" | "create" | "select" | "invite" | "join" | "devices" | "revoke"
                    ))
                || (command == "peer"
                    && matches!(action.as_str(), "add" | "list" | "distrust"))
                || (command == "mailbox"
                    && matches!(action.as_str(), "list" | "grant" | "revoke"))
                || (command == "relay"
                    && matches!(
                        action.as_str(),
                        "add" | "list" | "remove" | "sync" | "status" | "repair"
                    )) =>
        {
            match command.as_str() {
                "daemon" => Some("Use `hq help daemon` for daemon command details.\n"),
                "identity" => Some("Use `hq help identity` for identity command details.\n"),
                "config" => Some("Use `hq help config` for configuration command details.\n"),
                "human" => Some("Use `hq help human` for human command details.\n"),
                "peer" => Some("Use `hq help peer` for peer command details.\n"),
                "mailbox" => Some("Use `hq help mailbox` for mailbox command details.\n"),
                "relay" => Some("Use `hq help relay` for relay command details.\n"),
                _ => None,
            }
        }
        _ => None,
    }
}

fn output_hint(arguments: &[OsString]) -> CliOutputFormat {
    arguments
        .windows(2)
        .find_map(|pair| {
            (pair[0] == "--output").then(|| match pair[1].to_str() {
                Some("json") => CliOutputFormat::Json,
                _ => CliOutputFormat::Human,
            })
        })
        .unwrap_or_default()
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

    use std::{collections::BTreeSet, ffi::OsString};

    use super::{
        CliCommand, CliError, CliOutputFormat, ConfigurationCommand, DaemonCommand, HumanCommand,
        HumanDeviceState, IdentityCommand, MailboxCommand, PeerCommand, RelayCommand,
        effect_outcome, execute_cli, human_devices_view, human_view, pairing_grant_id, parse_cli,
        read_password, run_cli, stable_relay_effect, stable_repair_operation,
    };
    use hq_domain::{
        AccountId, FactId, InstallationAddress, InstallationId, RelayHints, SigningPublicKey,
    };
    use hq_local_api::protocol::v1::{
        AuthoritativeSnapshotDto, DeviceGrantDto, EffectOutcomeDto, Id32, RelayAccessDto,
        RelayAuthenticationDto, SnapshotItem, SynchronizationRequestDto,
    };

    #[test]
    fn parser_accepts_global_output_and_explicit_daemon_roles() {
        let root = std::env::temp_dir().join("hq-cli-parser");
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
    fn help_snapshots_cover_the_complete_foundation_tree() {
        let root = run_cli(&parse_cli([]).expect("root help parses")).expect("root help");
        assert_eq!(
            root,
            "HQ local client\n\n\
             Usage: hq [--output human|json] [--state-root ABSOLUTE_PATH] <COMMAND>\n\n\
             Commands:\n  help [COMMAND]  Show complete command help\n  version         Show build and protocol metadata\n  identity        Manage installation identity offline\n  config          Manage typed local defaults offline\n  human           Manage the local human account\n  peer            Manage directional peer routes\n  mailbox         Manage directional mailbox capabilities\n  relay           Manage relay policy, synchronization, and health\n  daemon          Manage the local node lifecycle\n\n\
             Global options:\n  --output human|json          Select human or hq-cli-output-v1 JSON records\n  --state-root ABSOLUTE_PATH   Select an installation state root\n  --help                       Show this help\n  --version                    Show build and protocol metadata\n"
        );
        let identity = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("identity")])
                .expect("identity help parses"),
        )
        .expect("identity help");
        assert!(identity.contains("--password-stdin"));
        assert!(!identity.contains("PASSWORD"));
        let config = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("config")])
                .expect("config help parses"),
        )
        .expect("config help");
        assert!(config.contains("set relays URL...|none"));
        let human = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("human")])
                .expect("human help parses"),
        )
        .expect("human help");
        assert!(human.contains("select ACCOUNT_ID"));
        let peer = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("peer")]).expect("peer help parses"),
        )
        .expect("peer help");
        assert!(peer.contains("add INSTALLATION_ID SIGNING_KEY ENCRYPTION_KEY"));
        assert!(peer.contains("distrust INSTALLATION_ID"));
        let mailbox = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("mailbox")])
                .expect("mailbox help parses"),
        )
        .expect("mailbox help");
        assert!(mailbox.contains("grant MAILBOX_ID PEER_INSTALLATION_ID"));
        assert!(mailbox.contains("revoke MAILBOX_ID PEER_INSTALLATION_ID"));
        let relay = run_cli(
            &parse_cli([OsString::from("help"), OsString::from("relay")])
                .expect("relay help parses"),
        )
        .expect("relay help");
        assert!(relay.contains("add URL [--access read|write|read-write]"));
        assert!(relay.contains("repair"));
        let daemon = run_cli(
            &parse_cli([OsString::from("daemon"), OsString::from("--help")])
                .expect("daemon help parses"),
        )
        .expect("daemon help");
        assert!(daemon.contains("run        Own the node in the foreground"));
        assert!(daemon.contains("restart    Converge on a fresh ready generation"));
        assert_eq!(
            run_cli(
                &parse_cli([OsString::from("help"), OsString::from("unknown")])
                    .expect("unknown help path parses")
            ),
            Err(CliError::Arguments)
        );
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
}
