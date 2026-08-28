//! Minimal single-binary lifecycle roles for the Rust node.

use std::{
    collections::BTreeSet,
    error::Error,
    ffi::OsString,
    fmt::{self, Write as _},
    io::Read,
    num::NonZeroUsize,
    path::PathBuf,
    time::Duration,
};

use hq_application::{
    ApplicationError, LocalFactInputs, LocalInstallationAuthority, plan_human_account_creation,
    plan_human_account_selection, plan_human_mailbox_creation,
};
use hq_domain::{
    AccountId, CommandId, FactId, InstallationId, ProviderId, ShortText, SigningPublicKey,
    Timestamp,
};
use hq_local_api::{
    ClientEvent, InitialView,
    protocol::v1::{
        AuthoritativeSnapshotDto, BuildMetadata, LifecycleRequest, LifecycleState,
        MutationAttemptDto, MutationOutcomeDto, MutationRequest, SnapshotItem,
    },
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

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
    /// Backup password input was absent, oversized, malformed, or unreadable.
    SecretInput,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments => formatter.write_str(
                "usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] \
                 <help|version|identity|config|human|daemon>",
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
        )
    {
        return Err(CliError::Arguments);
    }
    Ok(CliInvocation { output, command })
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
        _ => return Err(CliError::Arguments),
    };
    Ok(CliCommand::Human {
        action,
        state: parsed_state(state_root)?,
    })
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
        CliCommand::Help { .. } | CliCommand::Version | CliCommand::Daemon { .. } => {}
    }
    let CliCommand::Daemon { action, state } = &invocation.command else {
        return match &invocation.command {
            CliCommand::Help { topic } => render_help(invocation.output, topic),
            CliCommand::Version => render_version(invocation.output),
            CliCommand::Daemon { .. }
            | CliCommand::Identity { .. }
            | CliCommand::Configuration { .. }
            | CliCommand::Human { .. } => unreachable!(),
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
        (CliOutputFormat::Json, CliResult::Completed { operation }) => {
            machine_record("completed", &serde_json::json!({ "operation": operation }))
        }
    }
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
             Commands:\n  help [COMMAND]  Show complete command help\n  version         Show build and protocol metadata\n  identity        Manage installation identity offline\n  config          Manage typed local defaults offline\n  human           Manage the local human account\n  daemon          Manage the local node lifecycle\n\n\
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
             Human commands start or connect to the local node and author only through application plans.\n",
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
                || (command == "human" && action == "show") =>
        {
            match command.as_str() {
                "daemon" => Some("Use `hq help daemon` for daemon command details.\n"),
                "identity" => Some("Use `hq help identity` for identity command details.\n"),
                "config" => Some("Use `hq help config` for configuration command details.\n"),
                "human" => Some("Use `hq help human` for human command details.\n"),
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

    use std::ffi::OsString;

    use super::{
        CliCommand, CliError, CliOutputFormat, ConfigurationCommand, DaemonCommand, HumanCommand,
        IdentityCommand, execute_cli, human_view, parse_cli, read_password, run_cli,
    };
    use hq_domain::InstallationId;
    use hq_local_api::protocol::v1::{AuthoritativeSnapshotDto, Id32, SnapshotItem};

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
             Commands:\n  help [COMMAND]  Show complete command help\n  version         Show build and protocol metadata\n  identity        Manage installation identity offline\n  config          Manage typed local defaults offline\n  human           Manage the local human account\n  daemon          Manage the local node lifecycle\n\n\
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
