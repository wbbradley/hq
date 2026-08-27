//! Minimal single-binary lifecycle roles for the Rust node.

use std::{error::Error, ffi::OsString, fmt, num::NonZeroUsize, path::PathBuf, time::Duration};

use hq_local_api::protocol::v1::{BuildMetadata, LifecycleRequest, LifecycleState};

use crate::{
    ForegroundNodeConfig, ForegroundNodeError, LifecycleClient, LifecycleClientConfig,
    LifecycleClientError, LifecycleObservation, NodeClientCoordinator, NodeCoordinatorConfig,
    NodeCoordinatorError, ProcessNodeLauncher, RuntimePathError, RuntimePaths, StatePaths,
    run_foreground,
};

/// Closed command roles exposed by the first Rust node binary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeCliCommand {
    /// Print executable version metadata.
    Version,
    /// Own node generations in the foreground until explicit stop or signal.
    Run {
        /// Explicit installation state layout.
        state: StatePaths,
    },
    /// Probe current node state without starting a child.
    Status {
        /// Explicit installation state layout.
        state: StatePaths,
    },
    /// Return a ready owner, autostarting one candidate when absent.
    Readiness {
        /// Explicit installation state layout.
        state: StatePaths,
    },
    /// Converge any current owner to absence.
    Stop {
        /// Explicit installation state layout.
        state: StatePaths,
    },
    /// Converge on a distinct ready generation, starting when absent.
    Restart {
        /// Explicit installation state layout.
        state: StatePaths,
    },
}

/// Stable CLI parsing, setup, or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeCliError {
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

impl fmt::Display for NodeCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments => formatter.write_str(
                "usage: hq node <run|status|readiness|stop|restart> [--state-root ABSOLUTE_PATH]",
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

impl Error for NodeCliError {}

impl From<LifecycleClientError> for NodeCliError {
    fn from(error: LifecycleClientError) -> Self {
        Self::Lifecycle(error)
    }
}

impl From<NodeCoordinatorError> for NodeCliError {
    fn from(error: NodeCoordinatorError) -> Self {
        Self::Coordinator(error)
    }
}

impl From<ForegroundNodeError> for NodeCliError {
    fn from(error: ForegroundNodeError) -> Self {
        Self::Foreground(error)
    }
}

/// Parses process arguments without consulting node state or opening runtime artifacts.
pub fn parse_node_cli(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<NodeCliCommand, NodeCliError> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if arguments.as_slice() == [OsString::from("--version")]
        || arguments.as_slice() == [OsString::from("version")]
    {
        return Ok(NodeCliCommand::Version);
    }
    let [node, action, rest @ ..] = arguments.as_slice() else {
        return Err(NodeCliError::Arguments);
    };
    if node != "node" {
        return Err(NodeCliError::Arguments);
    }
    let state = parse_state(rest)?;
    match action.to_str() {
        Some("run") => Ok(NodeCliCommand::Run { state }),
        Some("status") => Ok(NodeCliCommand::Status { state }),
        Some("readiness") => Ok(NodeCliCommand::Readiness { state }),
        Some("stop") => Ok(NodeCliCommand::Stop { state }),
        Some("restart") => Ok(NodeCliCommand::Restart { state }),
        _ => Err(NodeCliError::Arguments),
    }
}

/// Executes one parsed role and returns its complete stdout record.
pub fn run_node_cli(command: &NodeCliCommand) -> Result<String, NodeCliError> {
    if *command == NodeCliCommand::Version {
        return Ok(format!("hq {}\n", env!("CARGO_PKG_VERSION")));
    }
    let state = match command {
        NodeCliCommand::Run { state }
        | NodeCliCommand::Status { state }
        | NodeCliCommand::Readiness { state }
        | NodeCliCommand::Stop { state }
        | NodeCliCommand::Restart { state } => state,
        NodeCliCommand::Version => return Err(NodeCliError::Arguments),
    };
    let runtime = RuntimePaths::new(state.root().join("runtime"))
        .map_err(|_error: RuntimePathError| NodeCliError::RuntimePath)?;
    let build = build()?;
    match command {
        NodeCliCommand::Version => Err(NodeCliError::Arguments),
        NodeCliCommand::Run { .. } => {
            let async_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| NodeCliError::Runtime)?;
            let report = async_runtime.block_on(run_foreground(foreground_config(
                state.clone(),
                runtime,
                build,
            )))?;
            Ok(format!("stopped intent={:?}\n", report.intent).to_lowercase())
        }
        NodeCliCommand::Status { .. } => {
            let mut client = lifecycle_client(runtime, build)?;
            client
                .request(LifecycleRequest::Status)
                .map(|observation| format_observation("status", &observation))
                .map_err(Into::into)
        }
        NodeCliCommand::Readiness { .. } => {
            let mut coordinator = coordinator(state, runtime, build)?;
            let ready = coordinator.ensure_ready()?;
            Ok(format_observation("readiness", &ready.observation))
        }
        NodeCliCommand::Stop { .. } => {
            let mut coordinator = coordinator(state, runtime, build)?;
            let stopped = coordinator.stop()?;
            Ok(format!("stop={stopped:?}\n").to_lowercase())
        }
        NodeCliCommand::Restart { .. } => {
            let mut coordinator = coordinator(state, runtime, build)?;
            let ready = coordinator.restart()?;
            Ok(format_observation("restart", &ready.observation))
        }
    }
}

fn parse_state(rest: &[OsString]) -> Result<StatePaths, NodeCliError> {
    match rest {
        [] => StatePaths::from_environment().map_err(|_| NodeCliError::StatePath),
        [flag, root] if flag == "--state-root" => {
            StatePaths::new(PathBuf::from(root)).map_err(|_| NodeCliError::StatePath)
        }
        _ => Err(NodeCliError::Arguments),
    }
}

fn build() -> Result<BuildMetadata, NodeCliError> {
    BuildMetadata::new(
        "hq",
        env!("CARGO_PKG_VERSION"),
        option_env!("HQ_BUILD_COMMIT"),
    )
    .map_err(|_| NodeCliError::Build)
}

fn lifecycle_client(
    runtime: RuntimePaths,
    build: BuildMetadata,
) -> Result<LifecycleClient, NodeCliError> {
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
) -> Result<NodeClientCoordinator<LifecycleClient, ProcessNodeLauncher>, NodeCliError> {
    let probe = lifecycle_client(runtime, build)?;
    let launcher = ProcessNodeLauncher::current_executable().map_err(|_| NodeCliError::Runtime)?;
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

fn format_observation(label: &str, observation: &LifecycleObservation) -> String {
    let state = match observation.status.state {
        LifecycleState::Starting => "starting",
        LifecycleState::Ready => "ready",
        LifecycleState::Draining => "draining",
        LifecycleState::Stopped => "stopped",
        LifecycleState::Failed => "failed",
    };
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

    use super::{NodeCliCommand, NodeCliError, parse_node_cli};

    #[test]
    fn parser_accepts_only_explicit_node_roles_and_absolute_state_roots() {
        let root = std::env::temp_dir().join("hq-cli-parser");
        let parsed = parse_node_cli([
            OsString::from("node"),
            OsString::from("restart"),
            OsString::from("--state-root"),
            root.clone().into_os_string(),
        ])
        .expect("restart parses");
        assert!(matches!(
            parsed,
            NodeCliCommand::Restart { state } if state.root() == root
        ));
        assert_eq!(
            parse_node_cli([OsString::from("daemon"), OsString::from("run")]),
            Err(NodeCliError::Arguments)
        );
        assert_eq!(
            parse_node_cli([
                OsString::from("node"),
                OsString::from("run"),
                OsString::from("--state-root"),
                OsString::from("relative"),
            ]),
            Err(NodeCliError::StatePath)
        );
    }
}
