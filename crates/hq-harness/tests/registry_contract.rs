//! Provider registration and exact session-readiness contracts.

#![allow(clippy::expect_used)]

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use hq_domain::{AgentId, ProviderId, ProviderSessionId, ShortText};
use hq_harness::{
    HarnessCapabilities, HarnessCapability, HarnessDrainOutcome, HarnessError, HarnessErrorClass,
    HarnessEventPoll, HarnessFactory, HarnessInstance, HarnessInstanceRequest, HarnessRegistry,
    HarnessSession, HarnessSessionRequest, HarnessSubmission, HarnessSubmissionLookup,
    HarnessSubmissionOutcome, OpenedHarnessSession,
};

#[test]
fn registration_rejects_unsafe_recovery_and_duplicate_provider_identity() {
    let mut registry = HarnessRegistry::new();
    let provider = provider("scripted");
    let unsafe_capabilities = HarnessCapabilities {
        supported: BTreeSet::from([
            HarnessCapability::StartSessions,
            HarnessCapability::ResumeSessions,
            HarnessCapability::OperationCancellation,
            HarnessCapability::InteractiveRequests,
        ]),
    };
    let error = registry
        .register(
            provider.clone(),
            unsafe_capabilities.clone(),
            Arc::new(StubFactory::new(session("new-session"))),
        )
        .expect_err("unsafe recovery declaration rejects");
    assert_eq!(error.class, HarnessErrorClass::UnsafeRecovery);

    let mut capabilities = unsafe_capabilities;
    capabilities
        .supported
        .insert(HarnessCapability::StableSubmissionIdempotency);
    registry
        .register(
            provider.clone(),
            capabilities.clone(),
            Arc::new(StubFactory::new(session("new-session"))),
        )
        .expect("safe provider registers");
    let error = registry
        .register(
            provider,
            capabilities,
            Arc::new(StubFactory::new(session("other-session"))),
        )
        .expect_err("duplicate provider rejects");
    assert_eq!(error.class, HarnessErrorClass::RegistrationConflict);
}

#[test]
fn named_provider_catalog_is_stable_and_keeps_user_facing_names() {
    let mut registry = HarnessRegistry::new();
    for (provider_id, name) in [("zeta", "Zeta service"), ("alpha", "Alpha service")] {
        registry
            .register_named(
                provider(provider_id),
                ShortText::new(name).expect("provider name"),
                safe_capabilities(),
                Arc::new(StubFactory::new(session("session"))),
            )
            .expect("named provider registers");
    }
    let catalog = registry.provider_catalog();
    assert_eq!(catalog[0].provider, provider("alpha"));
    assert_eq!(catalog[0].name.as_str(), "Alpha service");
    assert_eq!(catalog[1].provider, provider("zeta"));
    assert_eq!(catalog[1].name.as_str(), "Zeta service");
}

#[test]
fn resume_requires_exact_ready_identity_and_force_stops_a_mismatch() {
    let requested = session("durable-session");
    let mismatched = session("replacement-session");
    let forced = Arc::new(Mutex::new(0_usize));
    let factory = StubFactory {
        ready: mismatched,
        forced: Arc::clone(&forced),
    };
    let mut registry = HarnessRegistry::new();
    registry
        .register(provider("scripted"), safe_capabilities(), Arc::new(factory))
        .expect("provider registers");

    let error = registry
        .open_session(
            &provider("scripted"),
            HarnessInstanceRequest {
                agent_id: AgentId::from_bytes([7; 32]),
                project_id: None,
                launch_directory: None,
                environment: hq_harness::HarnessEnvironment::default(),
            },
            HarnessSessionRequest::Resume {
                session_id: requested,
            },
        )
        .expect_err("mismatched resume rejects");
    assert_eq!(error.class, HarnessErrorClass::SessionIdentityMismatch);
    assert_eq!(*forced.lock().expect("forced count locks"), 1);
}

#[test]
fn registration_exposes_passive_capabilities_and_gates_unsupported_session_modes() {
    let provider_id = provider("resume-only");
    let capabilities = HarnessCapabilities {
        supported: BTreeSet::from([
            HarnessCapability::ResumeSessions,
            HarnessCapability::SubmissionLookup,
        ]),
    };
    let mut registry = HarnessRegistry::new();
    registry
        .register(
            provider_id.clone(),
            capabilities.clone(),
            Arc::new(StubFactory::new(session("should-not-open"))),
        )
        .expect("safe resume-only provider registers");

    assert_eq!(registry.capabilities(&provider_id), Some(&capabilities));
    let error = registry
        .open_session(
            &provider_id,
            HarnessInstanceRequest {
                agent_id: AgentId::from_bytes([8; 32]),
                project_id: None,
                launch_directory: None,
                environment: hq_harness::HarnessEnvironment::default(),
            },
            HarnessSessionRequest::Start,
        )
        .expect_err("unadvertised start rejects before readiness");
    assert_eq!(error.class, HarnessErrorClass::Unsupported);
}

fn safe_capabilities() -> HarnessCapabilities {
    HarnessCapabilities {
        supported: BTreeSet::from([
            HarnessCapability::StartSessions,
            HarnessCapability::ResumeSessions,
            HarnessCapability::SubmissionLookup,
            HarnessCapability::OperationCancellation,
            HarnessCapability::InteractiveRequests,
        ]),
    }
}

fn provider(value: &str) -> ProviderId {
    ProviderId::new(value).expect("provider validates")
}

fn session(value: &str) -> ProviderSessionId {
    ProviderSessionId::new(value).expect("session validates")
}

struct StubFactory {
    ready: ProviderSessionId,
    forced: Arc<Mutex<usize>>,
}

impl StubFactory {
    fn new(ready: ProviderSessionId) -> Self {
        Self {
            ready,
            forced: Arc::new(Mutex::new(0)),
        }
    }
}

impl HarnessFactory for StubFactory {
    fn create_instance(
        &self,
        _request: HarnessInstanceRequest,
    ) -> Result<Box<dyn HarnessInstance>, HarnessError> {
        Ok(Box::new(StubInstance {
            ready: self.ready.clone(),
            forced: Arc::clone(&self.forced),
        }))
    }
}

struct StubInstance {
    ready: ProviderSessionId,
    forced: Arc<Mutex<usize>>,
}

impl HarnessInstance for StubInstance {
    fn open_session(
        self: Box<Self>,
        _request: HarnessSessionRequest,
    ) -> Result<OpenedHarnessSession, HarnessError> {
        Ok(OpenedHarnessSession {
            session_id: self.ready,
            session: Box::new(StubSession {
                forced: self.forced,
            }),
        })
    }
}

struct StubSession {
    forced: Arc<Mutex<usize>>,
}

impl HarnessSession for StubSession {
    fn register_event_notifier(
        &mut self,
        notifier: hq_harness::HarnessEventNotifier,
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
        _operation_id: hq_domain::OperationId,
    ) -> Result<hq_harness::HarnessCancellationOutcome, HarnessError> {
        Ok(hq_harness::HarnessCancellationOutcome::AlreadyFinished)
    }

    fn next_event(&mut self) -> Result<HarnessEventPoll, HarnessError> {
        Ok(HarnessEventPoll::Pending)
    }

    fn answer_interactive(
        &mut self,
        _answer: hq_harness::HarnessInteractiveAnswer,
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
        *self.forced.lock().expect("forced count locks") += 1;
        Ok(())
    }
}
