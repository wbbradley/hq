//! Concrete node lifecycle and application control around the neutral harness supervisor.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

use hq_application::{
    AgentSessionResult, ApplicationError, ApplicationErrorCode, ControlHarness, EffectOutcome,
    EffectRequest, SessionControl,
};
use hq_harness::{
    HarnessActivity, HarnessClock, HarnessDeliveryRecord, HarnessDeliveryState, HarnessEnvironment,
    HarnessError, HarnessErrorClass, HarnessLaunchRequest, HarnessOutput, HarnessOwnerToken,
    HarnessPersistencePort, HarnessRegistry, HarnessSessionControlOutcome, HarnessSessionOperation,
    HarnessSessionOperationKind, HarnessSessionOperationState, HarnessSessionRequest,
    HarnessSubmission, HarnessSupervisor, HarnessSupervisorConfig, HarnessSupervisorDependencies,
    HarnessTokenSource,
};
use hq_projects::{ProjectRuntimeDelivery, ProjectRuntimePort, ProjectRuntimeRequest};
use hq_store::Store;

use crate::{
    AgentSessionCanonicalPort, AgentSessionSelectionOutcome, CancellationToken, ComponentDrain,
    ComponentError, HarnessStoreAdapter, NodeComponent, PreparedAgentSessionSelection,
};

/// Node lifecycle owner for the complete neutral managed-runtime supervisor.
#[derive(Clone)]
pub struct HarnessNodeComponent {
    inner: Arc<HarnessNodeInner>,
}

struct HarnessNodeInner {
    config: HarnessSupervisorConfig,
    dependencies: HarnessSupervisorDependencies,
    canonical: Arc<dyn AgentSessionCanonicalPort>,
    supervisor: Mutex<Option<HarnessSupervisor>>,
    event_task: Mutex<Option<JoinHandle<Result<(), HarnessError>>>>,
    event_stop: AtomicBool,
    accepting: AtomicBool,
}

enum CanonicalPreparation {
    Ready(Option<Box<PreparedAgentSessionSelection>>),
    Rejected,
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
        canonical: Arc<dyn AgentSessionCanonicalPort>,
    ) -> Self {
        Self {
            inner: Arc::new(HarnessNodeInner {
                config,
                dependencies: HarnessSupervisorDependencies {
                    registry,
                    state: Arc::new(HarnessStoreAdapter::new(store)),
                    persistence,
                    clock,
                    tokens,
                },
                canonical,
                supervisor: Mutex::new(None),
                event_task: Mutex::new(None),
                event_stop: AtomicBool::new(false),
                accepting: AtomicBool::new(false),
            }),
        }
    }

    /// Composes a durable supervisor with no registered providers for the foreground baseline.
    pub fn without_providers(store: &Store) -> Self {
        Self::without_providers_with_canonical(store, Arc::new(UnavailableAgentSessionCanonical))
    }

    /// Composes the foreground baseline with canonical readiness selection but no providers.
    pub fn without_providers_with_canonical(
        store: &Store,
        canonical: Arc<dyn AgentSessionCanonicalPort>,
    ) -> Self {
        Self::with_registry_and_canonical(store, Arc::new(HarnessRegistry::new()), canonical)
    }

    /// Composes the foreground supervisor with an already validated provider registry.
    pub fn with_registry_and_canonical(
        store: &Store,
        registry: Arc<HarnessRegistry>,
        canonical: Arc<dyn AgentSessionCanonicalPort>,
    ) -> Self {
        Self::with_registry_persistence_and_canonical(
            store,
            registry,
            Arc::new(UnavailableHarnessPersistence),
            canonical,
        )
    }

    /// Composes the foreground supervisor with provider and canonical persistence capabilities.
    pub fn with_registry_persistence_and_canonical(
        store: &Store,
        registry: Arc<HarnessRegistry>,
        persistence: Arc<dyn HarnessPersistencePort>,
        canonical: Arc<dyn AgentSessionCanonicalPort>,
    ) -> Self {
        Self::new(
            HarnessSupervisorConfig::default(),
            store,
            registry,
            persistence,
            Arc::new(SystemHarnessClock),
            Arc::new(RandomHarnessTokens),
            canonical,
        )
    }

    fn prepare_session(
        &self,
        request: &EffectRequest<hq_application::AgentSessionRequest>,
    ) -> Result<CanonicalPreparation, ApplicationError> {
        let (context, resume) = match (&request.body.control, request.body.launch.as_ref()) {
            (SessionControl::Start, Some(context)) => (context, None),
            (SessionControl::Resume { session }, Some(context)) => (context, Some(session)),
            (SessionControl::Stop, None) => return Ok(CanonicalPreparation::Ready(None)),
            (SessionControl::Start | SessionControl::Resume { .. }, None)
            | (SessionControl::Stop, Some(_)) => {
                return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
            }
        };
        match self.inner.canonical.prepare(
            request.body.agent_id,
            &request.body.provider,
            resume,
            &context.directory,
        ) {
            Ok(prepared) => Ok(CanonicalPreparation::Ready(Some(Box::new(prepared)))),
            Err(error)
                if matches!(
                    error.code(),
                    ApplicationErrorCode::InvalidRequest
                        | ApplicationErrorCode::StateIdentityConflict
                        | ApplicationErrorCode::ItemNotFound
                ) =>
            {
                Ok(CanonicalPreparation::Rejected)
            }
            Err(error) => Err(error),
        }
    }

    fn launch_request(
        request: &EffectRequest<hq_application::AgentSessionRequest>,
    ) -> Result<Option<HarnessLaunchRequest>, ApplicationError> {
        let Some(context) = request.body.launch.as_ref() else {
            return if matches!(request.body.control, SessionControl::Stop) {
                Ok(None)
            } else {
                Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest))
            };
        };
        let session = match &request.body.control {
            SessionControl::Start => HarnessSessionRequest::Start,
            SessionControl::Resume { session } => HarnessSessionRequest::Resume {
                session_id: session.clone(),
            },
            SessionControl::Stop => {
                return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
            }
        };
        Ok(Some(HarnessLaunchRequest {
            agent_id: request.body.agent_id,
            project_id: None,
            launch_directory: Some(context.directory.clone()),
            provider_id: request.body.provider.clone(),
            session,
            environment: copy_launch_environment(&context.environment)?,
        }))
    }

    fn finish_session_control(
        &self,
        request: &EffectRequest<hq_application::AgentSessionRequest>,
        prepared: Option<&PreparedAgentSessionSelection>,
        outcome: HarnessSessionControlOutcome,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        match outcome {
            HarnessSessionControlOutcome::Ready(session) => {
                let prepared = prepared.ok_or_else(|| {
                    ApplicationError::new(ApplicationErrorCode::InvariantViolation)
                })?;
                match self.inner.canonical.select_ready(
                    request.operation_id,
                    request.request_digest,
                    request.issued_at,
                    prepared,
                    &request.body.provider,
                    &session,
                )? {
                    AgentSessionSelectionOutcome::Complete => {
                        Ok(EffectOutcome::Accepted(AgentSessionResult::Ready(session)))
                    }
                    AgentSessionSelectionOutcome::Uncertain => {
                        Ok(EffectOutcome::Uncertain(request.operation_id))
                    }
                    AgentSessionSelectionOutcome::Rejected => Ok(EffectOutcome::Rejected(
                        harness_domain_error("managed_session_selection_rejected"),
                    )),
                }
            }
            HarnessSessionControlOutcome::Stopped => {
                Ok(EffectOutcome::Accepted(AgentSessionResult::Stopped))
            }
            HarnessSessionControlOutcome::Rejected => Ok(EffectOutcome::Rejected(
                harness_domain_error("managed_session_rejected"),
            )),
            HarnessSessionControlOutcome::Uncertain => {
                Ok(EffectOutcome::Uncertain(request.operation_id))
            }
        }
    }

    fn with_supervisor<T>(
        &self,
        operation: impl FnOnce(&HarnessSupervisor) -> Result<T, HarnessError>,
    ) -> Result<T, ApplicationError> {
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ));
        }
        self.inner
            .supervisor
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))?
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))
            .and_then(|supervisor| operation(supervisor).map_err(map_harness_error))
    }

    fn wake_event_task(&self) {
        if let Ok(task) = self.inner.event_task.lock()
            && let Some(task) = task.as_ref()
        {
            task.thread().unpark();
        }
    }

    fn shutdown_supervisor(&self) -> Result<ComponentDrain, ComponentError> {
        self.inner.event_stop.store(true, Ordering::Release);
        let event_failed = self
            .inner
            .event_task
            .lock()
            .map_err(|_| ComponentError::unavailable())?
            .take()
            .is_some_and(|task| {
                task.thread().unpark();
                !matches!(task.join(), Ok(Ok(())))
            });
        let supervisor = self
            .inner
            .supervisor
            .lock()
            .map_err(|_| ComponentError::unavailable())?
            .take();
        let supervisor_result = supervisor.map_or(Ok(ComponentDrain::Complete), |supervisor| {
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
        })?;
        if event_failed || supervisor_result == ComponentDrain::Escalate {
            Ok(ComponentDrain::Escalate)
        } else {
            Ok(ComponentDrain::Complete)
        }
    }
}

fn run_harness_events(
    inner: &HarnessNodeInner,
    cancellation: &CancellationToken,
) -> Result<(), HarnessError> {
    while !inner.event_stop.load(Ordering::Acquire) && !cancellation.is_cancelled() {
        {
            let supervisor = inner
                .supervisor
                .lock()
                .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
            if let Err(error) = supervisor
                .as_ref()
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?
                .poll_events()
            {
                inner.accepting.store(false, Ordering::Release);
                return Err(error);
            }
        }
        thread::park_timeout(inner.config.event_poll_interval);
    }
    let supervisor = inner
        .supervisor
        .lock()
        .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
    let result = supervisor
        .as_ref()
        .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?
        .drain_event_streams(inner.config.drain_wait)
        .map(|_| ());
    if result.is_err() {
        inner.accepting.store(false, Ordering::Release);
    }
    result
}

struct UnavailableHarnessPersistence;

struct UnavailableAgentSessionCanonical;

impl AgentSessionCanonicalPort for UnavailableAgentSessionCanonical {
    fn prepare(
        &self,
        _agent_id: hq_domain::AgentId,
        _provider: &hq_domain::ProviderId,
        _resume_session: Option<&hq_domain::ProviderSessionId>,
        _directory: &hq_domain::ResourceLocator,
    ) -> Result<PreparedAgentSessionSelection, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    fn select_ready(
        &self,
        _operation_id: hq_domain::OperationId,
        _request_digest: hq_domain::CommandDigest,
        _issued_at: hq_domain::Timestamp,
        _prepared: &PreparedAgentSessionSelection,
        _provider: &hq_domain::ProviderId,
        _session: &hq_domain::ProviderSessionId,
    ) -> Result<AgentSessionSelectionOutcome, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl HarnessPersistencePort for UnavailableHarnessPersistence {
    fn persist_output(
        &self,
        _agent_id: hq_domain::AgentId,
        _provider_id: &hq_domain::ProviderId,
        _session_id: &hq_domain::ProviderSessionId,
        _output: &HarnessOutput,
    ) -> Result<(), HarnessError> {
        Err(HarnessError::new(HarnessErrorClass::Unavailable))
    }

    fn persist_activity(
        &self,
        _agent_id: hq_domain::AgentId,
        _provider_id: &hq_domain::ProviderId,
        _session_id: &hq_domain::ProviderSessionId,
        _activity: &HarnessActivity,
    ) -> Result<(), HarnessError> {
        Err(HarnessError::new(HarnessErrorClass::Unavailable))
    }
}

pub(crate) struct SystemHarnessClock;

impl HarnessClock for SystemHarnessClock {
    fn now_millis(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

struct RandomHarnessTokens;

impl HarnessTokenSource for RandomHarnessTokens {
    fn next_token(&self) -> Result<HarnessOwnerToken, HarnessError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
        HarnessOwnerToken::from_bytes(bytes)
    }
}

impl std::fmt::Debug for HarnessNodeComponent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HarnessNodeComponent")
            .field("config", &self.inner.config)
            .field("accepting", &self.inner.accepting.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl NodeComponent for HarnessNodeComponent {
    fn start(&mut self, cancellation: CancellationToken) -> Result<(), ComponentError> {
        let mut supervisor = self
            .inner
            .supervisor
            .lock()
            .map_err(|_| ComponentError::unavailable())?;
        if supervisor.is_none() {
            let started =
                HarnessSupervisor::new(self.inner.config.clone(), self.inner.dependencies.clone())
                    .map_err(|_| ComponentError::unavailable())?;
            *supervisor = Some(started);
        }
        self.inner.event_stop.store(false, Ordering::Release);
        let mut event_task = self
            .inner
            .event_task
            .lock()
            .map_err(|_| ComponentError::unavailable())?;
        if event_task.is_none() {
            let inner = Arc::clone(&self.inner);
            let task = thread::Builder::new()
                .name("hq-harness-events".to_owned())
                .spawn(move || run_harness_events(&inner, &cancellation))
                .map_err(|_| ComponentError::unavailable())?;
            *event_task = Some(task);
        }
        self.inner.accepting.store(true, Ordering::Release);
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.inner.accepting.store(false, Ordering::Release);
        let result = if let Some(supervisor) = self
            .inner
            .supervisor
            .lock()
            .map_err(|_| ComponentError::unavailable())?
            .as_ref()
        {
            supervisor
                .stop_intake()
                .map_err(|_| ComponentError::unavailable())
        } else {
            Ok(())
        };
        self.inner.event_stop.store(true, Ordering::Release);
        if let Some(task) = self
            .inner
            .event_task
            .lock()
            .map_err(|_| ComponentError::unavailable())?
            .as_ref()
        {
            task.thread().unpark();
        }
        result
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
        let kind = match &request.body.control {
            SessionControl::Start => HarnessSessionOperationKind::Start,
            SessionControl::Resume { session } => {
                HarnessSessionOperationKind::Resume(session.clone())
            }
            SessionControl::Stop => HarnessSessionOperationKind::Stop,
        };
        let prepared = match self.prepare_session(request)? {
            CanonicalPreparation::Ready(prepared) => prepared,
            CanonicalPreparation::Rejected => {
                return Ok(EffectOutcome::Rejected(harness_domain_error(
                    "managed_session_precondition",
                )));
            }
        };
        let launch = Self::launch_request(request)?;
        let operation = HarnessSessionOperation {
            operation_id: request.operation_id,
            request_digest: request.request_digest,
            agent_id: request.body.agent_id,
            provider_id: request.body.provider.clone(),
            kind,
            state: HarnessSessionOperationState::Prepared,
        };
        let outcome =
            self.with_supervisor(|supervisor| supervisor.control_session(&operation, launch))?;
        self.wake_event_task();
        self.finish_session_control(request, prepared.as_deref(), outcome)
    }
}

fn copy_launch_environment(
    source: &hq_application::LaunchEnvironment,
) -> Result<HarnessEnvironment, ApplicationError> {
    let mut entries = Vec::with_capacity(source.len());
    source.visit(|name, value| entries.push((name.to_owned(), value.to_vec())));
    HarnessEnvironment::copy_from(
        entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_slice())),
    )
    .map_err(map_harness_error)
}

impl ProjectRuntimePort for HarnessNodeComponent {
    fn start_or_resume(
        &self,
        request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<hq_domain::ProviderSessionId>, ApplicationError> {
        let outcome = self.with_supervisor(|supervisor| {
            let launch = HarnessLaunchRequest {
                agent_id: request.body.agent_id,
                project_id: Some(request.body.project_id),
                launch_directory: request.body.launch_directory.clone(),
                provider_id: request.body.provider.clone(),
                session: request.body.resume_session.as_ref().map_or(
                    HarnessSessionRequest::Start,
                    |session| HarnessSessionRequest::Resume {
                        session_id: session.clone(),
                    },
                ),
                environment: HarnessEnvironment::default(),
            };
            if request.body.resume_session.is_some() {
                supervisor.recover(launch)
            } else {
                supervisor.launch(launch)
            }
        });
        let outcome = match outcome {
            Ok(session) => Ok(EffectOutcome::Accepted(session)),
            Err(error) if error.code() == ApplicationErrorCode::ItemNotFound => Ok(
                EffectOutcome::Rejected(harness_domain_error("project_runtime_unavailable")),
            ),
            Err(error) => Err(error),
        };
        self.wake_event_task();
        outcome
    }

    fn deliver(
        &self,
        request: &EffectRequest<ProjectRuntimeDelivery>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        let outcome = self.with_supervisor(|supervisor| {
            let delivery = HarnessDeliveryRecord {
                agent_id: request.body.binding.agent_id,
                provider_id: request.body.binding.provider.clone(),
                session_id: request.body.binding.session.clone(),
                submission: HarnessSubmission {
                    submission_id: request.body.submission_id,
                    digest: request.request_digest,
                    operation_id: request.operation_id,
                    body: request.body.body.clone(),
                },
                queued_at_millis: 0,
                state: HarnessDeliveryState::Pending,
            };
            let attempt = supervisor.deliver(delivery);
            if attempt.as_ref().is_err_and(|error| {
                matches!(
                    error.class,
                    HarnessErrorClass::SubmissionIdentityConflict
                        | HarnessErrorClass::PersistenceCollision
                )
            }) {
                return Ok(EffectOutcome::Rejected(harness_domain_error(
                    "project_delivery_identity_conflict",
                )));
            }
            let retained =
                supervisor.delivery(request.body.binding.agent_id, request.body.submission_id)?;
            if retained
                .as_ref()
                .is_some_and(|delivery| !same_project_delivery(request, delivery))
            {
                return Ok(EffectOutcome::Rejected(harness_domain_error(
                    "project_delivery_identity_conflict",
                )));
            }
            match retained.map(|delivery| delivery.state) {
                Some(HarnessDeliveryState::Accepted) => Ok(EffectOutcome::Accepted(())),
                Some(HarnessDeliveryState::Rejected) => Ok(EffectOutcome::Rejected(
                    harness_domain_error("project_delivery_rejected"),
                )),
                Some(HarnessDeliveryState::Pending | HarnessDeliveryState::Uncertain) => {
                    Ok(EffectOutcome::Uncertain(request.operation_id))
                }
                None => attempt.map(|()| EffectOutcome::Uncertain(request.operation_id)),
            }
        });
        self.wake_event_task();
        outcome
    }

    fn stop(
        &self,
        request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.with_supervisor(|supervisor| {
            supervisor
                .stop(request.body.agent_id)
                .map(|_| EffectOutcome::Accepted(()))
        })
    }
}

fn same_project_delivery(
    request: &EffectRequest<ProjectRuntimeDelivery>,
    delivery: &HarnessDeliveryRecord,
) -> bool {
    delivery.agent_id == request.body.binding.agent_id
        && delivery.provider_id == request.body.binding.provider
        && delivery.session_id == request.body.binding.session
        && delivery.submission.submission_id == request.body.submission_id
        && delivery.submission.digest == request.request_digest
        && delivery.submission.operation_id == request.operation_id
        && delivery.submission.body == request.body.body
}

#[allow(
    clippy::expect_used,
    reason = "all callers pass reviewed static error codes"
)]
fn harness_domain_error(code: &'static str) -> hq_domain::DomainError {
    hq_domain::DomainError::new(
        hq_domain::ErrorCategory::Unresolved,
        hq_domain::ErrorCode::new(code).expect("static harness project error code"),
    )
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
        | HarnessErrorClass::ProtocolViolation
        | HarnessErrorClass::TransportClosed
        | HarnessErrorClass::ProcessFailed
        | HarnessErrorClass::CompatibilityMismatch
        | HarnessErrorClass::Unavailable
        | HarnessErrorClass::CleanupFailed => ApplicationErrorCode::AdapterUnavailable,
    };
    ApplicationError::new(code)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        collections::VecDeque,
        fs,
        num::{NonZeroU64, NonZeroUsize},
        path::PathBuf,
        sync::atomic::AtomicUsize,
        time::{Duration, Instant},
    };

    use hq_domain::{
        AgentId, AssignmentBinding, AssignmentId, CommandDigest, ContentText, MessageId,
        OperationId, ProjectId, ProviderId, ProviderSessionId, ThreadId, Timestamp,
    };
    use hq_harness::{
        HarnessCapabilities, HarnessCapability, HarnessDrainOutcome, HarnessEvent,
        HarnessEventPoll, HarnessFactory, HarnessInstance, HarnessInstanceRequest,
        HarnessInteractiveAnswer, HarnessOutputKind, HarnessSession, HarnessSubmissionLookup,
        HarnessSubmissionOutcome, OpenedHarnessSession,
    };

    use super::*;

    #[test]
    fn retained_project_delivery_must_match_the_exact_current_request() {
        let request = delivery_request();
        let mut retained = HarnessDeliveryRecord {
            agent_id: request.body.binding.agent_id,
            provider_id: request.body.binding.provider.clone(),
            session_id: request.body.binding.session.clone(),
            submission: HarnessSubmission {
                submission_id: request.body.submission_id,
                digest: request.request_digest,
                operation_id: request.operation_id,
                body: request.body.body.clone(),
            },
            queued_at_millis: 42,
            state: HarnessDeliveryState::Accepted,
        };
        assert!(same_project_delivery(&request, &retained));

        retained.submission.digest = CommandDigest::from_bytes([99; 32]);
        assert!(!same_project_delivery(&request, &retained));
        retained.submission.digest = request.request_digest;
        retained.submission.body = ContentText::new("changed").expect("body");
        assert!(!same_project_delivery(&request, &retained));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn component_owned_event_task_drains_live_output_and_joins_before_release() {
        let database = TestDatabase::new();
        let store = Store::open(&database.path, NonZeroUsize::MIN).expect("store opens");
        let provider_id = ProviderId::new("event-provider").expect("provider");
        let session_id = ProviderSessionId::new("event-session").expect("session");
        let events = Arc::new(Mutex::new(VecDeque::from([
            Ok(HarnessEventPoll::Event(HarnessEvent::Output(
                HarnessOutput {
                    output_id: MessageId::from_bytes([71; 32]),
                    operation_id: OperationId::from_bytes([72; 32]),
                    kind: HarnessOutputKind::FinalAnswer,
                    status: hq_domain::ActivityStatus::Succeeded,
                    body: ContentText::new("background answer").expect("body"),
                },
            ))),
            Ok(HarnessEventPoll::Closed),
        ])));
        let mut registry = HarnessRegistry::new();
        registry
            .register(
                provider_id.clone(),
                HarnessCapabilities {
                    supported: [
                        HarnessCapability::StartSessions,
                        HarnessCapability::SubmissionLookup,
                    ]
                    .into_iter()
                    .collect(),
                },
                Arc::new(EventFactory {
                    session_id: session_id.clone(),
                    events,
                }),
            )
            .expect("provider registers");
        let persistence = Arc::new(CountingPersistence::default());
        let mut component = HarnessNodeComponent::new(
            HarnessSupervisorConfig {
                event_poll_interval: Duration::from_secs(60),
                drain_wait: Duration::from_millis(20),
                ..HarnessSupervisorConfig::default()
            },
            &store,
            Arc::new(registry),
            persistence.clone(),
            Arc::new(SystemHarnessClock),
            Arc::new(RandomHarnessTokens),
            Arc::new(UnavailableAgentSessionCanonical),
        );
        let cancellation = CancellationToken::new();
        component
            .start(cancellation.child())
            .expect("component starts event task");
        component
            .inner
            .supervisor
            .lock()
            .expect("supervisor locks")
            .as_ref()
            .expect("supervisor started")
            .launch(HarnessLaunchRequest {
                agent_id: AgentId::from_bytes([73; 32]),
                project_id: None,
                launch_directory: None,
                provider_id,
                session: HarnessSessionRequest::Start,
                environment: HarnessEnvironment::default(),
            })
            .expect("worker launches");
        component.wake_event_task();

        let deadline = Instant::now() + Duration::from_secs(1);
        while persistence.outputs.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(persistence.outputs.load(Ordering::SeqCst), 1);
        assert!(
            component
                .inner
                .event_task
                .lock()
                .expect("task locks")
                .is_some()
        );

        let shutdown_started = Instant::now();
        component.stop_intake().expect("intake closes");
        cancellation.cancel();
        assert_eq!(component.drain(), Ok(ComponentDrain::Complete));
        assert!(shutdown_started.elapsed() < Duration::from_secs(1));
        assert!(
            component
                .inner
                .event_task
                .lock()
                .expect("task locks")
                .is_none()
        );
        assert!(
            component
                .inner
                .supervisor
                .lock()
                .expect("supervisor locks")
                .is_none()
        );
    }

    fn delivery_request() -> EffectRequest<ProjectRuntimeDelivery> {
        EffectRequest {
            operation_id: OperationId::from_bytes([1; 32]),
            request_digest: CommandDigest::from_bytes([2; 32]),
            issued_at: Timestamp::from_unix_millis(3),
            body: ProjectRuntimeDelivery {
                project_id: ProjectId::from_bytes([4; 32]),
                binding: AssignmentBinding {
                    assignment_id: AssignmentId::from_bytes([5; 32]),
                    agent_id: AgentId::from_bytes([6; 32]),
                    provider: ProviderId::new("provider").expect("provider"),
                    session: ProviderSessionId::new("session").expect("session"),
                },
                thread_id: ThreadId::from_bytes([7; 32]),
                submission_id: MessageId::from_bytes([8; 32]),
                sequence: NonZeroU64::new(9).expect("nonzero"),
                body: ContentText::new("body").expect("body"),
            },
        }
    }

    #[derive(Default)]
    struct CountingPersistence {
        outputs: AtomicUsize,
    }

    impl HarnessPersistencePort for CountingPersistence {
        fn persist_output(
            &self,
            _agent_id: AgentId,
            _provider_id: &ProviderId,
            _session_id: &ProviderSessionId,
            _output: &HarnessOutput,
        ) -> Result<(), HarnessError> {
            self.outputs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn persist_activity(
            &self,
            _agent_id: AgentId,
            _provider_id: &ProviderId,
            _session_id: &ProviderSessionId,
            _activity: &HarnessActivity,
        ) -> Result<(), HarnessError> {
            Ok(())
        }
    }

    struct EventFactory {
        session_id: ProviderSessionId,
        events: Arc<Mutex<VecDeque<Result<HarnessEventPoll, HarnessError>>>>,
    }

    impl HarnessFactory for EventFactory {
        fn create_instance(
            &self,
            _request: HarnessInstanceRequest,
        ) -> Result<Box<dyn HarnessInstance>, HarnessError> {
            Ok(Box::new(EventInstance {
                session_id: self.session_id.clone(),
                events: Arc::clone(&self.events),
            }))
        }
    }

    struct EventInstance {
        session_id: ProviderSessionId,
        events: Arc<Mutex<VecDeque<Result<HarnessEventPoll, HarnessError>>>>,
    }

    impl HarnessInstance for EventInstance {
        fn open_session(
            self: Box<Self>,
            _request: HarnessSessionRequest,
        ) -> Result<OpenedHarnessSession, HarnessError> {
            Ok(OpenedHarnessSession {
                session_id: self.session_id,
                session: Box::new(EventSession {
                    events: self.events,
                }),
            })
        }
    }

    struct EventSession {
        events: Arc<Mutex<VecDeque<Result<HarnessEventPoll, HarnessError>>>>,
    }

    impl HarnessSession for EventSession {
        fn submit(
            &mut self,
            _submission: HarnessSubmission,
        ) -> Result<HarnessSubmissionOutcome, HarnessError> {
            Ok(HarnessSubmissionOutcome::Accepted)
        }

        fn lookup_submission(
            &mut self,
            _submission: &HarnessSubmission,
        ) -> Result<HarnessSubmissionLookup, HarnessError> {
            Ok(HarnessSubmissionLookup::Missing)
        }

        fn cancel_operation(
            &mut self,
            _operation_id: OperationId,
        ) -> Result<hq_harness::HarnessCancellationOutcome, HarnessError> {
            Ok(hq_harness::HarnessCancellationOutcome::AlreadyFinished)
        }

        fn poll_event(&mut self, _wait: Duration) -> Result<HarnessEventPoll, HarnessError> {
            self.events
                .lock()
                .expect("events lock")
                .pop_front()
                .unwrap_or(Ok(HarnessEventPoll::TimedOut))
        }

        fn answer_interactive(
            &mut self,
            _answer: HarnessInteractiveAnswer,
        ) -> Result<(), HarnessError> {
            Ok(())
        }

        fn stop_intake(&mut self) -> Result<(), HarnessError> {
            Ok(())
        }

        fn drain(&mut self, _wait: Duration) -> Result<HarnessDrainOutcome, HarnessError> {
            Ok(HarnessDrainOutcome::Complete)
        }

        fn force_stop(&mut self) -> Result<(), HarnessError> {
            Ok(())
        }
    }

    struct TestDatabase {
        path: PathBuf,
    }

    impl TestDatabase {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(1);
            let root = std::env::temp_dir().join(format!(
                "hq-harness-component-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("test directory creates");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                    .expect("test directory permissions restrict");
            }
            Self {
                path: root.join("hq.sqlite3"),
            }
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            if let Some(parent) = self.path.parent() {
                let _ = fs::remove_dir_all(parent);
            }
        }
    }
}
