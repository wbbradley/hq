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

    fn shutdown_supervisor(&self) -> Result<ComponentDrain, ComponentError> {
        let supervisor = self
            .inner
            .supervisor
            .lock()
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
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
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
        self.inner.accepting.store(true, Ordering::Release);
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.inner.accepting.store(false, Ordering::Release);
        if let Some(supervisor) = self
            .inner
            .supervisor
            .lock()
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
        self.with_supervisor(|supervisor| {
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
            .map(EffectOutcome::Accepted)
        })
    }

    fn deliver(
        &self,
        request: &EffectRequest<ProjectRuntimeDelivery>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.with_supervisor(|supervisor| {
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
        })
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

    use std::num::NonZeroU64;

    use hq_domain::{
        AgentId, AssignmentBinding, AssignmentId, CommandDigest, ContentText, MessageId,
        OperationId, ProjectId, ProviderId, ProviderSessionId, ThreadId, Timestamp,
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
}
