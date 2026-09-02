//! Concrete node lifecycle and application control around the neutral harness supervisor.

use std::thread::{self, JoinHandle};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use hq_application::{
    AgentSessionResult, ApplicationError, ApplicationErrorCode, ControlHarness,
    ControlInteractions, EffectOutcome, EffectRequest, InteractionAnswerOutcome,
    InteractionAnswerRequest, InteractionChoice, InteractionId, InteractionKind,
    InteractionResponderLease, InteractionResponse, PendingInteraction, ProviderAvailability,
    ProviderCatalog, QueryInteractions, QueryProviders, SessionControl,
};
use hq_harness::{
    HarnessActivity, HarnessClock, HarnessDeliveryRecord, HarnessDeliveryState, HarnessEnvironment,
    HarnessError, HarnessErrorClass, HarnessEventNotifier, HarnessInteractiveAnswer,
    HarnessInteractiveResponse, HarnessLaunchRequest, HarnessOutput, HarnessOwnerToken,
    HarnessPersistencePort, HarnessProjectDelivery, HarnessRegistry, HarnessRequestKind,
    HarnessResponderId, HarnessSessionControlOutcome, HarnessSessionOperation,
    HarnessSessionOperationKind, HarnessSessionOperationState, HarnessSessionRequest,
    HarnessSubmission, HarnessSupervisor, HarnessSupervisorConfig, HarnessSupervisorDependencies,
    HarnessTokenSource,
};
use hq_projects::{ProjectRuntimeDelivery, ProjectRuntimePort, ProjectRuntimeRequest};
use hq_store::Store;

use crate::{
    AgentSessionCanonicalPort, AgentSessionSelectionOutcome, CancellationToken, ComponentDrain,
    ComponentError, HarnessStoreAdapter, NodeComponent, PreparedAgentSessionSelection,
    boundary_trace::{BoundaryIds, BoundaryKind, BoundaryProcess, BoundaryTrace},
};

/// Node lifecycle owner for the complete neutral managed-runtime supervisor.
#[derive(Clone)]
pub struct HarnessNodeComponent {
    inner: Arc<HarnessNodeInner>,
}

struct HarnessNodeInner {
    config: HarnessSupervisorConfig,
    dependencies: HarnessSupervisorDependencies,
    default_provider: Option<hq_domain::ProviderId>,
    canonical: Arc<dyn AgentSessionCanonicalPort>,
    supervisor: Mutex<Option<HarnessSupervisor>>,
    event_task: Mutex<Option<JoinHandle<Result<(), HarnessError>>>>,
    event_notifications: HarnessEventNotifier,
    event_stop: AtomicBool,
    accepting: AtomicBool,
    interaction_answers: Mutex<BTreeMap<hq_domain::OperationId, InteractionCommandRecord>>,
    application_state: hq_store::ApplicationStateHandle,
    revisions: Mutex<Option<hq_local_api::RevisionHub>>,
    trace: BoundaryTrace,
}

#[derive(Clone)]
struct InteractionCommandRecord {
    digest: hq_domain::CommandDigest,
    outcome: InteractionAnswerOutcome,
}

const MAX_INTERACTION_COMMANDS: usize = 1_024;

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
        Self::new_with_default(
            config,
            store,
            registry,
            persistence,
            clock,
            tokens,
            canonical,
            None,
        )
    }

    /// Composes the supervisor with one installation-local configured provider preference.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_default(
        config: HarnessSupervisorConfig,
        store: &Store,
        registry: Arc<HarnessRegistry>,
        persistence: Arc<dyn HarnessPersistencePort>,
        clock: Arc<dyn HarnessClock>,
        tokens: Arc<dyn HarnessTokenSource>,
        canonical: Arc<dyn AgentSessionCanonicalPort>,
        default_provider: Option<hq_domain::ProviderId>,
    ) -> Self {
        let event_notifications = HarnessEventNotifier::default();
        Self {
            inner: Arc::new(HarnessNodeInner {
                config,
                dependencies: HarnessSupervisorDependencies {
                    registry,
                    state: Arc::new(HarnessStoreAdapter::new(store)),
                    persistence,
                    clock,
                    tokens,
                    events: event_notifications.clone(),
                },
                default_provider,
                canonical,
                supervisor: Mutex::new(None),
                event_task: Mutex::new(None),
                event_notifications,
                event_stop: AtomicBool::new(false),
                accepting: AtomicBool::new(false),
                interaction_answers: Mutex::new(BTreeMap::new()),
                application_state: store.application_state_handle(),
                revisions: Mutex::new(None),
                trace: BoundaryTrace::disabled(BoundaryProcess::Node),
            }),
        }
    }

    /// Replaces the disabled diagnostic sink before this component is shared or started.
    #[must_use]
    pub fn with_boundary_trace(mut self, trace: BoundaryTrace) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.trace = trace;
        }
        self
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
        Self::with_registry_persistence_canonical_and_default(
            store,
            registry,
            persistence,
            canonical,
            None,
        )
    }

    /// Composes the foreground supervisor with its configured provider preference.
    pub fn with_registry_persistence_canonical_and_default(
        store: &Store,
        registry: Arc<HarnessRegistry>,
        persistence: Arc<dyn HarnessPersistencePort>,
        canonical: Arc<dyn AgentSessionCanonicalPort>,
        default_provider: Option<hq_domain::ProviderId>,
    ) -> Self {
        Self::new_with_default(
            HarnessSupervisorConfig::default(),
            store,
            registry,
            persistence,
            Arc::new(SystemHarnessClock),
            Arc::new(RandomHarnessTokens),
            canonical,
            default_provider,
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
        let _ = self.inner.event_notifications.notify();
    }

    fn shutdown_supervisor(&self) -> Result<ComponentDrain, ComponentError> {
        self.inner.event_stop.store(true, Ordering::Release);
        let event_failed = self
            .inner
            .event_task
            .lock()
            .map_err(|_| ComponentError::unavailable())?
            .take();
        self.inner
            .event_notifications
            .notify()
            .map_err(|_| ComponentError::unavailable())?;
        let event_failed = event_failed.is_some_and(|task| !matches!(task.join(), Ok(Ok(()))));
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

struct HarnessInteractionResponderLease {
    inner: Weak<HarnessNodeInner>,
    responder_id: HarnessResponderId,
    active: bool,
}

impl InteractionResponderLease for HarnessInteractionResponderLease {
    fn activate(&mut self) -> Result<(), ApplicationError> {
        if self.active {
            return Ok(());
        }
        let inner = self
            .inner
            .upgrade()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))?;
        if !inner.accepting.load(Ordering::Acquire) {
            return Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ));
        }
        let supervisor = inner
            .supervisor
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))?;
        supervisor
            .as_ref()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))?
            .register_responder(self.responder_id)
            .map_err(map_harness_error)?;
        self.active = true;
        Ok(())
    }
}

impl Drop for HarnessInteractionResponderLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Ok(supervisor) = inner.supervisor.lock() else {
            return;
        };
        if let Some(supervisor) = supervisor.as_ref() {
            let _ = supervisor.unregister_responder(self.responder_id);
        }
        publish_interaction_invalidation(&inner);
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
            let report = supervisor
                .as_ref()
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?
                .drain_ready_events();
            match report {
                Ok(report)
                    if report.interactive_requests > 0
                        || report.interactive_requests_failed_closed > 0
                        || report.workers_closed > 0
                        || report.workers_failed > 0 =>
                {
                    if let Ok(pending) = supervisor
                        .as_ref()
                        .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?
                        .pending_interactions(hq_application::MAX_PENDING_INTERACTIONS)
                    {
                        for pending in pending {
                            inner.trace.record(
                                BoundaryKind::ProviderEventReceived,
                                BoundaryIds {
                                    operation: Some(*pending.request.operation_id.as_bytes()),
                                    provider_request: Some(*pending.request.request_id.as_bytes()),
                                    ..BoundaryIds::default()
                                },
                            );
                            inner.trace.record(
                                BoundaryKind::InteractionPublished,
                                BoundaryIds {
                                    operation: Some(*pending.request.operation_id.as_bytes()),
                                    provider_request: Some(*pending.request.request_id.as_bytes()),
                                    ..BoundaryIds::default()
                                },
                            );
                        }
                    }
                    publish_interaction_invalidation(inner);
                }
                Ok(_) => {}
                Err(error) => {
                    inner.accepting.store(false, Ordering::Release);
                    return Err(error);
                }
            }
        }
        inner.event_notifications.wait(None)?;
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
    fn running_agent_turns(
        &self,
        _agent_id: hq_domain::AgentId,
        _provider_id: &hq_domain::ProviderId,
        _session_id: &hq_domain::ProviderSessionId,
        _limit: usize,
    ) -> Result<Vec<HarnessActivity>, HarnessError> {
        Err(HarnessError::new(HarnessErrorClass::Unavailable))
    }

    fn persist_output(
        &self,
        _agent_id: hq_domain::AgentId,
        _provider_id: &hq_domain::ProviderId,
        _session_id: &hq_domain::ProviderSessionId,
        _delivery: Option<&hq_harness::HarnessDeliveryRecord>,
        _output: &HarnessOutput,
    ) -> Result<(), HarnessError> {
        Err(HarnessError::new(HarnessErrorClass::Unavailable))
    }

    fn persist_activity(
        &self,
        _agent_id: hq_domain::AgentId,
        _provider_id: &hq_domain::ProviderId,
        _session_id: &hq_domain::ProviderSessionId,
        _delivery: Option<&hq_harness::HarnessDeliveryRecord>,
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
    fn configure_revision_hub(&mut self, revisions: hq_local_api::RevisionHub) {
        if let Ok(mut configured) = self.inner.revisions.lock() {
            *configured = Some(revisions);
        }
    }

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
        self.inner
            .event_notifications
            .notify()
            .map_err(|_| ComponentError::unavailable())?;
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

impl QueryProviders for HarnessNodeComponent {
    fn provider_catalog(&self) -> Result<ProviderCatalog, ApplicationError> {
        let mut providers = self
            .inner
            .dependencies
            .registry
            .provider_catalog()
            .into_iter()
            .map(|provider| ProviderAvailability {
                provider: provider.provider,
                name: provider.name,
                available: true,
            })
            .collect::<Vec<_>>();
        if let Some(default_provider) = &self.inner.default_provider
            && !providers
                .iter()
                .any(|candidate| candidate.provider == *default_provider)
        {
            let name = hq_domain::ShortText::new(default_provider.as_str())
                .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvariantViolation))?;
            providers.push(ProviderAvailability {
                provider: default_provider.clone(),
                name,
                available: false,
            });
            providers.sort_by(|left, right| left.provider.cmp(&right.provider));
        }
        ProviderCatalog::new(providers, self.inner.default_provider.clone())
            .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvariantViolation))
    }
}

impl QueryInteractions for HarnessNodeComponent {
    fn pending_interactions(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingInteraction>, ApplicationError> {
        if limit == 0 || limit > hq_application::MAX_PENDING_INTERACTIONS {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
        }
        self.with_supervisor(|supervisor| supervisor.pending_interactions(limit))?
            .into_iter()
            .map(|pending| {
                let kind = match pending.request.kind {
                    HarnessRequestKind::Question => InteractionKind::Question,
                    HarnessRequestKind::CommandApproval => InteractionKind::CommandApproval,
                    HarnessRequestKind::FileApproval => InteractionKind::FileApproval,
                    HarnessRequestKind::Permission => InteractionKind::Permission,
                    HarnessRequestKind::McpUrl => InteractionKind::McpUrl,
                    HarnessRequestKind::McpForm => InteractionKind::McpForm,
                };
                let choices =
                    hq_domain::BoundedVec::new(pending.request.choices.into_vec().into_iter().map(
                        |choice| InteractionChoice {
                            value: choice.value,
                            label: choice.label,
                        },
                    ))
                    .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvariantViolation))?;
                Ok(PendingInteraction {
                    agent_id: pending.agent_id,
                    project_id: pending.project_id,
                    provider: pending.provider_id,
                    session: pending.session_id,
                    request_id: InteractionId::from_bytes(*pending.request.request_id.as_bytes()),
                    operation_id: pending.request.operation_id,
                    kind,
                    prompt: pending.request.prompt,
                    choices,
                    allow_text: pending.request.allow_text,
                })
            })
            .collect()
    }
}

impl ControlInteractions for HarnessNodeComponent {
    fn answer_interaction(
        &self,
        request: InteractionAnswerRequest,
    ) -> Result<InteractionAnswerOutcome, ApplicationError> {
        let mut commands = self
            .inner
            .interaction_answers
            .lock()
            .map_err(|_| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))?;
        if let Some(prior) = commands.get(&request.command_id()) {
            return if prior.digest == request.request_digest() {
                Ok(prior.outcome)
            } else {
                Err(ApplicationError::new(
                    ApplicationErrorCode::CommandIdentityConflict,
                ))
            };
        }
        if commands.len() == MAX_INTERACTION_COMMANDS {
            return Err(ApplicationError::new(ApplicationErrorCode::IntakeFull));
        }
        let response = match request.response().clone() {
            InteractionResponse::Text(value) => HarnessInteractiveResponse::Text(value),
            InteractionResponse::Choice(value) => HarnessInteractiveResponse::Choice(value),
            InteractionResponse::Approval(value) => HarnessInteractiveResponse::Approval(value),
            InteractionResponse::Cancelled => HarnessInteractiveResponse::Cancelled,
        };
        let answer = HarnessInteractiveAnswer {
            request_id: hq_harness::HarnessRequestId::from_bytes(*request.request_id().as_bytes()),
            response,
        };
        let outcome = match self
            .with_supervisor(|supervisor| supervisor.answer(request.agent_id(), answer))
        {
            Ok(()) => InteractionAnswerOutcome::Answered,
            Err(error)
                if matches!(
                    error.code(),
                    ApplicationErrorCode::InvalidRequest
                        | ApplicationErrorCode::ItemNotFound
                        | ApplicationErrorCode::StateIdentityConflict
                ) =>
            {
                InteractionAnswerOutcome::Stale
            }
            Err(error) => return Err(error),
        };
        commands.insert(
            request.command_id(),
            InteractionCommandRecord {
                digest: request.request_digest(),
                outcome,
            },
        );
        publish_interaction_invalidation(&self.inner);
        self.wake_event_task();
        Ok(outcome)
    }

    fn prepare_interaction_responder(
        &self,
        responder_id: hq_domain::OperationId,
    ) -> Result<Box<dyn InteractionResponderLease>, ApplicationError> {
        let responder_id =
            HarnessResponderId::from_bytes(*responder_id.as_bytes()).map_err(map_harness_error)?;
        Ok(Box::new(HarnessInteractionResponderLease {
            inner: Arc::downgrade(&self.inner),
            responder_id,
            active: false,
        }))
    }
}

fn publish_interaction_invalidation(inner: &HarnessNodeInner) {
    let Ok(revision) = inner.application_state.current_revision() else {
        return;
    };
    let Ok(revisions) = inner.revisions.lock() else {
        return;
    };
    if let Some(revisions) = revisions.as_ref() {
        inner.trace.record(
            BoundaryKind::LocalInvalidationPublished,
            BoundaryIds {
                revision: Some(revision.value()),
                ..BoundaryIds::default()
            },
        );
        let _ = revisions.publish(
            revision,
            [hq_application::SubscriptionTopic::Operations],
            false,
        );
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

fn project_launch_environment() -> Result<HarnessEnvironment, ApplicationError> {
    let entries = std::env::vars_os()
        .map(|(name, value)| {
            let name = name
                .into_string()
                .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvalidRequest))?;
            Ok((name, value.as_encoded_bytes().to_vec()))
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
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
        let environment = project_launch_environment()?;
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
                // Guided project work is launched by the long-running node rather than by a
                // one-shot CLI request. Preserve the node's copied launch environment so a
                // relative provider executable, its interpreter, and its user configuration
                // remain resolvable after the process starter clears ambient inheritance.
                environment,
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
        self.inner.trace.record(
            BoundaryKind::ProjectDispatched,
            BoundaryIds {
                message: Some(*request.body.submission_id.as_bytes()),
                dispatch: Some(*request.body.dispatch_id.as_bytes()),
                operation: Some(*request.operation_id.as_bytes()),
                ..BoundaryIds::default()
            },
        );
        self.inner.trace.record(
            BoundaryKind::CodexSubmitted,
            BoundaryIds {
                message: Some(*request.body.submission_id.as_bytes()),
                dispatch: Some(*request.body.dispatch_id.as_bytes()),
                operation: Some(*request.operation_id.as_bytes()),
                ..BoundaryIds::default()
            },
        );
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
                project: Some(HarnessProjectDelivery {
                    project_id: request.body.project_id,
                    dispatch_id: request.body.dispatch_id,
                    assignment_id: request.body.binding.assignment_id,
                    thread_id: request.body.thread_id,
                    sequence: request.body.sequence,
                }),
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
        && delivery.project.as_ref().is_some_and(|project| {
            project.project_id == request.body.project_id
                && project.dispatch_id == request.body.dispatch_id
                && project.assignment_id == request.body.binding.assignment_id
                && project.thread_id == request.body.thread_id
                && project.sequence == request.body.sequence
        })
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
        AgentId, AssignmentBinding, AssignmentId, BoundedVec, CommandDigest, ContentText,
        MessageId, OperationId, ProjectId, ProviderId, ProviderSessionId, ShortText, ThreadId,
        Timestamp,
    };
    use hq_harness::{
        HarnessCapabilities, HarnessCapability, HarnessDrainOutcome, HarnessEvent,
        HarnessEventPoll, HarnessFactory, HarnessInstance, HarnessInstanceRequest,
        HarnessInteractiveAnswer, HarnessInteractiveRequest, HarnessOutputKind,
        HarnessRequestChoice, HarnessRequestId, HarnessRequestKind, HarnessSession,
        HarnessSubmissionLookup, HarnessSubmissionOutcome, OpenedHarnessSession,
    };

    use super::*;

    #[test]
    fn project_launch_environment_preserves_the_nodes_executable_search_path() {
        let expected = std::env::var_os("PATH").expect("test process has PATH");
        let environment = project_launch_environment().expect("node environment copies");
        let mut actual = None;
        environment.visit(|name, value| {
            if name == "PATH" {
                actual = Some(value.to_vec());
            }
        });
        assert_eq!(actual.as_deref(), Some(expected.as_encoded_bytes()));
    }

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
            project: Some(HarnessProjectDelivery {
                project_id: request.body.project_id,
                dispatch_id: request.body.dispatch_id,
                assignment_id: request.body.binding.assignment_id,
                thread_id: request.body.thread_id,
                sequence: request.body.sequence,
            }),
            queued_at_millis: 42,
            state: HarnessDeliveryState::Accepted,
        };
        assert!(same_project_delivery(&request, &retained));

        retained.submission.digest = CommandDigest::from_bytes([99; 32]);
        assert!(!same_project_delivery(&request, &retained));
        retained.submission.digest = request.request_digest;
        retained.submission.body = ContentText::new("changed").expect("body");
        assert!(!same_project_delivery(&request, &retained));
        retained.submission.body = request.body.body.clone();

        let exact_project = retained.project.clone().expect("project provenance");
        retained.project = None;
        assert!(!same_project_delivery(&request, &retained));
        retained.project = Some(HarnessProjectDelivery {
            project_id: ProjectId::from_bytes([99; 32]),
            ..exact_project.clone()
        });
        assert!(!same_project_delivery(&request, &retained));
        retained.project = Some(HarnessProjectDelivery {
            dispatch_id: hq_domain::DispatchId::from_bytes([99; 32]),
            ..exact_project.clone()
        });
        assert!(!same_project_delivery(&request, &retained));
        retained.project = Some(HarnessProjectDelivery {
            assignment_id: AssignmentId::from_bytes([99; 32]),
            ..exact_project.clone()
        });
        assert!(!same_project_delivery(&request, &retained));
        retained.project = Some(HarnessProjectDelivery {
            thread_id: ThreadId::from_bytes([99; 32]),
            ..exact_project.clone()
        });
        assert!(!same_project_delivery(&request, &retained));
        retained.project = Some(HarnessProjectDelivery {
            sequence: NonZeroU64::new(99).expect("positive"),
            ..exact_project.clone()
        });
        assert!(!same_project_delivery(&request, &retained));
        retained.project = Some(exact_project);
        assert!(same_project_delivery(&request, &retained));
    }

    #[test]
    fn provider_catalog_exposes_empty_and_stale_configured_states_without_adapter_details() {
        let database = TestDatabase::new();
        let store = Store::open(&database.path, NonZeroUsize::MIN).expect("store opens");
        let empty = HarnessNodeComponent::without_providers(&store);
        assert!(
            empty
                .provider_catalog()
                .expect("empty catalog")
                .providers
                .is_empty()
        );

        let stale_id = ProviderId::new("removed").expect("provider");
        let stale = HarnessNodeComponent::new_with_default(
            HarnessSupervisorConfig::default(),
            &store,
            Arc::new(HarnessRegistry::new()),
            Arc::new(UnavailableHarnessPersistence),
            Arc::new(SystemHarnessClock),
            Arc::new(RandomHarnessTokens),
            Arc::new(UnavailableAgentSessionCanonical),
            Some(stale_id.clone()),
        );
        let catalog = stale.provider_catalog().expect("stale default catalog");
        assert_eq!(catalog.default_provider, Some(stale_id.clone()));
        assert!(matches!(
            catalog.providers.as_slice(),
            [ProviderAvailability { provider, available: false, .. }] if provider == &stale_id
        ));
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
        let answers = Arc::new(Mutex::new(Vec::new()));
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
                    answers,
                }),
            )
            .expect("provider registers");
        let persistence = Arc::new(CountingPersistence::default());
        let mut component = HarnessNodeComponent::new(
            HarnessSupervisorConfig {
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

    #[test]
    #[allow(clippy::too_many_lines)]
    fn interaction_answers_replay_equal_commands_and_reject_changed_reuse() {
        let database = TestDatabase::new();
        let store = Store::open(&database.path, NonZeroUsize::MIN).expect("store opens");
        let provider_id = ProviderId::new("event-provider").expect("provider");
        let session_id = ProviderSessionId::new("event-session").expect("session");
        let agent_id = AgentId::from_bytes([81; 32]);
        let request_id = HarnessRequestId::from_bytes([82; 32]);
        let events = Arc::new(Mutex::new(VecDeque::from([Ok(HarnessEventPoll::Event(
            HarnessEvent::InteractiveRequest(HarnessInteractiveRequest {
                request_id,
                operation_id: OperationId::from_bytes([83; 32]),
                kind: HarnessRequestKind::CommandApproval,
                prompt: ContentText::new("Run tests?").expect("prompt"),
                choices: BoundedVec::new([HarnessRequestChoice {
                    value: ShortText::new("accept").expect("value"),
                    label: ShortText::new("Allow once").expect("label"),
                }])
                .expect("choices"),
                allow_text: false,
            }),
        ))])));
        let answers = Arc::new(Mutex::new(Vec::new()));
        let mut registry = HarnessRegistry::new();
        registry
            .register(
                provider_id.clone(),
                HarnessCapabilities {
                    supported: [
                        HarnessCapability::StartSessions,
                        HarnessCapability::SubmissionLookup,
                        HarnessCapability::InteractiveRequests,
                    ]
                    .into_iter()
                    .collect(),
                },
                Arc::new(EventFactory {
                    session_id,
                    events,
                    answers: Arc::clone(&answers),
                }),
            )
            .expect("provider registers");
        let mut component = HarnessNodeComponent::new(
            HarnessSupervisorConfig::default(),
            &store,
            Arc::new(registry),
            Arc::new(CountingPersistence::default()),
            Arc::new(SystemHarnessClock),
            Arc::new(RandomHarnessTokens),
            Arc::new(UnavailableAgentSessionCanonical),
        );
        let cancellation = CancellationToken::new();
        component
            .start(cancellation.child())
            .expect("component starts");
        let mut responder = component
            .prepare_interaction_responder(OperationId::from_bytes([84; 32]))
            .expect("responder prepared");
        responder.activate().expect("responder activates");
        component
            .inner
            .supervisor
            .lock()
            .expect("supervisor locks")
            .as_ref()
            .expect("supervisor started")
            .launch(HarnessLaunchRequest {
                agent_id,
                project_id: None,
                launch_directory: None,
                provider_id,
                session: HarnessSessionRequest::Start,
                environment: HarnessEnvironment::default(),
            })
            .expect("worker launches");
        let deadline = Instant::now() + Duration::from_secs(1);
        while component
            .pending_interactions(1)
            .expect("pending query")
            .is_empty()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(1));
        }

        let command_id = OperationId::from_bytes([85; 32]);
        let request = InteractionAnswerRequest::new(
            command_id,
            agent_id,
            InteractionId::from_bytes(*request_id.as_bytes()),
            InteractionResponse::Choice(ShortText::new("accept").expect("choice")),
        );
        assert_eq!(
            component
                .answer_interaction(request.clone())
                .expect("first answer"),
            InteractionAnswerOutcome::Answered
        );
        assert_eq!(
            component
                .answer_interaction(request)
                .expect("equal retry replays"),
            InteractionAnswerOutcome::Answered
        );
        let changed = InteractionAnswerRequest::new(
            command_id,
            agent_id,
            InteractionId::from_bytes(*request_id.as_bytes()),
            InteractionResponse::Cancelled,
        );
        assert_eq!(
            component
                .answer_interaction(changed)
                .expect_err("changed command identity conflicts")
                .code(),
            ApplicationErrorCode::CommandIdentityConflict
        );
        assert_eq!(answers.lock().expect("answers lock").len(), 1);

        drop(responder);
        component.stop_intake().expect("intake closes");
        cancellation.cancel();
        assert_eq!(component.drain(), Ok(ComponentDrain::Complete));
    }

    fn delivery_request() -> EffectRequest<ProjectRuntimeDelivery> {
        EffectRequest {
            operation_id: OperationId::from_bytes([1; 32]),
            request_digest: CommandDigest::from_bytes([2; 32]),
            issued_at: Timestamp::from_unix_millis(3),
            body: ProjectRuntimeDelivery {
                project_id: ProjectId::from_bytes([4; 32]),
                dispatch_id: hq_domain::DispatchId::from_bytes([10; 32]),
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
        fn running_agent_turns(
            &self,
            _agent_id: AgentId,
            _provider_id: &ProviderId,
            _session_id: &ProviderSessionId,
            _limit: usize,
        ) -> Result<Vec<HarnessActivity>, HarnessError> {
            Ok(Vec::new())
        }

        fn persist_output(
            &self,
            _agent_id: AgentId,
            _provider_id: &ProviderId,
            _session_id: &ProviderSessionId,
            _delivery: Option<&hq_harness::HarnessDeliveryRecord>,
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
            _delivery: Option<&hq_harness::HarnessDeliveryRecord>,
            _activity: &HarnessActivity,
        ) -> Result<(), HarnessError> {
            Ok(())
        }
    }

    struct EventFactory {
        session_id: ProviderSessionId,
        events: Arc<Mutex<VecDeque<Result<HarnessEventPoll, HarnessError>>>>,
        answers: Arc<Mutex<Vec<HarnessInteractiveAnswer>>>,
    }

    impl HarnessFactory for EventFactory {
        fn create_instance(
            &self,
            _request: HarnessInstanceRequest,
        ) -> Result<Box<dyn HarnessInstance>, HarnessError> {
            Ok(Box::new(EventInstance {
                session_id: self.session_id.clone(),
                events: Arc::clone(&self.events),
                answers: Arc::clone(&self.answers),
            }))
        }
    }

    struct EventInstance {
        session_id: ProviderSessionId,
        events: Arc<Mutex<VecDeque<Result<HarnessEventPoll, HarnessError>>>>,
        answers: Arc<Mutex<Vec<HarnessInteractiveAnswer>>>,
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
                    answers: self.answers,
                }),
            })
        }
    }

    struct EventSession {
        events: Arc<Mutex<VecDeque<Result<HarnessEventPoll, HarnessError>>>>,
        answers: Arc<Mutex<Vec<HarnessInteractiveAnswer>>>,
    }

    impl HarnessSession for EventSession {
        fn register_event_notifier(
            &mut self,
            notifier: HarnessEventNotifier,
        ) -> Result<(), HarnessError> {
            notifier.notify()
        }

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

        fn next_event(&mut self) -> Result<HarnessEventPoll, HarnessError> {
            self.events
                .lock()
                .expect("events lock")
                .pop_front()
                .unwrap_or(Ok(HarnessEventPoll::Pending))
        }

        fn answer_interactive(
            &mut self,
            answer: HarnessInteractiveAnswer,
        ) -> Result<(), HarnessError> {
            self.answers.lock().expect("answers lock").push(answer);
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
