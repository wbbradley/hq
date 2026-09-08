//! Closed policy for bounded persisted runtime recovery.
use hq_application::{ProjectRuntimeFailure, RuntimeFailureReason, RuntimeRecoveryStop};
use std::num::{NonZeroU32, NonZeroU64};

/// Bounded retry policy; attempt counters belong to durable exact assignment/session state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeRecoveryPolicy {
    /// Deadline for recovering an attempt whose process or response is lost.
    pub attempt_timeout_millis: NonZeroU64,
    /// Total automatic attempts allowed, including the first attempt.
    pub max_attempts: NonZeroU32,
    /// Delay following the first failed attempt.
    pub initial_delay_millis: NonZeroU64,
    /// Maximum exponential delay; exact retained leases may require a later deadline.
    pub max_delay_millis: NonZeroU64,
}
impl Default for RuntimeRecoveryPolicy {
    fn default() -> Self {
        Self {
            attempt_timeout_millis: NonZeroU64::new(30_000).unwrap_or(NonZeroU64::MIN),
            max_attempts: NonZeroU32::new(5).unwrap_or(NonZeroU32::MIN),
            initial_delay_millis: NonZeroU64::new(1_000).unwrap_or(NonZeroU64::MIN),
            max_delay_millis: NonZeroU64::new(30_000).unwrap_or(NonZeroU64::MIN),
        }
    }
}
/// Deterministic scheduling decision to retain in durable recovery state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRecoveryDecision {
    /// Earliest absolute time of a permitted retry.
    RetryAt(u64),
    /// Automatic retries stop until an exact explicit recovery action.
    Blocked(RuntimeRecoveryStop),
}
impl RuntimeRecoveryPolicy {
    /// Computes the next deadline from typed evidence and a persisted failed-attempt count.
    pub fn after_failure(
        &self,
        failure: &ProjectRuntimeFailure,
        attempts: u32,
        now: u64,
    ) -> RuntimeRecoveryDecision {
        if attempts == 0 || self.initial_delay_millis > self.max_delay_millis {
            return RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::InvalidPolicy);
        }
        if !transient(failure.reason) {
            return RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::PermanentFailure);
        }
        if attempts >= self.max_attempts.get() {
            return RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::AttemptsExhausted);
        }
        let multiplier = 1_u64.checked_shl(attempts - 1).unwrap_or(u64::MAX);
        let delay = self
            .initial_delay_millis
            .get()
            .saturating_mul(multiplier)
            .min(self.max_delay_millis.get());
        let Some(deadline) = now.checked_add(delay) else {
            return RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::ClockRange);
        };
        RuntimeRecoveryDecision::RetryAt(
            failure
                .lease
                .map_or(deadline, |lease| deadline.max(lease.expires_at_millis)),
        )
    }
}
const fn transient(reason: RuntimeFailureReason) -> bool {
    match reason {
        RuntimeFailureReason::GenerationChanged
        | RuntimeFailureReason::IntakeClosed
        | RuntimeFailureReason::Crashed
        | RuntimeFailureReason::TransportClosed
        | RuntimeFailureReason::ProcessFailed
        | RuntimeFailureReason::Unavailable
        | RuntimeFailureReason::OwnershipConflict
        | RuntimeFailureReason::Backpressure => true,
        RuntimeFailureReason::InvalidInput
        | RuntimeFailureReason::Unsupported
        | RuntimeFailureReason::ProviderNotRegistered
        | RuntimeFailureReason::RegistrationConflict
        | RuntimeFailureReason::UnsafeRecovery
        | RuntimeFailureReason::SessionIdentityMismatch
        | RuntimeFailureReason::SessionNotFound
        | RuntimeFailureReason::SubmissionIdentityConflict
        | RuntimeFailureReason::InteractiveAlreadyAnswered
        | RuntimeFailureReason::SecretInputRejected
        | RuntimeFailureReason::ProtocolViolation
        | RuntimeFailureReason::CompatibilityMismatch
        | RuntimeFailureReason::CleanupFailed
        | RuntimeFailureReason::PersistenceCollision => false,
    }
}

pub(crate) const fn runtime_failure_code(reason: RuntimeFailureReason) -> &'static str {
    match reason {
        RuntimeFailureReason::GenerationChanged => "project_runtime_resume_generation_changed",
        RuntimeFailureReason::InvalidInput => "project_runtime_resume_invalid_input",
        RuntimeFailureReason::Unsupported => "project_runtime_resume_unsupported",
        RuntimeFailureReason::ProviderNotRegistered => {
            "project_runtime_resume_provider_not_registered"
        }
        RuntimeFailureReason::RegistrationConflict => {
            "project_runtime_resume_registration_conflict"
        }
        RuntimeFailureReason::UnsafeRecovery => "project_runtime_resume_unsafe_recovery",
        RuntimeFailureReason::SessionIdentityMismatch => {
            "project_runtime_resume_session_identity_mismatch"
        }
        RuntimeFailureReason::SessionNotFound => "project_runtime_resume_session_not_found",
        RuntimeFailureReason::SubmissionIdentityConflict => {
            "project_runtime_resume_submission_identity_conflict"
        }
        RuntimeFailureReason::InteractiveAlreadyAnswered => {
            "project_runtime_resume_interactive_already_answered"
        }
        RuntimeFailureReason::SecretInputRejected => "project_runtime_resume_secret_input_rejected",
        RuntimeFailureReason::IntakeClosed => "project_runtime_resume_intake_closed",
        RuntimeFailureReason::Crashed => "project_runtime_resume_crashed",
        RuntimeFailureReason::ProtocolViolation => "project_runtime_resume_protocol_violation",
        RuntimeFailureReason::TransportClosed => "project_runtime_resume_transport_closed",
        RuntimeFailureReason::ProcessFailed => "project_runtime_resume_process_failed",
        RuntimeFailureReason::CompatibilityMismatch => {
            "project_runtime_resume_compatibility_mismatch"
        }
        RuntimeFailureReason::Unavailable => "project_runtime_resume_unavailable",
        RuntimeFailureReason::CleanupFailed => "project_runtime_resume_cleanup_failed",
        RuntimeFailureReason::OwnershipConflict => "project_runtime_resume_ownership_conflict",
        RuntimeFailureReason::Backpressure => "project_runtime_resume_backpressure",
        RuntimeFailureReason::PersistenceCollision => {
            "project_runtime_resume_persistence_collision"
        }
    }
}

/// Validates exact assignment correlation before a readiness result can permit dispatch.
pub(crate) fn validate_readiness(
    outcome: Result<hq_application::ProjectReadinessOutcome, hq_application::ApplicationError>,
    expected: &hq_application::ProjectReadinessRequest,
) -> Result<hq_application::ProjectRuntimeReady, ProjectRuntimeFailure> {
    match outcome {
        Ok(hq_application::ProjectReadinessOutcome::Ready(ready))
            if ready.scope == expected.scope && ready.generation == expected.generation =>
        {
            Ok(ready)
        }
        Ok(hq_application::ProjectReadinessOutcome::Ready(ready))
            if ready.scope == expected.scope =>
        {
            Err(ProjectRuntimeFailure {
                reason: RuntimeFailureReason::GenerationChanged,
                lease: None,
            })
        }
        Ok(hq_application::ProjectReadinessOutcome::Ready(_)) => Err(ProjectRuntimeFailure {
            reason: RuntimeFailureReason::SessionIdentityMismatch,
            lease: None,
        }),
        Ok(hq_application::ProjectReadinessOutcome::Failed(failure)) => Err(failure),
        Err(_) => Err(ProjectRuntimeFailure {
            reason: RuntimeFailureReason::Unavailable,
            lease: None,
        }),
    }
}

/// Exact durable recovery capability, separate from canonical project authority.
pub trait ProjectRecoveryStore {
    /// Loads the active reservation for one exact project using the active-scope index.
    fn recovery_active(
        &self,
        project: hq_domain::ProjectId,
    ) -> Result<Option<hq_application::ProjectRecoveryRecord>, crate::SagaStoreError>;

    /// Loads one exact saga-linked recovery episode.
    fn recovery_find(
        &self,
        operation: hq_domain::OperationId,
    ) -> Result<Option<hq_application::ProjectRecoveryRecord>, crate::SagaStoreError>;
    /// Commits a revision-fenced transition using injected current time.
    fn recovery_compare_exchange(
        &self,
        expected: Option<u64>,
        record: hq_application::ProjectRecoveryRecord,
        now: u64,
    ) -> Result<hq_application::ProjectRecoveryWriteOutcome, crate::SagaStoreError>;
    /// Reads a bounded prefix from the due-deadline index.
    fn recovery_due(
        &self,
        now: u64,
        limit: usize,
    ) -> Result<Vec<hq_application::ProjectRecoveryRecord>, crate::SagaStoreError>;
    /// Reads the earliest indexed deadline without scanning projects.
    fn recovery_deadline(&self) -> Result<Option<u64>, crate::SagaStoreError>;
    /// Resets one exact blocked budget once after canonical eligibility and explicit intent checks.
    fn recovery_retry(
        &self,
        request: hq_application::ProjectRecoveryRetryRequest,
        now: u64,
    ) -> Result<hq_application::ProjectRecoveryWriteOutcome, crate::SagaStoreError>;
}
