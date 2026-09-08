//! Bounded runtime recovery policy independent of provider prose or ambient time.
#![allow(clippy::expect_used)]
use hq_application::{ProjectRuntimeFailure, RuntimeFailureReason};
use hq_projects::{RuntimeRecoveryDecision, RuntimeRecoveryPolicy, RuntimeRecoveryStop};

#[test]
fn transient_backoff_stops_at_the_supplied_attempt_budget() {
    let policy = RuntimeRecoveryPolicy::default();
    let failure = ProjectRuntimeFailure {
        reason: RuntimeFailureReason::TransportClosed,
        lease: None,
    };
    assert_eq!(
        policy.after_failure(&failure, 1, 10),
        RuntimeRecoveryDecision::RetryAt(1_010)
    );
    assert_eq!(
        policy.after_failure(&failure, 2, 10),
        RuntimeRecoveryDecision::RetryAt(2_010)
    );
    assert_eq!(
        policy.after_failure(&failure, 4, 10),
        RuntimeRecoveryDecision::RetryAt(8_010)
    );
    assert_eq!(
        policy.after_failure(&failure, 5, 10),
        RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::AttemptsExhausted)
    );
    assert_eq!(
        policy.after_failure(&failure, 5, 99_999),
        RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::AttemptsExhausted)
    );
}

#[test]
fn permanent_failure_and_clock_overflow_do_not_schedule_automatic_retry() {
    let policy = RuntimeRecoveryPolicy::default();
    for reason in [
        RuntimeFailureReason::SessionNotFound,
        RuntimeFailureReason::SessionIdentityMismatch,
        RuntimeFailureReason::UnsafeRecovery,
        RuntimeFailureReason::CompatibilityMismatch,
    ] {
        assert_eq!(
            policy.after_failure(
                &ProjectRuntimeFailure {
                    reason,
                    lease: None
                },
                1,
                0
            ),
            RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::PermanentFailure)
        );
    }
    assert_eq!(
        policy.after_failure(
            &ProjectRuntimeFailure {
                reason: RuntimeFailureReason::Unavailable,
                lease: None
            },
            1,
            u64::MAX
        ),
        RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::ClockRange)
    );
}

#[test]
fn retained_foreign_lease_delays_retry_without_shortening_backoff() {
    let policy = RuntimeRecoveryPolicy::default();
    let owner = hq_application::RuntimeWorkerOwner::from_bytes([1; 32]).expect("owner");
    let mut failure = ProjectRuntimeFailure {
        reason: RuntimeFailureReason::OwnershipConflict,
        lease: Some(hq_application::RuntimeLeaseEvidence {
            owner,
            expires_at_millis: 50_000,
        }),
    };
    assert_eq!(
        policy.after_failure(&failure, 1, 10),
        RuntimeRecoveryDecision::RetryAt(50_000)
    );
    failure.lease.as_mut().expect("lease").expires_at_millis = 1;
    assert_eq!(
        policy.after_failure(&failure, 1, 10),
        RuntimeRecoveryDecision::RetryAt(1_010)
    );
    assert_eq!(
        policy.after_failure(&failure, 0, 10),
        RuntimeRecoveryDecision::Blocked(RuntimeRecoveryStop::InvalidPolicy)
    );
}

#[test]
fn large_attempt_counts_clamp_delay_without_arithmetic_wrap() {
    let policy = RuntimeRecoveryPolicy {
        max_attempts: std::num::NonZeroU32::MAX,
        ..RuntimeRecoveryPolicy::default()
    };
    assert_eq!(
        policy.after_failure(
            &ProjectRuntimeFailure {
                reason: RuntimeFailureReason::Backpressure,
                lease: None
            },
            100,
            1
        ),
        RuntimeRecoveryDecision::RetryAt(30_001)
    );
}
