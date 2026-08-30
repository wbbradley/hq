//! End-to-end neutral supervisor recovery over deterministic in-memory ports.

#![allow(clippy::expect_used)]

use std::{
    collections::{BTreeMap, VecDeque},
    num::{NonZeroU64, NonZeroUsize},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use hq_domain::{
    ActivityKind, ActivityStatus, AgentId, CommandDigest, ContentText, MessageId, OperationId,
    ProviderId, ProviderSessionId, ShortText,
};
use hq_harness::{
    HarnessActivity, HarnessBufferedEvent, HarnessCancellationOutcome, HarnessCapabilities,
    HarnessCapability, HarnessClock, HarnessDeliveryRecord, HarnessDeliveryState,
    HarnessDrainOutcome, HarnessEnvironment, HarnessError, HarnessErrorClass, HarnessEvent,
    HarnessEventCheckpoint, HarnessEventPoll, HarnessFactory, HarnessInstance,
    HarnessInstanceRequest, HarnessInteractiveAnswer, HarnessLaunchRequest, HarnessLeaseOutcome,
    HarnessOutput, HarnessOutputKind, HarnessOwnerToken, HarnessPersistencePort,
    HarnessReadySession, HarnessRegistry, HarnessSession, HarnessSessionControlOutcome,
    HarnessSessionOperation, HarnessSessionOperationKind, HarnessSessionOperationState,
    HarnessSessionRequest, HarnessStateMutation, HarnessStatePort, HarnessStateSnapshot,
    HarnessSubmission, HarnessSubmissionLookup, HarnessSubmissionOutcome, HarnessSupervisor,
    HarnessSupervisorConfig, HarnessSupervisorDependencies, HarnessTokenSource, HarnessWorkerLease,
    OpenedHarnessSession,
};

#[test]
fn live_polling_persists_source_order_and_releases_closed_workers() {
    let agent = AgentId::from_bytes([41; 32]);
    let provider_id = ProviderId::new("scripted").expect("provider validates");
    let session_id = ProviderSessionId::new("event-session").expect("session validates");
    let provider = Arc::new(ProviderState::default());
    provider.queue([
        Ok(HarnessEventPoll::Event(HarnessEvent::Output(output(
            41,
            "first output",
        )))),
        Ok(HarnessEventPoll::Event(HarnessEvent::Activity(activity(
            1,
            ActivityStatus::Succeeded,
            "completed",
        )))),
        Ok(HarnessEventPoll::Closed),
    ]);
    let state = Arc::new(MemoryState::default());
    let persistence = Arc::new(MemoryPersistence::available());
    let runtime = supervisor(dependencies(
        registry(provider_id.clone(), session_id, provider),
        state.clone(),
        persistence.clone(),
        Arc::new(TestClock::new(10)),
        Arc::new(TestTokens::default()),
    ));
    runtime
        .launch(launch(agent, provider_id, HarnessSessionRequest::Start))
        .expect("worker starts");

    assert_eq!(
        runtime.poll_events().expect("output polls").events_polled,
        1
    );
    assert_eq!(
        runtime.poll_events().expect("activity polls").events_polled,
        1
    );
    let closed = runtime.poll_events().expect("closure polls");
    assert_eq!(closed.workers_closed, 1);
    assert_eq!(closed.live_workers, 0);
    assert!(closed.failures.is_empty());
    assert_eq!(
        persistence
            .persisted
            .lock()
            .expect("persisted locks")
            .as_slice(),
        ["output:first output", "activity:completed"]
    );
    assert!(state.snapshot().leases.is_empty());
}

#[test]
fn saturation_stages_durable_values_and_coalesces_only_exact_snapshots() {
    let agent = AgentId::from_bytes([42; 32]);
    let provider_id = ProviderId::new("scripted").expect("provider validates");
    let provider = Arc::new(ProviderState::default());
    provider.queue([
        Ok(HarnessEventPoll::Event(HarnessEvent::Activity(activity(
            1,
            ActivityStatus::Snapshot,
            "old plan",
        )))),
        Ok(HarnessEventPoll::Event(HarnessEvent::Activity(activity(
            2,
            ActivityStatus::Snapshot,
            "new plan",
        )))),
        Ok(HarnessEventPoll::Event(HarnessEvent::Output(output(
            42,
            "durable one",
        )))),
        Ok(HarnessEventPoll::Event(HarnessEvent::Output(output(
            43,
            "durable two",
        )))),
    ]);
    let persistence = Arc::new(MemoryPersistence::with_failures(20, 20));
    let runtime = supervisor(dependencies(
        registry(
            provider_id.clone(),
            ProviderSessionId::new("event-session").expect("session"),
            provider,
        ),
        Arc::new(MemoryState::default()),
        persistence.clone(),
        Arc::new(TestClock::new(10)),
        Arc::new(TestTokens::default()),
    ));
    runtime
        .launch(launch(agent, provider_id, HarnessSessionRequest::Start))
        .expect("worker starts");

    runtime.poll_events().expect("old snapshot stages");
    let replacement = runtime.poll_events().expect("new snapshot coalesces");
    assert_eq!(replacement.snapshots_replaced, 1);
    runtime
        .poll_events()
        .expect("first durable value fills buffer");
    let staged = runtime.poll_events().expect("second durable value stages");
    assert_eq!(staged.pending_values, 3);

    persistence.allow_all();
    let drained = runtime.poll_events().expect("owned values drain");
    assert_eq!(drained.pending_values, 0);
    assert_eq!(
        persistence
            .persisted
            .lock()
            .expect("persisted locks")
            .as_slice(),
        [
            "activity:new plan",
            "output:durable one",
            "output:durable two"
        ]
    );
    runtime.shutdown().expect("worker shuts down");
}

#[test]
fn restart_replay_recovers_a_polled_value_after_persistence_outage() {
    let agent = AgentId::from_bytes([43; 32]);
    let provider_id = ProviderId::new("scripted").expect("provider validates");
    let session_id = ProviderSessionId::new("restart-session").expect("session validates");
    let provider = Arc::new(ProviderState::default());
    provider.queue([Ok(HarnessEventPoll::Event(HarnessEvent::Output(output(
        44,
        "replayed output",
    ))))]);
    let state = Arc::new(MemoryState::default());
    let persistence = Arc::new(MemoryPersistence::with_failures(20, 0));
    let dependencies = dependencies(
        registry(provider_id.clone(), session_id.clone(), provider.clone()),
        state.clone(),
        persistence.clone(),
        Arc::new(TestClock::new(10)),
        Arc::new(TestTokens::default()),
    );
    let first = supervisor(dependencies.clone());
    first
        .launch(launch(
            agent,
            provider_id.clone(),
            HarnessSessionRequest::Start,
        ))
        .expect("first worker starts");
    assert_eq!(
        first
            .poll_events()
            .expect("event remains owned")
            .pending_values,
        1
    );
    first.stop(agent).expect("failed value remains restartable");

    persistence.allow_all();
    provider.queue([Ok(HarnessEventPoll::Event(HarnessEvent::Output(output(
        44,
        "replayed output",
    ))))]);
    let restarted = supervisor(dependencies);
    restarted
        .recover(launch(
            agent,
            provider_id,
            HarnessSessionRequest::Resume { session_id },
        ))
        .expect("exact session resumes");
    restarted.poll_events().expect("replayed event persists");
    assert_eq!(state.event_progress(agent), Some((true, true)));
    assert_eq!(
        persistence
            .persisted
            .lock()
            .expect("persisted locks")
            .as_slice(),
        ["output:replayed output"]
    );
    restarted.shutdown().expect("restart shuts down");
}

#[test]
fn provider_poll_failure_is_redacted_and_releases_exact_worker_ownership() {
    let agent = AgentId::from_bytes([44; 32]);
    let provider_id = ProviderId::new("scripted").expect("provider validates");
    let provider = Arc::new(ProviderState::default());
    provider.queue([Err(HarnessError::new(HarnessErrorClass::TransportClosed))]);
    let state = Arc::new(MemoryState::default());
    let runtime = supervisor(dependencies(
        registry(
            provider_id.clone(),
            ProviderSessionId::new("failed-session").expect("session"),
            provider,
        ),
        state.clone(),
        Arc::new(MemoryPersistence::available()),
        Arc::new(TestClock::new(10)),
        Arc::new(TestTokens::default()),
    ));
    runtime
        .launch(launch(agent, provider_id, HarnessSessionRequest::Start))
        .expect("worker starts");
    let report = runtime.poll_events().expect("failure is contained");
    assert_eq!(report.workers_failed, 1);
    assert_eq!(report.live_workers, 0);
    assert_eq!(report.failures, [HarnessErrorClass::TransportClosed]);
    assert!(!format!("{report:?}").contains("provider diagnostic"));
    assert!(state.snapshot().leases.is_empty());
}

#[test]
fn restart_reconciles_response_loss_and_partial_event_persistence_before_forced_teardown() {
    let agent = AgentId::from_bytes([1; 32]);
    let provider_id = ProviderId::new("scripted").expect("provider validates");
    let session_id = ProviderSessionId::new("durable-session").expect("session validates");
    let provider = Arc::new(ProviderState::default());
    let state = Arc::new(MemoryState::default());
    let persistence = Arc::new(MemoryPersistence::with_one_activity_failure());
    let clock = Arc::new(TestClock::new(10));
    let tokens = Arc::new(TestTokens::default());
    let registry = registry(
        provider_id.clone(),
        session_id.clone(),
        Arc::clone(&provider),
    );
    let dependencies = dependencies(registry, state.clone(), persistence.clone(), clock, tokens);

    let first = supervisor(dependencies.clone());
    first
        .launch(launch(
            agent,
            provider_id.clone(),
            HarnessSessionRequest::Start,
        ))
        .expect("first worker becomes ready");
    first
        .deliver(delivery(agent, &provider_id, &session_id))
        .expect("response loss is checkpointed");
    assert_eq!(
        state.delivery_state(agent),
        Some(HarnessDeliveryState::Uncertain)
    );
    assert_eq!(provider.submission_calls.load(Ordering::SeqCst), 1);
    first.stop(agent).expect("first daemon worker releases");

    let restarted = supervisor(dependencies.clone());
    restarted
        .recover(launch(
            agent,
            provider_id.clone(),
            HarnessSessionRequest::Resume {
                session_id: session_id.clone(),
            },
        ))
        .expect("restarted worker resumes and wakes durable work");
    assert_eq!(
        state.delivery_state(agent),
        Some(HarnessDeliveryState::Accepted)
    );
    assert_eq!(provider.submission_calls.load(Ordering::SeqCst), 1);
    restarted
        .deliver(delivery(agent, &provider_id, &session_id))
        .expect("exact accepted client replay is a no-op");
    assert_eq!(provider.submission_calls.load(Ordering::SeqCst), 1);

    let sibling = AgentId::from_bytes([2; 32]);
    restarted
        .launch(launch(
            sibling,
            provider_id.clone(),
            HarnessSessionRequest::Start,
        ))
        .expect("an independent agent runs concurrently");
    assert_eq!(state.snapshot().leases.len(), 2);
    restarted
        .stop(sibling)
        .expect("stopping a sibling preserves the first worker");

    let competitor = supervisor(dependencies);
    let error = competitor
        .launch(launch(
            agent,
            provider_id.clone(),
            HarnessSessionRequest::Resume {
                session_id: session_id.clone(),
            },
        ))
        .expect_err("a second live owner cannot acquire the same agent");
    assert_eq!(error.class, HarnessErrorClass::OwnershipConflict);

    let event = output_and_activity(8, "answer", "completed");
    let error = restarted
        .persist_event(agent, event)
        .expect_err("activity failpoint leaves accepted pair pending");
    assert_eq!(error.class, HarnessErrorClass::Unavailable);
    assert_eq!(state.event_progress(agent), Some((true, false)));
    restarted
        .flush(agent)
        .expect("retry finishes the retained pair");
    assert_eq!(state.event_progress(agent), Some((true, true)));
    assert_eq!(
        persistence.calls.lock().expect("calls lock").as_slice(),
        ["output", "activity-failed", "output", "activity"]
    );

    assert_cancellation_intake(&restarted, agent);

    provider.drain_pending.store(true, Ordering::SeqCst);
    let report = restarted
        .shutdown()
        .expect("shutdown reports complete ownership release");
    assert_eq!(report.workers_released, 1);
    assert_eq!(report.workers_forced, 1);
    assert_eq!(provider.force_stops.load(Ordering::SeqCst), 3);
    assert!(report.failures.is_empty());
    assert!(state.snapshot().leases.is_empty());
    assert!(!format!("{:?}", state.snapshot()).contains("super-secret-token"));
}

#[test]
fn stable_output_identity_rejects_changed_content() {
    let agent = AgentId::from_bytes([3; 32]);
    let provider_id = ProviderId::new("scripted").expect("provider validates");
    let session_id = ProviderSessionId::new("durable-session").expect("session validates");
    let provider = Arc::new(ProviderState::default());
    let state = Arc::new(MemoryState::default());
    let persistence = Arc::new(MemoryPersistence::with_one_activity_failure());
    let runtime = supervisor(dependencies(
        registry(provider_id.clone(), session_id, provider),
        state,
        persistence,
        Arc::new(TestClock::new(10)),
        Arc::new(TestTokens::default()),
    ));
    runtime
        .launch(launch(agent, provider_id, HarnessSessionRequest::Start))
        .expect("worker becomes ready");
    runtime
        .persist_event(agent, output_event(9, "stable answer"))
        .expect("first stable output persists");
    let collision = runtime
        .persist_event(agent, output_event(9, "changed answer"))
        .expect_err("changed content under one output identity collides");
    assert_eq!(collision.class, HarnessErrorClass::PersistenceCollision);
    let report = runtime.shutdown().expect("worker ownership releases");
    assert!(
        report
            .failures
            .contains(&HarnessErrorClass::PersistenceCollision)
    );
}

#[test]
fn managed_session_control_replays_exact_readiness_and_preserves_uncertainty() {
    let agent = AgentId::from_bytes([31; 32]);
    let provider_id = ProviderId::new("scripted").expect("provider validates");
    let session_id = ProviderSessionId::new("durable-session").expect("session validates");
    let provider = Arc::new(ProviderState::default());
    let state = Arc::new(MemoryState::default());
    let runtime = supervisor(dependencies(
        registry(provider_id.clone(), session_id.clone(), provider),
        state.clone(),
        Arc::new(MemoryPersistence::with_one_activity_failure()),
        Arc::new(TestClock::new(10)),
        Arc::new(TestTokens::default()),
    ));
    let operation = HarnessSessionOperation {
        operation_id: OperationId::from_bytes([32; 32]),
        request_digest: CommandDigest::from_bytes([33; 32]),
        agent_id: agent,
        provider_id: provider_id.clone(),
        kind: HarnessSessionOperationKind::Start,
        state: HarnessSessionOperationState::Prepared,
    };
    assert_eq!(
        runtime
            .control_session(
                &operation,
                Some(launch(
                    agent,
                    provider_id.clone(),
                    HarnessSessionRequest::Start,
                )),
            )
            .expect("new session becomes ready"),
        HarnessSessionControlOutcome::Ready(session_id.clone())
    );
    assert_eq!(
        runtime
            .control_session(
                &operation,
                Some(launch(
                    agent,
                    provider_id.clone(),
                    HarnessSessionRequest::Start,
                )),
            )
            .expect("exact response-loss replay returns retained readiness"),
        HarnessSessionControlOutcome::Ready(session_id)
    );
    let mut changed = operation;
    changed.request_digest = CommandDigest::from_bytes([99; 32]);
    assert_eq!(
        runtime
            .control_session(
                &changed,
                Some(launch(
                    agent,
                    provider_id.clone(),
                    HarnessSessionRequest::Start,
                )),
            )
            .expect_err("changed operation identity fails closed")
            .class,
        HarnessErrorClass::PersistenceCollision
    );

    let uncertain = HarnessSessionOperation {
        operation_id: OperationId::from_bytes([34; 32]),
        request_digest: CommandDigest::from_bytes([35; 32]),
        agent_id: AgentId::from_bytes([36; 32]),
        provider_id: provider_id.clone(),
        kind: HarnessSessionOperationKind::Resume(
            ProviderSessionId::new("missing-session").expect("session"),
        ),
        state: HarnessSessionOperationState::Prepared,
    };
    state
        .apply(HarnessStateMutation::QueueSessionOperation(
            uncertain.clone(),
        ))
        .expect("operation prepares");
    state
        .apply(HarnessStateMutation::SetSessionOperationState {
            operation_id: uncertain.operation_id,
            state: HarnessSessionOperationState::Uncertain,
        })
        .expect("uncertainty checkpoints");
    assert_eq!(
        runtime
            .control_session(
                &uncertain,
                Some(launch(
                    AgentId::from_bytes([36; 32]),
                    provider_id,
                    HarnessSessionRequest::Resume {
                        session_id: ProviderSessionId::new("missing-session").expect("session"),
                    },
                )),
            )
            .expect("restart observation remains explicit"),
        HarnessSessionControlOutcome::Uncertain
    );
}

fn supervisor(dependencies: HarnessSupervisorDependencies) -> HarnessSupervisor {
    HarnessSupervisor::new(
        HarnessSupervisorConfig {
            max_workers: 4,
            state_query_items: 32,
            lease_duration: Duration::from_secs(1),
            event_capacity: NonZeroUsize::new(2).expect("capacity is nonzero"),
            drain_wait: Duration::from_millis(1),
            event_poll_interval: Duration::from_millis(1),
        },
        dependencies,
    )
    .expect("supervisor config validates")
}

fn assert_cancellation_intake(supervisor: &HarnessSupervisor, agent_id: AgentId) {
    let operation_id = OperationId::from_bytes([6; 32]);
    assert_eq!(
        supervisor
            .cancel(agent_id, operation_id)
            .expect("live operation cancellation routes through the owner"),
        HarnessCancellationOutcome::Cancelled
    );
    supervisor
        .stop_intake()
        .expect("intake closes idempotently");
    assert_eq!(
        supervisor
            .cancel(agent_id, operation_id)
            .expect_err("closed intake rejects new cancellation")
            .class,
        HarnessErrorClass::IntakeClosed
    );
}

fn dependencies(
    registry: Arc<HarnessRegistry>,
    state: Arc<MemoryState>,
    persistence: Arc<MemoryPersistence>,
    clock: Arc<TestClock>,
    tokens: Arc<TestTokens>,
) -> HarnessSupervisorDependencies {
    HarnessSupervisorDependencies {
        registry,
        state,
        persistence,
        clock,
        tokens,
    }
}

fn registry(
    provider_id: ProviderId,
    session_id: ProviderSessionId,
    state: Arc<ProviderState>,
) -> Arc<HarnessRegistry> {
    let mut registry = HarnessRegistry::new();
    registry
        .register(
            provider_id,
            HarnessCapabilities {
                supported: [
                    HarnessCapability::StartSessions,
                    HarnessCapability::ResumeSessions,
                    HarnessCapability::SubmissionLookup,
                    HarnessCapability::OperationCancellation,
                    HarnessCapability::InteractiveRequests,
                ]
                .into_iter()
                .collect(),
            },
            Arc::new(TestFactory { session_id, state }),
        )
        .expect("provider registers");
    Arc::new(registry)
}

fn launch(
    agent_id: AgentId,
    provider_id: ProviderId,
    session: HarnessSessionRequest,
) -> HarnessLaunchRequest {
    HarnessLaunchRequest {
        agent_id,
        project_id: None,
        launch_directory: None,
        provider_id,
        session,
        environment: HarnessEnvironment::copy_from([(
            "HQ_TEST_TOKEN",
            b"super-secret-token".as_slice(),
        )])
        .expect("environment copies"),
    }
}

fn delivery(
    agent_id: AgentId,
    provider_id: &ProviderId,
    session_id: &ProviderSessionId,
) -> HarnessDeliveryRecord {
    HarnessDeliveryRecord {
        agent_id,
        provider_id: provider_id.clone(),
        session_id: session_id.clone(),
        submission: HarnessSubmission {
            submission_id: MessageId::from_bytes([4; 32]),
            digest: CommandDigest::from_bytes([5; 32]),
            operation_id: OperationId::from_bytes([6; 32]),
            body: ContentText::new("durable input").expect("body validates"),
        },
        project: None,
        queued_at_millis: 0,
        state: HarnessDeliveryState::Pending,
    }
}

fn output_and_activity(
    identity: u8,
    output_body: &str,
    activity_body: &str,
) -> HarnessBufferedEvent {
    HarnessBufferedEvent::OutputAndActivity {
        event_id: MessageId::from_bytes([identity; 32]),
        digest: CommandDigest::from_bytes([identity.saturating_add(1); 32]),
        output: HarnessOutput {
            output_id: MessageId::from_bytes([identity.saturating_add(2); 32]),
            operation_id: OperationId::from_bytes([6; 32]),
            kind: HarnessOutputKind::FinalAnswer,
            status: ActivityStatus::Succeeded,
            body: ContentText::new(output_body).expect("output validates"),
        },
        activity: HarnessActivity {
            operation_id: OperationId::from_bytes([6; 32]),
            item: None,
            kind: ActivityKind::Status,
            logical_key: ShortText::new("terminal").expect("key validates"),
            runtime: ShortText::new("scripted").expect("runtime validates"),
            sequence: NonZeroU64::new(1).expect("sequence is positive"),
            status: ActivityStatus::Succeeded,
            content: ContentText::new(activity_body).expect("activity validates"),
            truncated: false,
        },
    }
}

fn output_event(identity: u8, body: &str) -> HarnessBufferedEvent {
    HarnessBufferedEvent::Output {
        event_id: MessageId::from_bytes([identity; 32]),
        digest: CommandDigest::from_bytes([identity.saturating_add(1); 32]),
        output: HarnessOutput {
            output_id: MessageId::from_bytes([identity.saturating_add(2); 32]),
            operation_id: OperationId::from_bytes([6; 32]),
            kind: HarnessOutputKind::FinalAnswer,
            status: ActivityStatus::Succeeded,
            body: ContentText::new(body).expect("output validates"),
        },
    }
}

fn output(identity: u8, body: &str) -> HarnessOutput {
    HarnessOutput {
        output_id: MessageId::from_bytes([identity; 32]),
        operation_id: OperationId::from_bytes([46; 32]),
        kind: HarnessOutputKind::Update,
        status: ActivityStatus::Running,
        body: ContentText::new(body).expect("output validates"),
    }
}

fn activity(sequence: u64, status: ActivityStatus, content: &str) -> HarnessActivity {
    HarnessActivity {
        operation_id: OperationId::from_bytes([46; 32]),
        item: None,
        kind: ActivityKind::Plan,
        logical_key: ShortText::new("plan").expect("key validates"),
        runtime: ShortText::new("scripted").expect("runtime validates"),
        sequence: NonZeroU64::new(sequence).expect("sequence is positive"),
        status,
        content: ContentText::new(content).expect("content validates"),
        truncated: false,
    }
}

#[derive(Default)]
struct MemoryState {
    inner: Mutex<MemoryStateData>,
}

#[derive(Default)]
struct MemoryStateData {
    leases: BTreeMap<AgentId, HarnessWorkerLease>,
    ready: BTreeMap<AgentId, HarnessReadySession>,
    session_operations: BTreeMap<OperationId, HarnessSessionOperation>,
    deliveries: BTreeMap<(AgentId, MessageId), HarnessDeliveryRecord>,
    events: BTreeMap<(AgentId, MessageId), HarnessEventCheckpoint>,
}

impl MemoryState {
    fn snapshot(&self) -> HarnessStateSnapshot {
        let state = self.inner.lock().expect("state locks");
        HarnessStateSnapshot {
            leases: state.leases.values().copied().collect(),
            ready_sessions: state.ready.values().cloned().collect(),
            session_operations: state.session_operations.values().cloned().collect(),
            deliveries: state.deliveries.values().cloned().collect(),
            events: state.events.values().cloned().collect(),
        }
    }

    fn delivery_state(&self, agent_id: AgentId) -> Option<HarnessDeliveryState> {
        self.inner
            .lock()
            .expect("state locks")
            .deliveries
            .values()
            .find(|delivery| delivery.agent_id == agent_id)
            .map(|delivery| delivery.state)
    }

    fn event_progress(&self, agent_id: AgentId) -> Option<(bool, bool)> {
        self.inner
            .lock()
            .expect("state locks")
            .events
            .values()
            .find(|event| event.agent_id == agent_id)
            .map(|event| (event.output_complete, event.activity_complete))
    }
}

impl HarnessStatePort for MemoryState {
    #[allow(clippy::too_many_lines)]
    fn apply(&self, mutation: HarnessStateMutation) -> Result<HarnessLeaseOutcome, HarnessError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
        match mutation {
            HarnessStateMutation::ClaimLease {
                agent_id,
                owner_token,
                now_millis,
                expires_at_millis,
            } => {
                if state.leases.get(&agent_id).is_some_and(|lease| {
                    lease.owner_token != owner_token && lease.expires_at_millis > now_millis
                }) {
                    return Ok(HarnessLeaseOutcome::Held);
                }
                state.leases.insert(
                    agent_id,
                    HarnessWorkerLease {
                        agent_id,
                        owner_token,
                        expires_at_millis,
                    },
                );
                Ok(HarnessLeaseOutcome::Acquired)
            }
            HarnessStateMutation::ReleaseLease {
                agent_id,
                owner_token,
            } => {
                if state
                    .leases
                    .get(&agent_id)
                    .is_some_and(|lease| lease.owner_token == owner_token)
                {
                    state.leases.remove(&agent_id);
                    Ok(HarnessLeaseOutcome::Released)
                } else {
                    Ok(HarnessLeaseOutcome::Held)
                }
            }
            HarnessStateMutation::SetReadySession { owner_token, ready } => {
                exact_owner(&state, ready.agent_id, owner_token)?;
                state.ready.insert(ready.agent_id, ready);
                Ok(HarnessLeaseOutcome::Acquired)
            }
            HarnessStateMutation::QueueSessionOperation(operation) => {
                if state
                    .session_operations
                    .get(&operation.operation_id)
                    .is_some_and(|prior| {
                        prior.request_digest != operation.request_digest
                            || prior.agent_id != operation.agent_id
                            || prior.provider_id != operation.provider_id
                            || prior.kind != operation.kind
                    })
                {
                    return Err(HarnessError::new(HarnessErrorClass::PersistenceCollision));
                }
                state
                    .session_operations
                    .entry(operation.operation_id)
                    .or_insert(operation);
                Ok(HarnessLeaseOutcome::Acquired)
            }
            HarnessStateMutation::SetSessionOperationState {
                operation_id,
                state: next,
            } => {
                let operation = state
                    .session_operations
                    .get_mut(&operation_id)
                    .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
                if matches!(
                    operation.state,
                    HarnessSessionOperationState::Ready(_)
                        | HarnessSessionOperationState::Stopped
                        | HarnessSessionOperationState::Rejected
                ) && operation.state != next
                {
                    return Err(HarnessError::new(HarnessErrorClass::PersistenceCollision));
                }
                operation.state = next;
                Ok(HarnessLeaseOutcome::Acquired)
            }
            HarnessStateMutation::QueueDelivery(delivery) => {
                let key = (delivery.agent_id, delivery.submission.submission_id);
                if state
                    .deliveries
                    .get(&key)
                    .is_some_and(|prior| !same_delivery_identity(prior, &delivery))
                {
                    return Err(HarnessError::new(
                        HarnessErrorClass::SubmissionIdentityConflict,
                    ));
                }
                state.deliveries.entry(key).or_insert(delivery);
                Ok(HarnessLeaseOutcome::Acquired)
            }
            HarnessStateMutation::SetDeliveryState {
                agent_id,
                submission_id,
                owner_token,
                state: next,
            } => {
                exact_owner(&state, agent_id, owner_token)?;
                let delivery = state
                    .deliveries
                    .get_mut(&(agent_id, submission_id))
                    .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
                if matches!(
                    delivery.state,
                    HarnessDeliveryState::Accepted | HarnessDeliveryState::Rejected
                ) && delivery.state != next
                {
                    return Err(HarnessError::new(
                        HarnessErrorClass::SubmissionIdentityConflict,
                    ));
                }
                delivery.state = next;
                Ok(HarnessLeaseOutcome::Acquired)
            }
            HarnessStateMutation::CheckpointEvent {
                owner_token,
                checkpoint,
            } => {
                exact_owner(&state, checkpoint.agent_id, owner_token)?;
                let key = (checkpoint.agent_id, checkpoint.event_id);
                if let Some(prior) = state.events.get(&key)
                    && (prior.digest != checkpoint.digest
                        || (prior.output_complete && !checkpoint.output_complete)
                        || (prior.activity_complete && !checkpoint.activity_complete))
                {
                    return Err(HarnessError::new(HarnessErrorClass::PersistenceCollision));
                }
                state.events.insert(key, checkpoint);
                Ok(HarnessLeaseOutcome::Acquired)
            }
        }
    }

    fn load(&self, limit: usize) -> Result<HarnessStateSnapshot, HarnessError> {
        if limit == 0 {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        Ok(self.snapshot())
    }

    fn session_operation(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<HarnessSessionOperation>, HarnessError> {
        Ok(self
            .inner
            .lock()
            .expect("state lock")
            .session_operations
            .get(&operation_id)
            .cloned())
    }

    fn delivery(
        &self,
        agent_id: AgentId,
        submission_id: MessageId,
    ) -> Result<Option<HarnessDeliveryRecord>, HarnessError> {
        Ok(self
            .inner
            .lock()
            .expect("state lock")
            .deliveries
            .get(&(agent_id, submission_id))
            .cloned())
    }

    fn runnable_deliveries(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> Result<Vec<HarnessDeliveryRecord>, HarnessError> {
        if limit == 0 {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        Ok(self
            .inner
            .lock()
            .expect("state lock")
            .deliveries
            .values()
            .filter(|delivery| {
                delivery.agent_id == agent_id
                    && matches!(
                        delivery.state,
                        HarnessDeliveryState::Pending | HarnessDeliveryState::Uncertain
                    )
            })
            .take(limit)
            .cloned()
            .collect())
    }
}

fn same_delivery_identity(left: &HarnessDeliveryRecord, right: &HarnessDeliveryRecord) -> bool {
    left.agent_id == right.agent_id
        && left.provider_id == right.provider_id
        && left.session_id == right.session_id
        && left.submission == right.submission
}

fn exact_owner(
    state: &MemoryStateData,
    agent_id: AgentId,
    owner_token: HarnessOwnerToken,
) -> Result<(), HarnessError> {
    if state
        .leases
        .get(&agent_id)
        .is_some_and(|lease| lease.owner_token == owner_token)
    {
        Ok(())
    } else {
        Err(HarnessError::new(HarnessErrorClass::OwnershipConflict))
    }
}

struct MemoryPersistence {
    outputs: Mutex<BTreeMap<MessageId, HarnessOutput>>,
    activities: Mutex<Vec<HarnessActivity>>,
    calls: Mutex<Vec<&'static str>>,
    persisted: Mutex<Vec<String>>,
    fail_outputs: AtomicUsize,
    fail_activities: AtomicUsize,
}

impl MemoryPersistence {
    fn with_one_activity_failure() -> Self {
        Self::with_failures(0, 1)
    }

    fn available() -> Self {
        Self::with_failures(0, 0)
    }

    fn with_failures(outputs: usize, activities: usize) -> Self {
        Self {
            outputs: Mutex::new(BTreeMap::new()),
            activities: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            persisted: Mutex::new(Vec::new()),
            fail_outputs: AtomicUsize::new(outputs),
            fail_activities: AtomicUsize::new(activities),
        }
    }

    fn allow_all(&self) {
        self.fail_outputs.store(0, Ordering::SeqCst);
        self.fail_activities.store(0, Ordering::SeqCst);
    }
}

impl HarnessPersistencePort for MemoryPersistence {
    fn persist_output(
        &self,
        _agent_id: AgentId,
        _provider_id: &ProviderId,
        _session_id: &ProviderSessionId,
        output: &HarnessOutput,
    ) -> Result<(), HarnessError> {
        if take_failure(&self.fail_outputs) {
            self.calls.lock().expect("calls lock").push("output-failed");
            return Err(HarnessError::new(HarnessErrorClass::Unavailable));
        }
        self.calls.lock().expect("calls lock").push("output");
        let mut outputs = self.outputs.lock().expect("outputs lock");
        if outputs
            .get(&output.output_id)
            .is_some_and(|prior| prior != output)
        {
            return Err(HarnessError::new(HarnessErrorClass::PersistenceCollision));
        }
        outputs
            .entry(output.output_id)
            .or_insert_with(|| output.clone());
        self.persisted
            .lock()
            .expect("persisted locks")
            .push(format!("output:{}", output.body.as_str()));
        Ok(())
    }

    fn persist_activity(
        &self,
        _agent_id: AgentId,
        _provider_id: &ProviderId,
        _session_id: &ProviderSessionId,
        activity: &HarnessActivity,
    ) -> Result<(), HarnessError> {
        if take_failure(&self.fail_activities) {
            self.calls
                .lock()
                .expect("calls lock")
                .push("activity-failed");
            return Err(HarnessError::new(HarnessErrorClass::Unavailable));
        }
        self.calls.lock().expect("calls lock").push("activity");
        let mut activities = self.activities.lock().expect("activities lock");
        if !activities.contains(activity) {
            activities.push(activity.clone());
            self.persisted
                .lock()
                .expect("persisted locks")
                .push(format!("activity:{}", activity.content.as_str()));
        }
        Ok(())
    }
}

fn take_failure(remaining: &AtomicUsize) -> bool {
    remaining
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            value.checked_sub(1)
        })
        .is_ok()
}

struct TestClock(AtomicU64);

impl TestClock {
    const fn new(now: u64) -> Self {
        Self(AtomicU64::new(now))
    }
}

impl HarnessClock for TestClock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct TestTokens(AtomicUsize);

impl HarnessTokenSource for TestTokens {
    fn next_token(&self) -> Result<HarnessOwnerToken, HarnessError> {
        let identity = self.0.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        let byte = u8::try_from(identity)
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
        HarnessOwnerToken::from_bytes([byte; 32])
    }
}

#[derive(Default)]
struct ProviderState {
    accepted: Mutex<BTreeMap<MessageId, CommandDigest>>,
    lost_once: AtomicBool,
    submission_calls: AtomicUsize,
    drain_pending: AtomicBool,
    force_stops: AtomicUsize,
    events: Mutex<VecDeque<Result<HarnessEventPoll, HarnessError>>>,
}

impl ProviderState {
    fn queue(&self, events: impl IntoIterator<Item = Result<HarnessEventPoll, HarnessError>>) {
        self.events.lock().expect("events lock").extend(events);
    }
}

struct TestFactory {
    session_id: ProviderSessionId,
    state: Arc<ProviderState>,
}

impl HarnessFactory for TestFactory {
    fn create_instance(
        &self,
        _request: HarnessInstanceRequest,
    ) -> Result<Box<dyn HarnessInstance>, HarnessError> {
        Ok(Box::new(TestInstance {
            session_id: self.session_id.clone(),
            state: Arc::clone(&self.state),
        }))
    }
}

struct TestInstance {
    session_id: ProviderSessionId,
    state: Arc<ProviderState>,
}

impl HarnessInstance for TestInstance {
    fn open_session(
        self: Box<Self>,
        request: HarnessSessionRequest,
    ) -> Result<OpenedHarnessSession, HarnessError> {
        if let HarnessSessionRequest::Resume { session_id } = request
            && session_id != self.session_id
        {
            return Err(HarnessError::new(HarnessErrorClass::SessionNotFound));
        }
        Ok(OpenedHarnessSession {
            session_id: self.session_id,
            session: Box::new(TestSession { state: self.state }),
        })
    }
}

struct TestSession {
    state: Arc<ProviderState>,
}

impl HarnessSession for TestSession {
    fn submit(
        &mut self,
        submission: HarnessSubmission,
    ) -> Result<HarnessSubmissionOutcome, HarnessError> {
        self.state.submission_calls.fetch_add(1, Ordering::SeqCst);
        let mut accepted = self.state.accepted.lock().expect("provider locks");
        if accepted
            .get(&submission.submission_id)
            .is_some_and(|digest| digest != &submission.digest)
        {
            return Err(HarnessError::new(
                HarnessErrorClass::SubmissionIdentityConflict,
            ));
        }
        accepted.insert(submission.submission_id, submission.digest);
        if self.state.lost_once.swap(true, Ordering::SeqCst) {
            Ok(HarnessSubmissionOutcome::Accepted)
        } else {
            Ok(HarnessSubmissionOutcome::Uncertain(
                HarnessErrorClass::Unavailable,
            ))
        }
    }

    fn lookup_submission(
        &mut self,
        submission: &HarnessSubmission,
    ) -> Result<HarnessSubmissionLookup, HarnessError> {
        match self
            .state
            .accepted
            .lock()
            .expect("provider locks")
            .get(&submission.submission_id)
        {
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
        _operation_id: OperationId,
    ) -> Result<HarnessCancellationOutcome, HarnessError> {
        Ok(HarnessCancellationOutcome::Cancelled)
    }

    fn poll_event(&mut self, _wait: Duration) -> Result<HarnessEventPoll, HarnessError> {
        self.state
            .events
            .lock()
            .expect("events lock")
            .pop_front()
            .unwrap_or(Ok(HarnessEventPoll::TimedOut))
    }

    fn answer_interactive(
        &mut self,
        _answer: HarnessInteractiveAnswer,
    ) -> Result<(), HarnessError> {
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), HarnessError> {
        Ok(())
    }

    fn drain(&mut self, _wait: Duration) -> Result<HarnessDrainOutcome, HarnessError> {
        if self.state.drain_pending.load(Ordering::SeqCst) {
            Ok(HarnessDrainOutcome::Pending {
                event_count: 1,
                request_count: 0,
            })
        } else {
            Ok(HarnessDrainOutcome::Complete)
        }
    }

    fn force_stop(&mut self) -> Result<(), HarnessError> {
        self.state.force_stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
