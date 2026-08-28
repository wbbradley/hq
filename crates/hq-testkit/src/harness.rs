//! Reusable provider-neutral harness conformance and deterministic scripted adapter.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::Duration,
};

use hq_domain::{
    ActivityKind, ActivityStatus, AgentId, BoundedVec, CommandDigest, ContentText, MessageId,
    OperationId, ProviderId, ProviderSessionId, ShortText,
};
use hq_harness::{
    HarnessActivity, HarnessCancellationOutcome, HarnessCapabilities, HarnessCapability,
    HarnessDrainOutcome, HarnessError, HarnessErrorClass, HarnessEvent, HarnessEventPoll,
    HarnessFactory, HarnessInstance, HarnessInstanceRequest, HarnessInteractiveAnswer,
    HarnessInteractiveRequest, HarnessInteractiveResponse, HarnessOutput, HarnessOutputKind,
    HarnessRegistry, HarnessRequestChoice, HarnessRequestId, HarnessRequestKind, HarnessSession,
    HarnessSessionRequest, HarnessSubmission, HarnessSubmissionLookup, HarnessSubmissionOutcome,
    OpenedHarnessSession,
};

const CONFORMANCE_EVENT_WAIT: Duration = Duration::from_secs(1);

/// Capability-named scenarios every managed provider adapter must exercise.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HarnessConformanceScenario {
    /// Registration rejects a provider with no safe uncertain-submission recovery path.
    UnsafeRegistration,
    /// A new durable session becomes ready only after an acknowledged nonempty identity.
    NewSession,
    /// An exact existing durable session resumes unchanged.
    ResumedSession,
    /// A missing resumed session fails without silently creating a replacement.
    MissingResume,
    /// A mismatched resume acknowledgement is force-stopped and rejected.
    MismatchedResume,
    /// Acceptance followed by response loss reconciles without a duplicate submission.
    ResponseLossAccepted,
    /// Definite lookup absence permits one exact retry with the stable identity and digest.
    ResponseLossMissingRetry,
    /// An active-operation change during uncertainty still reconciles before exact retry.
    ActiveOperationRace,
    /// Changed input under one stable identity fails closed.
    ChangedInputCollision,
    /// Structured requests answer once and cancellation releases an outstanding request.
    InteractiveRequest,
    /// Secret-bearing provider requests fail closed without exposing their content.
    SecretRequestRejection,
    /// Normalized output and activity retain provider source order and exact typed content.
    OutputActivityOrder,
    /// One crashed logical instance cannot corrupt a sibling instance.
    CrashIsolation,
    /// Intake closure, bounded drain, and idempotent forced stop leave no accepted work.
    Teardown,
}

impl HarnessConformanceScenario {
    /// Complete deterministic scenario order for conformance reports.
    pub const ALL: [Self; 14] = [
        Self::UnsafeRegistration,
        Self::NewSession,
        Self::ResumedSession,
        Self::MissingResume,
        Self::MismatchedResume,
        Self::ResponseLossAccepted,
        Self::ResponseLossMissingRetry,
        Self::ActiveOperationRace,
        Self::ChangedInputCollision,
        Self::InteractiveRequest,
        Self::SecretRequestRejection,
        Self::OutputActivityOrder,
        Self::CrashIsolation,
        Self::Teardown,
    ];
}

/// Stable neutral observations emitted by a conformance fixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessConformanceObservation {
    /// One independent logical instance was created.
    InstanceCreated,
    /// One new session request reached the adapter.
    SessionStarted,
    /// One exact resume request reached the adapter.
    SessionResumed(ProviderSessionId),
    /// One provider submission call received a stable identity and digest.
    SubmissionAttempt {
        /// Stable HQ submission identity.
        submission_id: MessageId,
        /// Exact input digest.
        digest: CommandDigest,
    },
    /// One authoritative lookup received a stable identity.
    SubmissionLookup {
        /// Stable HQ submission identity.
        submission_id: MessageId,
        /// Exact input digest being reconciled.
        digest: CommandDigest,
    },
    /// One structured request received its sole answer.
    InteractiveAnswered(HarnessRequestId),
    /// One exact operation cancellation was requested.
    OperationCancelled(OperationId),
    /// New adapter intake was stopped.
    IntakeStopped,
    /// The adapter reported one drain observation.
    DrainObserved(HarnessDrainOutcome),
    /// Remaining runtime ownership was force-stopped once.
    ForceStopped,
    /// One logical instance observed its own crash.
    Crashed,
}

/// Read-only trace capability supplied by a conformance fixture.
pub trait HarnessConformanceTrace: Send + Sync {
    /// Returns the complete neutral observation trace in source order.
    fn observations(&self)
    -> Result<Vec<HarnessConformanceObservation>, HarnessConformanceFailure>;
}

/// One provider registration and its independent test trace.
pub struct HarnessConformanceFixture {
    /// Provider namespace used for registry composition.
    pub provider_id: ProviderId,
    /// Advertised neutral capabilities.
    pub capabilities: HarnessCapabilities,
    /// Factory under test.
    pub factory: Arc<dyn HarnessFactory>,
    /// Read-only neutral trace for direct assertions.
    pub trace: Arc<dyn HarnessConformanceTrace>,
    /// Adapter-specific exact events expected by the output/activity scenario.
    pub expected_output_activity: Vec<HarnessEvent>,
}

/// Adapter-specific fixture source consumed by the reusable neutral runner.
pub trait HarnessConformanceSubject {
    /// Creates a fresh independent fixture configured for one named scenario.
    fn fixture(
        &self,
        scenario: HarnessConformanceScenario,
    ) -> Result<HarnessConformanceFixture, HarnessConformanceFailure>;
}

/// One failed neutral conformance assertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HarnessConformanceFailure {
    /// Scenario that failed.
    pub scenario: HarnessConformanceScenario,
    /// Stable assertion name; provider diagnostics are never parsed.
    pub check: &'static str,
}

impl fmt::Display for HarnessConformanceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "harness conformance failed: {:?}: {}",
            self.scenario, self.check
        )
    }
}

impl Error for HarnessConformanceFailure {}

/// Completed reusable neutral conformance scenarios.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessConformanceReport {
    /// Scenarios completed in the normative deterministic order.
    pub scenarios: Vec<HarnessConformanceScenario>,
}

/// Runs every provider-neutral scenario against fresh subject fixtures.
pub fn run_harness_conformance(
    subject: &impl HarnessConformanceSubject,
) -> Result<HarnessConformanceReport, HarnessConformanceFailure> {
    let mut completed = Vec::with_capacity(HarnessConformanceScenario::ALL.len());
    for scenario in HarnessConformanceScenario::ALL {
        run_scenario(subject, scenario)?;
        completed.push(scenario);
    }
    Ok(HarnessConformanceReport {
        scenarios: completed,
    })
}

fn run_scenario(
    subject: &impl HarnessConformanceSubject,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let fixture = subject.fixture(scenario)?;
    match scenario {
        HarnessConformanceScenario::UnsafeRegistration => unsafe_registration(&fixture, scenario),
        HarnessConformanceScenario::NewSession => new_session(&fixture, scenario),
        HarnessConformanceScenario::ResumedSession => resumed_session(&fixture, scenario),
        HarnessConformanceScenario::MissingResume => missing_resume(&fixture, scenario),
        HarnessConformanceScenario::MismatchedResume => mismatched_resume(&fixture, scenario),
        HarnessConformanceScenario::ResponseLossAccepted => {
            response_loss_accepted(&fixture, scenario)
        }
        HarnessConformanceScenario::ResponseLossMissingRetry => {
            response_loss_missing_retry(&fixture, scenario)
        }
        HarnessConformanceScenario::ActiveOperationRace => {
            active_operation_race(&fixture, scenario)
        }
        HarnessConformanceScenario::ChangedInputCollision => {
            changed_input_collision(&fixture, scenario)
        }
        HarnessConformanceScenario::InteractiveRequest => interactive_request(&fixture, scenario),
        HarnessConformanceScenario::SecretRequestRejection => {
            secret_request_rejection(&fixture, scenario)
        }
        HarnessConformanceScenario::OutputActivityOrder => {
            output_activity_order(&fixture, scenario)
        }
        HarnessConformanceScenario::CrashIsolation => crash_isolation(&fixture, scenario),
        HarnessConformanceScenario::Teardown => teardown(&fixture, scenario),
    }
}

fn unsafe_registration(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut registry = HarnessRegistry::new();
    let error = registry
        .register(
            fixture.provider_id.clone(),
            fixture.capabilities.clone(),
            Arc::clone(&fixture.factory),
        )
        .err()
        .ok_or_else(|| failure(scenario, "unsafe provider registration was accepted"))?;
    ensure(
        scenario,
        error.class == HarnessErrorClass::UnsafeRecovery,
        "unsafe registration returned the wrong class",
    )
}

fn new_session(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    ensure(
        scenario,
        opened.session_id == known_session(scenario)?,
        "new session acknowledgement was not exact",
    )?;
    ensure_trace_count(
        fixture,
        scenario,
        |entry| matches!(entry, HarnessConformanceObservation::SessionStarted),
        1,
        "new session did not start exactly once",
    )
}

fn resumed_session(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let requested = known_session(scenario)?;
    let opened = open(
        fixture,
        HarnessSessionRequest::Resume {
            session_id: requested.clone(),
        },
        scenario,
    )?;
    ensure(
        scenario,
        opened.session_id == requested,
        "resume acknowledgement changed the durable session identity",
    )
}

fn missing_resume(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let error = open(
        fixture,
        HarnessSessionRequest::Resume {
            session_id: known_session(scenario)?,
        },
        scenario,
    )
    .err()
    .ok_or_else(|| failure(scenario, "missing resume silently became ready"))?;
    ensure(
        scenario,
        error.check == "session open returned SessionNotFound",
        "missing resume returned the wrong failure",
    )
}

fn mismatched_resume(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let error = open(
        fixture,
        HarnessSessionRequest::Resume {
            session_id: known_session(scenario)?,
        },
        scenario,
    )
    .err()
    .ok_or_else(|| failure(scenario, "mismatched resume silently became ready"))?;
    ensure(
        scenario,
        error.check == "session open returned SessionIdentityMismatch",
        "mismatched resume returned the wrong failure",
    )?;
    ensure_trace_count(
        fixture,
        scenario,
        |entry| matches!(entry, HarnessConformanceObservation::ForceStopped),
        1,
        "mismatched ready session was not force-stopped once",
    )
}

fn response_loss_accepted(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    let submission = submission(scenario)?;
    let outcome = opened
        .session
        .submit(submission.clone())
        .map_err(|_| failure(scenario, "response-loss submission call failed"))?;
    ensure(
        scenario,
        matches!(outcome, HarnessSubmissionOutcome::Uncertain(_)),
        "response loss was not classified uncertain",
    )?;
    let lookup = opened
        .session
        .lookup_submission(&submission)
        .map_err(|_| failure(scenario, "authoritative lookup failed"))?;
    ensure(
        scenario,
        lookup == HarnessSubmissionLookup::Accepted,
        "accepted response loss was not reconciled",
    )?;
    ensure_submission_trace(fixture, scenario, 1, 1)
}

fn response_loss_missing_retry(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    let submission = submission(scenario)?;
    let first = opened
        .session
        .submit(submission.clone())
        .map_err(|_| failure(scenario, "uncertain first submission failed"))?;
    ensure(
        scenario,
        matches!(first, HarnessSubmissionOutcome::Uncertain(_)),
        "first response loss was not uncertain",
    )?;
    let lookup = opened
        .session
        .lookup_submission(&submission)
        .map_err(|_| failure(scenario, "missing lookup failed"))?;
    ensure(
        scenario,
        lookup == HarnessSubmissionLookup::Missing,
        "lookup did not prove stable identity absent",
    )?;
    let second = opened
        .session
        .submit(submission)
        .map_err(|_| failure(scenario, "exact retry failed"))?;
    ensure(
        scenario,
        second == HarnessSubmissionOutcome::Accepted,
        "exact retry was not accepted",
    )?;
    ensure_submission_trace(fixture, scenario, 2, 1)
}

fn active_operation_race(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    let submission = submission(scenario)?;
    let first = opened
        .session
        .submit(submission.clone())
        .map_err(|_| failure(scenario, "racing first submission failed"))?;
    ensure(
        scenario,
        matches!(first, HarnessSubmissionOutcome::Uncertain(_)),
        "active-operation race was not uncertain",
    )?;
    let event = opened
        .session
        .poll_event(CONFORMANCE_EVENT_WAIT)
        .map_err(|_| failure(scenario, "operation-race event failed"))?;
    ensure(
        scenario,
        matches!(
            event,
            HarnessEventPoll::Event(HarnessEvent::Activity(HarnessActivity {
                kind: ActivityKind::Status,
                ..
            }))
        ),
        "active-operation change was not observed before recovery",
    )?;
    let lookup = opened
        .session
        .lookup_submission(&submission)
        .map_err(|_| failure(scenario, "racing lookup failed"))?;
    ensure(
        scenario,
        lookup == HarnessSubmissionLookup::Missing,
        "racing lookup did not prove absence",
    )?;
    let retry = opened
        .session
        .submit(submission)
        .map_err(|_| failure(scenario, "racing exact retry failed"))?;
    ensure(
        scenario,
        retry == HarnessSubmissionOutcome::Accepted,
        "racing exact retry was not accepted",
    )?;
    ensure_submission_trace(fixture, scenario, 2, 1)
}

fn changed_input_collision(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    let first = submission(scenario)?;
    ensure(
        scenario,
        opened
            .session
            .submit(first.clone())
            .map_err(|_| failure(scenario, "initial stable submission failed"))?
            == HarnessSubmissionOutcome::Accepted,
        "initial stable submission was not accepted",
    )?;
    let mut changed = first;
    changed.digest = CommandDigest::from_bytes([99; 32]);
    let error = opened
        .session
        .submit(changed)
        .err()
        .ok_or_else(|| failure(scenario, "changed stable input was accepted"))?;
    ensure(
        scenario,
        error.class == HarnessErrorClass::SubmissionIdentityConflict,
        "changed stable input returned the wrong class",
    )
}

fn interactive_request(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    let first = expect_request(&mut *opened.session, scenario)?;
    let answer = HarnessInteractiveAnswer {
        request_id: first.request_id,
        response: HarnessInteractiveResponse::Choice(short("approve", scenario)?),
    };
    opened
        .session
        .answer_interactive(answer.clone())
        .map_err(|_| failure(scenario, "valid interactive answer failed"))?;
    let duplicate = opened
        .session
        .answer_interactive(answer)
        .err()
        .ok_or_else(|| failure(scenario, "duplicate interactive answer was accepted"))?;
    ensure(
        scenario,
        duplicate.class == HarnessErrorClass::InteractiveAlreadyAnswered,
        "duplicate answer returned the wrong class",
    )?;
    let second = expect_request(&mut *opened.session, scenario)?;
    ensure(
        scenario,
        opened
            .session
            .cancel_operation(second.operation_id)
            .map_err(|_| failure(scenario, "interactive cancellation failed"))?
            == HarnessCancellationOutcome::Cancelled,
        "interactive cancellation was not explicit",
    )?;
    let cancelled_answer = HarnessInteractiveAnswer {
        request_id: second.request_id,
        response: HarnessInteractiveResponse::Cancelled,
    };
    let error = opened
        .session
        .answer_interactive(cancelled_answer)
        .err()
        .ok_or_else(|| failure(scenario, "cancelled request accepted a late answer"))?;
    ensure(
        scenario,
        error.class == HarnessErrorClass::InvalidInput,
        "late cancelled answer returned the wrong class",
    )
}

fn secret_request_rejection(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    let error = opened
        .session
        .poll_event(CONFORMANCE_EVENT_WAIT)
        .err()
        .ok_or_else(|| {
            failure(
                scenario,
                "secret request crossed the neutral event boundary",
            )
        })?;
    ensure(
        scenario,
        error.class == HarnessErrorClass::SecretInputRejected,
        "secret request returned the wrong class",
    )?;
    let observations = fixture.trace.observations()?;
    ensure(
        scenario,
        observations
            .iter()
            .all(|entry| !matches!(entry, HarnessConformanceObservation::InteractiveAnswered(_))),
        "secret request produced a persisted interactive observation",
    )
}

fn output_activity_order(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    let expected = fixture.expected_output_activity.clone();
    for expected_event in expected {
        let actual = opened
            .session
            .poll_event(CONFORMANCE_EVENT_WAIT)
            .map_err(|_| failure(scenario, "normalized event poll failed"))?;
        ensure(
            scenario,
            actual == HarnessEventPoll::Event(expected_event),
            "normalized output/activity source order changed",
        )?;
    }
    ensure(
        scenario,
        opened
            .session
            .poll_event(Duration::ZERO)
            .map_err(|_| failure(scenario, "empty event poll failed"))?
            == HarnessEventPoll::TimedOut,
        "event stream did not become empty after exact events",
    )
}

fn crash_isolation(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let registry = registered(fixture, scenario)?;
    let mut first = registry
        .open_session(
            &fixture.provider_id,
            instance_request(),
            HarnessSessionRequest::Start,
        )
        .map_err(|_| failure(scenario, "first sibling did not open"))?;
    let mut second = registry
        .open_session(
            &fixture.provider_id,
            instance_request(),
            HarnessSessionRequest::Start,
        )
        .map_err(|_| failure(scenario, "second sibling did not open"))?;
    let first_error = first
        .session
        .poll_event(CONFORMANCE_EVENT_WAIT)
        .err()
        .ok_or_else(|| failure(scenario, "scripted sibling did not crash"))?;
    ensure(
        scenario,
        matches!(
            first_error.class,
            HarnessErrorClass::Crashed
                | HarnessErrorClass::ProtocolViolation
                | HarnessErrorClass::TransportClosed
                | HarnessErrorClass::ProcessFailed
                | HarnessErrorClass::CompatibilityMismatch
        ),
        "crashed sibling returned the wrong class",
    )?;
    ensure(
        scenario,
        second
            .session
            .submit(submission(scenario)?)
            .map_err(|_| failure(scenario, "healthy sibling was affected by crash"))?
            == HarnessSubmissionOutcome::Accepted,
        "healthy sibling did not remain independent",
    )
}

fn teardown(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<(), HarnessConformanceFailure> {
    let mut opened = open(fixture, HarnessSessionRequest::Start, scenario)?;
    opened
        .session
        .stop_intake()
        .map_err(|_| failure(scenario, "stop intake failed"))?;
    let closed = opened
        .session
        .submit(submission(scenario)?)
        .err()
        .ok_or_else(|| failure(scenario, "submission entered after stop intake"))?;
    ensure(
        scenario,
        closed.class == HarnessErrorClass::IntakeClosed,
        "closed intake returned the wrong class",
    )?;
    let pending = opened
        .session
        .drain(CONFORMANCE_EVENT_WAIT)
        .map_err(|_| failure(scenario, "pending drain failed"))?;
    ensure(
        scenario,
        pending
            == HarnessDrainOutcome::Pending {
                event_count: 1,
                request_count: 0,
            },
        "drain did not report exact accepted pending work",
    )?;
    ensure(
        scenario,
        matches!(
            opened
                .session
                .poll_event(CONFORMANCE_EVENT_WAIT)
                .map_err(|_| failure(scenario, "teardown event drain failed"))?,
            HarnessEventPoll::Event(HarnessEvent::Output(_))
        ),
        "teardown did not drain accepted output",
    )?;
    ensure(
        scenario,
        opened
            .session
            .drain(Duration::ZERO)
            .map_err(|_| failure(scenario, "complete drain failed"))?
            == HarnessDrainOutcome::Complete,
        "teardown did not reach complete drain",
    )?;
    opened
        .session
        .force_stop()
        .and_then(|()| opened.session.force_stop())
        .map_err(|_| failure(scenario, "idempotent forced stop failed"))?;
    ensure_trace_count(
        fixture,
        scenario,
        |entry| matches!(entry, HarnessConformanceObservation::ForceStopped),
        1,
        "forced stop was not idempotent",
    )
}

fn open(
    fixture: &HarnessConformanceFixture,
    request: HarnessSessionRequest,
    scenario: HarnessConformanceScenario,
) -> Result<OpenedHarnessSession, HarnessConformanceFailure> {
    let registry = registered(fixture, scenario)?;
    registry
        .open_session(&fixture.provider_id, instance_request(), request)
        .map_err(|error| {
            let check = match error.class {
                HarnessErrorClass::SessionNotFound => "session open returned SessionNotFound",
                HarnessErrorClass::SessionIdentityMismatch => {
                    "session open returned SessionIdentityMismatch"
                }
                _ => "session open returned an unexpected failure",
            };
            failure(scenario, check)
        })
}

fn registered(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
) -> Result<HarnessRegistry, HarnessConformanceFailure> {
    let mut registry = HarnessRegistry::new();
    registry
        .register(
            fixture.provider_id.clone(),
            fixture.capabilities.clone(),
            Arc::clone(&fixture.factory),
        )
        .map_err(|_| failure(scenario, "safe provider registration failed"))?;
    Ok(registry)
}

fn instance_request() -> HarnessInstanceRequest {
    HarnessInstanceRequest {
        agent_id: AgentId::from_bytes([1; 32]),
        project_id: None,
        launch_directory: None,
        environment: hq_harness::HarnessEnvironment::default(),
    }
}

fn submission(
    scenario: HarnessConformanceScenario,
) -> Result<HarnessSubmission, HarnessConformanceFailure> {
    Ok(HarnessSubmission {
        submission_id: MessageId::from_bytes([10; 32]),
        digest: CommandDigest::from_bytes([11; 32]),
        operation_id: OperationId::from_bytes([12; 32]),
        body: content("deterministic provider input", scenario)?,
    })
}

fn known_session(
    scenario: HarnessConformanceScenario,
) -> Result<ProviderSessionId, HarnessConformanceFailure> {
    ProviderSessionId::new("scripted-session")
        .map_err(|_| failure(scenario, "fixed session fixture was invalid"))
}

fn expect_request(
    session: &mut dyn HarnessSession,
    scenario: HarnessConformanceScenario,
) -> Result<HarnessInteractiveRequest, HarnessConformanceFailure> {
    match session
        .poll_event(CONFORMANCE_EVENT_WAIT)
        .map_err(|_| failure(scenario, "interactive request poll failed"))?
    {
        HarnessEventPoll::Event(HarnessEvent::InteractiveRequest(request)) => Ok(request),
        _ => Err(failure(
            scenario,
            "interactive request was not emitted in source order",
        )),
    }
}

fn ensure_submission_trace(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
    attempts: usize,
    lookups: usize,
) -> Result<(), HarnessConformanceFailure> {
    let observations = fixture.trace.observations()?;
    let submission = submission(scenario)?;
    let actual_attempts = observations
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                HarnessConformanceObservation::SubmissionAttempt {
                    submission_id,
                    digest,
                } if *submission_id == submission.submission_id && *digest == submission.digest
            )
        })
        .count();
    let actual_lookups = observations
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                HarnessConformanceObservation::SubmissionLookup {
                    submission_id,
                    digest,
                } if *submission_id == submission.submission_id && *digest == submission.digest
            )
        })
        .count();
    ensure(
        scenario,
        actual_attempts == attempts && actual_lookups == lookups,
        "submission/lookup call trace violated reconciliation",
    )
}

fn ensure_trace_count(
    fixture: &HarnessConformanceFixture,
    scenario: HarnessConformanceScenario,
    predicate: impl Fn(&HarnessConformanceObservation) -> bool,
    expected: usize,
    check: &'static str,
) -> Result<(), HarnessConformanceFailure> {
    let count = fixture
        .trace
        .observations()?
        .iter()
        .filter(|entry| predicate(entry))
        .count();
    ensure(scenario, count == expected, check)
}

fn ensure(
    scenario: HarnessConformanceScenario,
    condition: bool,
    check: &'static str,
) -> Result<(), HarnessConformanceFailure> {
    if condition {
        Ok(())
    } else {
        Err(failure(scenario, check))
    }
}

const fn failure(
    scenario: HarnessConformanceScenario,
    check: &'static str,
) -> HarnessConformanceFailure {
    HarnessConformanceFailure { scenario, check }
}

/// Deterministic in-memory subject used to prove the reusable neutral suite itself.
pub struct ScriptedHarnessSubject;

impl HarnessConformanceSubject for ScriptedHarnessSubject {
    fn fixture(
        &self,
        scenario: HarnessConformanceScenario,
    ) -> Result<HarnessConformanceFixture, HarnessConformanceFailure> {
        let state = Arc::new(Mutex::new(ScriptedState::default()));
        let capabilities = if scenario == HarnessConformanceScenario::UnsafeRegistration {
            HarnessCapabilities {
                supported: BTreeSet::from([
                    HarnessCapability::StartSessions,
                    HarnessCapability::ResumeSessions,
                    HarnessCapability::OperationCancellation,
                    HarnessCapability::InteractiveRequests,
                ]),
            }
        } else {
            HarnessCapabilities {
                supported: BTreeSet::from([
                    HarnessCapability::StartSessions,
                    HarnessCapability::ResumeSessions,
                    HarnessCapability::SubmissionLookup,
                    HarnessCapability::OperationCancellation,
                    HarnessCapability::InteractiveRequests,
                ]),
            }
        };
        Ok(HarnessConformanceFixture {
            provider_id: ProviderId::new("scripted")
                .map_err(|_| failure(scenario, "fixed provider fixture was invalid"))?,
            capabilities,
            factory: Arc::new(ScriptedFactory {
                scenario,
                state: Arc::clone(&state),
            }),
            trace: Arc::new(ScriptedTrace { scenario, state }),
            expected_output_activity: scripted_events(scenario)?,
        })
    }
}

#[derive(Default)]
struct ScriptedState {
    next_instance: usize,
    observations: Vec<HarnessConformanceObservation>,
}

struct ScriptedTrace {
    scenario: HarnessConformanceScenario,
    state: Arc<Mutex<ScriptedState>>,
}

impl HarnessConformanceTrace for ScriptedTrace {
    fn observations(
        &self,
    ) -> Result<Vec<HarnessConformanceObservation>, HarnessConformanceFailure> {
        self.state
            .lock()
            .map(|state| state.observations.clone())
            .map_err(|_| failure(self.scenario, "scripted trace lock was poisoned"))
    }
}

struct ScriptedFactory {
    scenario: HarnessConformanceScenario,
    state: Arc<Mutex<ScriptedState>>,
}

impl HarnessFactory for ScriptedFactory {
    fn create_instance(
        &self,
        _request: HarnessInstanceRequest,
    ) -> Result<Box<dyn HarnessInstance>, HarnessError> {
        let ordinal = with_state(&self.state, |state| {
            let ordinal = state.next_instance;
            state.next_instance = state.next_instance.saturating_add(1);
            state
                .observations
                .push(HarnessConformanceObservation::InstanceCreated);
            ordinal
        })?;
        Ok(Box::new(ScriptedInstance {
            scenario: self.scenario,
            ordinal,
            state: Arc::clone(&self.state),
        }))
    }
}

struct ScriptedInstance {
    scenario: HarnessConformanceScenario,
    ordinal: usize,
    state: Arc<Mutex<ScriptedState>>,
}

impl HarnessInstance for ScriptedInstance {
    fn open_session(
        self: Box<Self>,
        request: HarnessSessionRequest,
    ) -> Result<OpenedHarnessSession, HarnessError> {
        match &request {
            HarnessSessionRequest::Start => {
                record(&self.state, HarnessConformanceObservation::SessionStarted)?;
            }
            HarnessSessionRequest::Resume { session_id } => {
                record(
                    &self.state,
                    HarnessConformanceObservation::SessionResumed(session_id.clone()),
                )?;
            }
        }
        if self.scenario == HarnessConformanceScenario::MissingResume
            && matches!(request, HarnessSessionRequest::Resume { .. })
        {
            return Err(HarnessError::new(HarnessErrorClass::SessionNotFound));
        }
        let session_id = if self.scenario == HarnessConformanceScenario::MismatchedResume {
            ProviderSessionId::new("replacement-session")
                .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?
        } else {
            match request {
                HarnessSessionRequest::Start => ProviderSessionId::new("scripted-session")
                    .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?,
                HarnessSessionRequest::Resume { session_id } => session_id,
            }
        };
        let events = scripted_events(self.scenario)
            .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?
            .into();
        Ok(OpenedHarnessSession {
            session_id,
            session: Box::new(ScriptedSession {
                scenario: self.scenario,
                ordinal: self.ordinal,
                state: self.state,
                accepted: BTreeMap::new(),
                events,
                pending_requests: BTreeSet::new(),
                answered_requests: BTreeSet::new(),
                submission_calls: 0,
                flags: BTreeSet::new(),
            }),
        })
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum ScriptedSessionFlag {
    IntakeStopped,
    SecretRejected,
    CrashObserved,
    ForceStopped,
}

struct ScriptedSession {
    scenario: HarnessConformanceScenario,
    ordinal: usize,
    state: Arc<Mutex<ScriptedState>>,
    accepted: BTreeMap<MessageId, CommandDigest>,
    events: VecDeque<HarnessEvent>,
    pending_requests: BTreeSet<HarnessRequestId>,
    answered_requests: BTreeSet<HarnessRequestId>,
    submission_calls: usize,
    flags: BTreeSet<ScriptedSessionFlag>,
}

impl HarnessSession for ScriptedSession {
    fn submit(
        &mut self,
        submission: HarnessSubmission,
    ) -> Result<HarnessSubmissionOutcome, HarnessError> {
        if self.flags.contains(&ScriptedSessionFlag::IntakeStopped) {
            return Err(HarnessError::new(HarnessErrorClass::IntakeClosed));
        }
        record(
            &self.state,
            HarnessConformanceObservation::SubmissionAttempt {
                submission_id: submission.submission_id,
                digest: submission.digest,
            },
        )?;
        self.submission_calls = self.submission_calls.saturating_add(1);
        if let Some(prior) = self.accepted.get(&submission.submission_id) {
            if prior != &submission.digest {
                return Err(HarnessError::new(
                    HarnessErrorClass::SubmissionIdentityConflict,
                ));
            }
            return Ok(HarnessSubmissionOutcome::Accepted);
        }
        match self.scenario {
            HarnessConformanceScenario::ResponseLossAccepted if self.submission_calls == 1 => {
                self.accepted
                    .insert(submission.submission_id, submission.digest);
                Ok(HarnessSubmissionOutcome::Uncertain(
                    HarnessErrorClass::Unavailable,
                ))
            }
            HarnessConformanceScenario::ResponseLossMissingRetry
            | HarnessConformanceScenario::ActiveOperationRace
                if self.submission_calls == 1 =>
            {
                Ok(HarnessSubmissionOutcome::Uncertain(
                    HarnessErrorClass::Unavailable,
                ))
            }
            _ => {
                self.accepted
                    .insert(submission.submission_id, submission.digest);
                Ok(HarnessSubmissionOutcome::Accepted)
            }
        }
    }

    fn lookup_submission(
        &mut self,
        submission: &HarnessSubmission,
    ) -> Result<HarnessSubmissionLookup, HarnessError> {
        record(
            &self.state,
            HarnessConformanceObservation::SubmissionLookup {
                submission_id: submission.submission_id,
                digest: submission.digest,
            },
        )?;
        match self.accepted.get(&submission.submission_id) {
            Some(accepted) if accepted == &submission.digest => {
                Ok(HarnessSubmissionLookup::Accepted)
            }
            Some(_) => Err(HarnessError::new(
                HarnessErrorClass::SubmissionIdentityConflict,
            )),
            None => Ok(HarnessSubmissionLookup::Missing),
        }
    }

    fn cancel_operation(
        &mut self,
        operation_id: OperationId,
    ) -> Result<HarnessCancellationOutcome, HarnessError> {
        record(
            &self.state,
            HarnessConformanceObservation::OperationCancelled(operation_id),
        )?;
        self.pending_requests.clear();
        Ok(HarnessCancellationOutcome::Cancelled)
    }

    fn poll_event(&mut self, _wait: Duration) -> Result<HarnessEventPoll, HarnessError> {
        if self.scenario == HarnessConformanceScenario::SecretRequestRejection
            && self.flags.insert(ScriptedSessionFlag::SecretRejected)
        {
            return Err(HarnessError::new(HarnessErrorClass::SecretInputRejected));
        }
        if self.scenario == HarnessConformanceScenario::CrashIsolation
            && self.ordinal == 0
            && self.flags.insert(ScriptedSessionFlag::CrashObserved)
        {
            record(&self.state, HarnessConformanceObservation::Crashed)?;
            return Err(HarnessError::new(HarnessErrorClass::Crashed));
        }
        let Some(event) = self.events.pop_front() else {
            return Ok(HarnessEventPoll::TimedOut);
        };
        if let HarnessEvent::InteractiveRequest(request) = &event {
            self.pending_requests.insert(request.request_id);
        }
        Ok(HarnessEventPoll::Event(event))
    }

    fn answer_interactive(&mut self, answer: HarnessInteractiveAnswer) -> Result<(), HarnessError> {
        if self.flags.contains(&ScriptedSessionFlag::IntakeStopped) {
            return Err(HarnessError::new(HarnessErrorClass::IntakeClosed));
        }
        if self.answered_requests.contains(&answer.request_id) {
            return Err(HarnessError::new(
                HarnessErrorClass::InteractiveAlreadyAnswered,
            ));
        }
        if !self.pending_requests.remove(&answer.request_id) {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        self.answered_requests.insert(answer.request_id);
        record(
            &self.state,
            HarnessConformanceObservation::InteractiveAnswered(answer.request_id),
        )
    }

    fn stop_intake(&mut self) -> Result<(), HarnessError> {
        if self.flags.insert(ScriptedSessionFlag::IntakeStopped) {
            self.pending_requests.clear();
            self.events
                .retain(|event| !matches!(event, HarnessEvent::InteractiveRequest(_)));
            record(&self.state, HarnessConformanceObservation::IntakeStopped)?;
        }
        Ok(())
    }

    fn drain(&mut self, _wait: Duration) -> Result<HarnessDrainOutcome, HarnessError> {
        let outcome = if self.events.is_empty() && self.pending_requests.is_empty() {
            HarnessDrainOutcome::Complete
        } else {
            HarnessDrainOutcome::Pending {
                event_count: self.events.len(),
                request_count: self.pending_requests.len(),
            }
        };
        record(
            &self.state,
            HarnessConformanceObservation::DrainObserved(outcome),
        )?;
        Ok(outcome)
    }

    fn force_stop(&mut self) -> Result<(), HarnessError> {
        if self.flags.insert(ScriptedSessionFlag::ForceStopped) {
            self.flags.insert(ScriptedSessionFlag::IntakeStopped);
            self.events.clear();
            self.pending_requests.clear();
            record(&self.state, HarnessConformanceObservation::ForceStopped)?;
        }
        Ok(())
    }
}

fn scripted_events(
    scenario: HarnessConformanceScenario,
) -> Result<Vec<HarnessEvent>, HarnessConformanceFailure> {
    match scenario {
        HarnessConformanceScenario::ActiveOperationRace => Ok(vec![HarnessEvent::Activity(
            activity("operation changed", ActivityKind::Status, 1, scenario)?,
        )]),
        HarnessConformanceScenario::InteractiveRequest => Ok(vec![
            HarnessEvent::InteractiveRequest(interactive_request_fixture(1, scenario)?),
            HarnessEvent::InteractiveRequest(interactive_request_fixture(2, scenario)?),
        ]),
        HarnessConformanceScenario::OutputActivityOrder => Ok(vec![
            HarnessEvent::Output(output(21, HarnessOutputKind::Update, "working", scenario)?),
            HarnessEvent::Activity(activity(
                "command complete",
                ActivityKind::CompletedItem,
                2,
                scenario,
            )?),
            HarnessEvent::Output(output(
                22,
                HarnessOutputKind::FinalAnswer,
                "finished",
                scenario,
            )?),
        ]),
        HarnessConformanceScenario::Teardown => Ok(vec![
            HarnessEvent::Output(output(23, HarnessOutputKind::Update, "accepted", scenario)?),
            HarnessEvent::InteractiveRequest(interactive_request_fixture(3, scenario)?),
        ]),
        _ => Ok(Vec::new()),
    }
}

fn output(
    identity: u8,
    kind: HarnessOutputKind,
    body: &str,
    scenario: HarnessConformanceScenario,
) -> Result<HarnessOutput, HarnessConformanceFailure> {
    Ok(HarnessOutput {
        output_id: MessageId::from_bytes([identity; 32]),
        operation_id: OperationId::from_bytes([12; 32]),
        kind,
        status: if kind == HarnessOutputKind::FinalAnswer {
            ActivityStatus::Succeeded
        } else {
            ActivityStatus::Running
        },
        body: content(body, scenario)?,
    })
}

fn activity(
    body: &str,
    kind: ActivityKind,
    sequence: u64,
    scenario: HarnessConformanceScenario,
) -> Result<HarnessActivity, HarnessConformanceFailure> {
    Ok(HarnessActivity {
        operation_id: OperationId::from_bytes([12; 32]),
        item: Some(short("item", scenario)?),
        kind,
        logical_key: short("logical-key", scenario)?,
        runtime: short("scripted-runtime", scenario)?,
        sequence: NonZeroU64::new(sequence)
            .ok_or_else(|| failure(scenario, "fixed activity sequence was zero"))?,
        status: ActivityStatus::Running,
        content: content(body, scenario)?,
        truncated: false,
    })
}

fn interactive_request_fixture(
    identity: u8,
    scenario: HarnessConformanceScenario,
) -> Result<HarnessInteractiveRequest, HarnessConformanceFailure> {
    let choice = HarnessRequestChoice {
        value: short("approve", scenario)?,
        label: short("Approve", scenario)?,
    };
    Ok(HarnessInteractiveRequest {
        request_id: HarnessRequestId::from_bytes([identity; 32]),
        operation_id: OperationId::from_bytes([12; 32]),
        kind: HarnessRequestKind::Approval,
        prompt: content("Allow this bounded action?", scenario)?,
        choices: BoundedVec::new([choice])
            .map_err(|_| failure(scenario, "fixed request choices were invalid"))?,
    })
}

fn content(
    value: &str,
    scenario: HarnessConformanceScenario,
) -> Result<ContentText, HarnessConformanceFailure> {
    ContentText::new(value).map_err(|_| failure(scenario, "fixed content fixture was invalid"))
}

fn short(
    value: &str,
    scenario: HarnessConformanceScenario,
) -> Result<ShortText, HarnessConformanceFailure> {
    ShortText::new(value).map_err(|_| failure(scenario, "fixed short fixture was invalid"))
}

fn record(
    state: &Arc<Mutex<ScriptedState>>,
    observation: HarnessConformanceObservation,
) -> Result<(), HarnessError> {
    with_state(state, |state| state.observations.push(observation))
}

fn with_state<T>(
    state: &Arc<Mutex<ScriptedState>>,
    operation: impl FnOnce(&mut ScriptedState) -> T,
) -> Result<T, HarnessError> {
    state
        .lock()
        .map(|mut state| operation(&mut state))
        .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))
}
