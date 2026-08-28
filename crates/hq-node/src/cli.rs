//! Minimal single-binary lifecycle roles for the Rust node.

use std::{error::Error, ffi::OsString, fmt, num::NonZeroUsize, path::PathBuf, time::Duration};

use hq_local_api::protocol::v1::{BuildMetadata, LifecycleRequest, LifecycleState};

use crate::{
    ForegroundNodeConfig, ForegroundNodeError, LifecycleClient, LifecycleClientConfig,
    LifecycleClientError, LifecycleObservation, NodeClientCoordinator, NodeCoordinatorConfig,
    NodeCoordinatorError, ProcessNodeLauncher, RuntimePathError, RuntimePaths, StatePaths,
    run_foreground,
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
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments => formatter.write_str(
                "usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] \
                 <help|version|daemon>",
            ),
            Self::StatePath => formatter.write_str("node state path is unavailable or invalid"),
            Self::RuntimePath => formatter.write_str("node runtime path is unavailable or invalid"),
            Self::Build => formatter.write_str("node build metadata is invalid"),
            Self::Lifecycle(error) => error.fmt(formatter),
            Self::Coordinator(error) => error.fmt(formatter),
            Self::Foreground(error) => error.fmt(formatter),
            Self::Runtime => formatter.write_str("node process runtime is unavailable"),
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
            let state = state_root
                .clone()
                .map_or_else(StatePaths::from_environment, StatePaths::new)
                .map_err(|_| CliError::StatePath)?;
            CliCommand::Daemon { action, state }
        }
        _ => return Err(CliError::Arguments),
    };
    if state_root.is_some() && !matches!(command, CliCommand::Daemon { .. }) {
        return Err(CliError::Arguments);
    }
    Ok(CliInvocation { output, command })
}

/// Parses and executes one complete invocation with deterministic stream and exit selection.
pub fn execute_cli(arguments: impl IntoIterator<Item = OsString>) -> CliExecution {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let format = output_hint(&arguments);
    match parse_cli(arguments).and_then(|invocation| run_cli(&invocation)) {
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
    let CliCommand::Daemon { action, state } = &invocation.command else {
        return match &invocation.command {
            CliCommand::Help { topic } => render_help(invocation.output, topic),
            CliCommand::Version => render_version(invocation.output),
            CliCommand::Daemon { .. } => unreachable!(),
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
}

fn render_result(format: CliOutputFormat, result: &CliResult) -> Result<String, CliError> {
    match (format, result) {
        (CliOutputFormat::Human, CliResult::Lifecycle { label, observation }) => {
            Ok(format_observation(label, observation))
        }
        (CliOutputFormat::Human, CliResult::Stopped { intent }) => {
            Ok(format!("stopped intent={intent}\n"))
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
    }
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
             Commands:\n  help [COMMAND]  Show complete command help\n  version         Show build and protocol metadata\n  daemon          Manage the local node lifecycle\n\n\
             Global options:\n  --output human|json          Select human or hq-cli-output-v1 JSON records\n  --state-root ABSOLUTE_PATH   Select an installation state root for daemon commands\n  --help                       Show this help\n  --version                    Show build and protocol metadata\n",
        ),
        [command] if command == "version" => Some(
            "Usage: hq [--output human|json] version\n\nShow executable version, local protocol version, and build commit metadata.\n",
        ),
        [command] if command == "daemon" => Some(
            "Usage: hq [--state-root ABSOLUTE_PATH] [--output human|json] daemon <COMMAND>\n\n\
             Commands:\n  run        Own the node in the foreground\n  status     Probe without starting a node\n  readiness  Return a ready node, starting one when absent\n  stop       Converge the node to absence\n  restart    Converge on a fresh ready generation\n",
        ),
        [command, action]
            if command == "daemon"
                && matches!(
                    action.as_str(),
                    "run" | "status" | "readiness" | "stop" | "restart"
                ) =>
        {
            Some("Use `hq help daemon` for daemon command details.\n")
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
        CliCommand, CliError, CliOutputFormat, DaemonCommand, execute_cli, parse_cli, run_cli,
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
             Commands:\n  help [COMMAND]  Show complete command help\n  version         Show build and protocol metadata\n  daemon          Manage the local node lifecycle\n\n\
             Global options:\n  --output human|json          Select human or hq-cli-output-v1 JSON records\n  --state-root ABSOLUTE_PATH   Select an installation state root for daemon commands\n  --help                       Show this help\n  --version                    Show build and protocol metadata\n"
        );
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
}
