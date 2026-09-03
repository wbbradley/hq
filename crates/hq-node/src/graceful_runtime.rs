//! Ordered composition of local protocol progress, lifecycle intent, and Unix signals.

use std::{cell::Cell, error::Error, fmt, future::Future, time::Duration};

use hq_application::{
    Application, ApplicationError, ApplicationErrorCode, ConfigureRelays, ControlHarness,
    ControlInteractions, ControlProjects, InspectResource, PublishWake, QueryInteractions,
    RetireAgents,
};
use hq_local_api::{
    LifecycleControl,
    protocol::v1::{BuildMetadata, LifecycleRequest, LifecycleState, LifecycleStatus},
};
use hq_reducer::AuthorityPolicy;
use tokio::{signal::unix::Signal, time::Instant};

use crate::{
    LocalSessionPump, LocalSessionPumpConfig, LocalSessionPumpEvent, LocalSessionPumpOpenError,
    LocalSessionPumpShutdownReport, NodeComponent, NodeLifecycleError, NodeOwner,
    NodeShutdownReport, ReadinessRecord, ScheduleProjectReconciliation, ShutdownIntent,
};

/// Explicit immutable inputs for one local node runtime generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalNodeRuntimeConfig {
    /// Listener/session capacities and boot-local connection identity seed.
    pub pump: LocalSessionPumpConfig,
    /// Safe client-visible build metadata.
    pub build: BuildMetadata,
    /// Installation-local authority inputs for application dispatch.
    pub authority_policy: AuthorityPolicy,
    /// Maximum wait for already-accepted response writes during drain.
    pub response_drain_timeout: Duration,
}

/// Failure before a local runtime generation enters its event loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalNodeRuntimeStartError {
    /// Accepted response drain must have a finite positive duration.
    InvalidResponseDrainTimeout,
    /// Runtime artifact binding, readiness publication, or listener transfer failed.
    Pump(LocalSessionPumpOpenError),
}

impl fmt::Display for LocalNodeRuntimeStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local node runtime failed to start: {self:?}")
    }
}

impl Error for LocalNodeRuntimeStartError {}

/// Stable runtime coordination failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalNodeRuntimeError {
    /// Node ownership or its application capabilities were unexpectedly absent.
    OwnerUnavailable,
    /// The requested stop/restart transition conflicted with retained lifecycle state.
    Lifecycle,
    /// Process signal handlers could not be registered.
    SignalRegistration,
}

impl fmt::Display for LocalNodeRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local node runtime failed: {self:?}")
    }
}

impl Error for LocalNodeRuntimeError {}

/// Complete ordered local-session and node-owner shutdown outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalNodeRuntimeReport {
    /// Terminal intent retained before cleanup began.
    pub intent: ShutdownIntent,
    /// Whether accepted response drain reached its configured deadline.
    pub response_drain_timed_out: bool,
    /// Complete local listener/session cleanup diagnostics.
    pub local_sessions: LocalSessionPumpShutdownReport,
    /// Complete component, task, and foundation cleanup diagnostics.
    pub node: NodeShutdownReport,
}

/// Sole asynchronous coordinator for one ready node owner and local session pump.
pub struct LocalNodeRuntime<L, R, H, P>
where
    L: NodeComponent,
    R: NodeComponent,
    H: NodeComponent,
    P: NodeComponent,
{
    owner: NodeOwner<L, R, H, P>,
    pump: LocalSessionPump,
    config: LocalNodeRuntimeConfig,
    generation: hq_local_api::protocol::v1::Id32,
}

impl<L, R, H, P> LocalNodeRuntime<L, R, H, P>
where
    L: NodeComponent,
    R: NodeComponent + PublishWake + ConfigureRelays,
    H: NodeComponent
        + ControlHarness
        + hq_application::QueryProviders
        + QueryInteractions
        + ControlInteractions,
    P: NodeComponent
        + InspectResource
        + ControlProjects
        + RetireAgents
        + ScheduleProjectReconciliation,
{
    /// Opens runtime artifacts for one already-ready node owner.
    pub fn start(
        mut owner: NodeOwner<L, R, H, P>,
        config: LocalNodeRuntimeConfig,
    ) -> Result<(Self, ReadinessRecord), LocalNodeRuntimeStartError> {
        if config.response_drain_timeout.is_zero() {
            return Err(LocalNodeRuntimeStartError::InvalidResponseDrainTimeout);
        }
        let (pump, readiness) = owner
            .open_local_session_pump(config.pump, config.build.clone())
            .map_err(LocalNodeRuntimeStartError::Pump)?;
        let generation = readiness.boot_nonce;
        Ok((
            Self {
                owner,
                pump,
                config,
                generation,
            },
            readiness,
        ))
    }

    /// Fairly drives protocol progress until one external or protocol lifecycle intent wins.
    pub async fn run_until<F>(
        mut self,
        shutdown: F,
    ) -> Result<LocalNodeRuntimeReport, LocalNodeRuntimeError>
    where
        F: Future<Output = ShutdownIntent>,
    {
        tokio::pin!(shutdown);
        let intent = loop {
            let status = self
                .owner
                .lifecycle_status(self.config.build.clone())
                .map_err(|_| LocalNodeRuntimeError::OwnerUnavailable)?
                .with_generation(self.generation);
            let lifecycle = CallLifecycle::new(status);
            let selected = {
                let ports = self
                    .owner
                    .application_ports(self.config.authority_policy)
                    .ok_or(LocalNodeRuntimeError::OwnerUnavailable)?;
                let application = Application::new(ports);
                tokio::select! {
                    intent = &mut shutdown => Selected::Intent(intent),
                    event = self.pump.drive_next(&application, &lifecycle) => {
                        Selected::Pump(event, lifecycle.intent.get())
                    }
                }
            };
            match selected {
                Selected::Intent(intent) | Selected::Pump(_, Some(intent)) => break intent,
                Selected::Pump(
                    LocalSessionPumpEvent::ListenerFailed { .. }
                    | LocalSessionPumpEvent::ConnectionIdsExhausted
                    | LocalSessionPumpEvent::Idle,
                    None,
                ) => break ShutdownIntent::Stop,
                Selected::Pump(_, None) => {}
            }
        };

        apply_intent(&mut self.owner, intent)?;
        self.pump.close_intake();
        self.pump.close_request_intake();
        let response_drain_timed_out = self.drain_accepted_responses().await?;
        let local_sessions = self.pump.shutdown().await;
        let node = self.owner.shutdown();
        Ok(LocalNodeRuntimeReport {
            intent,
            response_drain_timed_out,
            local_sessions,
            node,
        })
    }

    /// Registers `SIGINT`/`SIGTERM` and runs them through the same ordered stop drain.
    pub async fn run_with_unix_signals(
        self,
    ) -> Result<LocalNodeRuntimeReport, LocalNodeRuntimeError> {
        let mut signals = UnixShutdownSignals::register()
            .map_err(|_| LocalNodeRuntimeError::SignalRegistration)?;
        self.run_until(signals.recv()).await
    }

    async fn drain_accepted_responses(&mut self) -> Result<bool, LocalNodeRuntimeError> {
        if self.pump.pending_response_count() == 0 {
            return Ok(false);
        }
        let deadline = Instant::now() + self.config.response_drain_timeout;
        while self.pump.pending_response_count() != 0 {
            let status = self
                .owner
                .lifecycle_status(self.config.build.clone())
                .map_err(|_| LocalNodeRuntimeError::OwnerUnavailable)?
                .with_generation(self.generation);
            let lifecycle = CallLifecycle::new(status);
            let progressed = {
                let ports = self
                    .owner
                    .application_ports(self.config.authority_policy)
                    .ok_or(LocalNodeRuntimeError::OwnerUnavailable)?;
                let application = Application::new(ports);
                tokio::time::timeout_at(deadline, self.pump.drive_next(&application, &lifecycle))
                    .await
            };
            if progressed.is_err() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

enum Selected {
    Intent(ShutdownIntent),
    Pump(LocalSessionPumpEvent, Option<ShutdownIntent>),
}

struct CallLifecycle {
    status: LifecycleStatus,
    intent: Cell<Option<ShutdownIntent>>,
}

impl CallLifecycle {
    const fn new(status: LifecycleStatus) -> Self {
        Self {
            status,
            intent: Cell::new(None),
        }
    }

    fn request(&self, intent: ShutdownIntent) -> Result<(), ApplicationError> {
        match self.intent.get() {
            None => {
                self.intent.set(Some(intent));
                Ok(())
            }
            Some(retained) if retained == intent => Ok(()),
            Some(_) => Err(ApplicationError::new(
                ApplicationErrorCode::InvariantViolation,
            )),
        }
    }
}

impl LifecycleControl for CallLifecycle {
    fn lifecycle(&self, request: LifecycleRequest) -> Result<LifecycleStatus, ApplicationError> {
        let intent = match request {
            LifecycleRequest::Status | LifecycleRequest::Readiness => {
                return Ok(self.status.clone());
            }
            LifecycleRequest::Stop => ShutdownIntent::Stop,
            LifecycleRequest::Restart => ShutdownIntent::Restart,
        };
        self.request(intent)?;
        let mut draining = self.status.clone();
        draining.state = LifecycleState::Draining;
        Ok(draining)
    }
}

fn apply_intent<L, R, H, P>(
    owner: &mut NodeOwner<L, R, H, P>,
    intent: ShutdownIntent,
) -> Result<(), LocalNodeRuntimeError>
where
    L: NodeComponent,
    R: NodeComponent,
    H: NodeComponent,
    P: NodeComponent,
{
    let transitioned: Result<(), NodeLifecycleError> = match intent {
        ShutdownIntent::Stop => owner.request_stop(),
        ShutdownIntent::Restart => owner.request_restart(),
    };
    transitioned.map_err(|_| LocalNodeRuntimeError::Lifecycle)
}

/// Stable inability to register process signal streams.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnixSignalRegistrationError;

impl fmt::Display for UnixSignalRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Unix shutdown signals could not be registered")
    }
}

impl Error for UnixSignalRegistrationError {}

/// Owned `SIGINT` and `SIGTERM` streams for the process runtime.
pub struct UnixShutdownSignals {
    interrupt: Signal,
    terminate: Signal,
}

impl UnixShutdownSignals {
    /// Registers both supported process shutdown signals.
    pub fn register() -> Result<Self, UnixSignalRegistrationError> {
        let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .map_err(|_| UnixSignalRegistrationError)?;
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|_| UnixSignalRegistrationError)?;
        Ok(Self {
            interrupt,
            terminate,
        })
    }

    /// Waits for either supported signal and maps both to orderly stop intent.
    pub async fn recv(&mut self) -> ShutdownIntent {
        tokio::select! {
            _ = self.interrupt.recv() => ShutdownIntent::Stop,
            _ = self.terminate.recv() => ShutdownIntent::Stop,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use hq_local_api::protocol::v1::{BuildMetadata, LifecycleRequest, LifecycleState};

    use super::{CallLifecycle, LifecycleControl, ShutdownIntent};

    fn lifecycle() -> CallLifecycle {
        CallLifecycle::new(hq_local_api::protocol::v1::LifecycleStatus {
            state: LifecycleState::Ready,
            build: BuildMetadata::new("hq", "0.1.0", Some("test")).expect("build"),
            revision: Some(7),
            generation: Some(hq_local_api::protocol::v1::Id32::new([9; 32])),
            detail: None,
        })
    }

    #[test]
    fn repeated_lifecycle_intent_is_idempotent_and_conflicting_intent_fails_closed() {
        let lifecycle = lifecycle();
        let first = lifecycle
            .lifecycle(LifecycleRequest::Stop)
            .expect("first stop");
        let repeated = lifecycle
            .lifecycle(LifecycleRequest::Stop)
            .expect("repeated stop");
        assert_eq!(first.state, LifecycleState::Draining);
        assert_eq!(repeated.state, LifecycleState::Draining);
        assert_eq!(lifecycle.intent.get(), Some(ShutdownIntent::Stop));
        assert!(lifecycle.lifecycle(LifecycleRequest::Restart).is_err());
        assert_eq!(lifecycle.intent.get(), Some(ShutdownIntent::Stop));
    }
}
