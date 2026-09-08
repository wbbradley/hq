//! Passive canonical/runtime join, kept outside the workflow execution lock.
use hq_application::{
    ApplicationError, ApplicationErrorCode, ProjectRecoveryQuery, ProjectRecoveryState,
    ProjectRecoveryView, ProjectRuntimeScope, QueryProjectRecovery,
};

use crate::{
    CanonicalProjectLifecycle, CanonicalProjectPort, ProjectRecoveryStore, ProjectRuntimePort,
    ProjectSagaStore, ProjectWorkflowSnapshot,
};

/// Independent read owner sharing adapters, never the serialized workflow manager.
pub struct ProjectRecoveryReader<S, C, R> {
    store: S,
    canonical: C,
    runtime: R,
}

impl<S, C, R> ProjectRecoveryReader<S, C, R> {
    /// Composes passive reads from independently shareable adapters.
    pub const fn new(store: S, canonical: C, runtime: R) -> Self {
        Self {
            store,
            canonical,
            runtime,
        }
    }
}

impl<S, C, R> QueryProjectRecovery for ProjectRecoveryReader<S, C, R>
where
    S: ProjectRecoveryStore + ProjectSagaStore,
    C: CanonicalProjectPort,
    R: ProjectRuntimePort,
{
    fn query_project_recovery(
        &self,
        request: ProjectRecoveryQuery,
    ) -> Result<ProjectRecoveryView, ApplicationError> {
        let before = self
            .canonical
            .snapshot(request.project_id, request.account_id, None)?;
        authorize(&before, &request)?;
        let scope = scope(&before);
        let recovery = self
            .store
            .recovery_active(request.project_id)
            .map_err(crate::workflow::store_error)?
            .filter(|record| {
                Some(&record.scope) == scope.as_ref()
                    && before.pending_inputs.first().is_some_and(|input| {
                        input.message_id == record.input.submission_id
                            && input.sequence == record.input.sequence
                    })
            });
        let retry_allowed = if let Some(record) = &recovery {
            let parent = self
                .store
                .find(record.input.saga_operation_id)
                .map_err(crate::workflow::store_error)?
                .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::StateCorrupt))?;
            if parent.project_id != request.project_id {
                return Err(ApplicationError::new(ApplicationErrorCode::StateCorrupt));
            }
            parent.account_id == request.account_id
                && parent.home == request.home
                && !parent.state.is_terminal()
                && matches!(record.state, ProjectRecoveryState::Blocked { .. })
        } else {
            false
        };
        let observation = scope
            .as_ref()
            .map(|scope| self.runtime.observe_runtime(scope))
            .transpose()?;
        if observation
            .as_ref()
            .is_some_and(|value| Some(&value.scope) != scope.as_ref())
        {
            return Err(ApplicationError::new(
                ApplicationErrorCode::StateIdentityConflict,
            ));
        }
        let after = self
            .canonical
            .snapshot(request.project_id, request.account_id, None)?;
        authorize(&after, &request)?;
        // A changed head may change pending input or assignment eligibility. Return no stale
        // action; the caller can reread without causing any runtime side effect.
        if before.head != after.head || before != after {
            return Err(ApplicationError::new(
                ApplicationErrorCode::StateIdentityConflict,
            ));
        }
        Ok(ProjectRecoveryView {
            head: after.head,
            observation,
            recovery,
            retry_allowed,
        })
    }
}

fn authorize(
    snapshot: &ProjectWorkflowSnapshot,
    request: &ProjectRecoveryQuery,
) -> Result<(), ApplicationError> {
    if snapshot.project_id != request.project_id
        || snapshot.home != request.home
        || !snapshot.active_human
    {
        return Err(ApplicationError::new(
            ApplicationErrorCode::AuthorityRejected,
        ));
    }
    Ok(())
}

fn scope(snapshot: &ProjectWorkflowSnapshot) -> Option<ProjectRuntimeScope> {
    if snapshot.lifecycle != CanonicalProjectLifecycle::Open
        || snapshot.archived
        || !snapshot.claimable
    {
        return None;
    }
    let assignment = snapshot
        .assignment
        .as_ref()
        .filter(|assignment| assignment.runnable)?;
    Some(ProjectRuntimeScope {
        project_id: snapshot.project_id,
        binding: assignment.binding.clone()?,
        thread_id: assignment.thread_id?,
    })
}
