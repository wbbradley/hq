//! Fair asynchronous listener and local-session event pumping.

use std::{error::Error, fmt, num::NonZeroU64};

use hq_application::{Application, ApplicationPorts};
use hq_local_api::{
    LifecycleControl, RevisionHub,
    protocol::v1::{BuildMetadata, Id32},
};
use tokio::io::unix::AsyncFd;

use crate::{
    AcceptedLocalStream, LocalSessionAdmissionError, LocalSessionDispatch,
    LocalSessionInvalidationReport, LocalSessionRegistry, LocalSessionRegistryConfig,
    LocalSessionShutdownReport, NodeFoundation, RuntimeArtifactErrorClass,
    local_transport::BoundLocalListener,
};

/// Plain fixed capacities and boot-local identity seed for one listener/session pump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalSessionPumpConfig {
    /// Existing fixed session, event, and per-session write capacities.
    pub registry: LocalSessionRegistryConfig,
    /// Fresh non-authoritative nonce for this process generation.
    pub boot_nonce: Id32,
}

/// Failure before the listener/session pump can own asynchronous readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSessionPumpStartError {
    /// No already-bound foundation listener was available for one-time transfer.
    Listener(RuntimeArtifactErrorClass),
    /// The descriptor could not be registered with the active Tokio reactor.
    RuntimeUnavailable,
    /// The process-generation nonce was zero.
    InvalidBootNonce,
}

impl fmt::Display for LocalSessionPumpStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local session pump failed to start: {self:?}")
    }
}

impl Error for LocalSessionPumpStartError {}

/// Failure while binding, publishing, and transferring the local runtime listener.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSessionPumpOpenError {
    /// The private listener could not be bound.
    Bind(RuntimeArtifactErrorClass),
    /// Atomic readiness metadata could not be published.
    Publish(RuntimeArtifactErrorClass),
    /// The bound listener could not transfer into asynchronous ownership.
    Start(LocalSessionPumpStartError),
}

impl fmt::Display for LocalSessionPumpOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local session pump could not open: {self:?}")
    }
}

impl Error for LocalSessionPumpOpenError {}

/// One bounded unit of progress from the sole local listener/session pump.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalSessionPumpEvent {
    /// A validated peer and its sole task entered the central registry.
    Accepted {
        /// Boot-local connection identity assigned by the pump.
        session_id: Id32,
    },
    /// A validated descriptor was dropped before spawning any session task.
    Rejected {
        /// Boot-local attempted connection identity.
        session_id: Id32,
        /// Stable admission rejection.
        error: LocalSessionAdmissionError,
    },
    /// One decoded/write/close/join event was processed through the registry.
    Session {
        /// Exact registry progress.
        dispatch: LocalSessionDispatch,
        /// Coalesced invalidations attempted at this safe point.
        invalidations: LocalSessionInvalidationReport,
    },
    /// One accepted peer failed kernel validation without affecting the listener.
    PeerRejected {
        /// Stable credential failure class.
        error: RuntimeArtifactErrorClass,
    },
    /// The listener failed and intake was closed.
    ListenerFailed {
        /// Stable listener failure class.
        error: RuntimeArtifactErrorClass,
    },
    /// The checked boot-local connection identity space was exhausted.
    ConnectionIdsExhausted,
    /// Intake is closed and no session task remains to make progress.
    Idle,
}

/// Plain outcome after closing the listener and joining every admitted session task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSessionPumpShutdownReport {
    /// Whether the transferred listener descriptor has been dropped.
    pub listener_closed: bool,
    /// Complete bounded registry drain diagnostics.
    pub sessions: LocalSessionShutdownReport,
}

/// Sole fair owner of one bound local listener and its bounded session registry.
#[derive(Debug)]
pub struct LocalSessionPump {
    listener: Option<AsyncFd<BoundLocalListener>>,
    sessions: LocalSessionRegistry,
    boot_nonce: Id32,
    next_connection: Option<NonZeroU64>,
    prefer_listener: bool,
}

impl LocalSessionPump {
    /// Transfers one already-bound foundation listener into the active Tokio reactor.
    pub fn start(
        foundation: &mut NodeFoundation,
        config: LocalSessionPumpConfig,
        hub: RevisionHub,
        build: BuildMetadata,
    ) -> Result<Self, LocalSessionPumpStartError> {
        if config.boot_nonce == Id32::new([0; 32]) {
            return Err(LocalSessionPumpStartError::InvalidBootNonce);
        }
        let listener = foundation
            .take_local_listener()
            .map_err(|error| LocalSessionPumpStartError::Listener(error.class()))?;
        let listener =
            AsyncFd::new(listener).map_err(|_| LocalSessionPumpStartError::RuntimeUnavailable)?;
        let sessions = LocalSessionRegistry::new(config.registry, hub, build);
        Ok(Self {
            listener: Some(listener),
            sessions,
            boot_nonce: config.boot_nonce,
            next_connection: NonZeroU64::new(1),
            prefer_listener: true,
        })
    }

    /// Waits for and processes one fairly selected listener or session event.
    pub async fn drive_next<P, L>(
        &mut self,
        application: &Application<P>,
        lifecycle: &L,
    ) -> LocalSessionPumpEvent
    where
        P: ApplicationPorts,
        L: LifecycleControl,
    {
        enum Ready {
            Listener(Result<AcceptedLocalStream, RuntimeArtifactErrorClass>),
            Session(Option<LocalSessionDispatch>),
        }

        let ready = match (self.listener.as_ref(), self.sessions.task_count()) {
            (None, 0) => return LocalSessionPumpEvent::Idle,
            (Some(listener), 0) => Ready::Listener(wait_for_peer(listener).await),
            (None, _) => Ready::Session(self.sessions.dispatch_next(application, lifecycle).await),
            (Some(listener), _) if self.prefer_listener => {
                tokio::select! {
                    biased;
                    accepted = wait_for_peer(listener) => Ready::Listener(accepted),
                    dispatch = self.sessions.dispatch_next(application, lifecycle) => {
                        Ready::Session(dispatch)
                    }
                }
            }
            (Some(listener), _) => {
                tokio::select! {
                    biased;
                    dispatch = self.sessions.dispatch_next(application, lifecycle) => {
                        Ready::Session(dispatch)
                    }
                    accepted = wait_for_peer(listener) => Ready::Listener(accepted),
                }
            }
        };

        match ready {
            Ready::Listener(Ok(accepted)) => {
                self.prefer_listener = false;
                self.admit(accepted)
            }
            Ready::Listener(Err(error))
                if matches!(
                    error,
                    RuntimeArtifactErrorClass::PeerCredentials
                        | RuntimeArtifactErrorClass::PeerMismatch
                ) =>
            {
                self.prefer_listener = false;
                LocalSessionPumpEvent::PeerRejected { error }
            }
            Ready::Listener(Err(error)) => {
                self.close_intake();
                LocalSessionPumpEvent::ListenerFailed { error }
            }
            Ready::Session(Some(dispatch)) => {
                self.prefer_listener = true;
                LocalSessionPumpEvent::Session {
                    dispatch,
                    invalidations: self.sessions.flush_invalidations(),
                }
            }
            Ready::Session(None) => LocalSessionPumpEvent::Idle,
        }
    }

    /// Drops listener readiness and closes future registry admission without closing live sessions.
    pub fn close_intake(&mut self) {
        self.listener.take();
        self.sessions.close_intake();
    }

    /// Stops decoded request dispatch while retaining accepted response writes.
    pub fn close_request_intake(&mut self) {
        self.sessions.close_request_intake();
    }

    /// Returns responses accepted by session writers but not yet confirmed written.
    pub fn pending_response_count(&self) -> usize {
        self.sessions.pending_response_count()
    }

    /// Performs one explicit bounded invalidation pass for an external revision wake.
    pub fn flush_invalidations(&mut self) -> LocalSessionInvalidationReport {
        self.sessions.flush_invalidations()
    }

    /// Closes intake and every session, then joins all admitted byte tasks.
    pub async fn shutdown(mut self) -> LocalSessionPumpShutdownReport {
        self.close_intake();
        LocalSessionPumpShutdownReport {
            listener_closed: self.listener.is_none(),
            sessions: self.sessions.shutdown().await,
        }
    }

    fn admit(&mut self, accepted: AcceptedLocalStream) -> LocalSessionPumpEvent {
        let Some(counter) = self.next_connection else {
            self.close_intake();
            return LocalSessionPumpEvent::ConnectionIdsExhausted;
        };
        self.next_connection = counter.get().checked_add(1).and_then(NonZeroU64::new);
        let mut bytes = self.boot_nonce.bytes();
        bytes[24..].copy_from_slice(&counter.get().to_be_bytes());
        let session_id = Id32::new(bytes);
        match self.sessions.admit(session_id, accepted) {
            Ok(()) => LocalSessionPumpEvent::Accepted { session_id },
            Err(error) => LocalSessionPumpEvent::Rejected { session_id, error },
        }
    }
}

async fn wait_for_peer(
    listener: &AsyncFd<BoundLocalListener>,
) -> Result<AcceptedLocalStream, RuntimeArtifactErrorClass> {
    loop {
        let mut ready = listener
            .readable()
            .await
            .map_err(|_| RuntimeArtifactErrorClass::OperatingSystem)?;
        let attempted = ready.try_io(|registered| match registered.get_ref().accept() {
            Err(error) if error.class() == RuntimeArtifactErrorClass::WouldBlock => {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
            result => Ok(result),
        });
        if let Ok(result) = attempted {
            return result
                .map_err(|_| RuntimeArtifactErrorClass::OperatingSystem)?
                .map_err(crate::RuntimeArtifactError::class);
        }
    }
}
