//! Convergent local node discovery, child launch, stop, and restart coordination.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use hq_local_api::protocol::v1::{LifecycleRequest, LifecycleState};

use crate::{LifecycleClient, LifecycleClientError, LifecycleObservation};

/// Protocol lifecycle requester used by the real client and deterministic fakes.
pub trait LifecycleProbe {
    /// Executes exactly one lifecycle request on a fresh negotiated connection.
    fn request(
        &mut self,
        request: LifecycleRequest,
    ) -> Result<LifecycleObservation, LifecycleClientError>;
}

impl LifecycleProbe for LifecycleClient {
    fn request(
        &mut self,
        request: LifecycleRequest,
    ) -> Result<LifecycleObservation, LifecycleClientError> {
        Self::request(self, request)
    }
}

/// Diagnostic child termination retained while waiting for a competing owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeChildExit {
    /// Portable process exit code when the platform supplied one.
    pub code: Option<i32>,
}

/// Stable process-launch adapter failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeLaunchError {
    /// The foreground node child could not be spawned.
    Spawn,
    /// Child status could not be inspected.
    Wait,
    /// A detached child could not be assigned a waiter.
    Reaper,
}

impl fmt::Display for NodeLaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "foreground node launch failed: {self:?}")
    }
}

impl Error for NodeLaunchError {}

/// Child-process operations required by the convergent coordinator.
pub trait NodeLauncher {
    /// Adapter-owned foreground child handle.
    type Child;

    /// Starts one candidate foreground node for the exact state root.
    fn spawn(&mut self, state_root: &Path) -> Result<Self::Child, NodeLaunchError>;
    /// Reports a terminal child without blocking.
    fn try_wait(
        &mut self,
        child: &mut Self::Child,
    ) -> Result<Option<NodeChildExit>, NodeLaunchError>;
    /// Releases a live or terminal child to adapter-owned reaping.
    fn release(&mut self, child: Self::Child) -> Result<(), NodeLaunchError>;
}

/// Real launcher for the current `hq` executable.
#[derive(Clone, Debug)]
pub struct ProcessNodeLauncher {
    executable: PathBuf,
}

impl ProcessNodeLauncher {
    /// Uses one explicit executable path without shell interpretation.
    pub const fn new(executable: PathBuf) -> Self {
        Self { executable }
    }

    /// Resolves the current executable for ordinary CLI autostart.
    pub fn current_executable() -> Result<Self, NodeLaunchError> {
        std::env::current_exe()
            .map(Self::new)
            .map_err(|_| NodeLaunchError::Spawn)
    }
}

impl NodeLauncher for ProcessNodeLauncher {
    type Child = Child;

    fn spawn(&mut self, state_root: &Path) -> Result<Self::Child, NodeLaunchError> {
        Command::new(&self.executable)
            .arg("--state-root")
            .arg(state_root)
            .arg("daemon")
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| NodeLaunchError::Spawn)
    }

    fn try_wait(
        &mut self,
        child: &mut Self::Child,
    ) -> Result<Option<NodeChildExit>, NodeLaunchError> {
        child
            .try_wait()
            .map(|status| {
                status.map(|status| NodeChildExit {
                    code: status.code(),
                })
            })
            .map_err(|_| NodeLaunchError::Wait)
    }

    fn release(&mut self, mut child: Self::Child) -> Result<(), NodeLaunchError> {
        std::thread::Builder::new()
            .name("hq-node-child-reaper".to_owned())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(|_| ())
            .map_err(|_| NodeLaunchError::Reaper)
    }
}

/// Explicit bounded convergence inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeCoordinatorConfig {
    /// Exact state root passed to candidate foreground children.
    pub state_root: PathBuf,
    /// Maximum wait for owner or generation convergence.
    pub readiness_timeout: Duration,
    /// Positive delay between bounded protocol probes.
    pub retry_interval: Duration,
}

/// Successful ready-node convergence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeReady {
    /// Typed ready observation from the authoritative protocol peer.
    pub observation: LifecycleObservation,
    /// Whether this coordinator spawned one candidate child.
    pub child_started: bool,
}

/// Idempotent terminal stop result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeStopped {
    /// No protocol owner existed when stop began.
    AlreadyAbsent,
    /// A live or uncertain owner converged to absence.
    Stopped,
}

/// Stable coordinator failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeCoordinatorError {
    /// State root, deadline, or retry interval was invalid.
    InvalidConfig,
    /// A live socket failed protocol probing and must not be shadowed by a child.
    Probe(LifecycleClientError),
    /// Candidate child process operations failed.
    Launch(NodeLaunchError),
    /// Restart could not identify the old diagnostic generation.
    GenerationUnavailable,
    /// No compatible owner converged before the fixed deadline.
    ReadinessTimeout {
        /// Candidate child exit observed while still allowing another winner.
        child_exit: Option<NodeChildExit>,
    },
}

impl fmt::Display for NodeCoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local node coordination failed: {self:?}")
    }
}

impl Error for NodeCoordinatorError {}

/// Sole convergence policy over one protocol probe and child launcher.
pub struct NodeClientCoordinator<P, L> {
    probe: P,
    launcher: L,
    config: NodeCoordinatorConfig,
}

impl<P: LifecycleProbe, L: NodeLauncher> NodeClientCoordinator<P, L> {
    /// Validates explicit coordinator dependencies and bounds.
    pub fn new(
        probe: P,
        launcher: L,
        config: NodeCoordinatorConfig,
    ) -> Result<Self, NodeCoordinatorError> {
        if !config.state_root.is_absolute()
            || config.readiness_timeout.is_zero()
            || config.retry_interval.is_zero()
            || config.retry_interval > config.readiness_timeout
            || Instant::now()
                .checked_add(config.readiness_timeout)
                .is_none()
        {
            return Err(NodeCoordinatorError::InvalidConfig);
        }
        Ok(Self {
            probe,
            launcher,
            config,
        })
    }

    /// Returns a live ready owner, starting one candidate only when the socket is absent.
    pub fn ensure_ready(&mut self) -> Result<NodeReady, NodeCoordinatorError> {
        match self.probe.request(LifecycleRequest::Readiness) {
            Ok(observation) if observation.status.state == LifecycleState::Ready => {
                return Ok(NodeReady {
                    observation,
                    child_started: false,
                });
            }
            Ok(_) => return self.wait_for_ready(None, None, false, true),
            Err(LifecycleClientError::Absent) => {}
            Err(LifecycleClientError::StaleReadiness) => {
                return self.wait_for_ready(None, None, false, true);
            }
            Err(error) => return Err(NodeCoordinatorError::Probe(error)),
        }
        let child = self
            .launcher
            .spawn(&self.config.state_root)
            .map_err(NodeCoordinatorError::Launch)?;
        self.wait_for_ready(Some(child), None, true, false)
    }

    /// Converges a missing or live owner to complete absence.
    pub fn stop(&mut self) -> Result<NodeStopped, NodeCoordinatorError> {
        match self.probe.request(LifecycleRequest::Stop) {
            Err(LifecycleClientError::Absent) => return Ok(NodeStopped::AlreadyAbsent),
            Ok(_) | Err(LifecycleClientError::ResponseLost) => {}
            Err(error) => return Err(NodeCoordinatorError::Probe(error)),
        }
        let deadline = self.deadline()?;
        loop {
            match self.probe.request(LifecycleRequest::Status) {
                Err(LifecycleClientError::Absent) => return Ok(NodeStopped::Stopped),
                Ok(observation) if observation.status.state == LifecycleState::Ready => {
                    match self.probe.request(LifecycleRequest::Stop) {
                        Ok(_) | Err(LifecycleClientError::ResponseLost) => {}
                        Err(LifecycleClientError::Absent) => return Ok(NodeStopped::Stopped),
                        Err(error) => return Err(NodeCoordinatorError::Probe(error)),
                    }
                }
                Ok(_) | Err(LifecycleClientError::ResponseLost) => {}
                Err(error) => return Err(NodeCoordinatorError::Probe(error)),
            }
            if Instant::now() >= deadline {
                return Err(NodeCoordinatorError::ReadinessTimeout { child_exit: None });
            }
            std::thread::sleep(self.config.retry_interval);
        }
    }

    /// Requests restart and waits for a distinct negotiated ready generation.
    pub fn restart(&mut self) -> Result<NodeReady, NodeCoordinatorError> {
        let old = match self.probe.request(LifecycleRequest::Status) {
            Ok(observation) => observation,
            Err(LifecycleClientError::Absent) => return self.ensure_ready(),
            Err(error) => return Err(NodeCoordinatorError::Probe(error)),
        };
        let old_nonce = old
            .readiness
            .as_ref()
            .map(|readiness| readiness.boot_nonce)
            .ok_or(NodeCoordinatorError::GenerationUnavailable)?;
        match self.probe.request(LifecycleRequest::Restart) {
            Ok(_) | Err(LifecycleClientError::ResponseLost) => {}
            Err(error) => return Err(NodeCoordinatorError::Probe(error)),
        }
        self.wait_for_ready(None, Some(old_nonce), false, true)
    }

    fn wait_for_ready(
        &mut self,
        mut child: Option<L::Child>,
        previous_nonce: Option<hq_local_api::protocol::v1::Id32>,
        mut child_started: bool,
        spawn_when_absent: bool,
    ) -> Result<NodeReady, NodeCoordinatorError> {
        let deadline = self.deadline()?;
        let mut child_exit = None;
        loop {
            match self.probe.request(LifecycleRequest::Readiness) {
                Ok(observation)
                    if observation.status.state == LifecycleState::Ready
                        && previous_nonce.is_none_or(|old| {
                            observation
                                .readiness
                                .as_ref()
                                .is_some_and(|readiness| readiness.boot_nonce != old)
                        }) =>
                {
                    if let Some(child) = child.take() {
                        self.launcher
                            .release(child)
                            .map_err(NodeCoordinatorError::Launch)?;
                    }
                    return Ok(NodeReady {
                        observation,
                        child_started,
                    });
                }
                Err(LifecycleClientError::Absent)
                    if spawn_when_absent && child.is_none() && child_exit.is_none() =>
                {
                    child = Some(
                        self.launcher
                            .spawn(&self.config.state_root)
                            .map_err(NodeCoordinatorError::Launch)?,
                    );
                    child_started = true;
                }
                Ok(_)
                | Err(
                    LifecycleClientError::Absent
                    | LifecycleClientError::ResponseLost
                    | LifecycleClientError::StaleReadiness,
                ) => {}
                Err(error) => {
                    if let Some(child) = child.take() {
                        self.launcher
                            .release(child)
                            .map_err(NodeCoordinatorError::Launch)?;
                    }
                    return Err(NodeCoordinatorError::Probe(error));
                }
            }
            if let Some(candidate) = child.as_mut() {
                child_exit = self
                    .launcher
                    .try_wait(candidate)
                    .map_err(NodeCoordinatorError::Launch)?
                    .or(child_exit);
            }
            if Instant::now() >= deadline {
                if let Some(child) = child.take() {
                    self.launcher
                        .release(child)
                        .map_err(NodeCoordinatorError::Launch)?;
                }
                return Err(NodeCoordinatorError::ReadinessTimeout { child_exit });
            }
            std::thread::sleep(self.config.retry_interval);
        }
    }

    fn deadline(&self) -> Result<Instant, NodeCoordinatorError> {
        Instant::now()
            .checked_add(self.config.readiness_timeout)
            .ok_or(NodeCoordinatorError::InvalidConfig)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{collections::VecDeque, path::Path, time::Duration};

    use hq_local_api::protocol::v1::{BuildMetadata, Id32, LifecycleState, LifecycleStatus};

    use super::{
        LifecycleClientError, LifecycleObservation, LifecycleProbe, NodeChildExit,
        NodeClientCoordinator, NodeCoordinatorConfig, NodeCoordinatorError, NodeLaunchError,
        NodeLauncher, NodeStopped,
    };
    use crate::ReadinessRecord;

    struct ScriptedProbe {
        responses: VecDeque<Result<LifecycleObservation, LifecycleClientError>>,
    }

    impl LifecycleProbe for ScriptedProbe {
        fn request(
            &mut self,
            _request: hq_local_api::protocol::v1::LifecycleRequest,
        ) -> Result<LifecycleObservation, LifecycleClientError> {
            self.responses
                .pop_front()
                .unwrap_or(Err(LifecycleClientError::Absent))
        }
    }

    #[derive(Default)]
    struct ScriptedLauncher {
        spawns: usize,
        exits: VecDeque<Option<NodeChildExit>>,
        releases: usize,
    }

    impl NodeLauncher for ScriptedLauncher {
        type Child = ();

        fn spawn(&mut self, _state_root: &Path) -> Result<Self::Child, NodeLaunchError> {
            self.spawns += 1;
            Ok(())
        }

        fn try_wait(
            &mut self,
            _child: &mut Self::Child,
        ) -> Result<Option<NodeChildExit>, NodeLaunchError> {
            Ok(self.exits.pop_front().flatten())
        }

        fn release(&mut self, _child: Self::Child) -> Result<(), NodeLaunchError> {
            self.releases += 1;
            Ok(())
        }
    }

    fn observation(nonce: u8) -> LifecycleObservation {
        let build = BuildMetadata::new("hq", "0.1.0", Some("coordinator")).expect("build");
        LifecycleObservation {
            status: LifecycleStatus {
                state: LifecycleState::Ready,
                build: build.clone(),
                revision: Some(0),
                generation: Some(Id32::new([nonce; 32])),
                detail: None,
            },
            readiness: Some(
                ReadinessRecord::new(
                    LifecycleState::Ready,
                    1,
                    build,
                    Id32::new([4; 32]),
                    0,
                    Id32::new([nonce; 32]),
                )
                .expect("readiness"),
            ),
        }
    }

    fn coordinator(
        responses: impl IntoIterator<Item = Result<LifecycleObservation, LifecycleClientError>>,
        launcher: ScriptedLauncher,
    ) -> NodeClientCoordinator<ScriptedProbe, ScriptedLauncher> {
        NodeClientCoordinator::new(
            ScriptedProbe {
                responses: responses.into_iter().collect(),
            },
            launcher,
            NodeCoordinatorConfig {
                state_root: std::env::temp_dir(),
                readiness_timeout: Duration::from_millis(20),
                retry_interval: Duration::from_millis(1),
            },
        )
        .expect("coordinator")
    }

    #[test]
    fn live_owner_is_returned_without_spawning() {
        let mut coordinator = coordinator([Ok(observation(1))], ScriptedLauncher::default());
        let ready = coordinator.ensure_ready().expect("ready");
        assert!(!ready.child_started);
        assert_eq!(coordinator.launcher.spawns, 0);
    }

    #[test]
    fn stale_readiness_is_retried_until_the_live_generation_matches() {
        let mut coordinator = coordinator(
            [
                Err(LifecycleClientError::StaleReadiness),
                Ok(observation(2)),
            ],
            ScriptedLauncher::default(),
        );
        let ready = coordinator
            .ensure_ready()
            .expect("ready generation converges");
        assert!(!ready.child_started);
        assert_eq!(coordinator.launcher.spawns, 0);
        assert_eq!(
            ready.observation.status.generation,
            Some(Id32::new([2; 32]))
        );
    }

    #[test]
    fn absent_owner_spawns_once_and_converges_even_if_the_candidate_child_exits() {
        let mut launcher = ScriptedLauncher::default();
        launcher
            .exits
            .push_back(Some(NodeChildExit { code: Some(1) }));
        let mut coordinator = coordinator(
            [
                Err(LifecycleClientError::Absent),
                Err(LifecycleClientError::Absent),
                Ok(observation(2)),
            ],
            launcher,
        );
        let ready = coordinator.ensure_ready().expect("competing owner wins");
        assert!(ready.child_started);
        assert_eq!(coordinator.launcher.spawns, 1);
        assert_eq!(coordinator.launcher.releases, 1);
    }

    #[test]
    fn incompatible_live_peer_is_not_shadowed_by_a_spawn() {
        let mut coordinator = coordinator(
            [Err(LifecycleClientError::Incompatible)],
            ScriptedLauncher::default(),
        );
        assert_eq!(
            coordinator.ensure_ready(),
            Err(NodeCoordinatorError::Probe(
                LifecycleClientError::Incompatible
            ))
        );
        assert_eq!(coordinator.launcher.spawns, 0);
    }

    #[test]
    fn lost_stop_acknowledgement_converges_to_absence() {
        let mut draining = observation(3);
        draining.status.state = LifecycleState::Draining;
        let mut coordinator = coordinator(
            [
                Err(LifecycleClientError::ResponseLost),
                Ok(draining),
                Err(LifecycleClientError::Absent),
            ],
            ScriptedLauncher::default(),
        );
        assert_eq!(coordinator.stop(), Ok(NodeStopped::Stopped));
        assert_eq!(coordinator.launcher.spawns, 0);
    }

    #[test]
    fn lost_restart_acknowledgement_converges_on_a_distinct_generation() {
        let mut coordinator = coordinator(
            [
                Ok(observation(4)),
                Err(LifecycleClientError::ResponseLost),
                Err(LifecycleClientError::Absent),
                Ok(observation(5)),
            ],
            ScriptedLauncher::default(),
        );
        let ready = coordinator.restart().expect("new generation");
        assert!(ready.child_started);
        assert_eq!(coordinator.launcher.spawns, 1);
        assert_eq!(
            ready.observation.readiness.expect("readiness").boot_nonce,
            Id32::new([5; 32])
        );
    }

    #[test]
    fn child_failure_is_reported_only_after_the_readiness_deadline() {
        let mut launcher = ScriptedLauncher::default();
        launcher
            .exits
            .push_back(Some(NodeChildExit { code: Some(7) }));
        let mut coordinator = coordinator([Err(LifecycleClientError::Absent)], launcher);
        assert_eq!(
            coordinator.ensure_ready(),
            Err(NodeCoordinatorError::ReadinessTimeout {
                child_exit: Some(NodeChildExit { code: Some(7) }),
            })
        );
        assert_eq!(coordinator.launcher.releases, 1);
    }
}
