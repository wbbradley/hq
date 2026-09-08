//! Revision-fenced admission and completion of exact input recovery episodes.
use crate::{ProjectRecoveryStore, RuntimeRecoveryDecision, RuntimeRecoveryPolicy, SagaStoreError};
use hq_application::{
    ProjectRecoveryInput, ProjectRecoveryRecord, ProjectRecoveryState as State,
    ProjectRecoveryWriteOutcome as Write, ProjectRuntimeFailure, ProjectRuntimeReady,
    ProjectRuntimeScope, RuntimeFailureReason, RuntimeRecoveryContext, RuntimeRecoveryStop,
};
use hq_domain::{MessageId, OperationId};
use sha2::{Digest, Sha256};

/// Admission result. Only a newly committed attempt permits crossing the runtime boundary.
#[derive(Debug, Eq, PartialEq)]
pub enum ProjectRecoveryAdmission {
    /// This caller durably acquired the next attempt revision.
    Attempt(ProjectRecoveryRecord),
    /// Existing work is not due, blocked, complete, cancelled, or owned by another attempt.
    Retained(ProjectRecoveryRecord),
}
impl ProjectRecoveryAdmission {
    /// Consumes admission into an attempt only when this caller acquired it.
    pub fn into_attempt(self) -> Option<ProjectRecoveryRecord> {
        match self {
            Self::Attempt(record) => Some(record),
            Self::Retained(_) => None,
        }
    }
}

/// Stable input episode identity; a new client command cannot reset an existing input budget.
pub fn project_recovery_operation(
    scope: &ProjectRuntimeScope,
    submission: MessageId,
) -> OperationId {
    let mut digest = Sha256::new();
    digest.update(b"hq-project-runtime-recovery-v1");
    digest.update(scope.project_id.as_bytes());
    digest.update(scope.binding.assignment_id.as_bytes());
    digest.update(submission.as_bytes());
    OperationId::from_bytes(digest.finalize().into())
}

/// Durable recovery owner. Canonical eligibility and exact acceptance remain workflow responsibilities.
pub struct ProjectRecoveryCoordinator<'a, S> {
    store: &'a S,
    policy: RuntimeRecoveryPolicy,
}
impl<'a, S: ProjectRecoveryStore> ProjectRecoveryCoordinator<'a, S> {
    /// Borrows the exact state capability and bounded scheduling policy.
    pub const fn new(store: &'a S, policy: RuntimeRecoveryPolicy) -> Self {
        Self { store, policy }
    }

    /// Acquires one due attempt after the caller validates the exact current pending input.
    pub fn admit(
        &self,
        scope: &ProjectRuntimeScope,
        input: &ProjectRecoveryInput,
        context: RuntimeRecoveryContext,
    ) -> Result<ProjectRecoveryAdmission, SagaStoreError> {
        let operation = project_recovery_operation(scope, input.submission_id);
        let mut record = if let Some(record) = self.store.recovery_find(operation)? {
            record
        } else {
            let proposed = ProjectRecoveryRecord {
                operation_id: operation,
                input: input.clone(),
                scope: scope.clone(),
                revision: 0,
                attempts: 0,
                state: State::Waiting {
                    retry_at_millis: context.now_millis,
                },
                failure: None,
            };
            self.store
                .recovery_compare_exchange(None, proposed, context.now_millis)?;
            self.current(operation)?
        };
        if record.scope != *scope || record.input != *input {
            return Err(SagaStoreError::Conflict);
        }
        let Some(due) = record.state.deadline() else {
            return Ok(ProjectRecoveryAdmission::Retained(record));
        };
        if due > context.now_millis {
            return Ok(ProjectRecoveryAdmission::Retained(record));
        }
        let stop = if self.policy.initial_delay_millis > self.policy.max_delay_millis {
            Some(RuntimeRecoveryStop::InvalidPolicy)
        } else if record.attempts >= self.policy.max_attempts.get() {
            Some(RuntimeRecoveryStop::AttemptsExhausted)
        } else if context
            .now_millis
            .checked_add(self.policy.attempt_timeout_millis.get())
            .is_none()
        {
            Some(RuntimeRecoveryStop::ClockRange)
        } else {
            None
        };
        let expected = record.revision;
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(SagaStoreError::Conflict)?;
        if let Some(reason) = stop {
            record.state = State::Blocked { reason };
            record.failure.get_or_insert(ProjectRuntimeFailure {
                reason: RuntimeFailureReason::Unavailable,
                lease: None,
            });
            self.store
                .recovery_compare_exchange(Some(expected), record, context.now_millis)?;
            return self
                .current(operation)
                .map(ProjectRecoveryAdmission::Retained);
        }
        record.attempts = record
            .attempts
            .checked_add(1)
            .ok_or(SagaStoreError::Conflict)?;
        record.state = State::Attempting {
            generation: context.generation,
            recover_at_millis: context
                .now_millis
                .checked_add(self.policy.attempt_timeout_millis.get())
                .ok_or(SagaStoreError::Conflict)?,
        };
        match self.store.recovery_compare_exchange(
            Some(expected),
            record.clone(),
            context.now_millis,
        )? {
            Write::Applied => Ok(ProjectRecoveryAdmission::Attempt(record)),
            Write::AlreadyApplied | Write::Conflict => self
                .current(operation)
                .map(ProjectRecoveryAdmission::Retained),
        }
    }

    /// Retains exact readiness while preserving a deadline until canonical delivery completes.
    /// Returns a record only when this caller committed the readiness transition.
    pub fn ready(
        &self,
        mut record: ProjectRecoveryRecord,
        ready: &ProjectRuntimeReady,
    ) -> Result<Option<ProjectRecoveryRecord>, SagaStoreError> {
        let State::Attempting {
            generation,
            recover_at_millis,
        } = record.state
        else {
            return Err(SagaStoreError::Conflict);
        };
        if ready.scope != record.scope || ready.generation != generation {
            return Err(SagaStoreError::Conflict);
        }
        record.state = State::Ready {
            generation,
            owner: ready.owner,
            recover_at_millis,
        };
        record.failure = None;
        self.replace(record, 0)
    }

    /// Persists typed failure and the next bounded deadline, retaining the input's attempt count.
    pub fn failed(
        &self,
        mut record: ProjectRecoveryRecord,
        failure: ProjectRuntimeFailure,
        now: u64,
    ) -> Result<Option<ProjectRecoveryRecord>, SagaStoreError> {
        if !matches!(record.state, State::Attempting { .. } | State::Ready { .. }) {
            return Err(SagaStoreError::Conflict);
        }
        record.state = match self.policy.after_failure(&failure, record.attempts, now) {
            RuntimeRecoveryDecision::RetryAt(retry_at_millis) => State::Waiting { retry_at_millis },
            RuntimeRecoveryDecision::Blocked(reason) => State::Blocked { reason },
        };
        record.failure = Some(failure);
        self.replace(record, now)
    }

    /// Stops this episode only after the workflow proves and commits the exact canonical dispatch.
    pub fn completed(
        &self,
        mut record: ProjectRecoveryRecord,
    ) -> Result<Option<ProjectRecoveryRecord>, SagaStoreError> {
        record.state = State::Completed;
        record.failure = None;
        self.replace(record, 0)
    }

    /// Releases an exact revision after canonical state proves it is no longer eligible.
    pub fn cancel(
        &self,
        mut record: ProjectRecoveryRecord,
    ) -> Result<Option<ProjectRecoveryRecord>, SagaStoreError> {
        record.state = State::Cancelled;
        self.replace(record, 0)
    }

    fn current(&self, operation: OperationId) -> Result<ProjectRecoveryRecord, SagaStoreError> {
        self.store
            .recovery_find(operation)?
            .ok_or(SagaStoreError::Conflict)
    }
    fn replace(
        &self,
        mut record: ProjectRecoveryRecord,
        now: u64,
    ) -> Result<Option<ProjectRecoveryRecord>, SagaStoreError> {
        let expected = record.revision;
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(SagaStoreError::Conflict)?;
        match self
            .store
            .recovery_compare_exchange(Some(expected), record.clone(), now)?
        {
            Write::Applied => Ok(Some(record)),
            Write::AlreadyApplied | Write::Conflict => Ok(None),
        }
    }
}
