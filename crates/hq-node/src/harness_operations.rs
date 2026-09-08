//! Authorized operation controls outside provider/workflow execution locks.

use std::sync::{Arc, atomic::Ordering};

use hq_application::{
    AgentCancellationRequest, AgentCancellationState as State, AgentOperationQuery,
    AgentOperationScope, AgentOperationTarget, AgentOperationView, ApplicationError,
    ApplicationErrorCode, RuntimeFailureReason as Reason, RuntimeGenerationId, RuntimeWorkerOwner,
};
use hq_domain::ActivityStatus;
use hq_harness::{
    HarnessCancellationOutcome, HarnessOwnerToken, HarnessReadyWorker, HarnessSupervisor,
};

use super::{HarnessNodeComponent, map_harness_error, project_runtime_failure_reason};

impl HarnessNodeComponent {
    fn operation_runtime(
        &self,
    ) -> Result<(RuntimeGenerationId, Arc<HarnessSupervisor>), ApplicationError> {
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let generation = self
            .inner
            .runtime_generation
            .lock()
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        let supervisor = self
            .inner
            .supervisor
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        Ok((generation, supervisor))
    }

    pub(super) fn operation_view(
        &self,
        query: &AgentOperationQuery,
    ) -> Result<AgentOperationView, ApplicationError> {
        let before = self.inner.canonical.operation_evidence(query)?;
        let mut view = AgentOperationView {
            target: None,
            tracked: before.tracked.clone(),
        };
        let (Some(scope), Some(running)) = (&before.scope, &before.running) else {
            return Ok(view);
        };
        let (generation, runtime) = self.operation_runtime()?;
        if let Some(worker) = runtime
            .cancellable_worker(scope.agent_id)
            .map_err(map_harness_error)?
            && worker_matches(&worker, scope)
        {
            view.target = Some(AgentOperationTarget {
                query: query.clone(),
                scope: scope.clone(),
                generation,
                owner: RuntimeWorkerOwner::from_bytes(*worker.owner_token.as_bytes())
                    .ok_or_else(unavailable)?,
                operation_id: running.operation_id,
                sequence: running.sequence,
            });
        }
        let after = self.inner.canonical.operation_evidence(query)?;
        if before != after || self.operation_runtime()?.0 != generation {
            return Err(ApplicationError::new(
                ApplicationErrorCode::StateIdentityConflict,
            ));
        }
        Ok(view)
    }

    pub(super) fn enqueue_cancellation(
        &self,
        request: AgentCancellationRequest,
    ) -> Result<State, ApplicationError> {
        self.operation_runtime()?;
        let current = self
            .inner
            .canonical
            .operation_evidence(&request.target.query)?;
        if current.scope.as_ref() != Some(&request.target.scope) {
            return Err(ApplicationError::new(
                ApplicationErrorCode::AuthorityRejected,
            ));
        }
        let worker = self.clone();
        let input = request.clone();
        self.inner
            .cancellation_jobs
            .submit(request, move || worker.execute_cancellation(&input))
    }

    fn execute_cancellation(&self, request: &AgentCancellationRequest) -> State {
        self.cancel_validated(request)
            .unwrap_or(State::Rejected(Reason::Unavailable))
    }

    fn cancel_validated(
        &self,
        request: &AgentCancellationRequest,
    ) -> Result<State, ApplicationError> {
        let target = &request.target;
        let mut query = target.query.clone();
        query.tracked_operation = Some(target.operation_id);
        let evidence = self.inner.canonical.operation_evidence(&query)?;
        if evidence.scope.as_ref() != Some(&target.scope) {
            return Ok(State::Rejected(Reason::OwnershipConflict));
        }
        if evidence.tracked.as_ref().is_some_and(|status| {
            matches!(
                status.status,
                ActivityStatus::Succeeded | ActivityStatus::Failed(_) | ActivityStatus::Interrupted
            )
        }) {
            return Ok(State::AlreadyFinished);
        }
        if !evidence.running.as_ref().is_some_and(|running| {
            running.operation_id == target.operation_id && running.sequence >= target.sequence
        }) {
            return Ok(State::Rejected(Reason::SessionIdentityMismatch));
        }
        let (generation, runtime) = self.operation_runtime()?;
        if generation != target.generation {
            return Ok(State::Rejected(Reason::GenerationChanged));
        }
        let expected = HarnessReadyWorker {
            agent_id: target.scope.agent_id,
            project_id: target
                .scope
                .project
                .as_ref()
                .map(|project| project.project_id),
            provider_id: target.scope.provider.clone(),
            session_id: target.scope.session.clone(),
            owner_token: HarnessOwnerToken::from_bytes(*target.owner.as_bytes())
                .map_err(map_harness_error)?,
        };
        Ok(match runtime.cancel_owned(&expected, target.operation_id) {
            Ok(HarnessCancellationOutcome::Requested) => State::Requested,
            Ok(HarnessCancellationOutcome::AlreadyFinished) => State::AlreadyFinished,
            Ok(HarnessCancellationOutcome::Rejected(reason)) => {
                State::Rejected(project_runtime_failure_reason(reason))
            }
            Ok(HarnessCancellationOutcome::Uncertain(reason)) => {
                State::Uncertain(project_runtime_failure_reason(reason))
            }
            Err(error) => State::Rejected(project_runtime_failure_reason(error.class)),
        })
    }
}

fn worker_matches(worker: &HarnessReadyWorker, scope: &AgentOperationScope) -> bool {
    worker.agent_id == scope.agent_id
        && worker.provider_id == scope.provider
        && worker.session_id == scope.session
        && worker.project_id == scope.project.as_ref().map(|project| project.project_id)
}

fn unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::AdapterUnavailable)
}
