//! Composition root and runtime ownership boundary.

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod agent_guidance;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod cli;
mod codex_component;
mod components;
mod coordination;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod foreground;
mod foundation;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod graceful_runtime;
mod harness_canonical;
mod harness_component;
mod harness_persistence;
mod harness_store;
mod identity;
mod lifecycle;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod lifecycle_client;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod local_client;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod local_transport;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod node_coordinator;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod pairing_file;
mod project_component;
mod project_resource;
mod project_store;
mod relay_component;
mod relay_store;
mod runtime;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod session_io;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod session_pump;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod session_registry;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod tui_client;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod tui_shell;
mod tui_theme;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix_frame;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use agent_guidance::AgentGuidanceTopic;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use cli::{
    AgentMailboxSelection, AgentMessageCommand, AuthorityAdminView, CliCommand, CliCompletion,
    CliError, CliExecution, CliExitClass, CliInvocation, CliOutputFormat, ConfigurationCommand,
    DaemonCommand, DomainHealthView, HumanAccountView, HumanCommand, HumanDeviceGrantView,
    HumanDeviceState, HumanDeviceView, HumanDevicesView, HumanMessageCommand, HumanMessageFilters,
    HumanRelayHintView, HumanView, IdentityCommand, MailboxCapabilityView, MailboxCommand,
    MailboxDiscoveryCandidate, MailboxDiscoveryView, MailboxView, NamedAgentCatalogView,
    NamedAgentCommand, NamedAgentRetirementView, NamedAgentSelector, NamedAgentSessionView,
    NamedAgentView, PeerCommand, PeerRouteBlockView, PeerRouteCandidateView, PeerRouteView,
    RelayAdminView, RelayCommand, RelayPolicyView, complete_cli_delivery, execute_cli,
    execute_cli_with_input, parse_cli, run_cli, run_cli_with_input,
};
pub use codex_component::{ForegroundCodexConfig, compose_codex_registry};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use foreground::{
    ForegroundNodeConfig, ForegroundNodeError, run_foreground, run_foreground_generation_until,
};
pub use foundation::{
    NodeFoundation, NodeFoundationConfig, NodeReadinessError, NodeShutdownError, NodeStartupError,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use graceful_runtime::{
    LocalNodeRuntime, LocalNodeRuntimeConfig, LocalNodeRuntimeError, LocalNodeRuntimeReport,
    LocalNodeRuntimeStartError, UnixShutdownSignals, UnixSignalRegistrationError,
};
pub use harness_canonical::{
    AgentSessionCanonicalPort, AgentSessionSelectionOutcome, ApplicationAgentSessionCanonicalPort,
    PreparedAgentSessionSelection,
};
pub use harness_component::HarnessNodeComponent;
pub use harness_persistence::CanonicalHarnessPersistence;
pub use harness_store::HarnessStoreAdapter;
pub use identity::{
    BackupPassword, IdentityError, IdentityErrorClass, InstallationIdentity,
    LocalCodexConfiguration, LocalConfiguration, PublicIdentity, RelayEndpoint,
    StateDirectoryOwner, StatePaths, ThemeSelection,
};
pub use lifecycle::{
    NodeAdmission, NodeLifecycle, NodeLifecycleError, NodePhase, NodeTransitionOutcome,
    OperatorAction, ShutdownIntent, StartupCause, StartupComponent, StartupDiagnostic,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use lifecycle_client::{
    LifecycleClient, LifecycleClientConfig, LifecycleClientError, LifecycleObservation,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use local_client::{
    LocalNodeClient, LocalNodeClientConfig, LocalNodeClientError, LocalNodeEventClient,
    UnixClientConnection, UnixClientInterrupt, UnixClientTransport, UnixClientTransportConfig,
    UnixClientTransportError, UnixClientWake,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use local_transport::{
    AcceptedLocalStream, MAX_READINESS_BYTES, ReadinessRecord, RuntimeArtifactError,
    RuntimeArtifactErrorClass,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use node_coordinator::{
    LifecycleProbe, NodeChildExit, NodeClientCoordinator, NodeCoordinatorConfig,
    NodeCoordinatorError, NodeLaunchError, NodeLauncher, NodeReady, NodeStopped,
    ProcessNodeLauncher,
};
pub use project_component::{
    ProjectMessageReconciliation, ProjectNodeComponent, ProjectNodeConfig,
    ReconcileProjectMessages, ScheduleProjectReconciliation, StandardProjectNodeComponent,
    StandardProjectWorker, WakingApplicationStore, compose_standard_project_component,
};
pub use project_resource::ProjectResourceAdapter;
pub use project_store::ProjectSagaStoreAdapter;
pub use relay_component::{RelayNodeComponent, RelayNodeConfig};
pub use relay_store::RelayStoreAdapter;
pub use runtime::{
    PORTABLE_UNIX_SOCKET_PATH_BYTES, RuntimeDirectoryOwner, RuntimePathError,
    RuntimePathErrorClass, RuntimePaths,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use session_io::{
    LocalSessionClose, LocalSessionEvent, LocalSessionHandle, LocalSessionSendError,
    LocalSessionStartError, prepare_local_session_io,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use session_pump::{
    LocalSessionPump, LocalSessionPumpConfig, LocalSessionPumpEvent, LocalSessionPumpOpenError,
    LocalSessionPumpShutdownReport, LocalSessionPumpStartError,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use session_registry::{
    LocalSessionAdmissionError, LocalSessionDisconnectCause, LocalSessionDispatch,
    LocalSessionInvalidationFailure, LocalSessionInvalidationReport, LocalSessionRegistry,
    LocalSessionRegistryConfig, LocalSessionShutdownReport, LocalSessionTaskFailure,
    LocalSessionTaskFailureKind,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use tui_client::{
    LocalTuiClient, LocalTuiObserver, MonotonicTuiClock, TuiClientObservation, TuiClientPort,
    TuiClock, TuiDraftError, TuiEffectExecutor, TuiEventWake, TuiExecutorError,
    TuiObservationControl, TuiObservationInterrupt, TuiObservationPort, tui_conversation_page,
    tui_snapshot, tui_snapshot_with_provider_catalog,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use tui_shell::{
    CrosstermTerminal, TuiShellError, TuiTerminalError, TuiTerminalEvent, TuiTerminalPort,
    normalize_crossterm_event, resolve_installed_tui_theme, run_installed_tui, run_tui_shell,
};
pub use tui_theme::{
    TuiThemeCatalogEntry, TuiThemeEnvironment, TuiThemeError, TuiThemeErrorClass, list_tui_themes,
    resolve_tui_theme,
};

use hq_application::InMemoryApplication;
use hq_protocol::{DecodeError, InMemoryFrame};
use hq_reducer::{GraphReductionReport, ReduceError};

/// Errors produced while composing the in-memory workspace skeleton.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InMemoryRunError {
    /// A frame failed protocol validation.
    Decode(DecodeError),
    /// Pure complete-batch reduction rejected an inconsistent domain stage result.
    Reduce(ReduceError),
}

/// Runs trusted in-memory frames through protocol, application, and reducer boundaries.
pub fn run_in_memory(
    frames: impl IntoIterator<Item = InMemoryFrame>,
) -> Result<GraphReductionReport, InMemoryRunError> {
    let mut application = InMemoryApplication::default();
    for frame in frames {
        let fact = frame.decode().map_err(InMemoryRunError::Decode)?;
        application.submit(fact);
    }

    application.summary().map_err(InMemoryRunError::Reduce)
}
pub use components::{
    ComponentDrain, ComponentError, ComponentKind, NodeApplicationPorts, NodeComponent,
    NodeComponents, NodeOwner, NodeOwnerStartError, NodeShutdownReport, ShutdownIssue,
    ShutdownStage,
};
pub use coordination::{
    CancellationToken, MAX_TASK_NAME_BYTES, MailboxReceiveError, MailboxReceiver, MailboxSendError,
    MailboxSender, TaskError, TaskFailure, TaskFailureKind, TaskJoinReport, TaskTracker,
    TaskTrackerError, bounded_mailbox,
};
