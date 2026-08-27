//! Pure node lifecycle and admission policy.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

/// Closed process-lifetime node phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodePhase {
    /// Required owners and components are still starting.
    Starting,
    /// Required components acknowledged readiness and ordinary intake is open.
    Ready,
    /// New side-effecting intake is closed while accepted work drains.
    Draining,
    /// Startup or runtime ownership failed with a stable diagnostic.
    Failed,
    /// Every owned component acknowledged shutdown.
    Stopped,
}

/// Intake family evaluated by lifecycle policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeAdmission {
    /// Read lifecycle and diagnostic state.
    Status,
    /// Perform a read-only authoritative query.
    Query,
    /// Begin a fact-backed mutation or other durable effect.
    Mutation,
    /// Launch a provider or external workflow.
    Launch,
}

/// Idempotent transition result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeTransitionOutcome {
    /// The phase changed.
    Changed,
    /// The requested terminal/idempotent state already held.
    Unchanged,
}

/// Requested terminal action retained throughout drain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownIntent {
    /// Stop completely and release ownership.
    Stop,
    /// Stop completely so a coordinator can start a fresh process generation.
    Restart,
}

/// Component whose startup failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupComponent {
    /// Process-lifetime state-directory lock.
    StateOwnership,
    /// Private installation identity.
    Identity,
    /// Unsigned installation-local defaults.
    Configuration,
    /// Private socket/readiness namespace.
    Runtime,
    /// Bounded synchronous durable store actor.
    Store,
}

/// Stable redacted startup cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupCause {
    /// A selected path was not absolute or usable.
    InvalidPath,
    /// An artifact was accessible beyond its owner.
    UnsafePermissions,
    /// An artifact was a symbolic link.
    SymbolicLink,
    /// Another process owns the same state root.
    AlreadyOwned,
    /// Required initialized state was absent.
    Missing,
    /// Durable state was malformed or failed verification.
    Malformed,
    /// Durable state belongs to an unsupported format or version.
    Incompatible,
    /// An operating-system, database, entropy, or worker capability was unavailable.
    Unavailable,
}

/// Stable operator response to one startup cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorAction {
    /// Supply or correct the selected path.
    CheckPath,
    /// Stop the existing owner or select a different state root.
    StopExistingNode,
    /// Initialize or import an installation identity.
    InitializeIdentity,
    /// Restrict ownership and permission bits.
    RepairPermissions,
    /// Inspect, repair, or restore durable state using an explicit administrative workflow.
    InspectState,
    /// Retry after the unavailable capability recovers.
    Retry,
}

/// Structured startup failure containing only stable values and explicitly selected paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupDiagnostic {
    component: StartupComponent,
    cause: StartupCause,
    action: OperatorAction,
    state_root: PathBuf,
    runtime_root: PathBuf,
}

impl StartupDiagnostic {
    /// Constructs one already-redacted diagnostic.
    pub fn new(
        component: StartupComponent,
        cause: StartupCause,
        state_root: PathBuf,
        runtime_root: PathBuf,
    ) -> Self {
        Self {
            component,
            cause,
            action: action_for(cause),
            state_root,
            runtime_root,
        }
    }

    /// Returns the failed component.
    pub const fn component(&self) -> StartupComponent {
        self.component
    }

    /// Returns the redacted stable cause.
    pub const fn cause(&self) -> StartupCause {
        self.cause
    }

    /// Returns the stable suggested operator action.
    pub const fn action(&self) -> OperatorAction {
        self.action
    }

    /// Returns the explicitly selected state root.
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Returns the explicitly selected runtime root.
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }
}

/// Out-of-order lifecycle transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeLifecycleError;

impl fmt::Display for NodeLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("node lifecycle transition is out of order")
    }
}

impl Error for NodeLifecycleError {}

/// Deterministic node lifecycle state machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeLifecycle {
    phase: NodePhase,
    revision: Option<u64>,
    failure: Option<StartupDiagnostic>,
    shutdown_intent: Option<ShutdownIntent>,
}

impl NodeLifecycle {
    /// Constructs a node before any readiness acknowledgement.
    pub const fn new() -> Self {
        Self {
            phase: NodePhase::Starting,
            revision: None,
            failure: None,
            shutdown_intent: None,
        }
    }

    /// Returns the current phase.
    pub const fn phase(&self) -> NodePhase {
        self.phase
    }

    /// Returns the readiness revision once acknowledged.
    pub const fn revision(&self) -> Option<u64> {
        self.revision
    }

    /// Returns the stable terminal/startup failure when present.
    pub const fn failure(&self) -> Option<&StartupDiagnostic> {
        self.failure.as_ref()
    }

    /// Returns the retained stop/restart intent after drain begins.
    pub const fn shutdown_intent(&self) -> Option<ShutdownIntent> {
        self.shutdown_intent
    }

    /// Evaluates lifecycle intake policy without performing I/O.
    pub fn admits(&self, admission: NodeAdmission) -> bool {
        match admission {
            NodeAdmission::Status => self.phase != NodePhase::Stopped,
            NodeAdmission::Query => {
                matches!(self.phase, NodePhase::Ready | NodePhase::Draining)
            }
            NodeAdmission::Mutation | NodeAdmission::Launch => self.phase == NodePhase::Ready,
        }
    }

    /// Acknowledges complete required-component readiness at one authoritative revision.
    pub fn mark_ready(
        &mut self,
        revision: u64,
    ) -> Result<NodeTransitionOutcome, NodeLifecycleError> {
        if self.phase != NodePhase::Starting {
            return Err(NodeLifecycleError);
        }
        self.phase = NodePhase::Ready;
        self.revision = Some(revision);
        Ok(NodeTransitionOutcome::Changed)
    }

    /// Closes side-effecting intake before component drain.
    pub fn begin_drain(&mut self) -> Result<NodeTransitionOutcome, NodeLifecycleError> {
        match self.phase {
            NodePhase::Starting | NodePhase::Ready | NodePhase::Failed => {
                self.phase = NodePhase::Draining;
                self.shutdown_intent = Some(ShutdownIntent::Stop);
                Ok(NodeTransitionOutcome::Changed)
            }
            NodePhase::Draining | NodePhase::Stopped => Ok(NodeTransitionOutcome::Unchanged),
        }
    }

    /// Records a clean restart intent and closes side-effecting intake.
    pub fn begin_restart(&mut self) -> Result<NodeTransitionOutcome, NodeLifecycleError> {
        match (self.phase, self.shutdown_intent) {
            (NodePhase::Ready, None) => {
                self.phase = NodePhase::Draining;
                self.shutdown_intent = Some(ShutdownIntent::Restart);
                Ok(NodeTransitionOutcome::Changed)
            }
            (NodePhase::Draining, Some(ShutdownIntent::Restart)) => {
                Ok(NodeTransitionOutcome::Unchanged)
            }
            (NodePhase::Starting | NodePhase::Failed | NodePhase::Stopped, _)
            | (NodePhase::Draining, None | Some(ShutdownIntent::Stop))
            | (NodePhase::Ready, Some(_)) => Err(NodeLifecycleError),
        }
    }

    /// Records a stable failure and closes every ordinary intake family.
    pub fn mark_failed(
        &mut self,
        diagnostic: StartupDiagnostic,
    ) -> Result<NodeTransitionOutcome, NodeLifecycleError> {
        if self.phase == NodePhase::Stopped {
            return Err(NodeLifecycleError);
        }
        if self.phase == NodePhase::Failed && self.failure.as_ref() == Some(&diagnostic) {
            return Ok(NodeTransitionOutcome::Unchanged);
        }
        self.phase = NodePhase::Failed;
        self.failure = Some(diagnostic);
        Ok(NodeTransitionOutcome::Changed)
    }

    /// Acknowledges that every owned component has stopped.
    pub fn acknowledge_stopped(&mut self) -> Result<NodeTransitionOutcome, NodeLifecycleError> {
        match self.phase {
            NodePhase::Draining | NodePhase::Failed => {
                self.phase = NodePhase::Stopped;
                Ok(NodeTransitionOutcome::Changed)
            }
            NodePhase::Stopped => Ok(NodeTransitionOutcome::Unchanged),
            NodePhase::Starting | NodePhase::Ready => Err(NodeLifecycleError),
        }
    }
}

impl Default for NodeLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

const fn action_for(cause: StartupCause) -> OperatorAction {
    match cause {
        StartupCause::InvalidPath => OperatorAction::CheckPath,
        StartupCause::UnsafePermissions => OperatorAction::RepairPermissions,
        StartupCause::SymbolicLink | StartupCause::Malformed | StartupCause::Incompatible => {
            OperatorAction::InspectState
        }
        StartupCause::AlreadyOwned => OperatorAction::StopExistingNode,
        StartupCause::Missing => OperatorAction::InitializeIdentity,
        StartupCause::Unavailable => OperatorAction::Retry,
    }
}
