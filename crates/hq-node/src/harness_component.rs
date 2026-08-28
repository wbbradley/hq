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
    HarnessClock, HarnessDeliveryRecord, HarnessDeliveryState, HarnessEnvironment, HarnessError,
    HarnessErrorClass, HarnessLaunchRequest, HarnessPersistencePort, HarnessRegistry,
    HarnessSessionRequest, HarnessSubmission, HarnessSupervisor, HarnessSupervisorConfig,
    HarnessSupervisorDependencies, HarnessTokenSource,
};
use hq_projects::{ProjectRuntimeDelivery, ProjectRuntimePort, ProjectRuntimeRequest};
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
        self.with_supervisor(|supervisor| match &request.body.control {
            SessionControl::Start => supervisor
                .launch(HarnessLaunchRequest {
                    agent_id: request.body.agent_id,
                    project_id: None,
                    provider_id: request.body.provider.clone(),
                    session: HarnessSessionRequest::Start,
                    environment: HarnessEnvironment::default(),
                })
                .map(|session| EffectOutcome::Accepted(AgentSessionResult::Ready(session))),
            SessionControl::Resume { session } => supervisor
                .recover(HarnessLaunchRequest {
                    agent_id: request.body.agent_id,
                    project_id: None,
                    provider_id: request.body.provider.clone(),
                    session: HarnessSessionRequest::Resume {
                        session_id: session.clone(),
                    },
                    environment: HarnessEnvironment::default(),
                })
                .map(|ready| EffectOutcome::Accepted(AgentSessionResult::Ready(ready))),
            SessionControl::Stop => supervisor
                .stop(request.body.agent_id)
                .map(|_| EffectOutcome::Accepted(AgentSessionResult::Stopped)),
        })
    }
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
