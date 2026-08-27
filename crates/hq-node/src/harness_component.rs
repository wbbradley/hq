//! Concrete node lifecycle and application control around the neutral harness supervisor.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use hq_application::{
    AgentSessionResult, ApplicationError, ApplicationErrorCode, ControlHarness, EffectOutcome,
    EffectRequest, SessionControl,
};
use hq_harness::{
    HarnessClock, HarnessEnvironment, HarnessError, HarnessErrorClass, HarnessLaunchRequest,
    HarnessPersistencePort, HarnessRegistry, HarnessSessionRequest, HarnessSupervisor,
    HarnessSupervisorConfig, HarnessSupervisorDependencies, HarnessTokenSource,
};
use hq_store::Store;

use crate::{
    CancellationToken, ComponentDrain, ComponentError, HarnessStoreAdapter, NodeComponent,
};

/// Node lifecycle owner for the complete neutral managed-runtime supervisor.
pub struct HarnessNodeComponent {
    config: HarnessSupervisorConfig,
    dependencies: HarnessSupervisorDependencies,
    supervisor: Mutex<Option<HarnessSupervisor>>,
    accepting: AtomicBool,
}

impl HarnessNodeComponent {
    /// Composes the neutral registry, durable store adapter, persistence, clock, and tokens.
    pub fn new(
        config: HarnessSupervisorConfig,
        store: &Store,
        registry: Arc<HarnessRegistry>,
        persistence: Arc<dyn HarnessPersistencePort>,
        clock: Arc<dyn HarnessClock>,
        tokens: Arc<dyn HarnessTokenSource>,
    ) -> Self {
        Self {
            config,
            dependencies: HarnessSupervisorDependencies {
                registry,
                state: Arc::new(HarnessStoreAdapter::new(store)),
                persistence,
                clock,
                tokens,
            },
            supervisor: Mutex::new(None),
            accepting: AtomicBool::new(false),
        }
    }

    fn with_supervisor<T>(
        &self,
        operation: impl FnOnce(&HarnessSupervisor) -> Result<T, HarnessError>,
    ) -> Result<T, ApplicationError> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ));
        }
        self.supervisor
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))?
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))
            .and_then(|supervisor| operation(supervisor).map_err(map_harness_error))
    }

    fn shutdown_supervisor(&mut self) -> Result<ComponentDrain, ComponentError> {
        let supervisor = self
            .supervisor
            .get_mut()
            .map_err(|_| ComponentError::unavailable())?
            .take();
        supervisor.map_or(Ok(ComponentDrain::Complete), |supervisor| {
            supervisor
                .shutdown()
                .map(|report| {
                    if report.workers_forced == 0 && report.failures.is_empty() {
                        ComponentDrain::Complete
                    } else {
                        ComponentDrain::Escalate
                    }
                })
                .map_err(|_| ComponentError::unavailable())
        })
    }
}

impl std::fmt::Debug for HarnessNodeComponent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HarnessNodeComponent")
            .field("config", &self.config)
            .field("accepting", &self.accepting.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl NodeComponent for HarnessNodeComponent {
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        if self
            .supervisor
            .get_mut()
            .map_err(|_| ComponentError::unavailable())?
            .is_none()
        {
            let supervisor = HarnessSupervisor::new(self.config.clone(), self.dependencies.clone())
                .map_err(|_| ComponentError::unavailable())?;
            *self
                .supervisor
                .get_mut()
                .map_err(|_| ComponentError::unavailable())? = Some(supervisor);
        }
        self.accepting.store(true, Ordering::Release);
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.accepting.store(false, Ordering::Release);
        if let Some(supervisor) = self
            .supervisor
            .get_mut()
            .map_err(|_| ComponentError::unavailable())?
            .as_ref()
        {
            supervisor
                .stop_intake()
                .map_err(|_| ComponentError::unavailable())?;
        }
        Ok(())
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        self.shutdown_supervisor()
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        self.shutdown_supervisor().map(|_| ())
    }
}

impl ControlHarness for HarnessNodeComponent {
    fn control_harness(
        &self,
        request: &EffectRequest<hq_application::AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        self.with_supervisor(|supervisor| match request.body().control() {
            SessionControl::Start => supervisor
                .launch(HarnessLaunchRequest {
                    agent_id: request.body().agent_id(),
                    project_id: None,
                    provider_id: request.body().provider().clone(),
                    session: HarnessSessionRequest::Start,
                    environment: HarnessEnvironment::default(),
                })
                .map(|session| EffectOutcome::Accepted(AgentSessionResult::Ready(session))),
            SessionControl::Resume { session } => supervisor
                .recover(HarnessLaunchRequest {
                    agent_id: request.body().agent_id(),
                    project_id: None,
                    provider_id: request.body().provider().clone(),
                    session: HarnessSessionRequest::Resume {
                        session_id: session.clone(),
                    },
                    environment: HarnessEnvironment::default(),
                })
                .map(|ready| EffectOutcome::Accepted(AgentSessionResult::Ready(ready))),
            SessionControl::Stop => supervisor
                .stop(request.body().agent_id())
                .map(|_| EffectOutcome::Accepted(AgentSessionResult::Stopped)),
        })
    }
}

const fn map_harness_error(error: HarnessError) -> ApplicationError {
    let code = match error.class {
        HarnessErrorClass::InvalidInput
        | HarnessErrorClass::Unsupported
        | HarnessErrorClass::SecretInputRejected
        | HarnessErrorClass::IntakeClosed => ApplicationErrorCode::InvalidRequest,
        HarnessErrorClass::ProviderNotRegistered | HarnessErrorClass::SessionNotFound => {
            ApplicationErrorCode::ItemNotFound
        }
        HarnessErrorClass::RegistrationConflict
        | HarnessErrorClass::UnsafeRecovery
        | HarnessErrorClass::SessionIdentityMismatch
        | HarnessErrorClass::SubmissionIdentityConflict
        | HarnessErrorClass::InteractiveAlreadyAnswered
        | HarnessErrorClass::OwnershipConflict
        | HarnessErrorClass::PersistenceCollision => ApplicationErrorCode::StateIdentityConflict,
        HarnessErrorClass::Backpressure => ApplicationErrorCode::IntakeFull,
        HarnessErrorClass::Crashed
        | HarnessErrorClass::Unavailable
        | HarnessErrorClass::CleanupFailed => ApplicationErrorCode::AdapterUnavailable,
    };
    ApplicationError::new(code)
}
