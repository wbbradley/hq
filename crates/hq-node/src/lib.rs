//! Composition root and runtime ownership boundary.

mod components;
mod coordination;
mod foundation;
mod identity;
mod lifecycle;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod local_transport;
mod runtime;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod session_io;

pub use foundation::{
    NodeFoundation, NodeFoundationConfig, NodeReadinessError, NodeShutdownError, NodeStartupError,
};
pub use identity::{
    BackupPassword, IdentityError, IdentityErrorClass, InstallationIdentity, LocalConfiguration,
    PublicIdentity, RelayEndpoint, StateDirectoryOwner, StatePaths,
};
pub use lifecycle::{
    NodeAdmission, NodeLifecycle, NodeLifecycleError, NodePhase, NodeTransitionOutcome,
    OperatorAction, ShutdownIntent, StartupCause, StartupComponent, StartupDiagnostic,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use local_transport::{
    AcceptedLocalStream, MAX_READINESS_BYTES, ReadinessRecord, RuntimeArtifactError,
    RuntimeArtifactErrorClass,
};
pub use runtime::{
    PORTABLE_UNIX_SOCKET_PATH_BYTES, RuntimeDirectoryOwner, RuntimePathError,
    RuntimePathErrorClass, RuntimePaths,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use session_io::{
    LocalSessionClose, LocalSessionEvent, LocalSessionHandle, LocalSessionSendError,
    LocalSessionStartError, prepare_local_session_io,
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
