//! Deterministic owned relay-session contracts.

#![allow(clippy::expect_used)]

use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    num::NonZeroU64,
    os::fd::{AsFd, BorrowedFd},
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use hq_application::{RelayAccess, RelayAuthentication};
use hq_domain::{FactId, InstallationId, Revision};
use hq_relay::{
    AttemptDisposition, CanonicalIngest, CatchupCursor, DurableEnvelope, FailureClass,
    LogicalEnvelopeId, OpenedRelayEnvelope, OutboundCursor, OutboundIntent, OutboxKey,
    PreparedEnvelopeMetadata, PreparedOutbound, PreparedRelayAuthentication, RejectedRelayEnvelope,
    RelayAttempt, RelayAttemptFailure, RelayClock, RelayConnection, RelayConnector,
    RelayEnvelopePort, RelayFrame, RelayJitter, RelayManager, RelayManagerConfig, RelayOpenOutcome,
    RelayPagePosition, RelayPolicy, RelayPortError, RelayReceive, RelaySession, RelaySessionConfig,
    RelaySessionDependencies, RelayStateMutation, RelayStatePage, RelayStatePort, RelayStateQuery,
    RelayStateSnapshot, RelayUrl, ResolvedRoute, RouteResolver, StagedInput,
};
use sha2::{Digest, Sha256};

#[test]
fn uncertain_publish_restarts_with_byte_identical_wrapper_and_acceptance_is_absorbing() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    let intent = outbound(4);
    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .push(intent);

    let mut session = fixture.session();
    let first = session.tick().expect("first publish tick succeeds");
    assert_eq!(first.published, 1);
    assert_eq!(first.retry_at_millis, Some(1_010));
    let first_wire = fixture.connection.published();
    assert_eq!(first_wire.len(), 1);
    drop(session);

    fixture.clock.set(2_000);
    let mut restarted = fixture.session();
    let retry = restarted.tick().expect("uncertain retry succeeds");
    assert_eq!(retry.published, 1);
    let wires = fixture.connection.published();
    assert_eq!(wires, vec![first_wire[0].clone(), first_wire[0].clone()]);

    let wrapper_id = [4; 32];
    fixture.connection.push(RelayReceive::Frame(RelayFrame::Ok {
        event_id: wrapper_id,
        accepted: false,
        message: "duplicate: already retained".to_owned(),
    }));
    restarted.tick().expect("positive duplicate commits");
    assert_eq!(
        fixture
            .state
            .attempts
            .lock()
            .expect("attempts lock")
            .get(&(fixture.url.as_str().to_owned(), wrapper_id))
            .expect("attempt exists")
            .disposition,
        AttemptDisposition::Accepted
    );
    fixture.clock.set(10_000);
    assert_eq!(
        restarted
            .tick()
            .expect("accepted work stays quiet")
            .published,
        0
    );
}

#[test]
fn bounded_outbound_scan_retains_the_earliest_retry_across_pages() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .extend((1..=17).map(outbound));
    let attempts = (1_u8..=17).map(|identity| {
        (
            (fixture.url.as_str().to_owned(), [identity; 32]),
            RelayAttempt {
                url: fixture.url.clone(),
                wrapper_id: [identity; 32],
                attempts: 1,
                disposition: AttemptDisposition::Uncertain,
                failure: None,
                last_attempt_millis: 900,
                retry_at_millis: Some(if identity == 1 { 1_005 } else { 2_000 }),
            },
        )
    });
    fixture
        .state
        .attempts
        .lock()
        .expect("attempts lock")
        .extend(attempts);

    let mut session = fixture.session();
    assert!(session.tick().expect("first page scans").immediate_work);
    assert!(session.tick().expect("second page scans").immediate_work);
    let complete = session.tick().expect("final page reaches quiescence");
    assert!(!complete.immediate_work);
    assert_eq!(complete.retry_at_millis, Some(1_005));
    assert_eq!(complete.published, 0);
}

#[test]
fn required_auth_precedes_live_then_retained_and_live_edge_drains_after_eose() {
    let fixture = Fixture::new(RelayAccess::Read, RelayAuthentication::Required);
    fixture.connection.extend([
        RelayReceive::Frame(RelayFrame::Auth("challenge-a".to_owned())),
        RelayReceive::Frame(RelayFrame::Ok {
            event_id: [9; 32],
            accepted: true,
            message: String::new(),
        }),
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-live-1".to_owned(),
            exact_event: vec![1, 60],
        }),
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-retained-1".to_owned(),
            exact_event: vec![2, 50],
        }),
        RelayReceive::Frame(RelayFrame::EndOfStoredEvents("hq-retained-1".to_owned())),
        RelayReceive::Pending,
    ]);

    let mut session = fixture.session();
    let progress = session.tick().expect("authenticated catch-up succeeds");
    assert_eq!(progress.ingested, 2);
    assert_eq!(
        fixture
            .ingest
            .committed
            .lock()
            .expect("ingest locks")
            .as_slice(),
        &[vec![2], vec![1]],
        "retained input is handled before the buffered live edge"
    );
    let sent = fixture.connection.sent.lock().expect("sent locks");
    assert!(matches!(&sent[0], RelayFrame::Auth(value) if value == "challenge-a"));
    assert!(matches!(
        &sent[1],
        RelayFrame::Request { subscription, .. } if subscription == "hq-live-1"
    ));
    assert!(matches!(
        &sent[2],
        RelayFrame::Request { subscription, .. } if subscription == "hq-retained-1"
    ));
    drop(sent);
    let cursor = fixture
        .state
        .cursors
        .lock()
        .expect("cursors lock")
        .get(fixture.url.as_str())
        .cloned()
        .expect("cursor stores");
    assert!(cursor.exhausted);
    assert_eq!(cursor.oldest_created_at, Some(50));
    assert_eq!(cursor.oldest_wrapper_id, Some([2; 32]));
}

#[test]
fn reconnect_refreshes_an_exhausted_cursor_across_arbitrary_downtime() {
    let fixture = Fixture::new(RelayAccess::Read, RelayAuthentication::Disabled);
    fixture.clock.set(200_000_000);
    fixture.connection.extend([
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-retained-1".to_owned(),
            exact_event: vec![2, 50],
        }),
        RelayReceive::Frame(RelayFrame::EndOfStoredEvents("hq-retained-1".to_owned())),
        RelayReceive::Pending,
    ]);
    let mut first = fixture.session();
    first.tick().expect("initial retained scan completes");
    first.close().expect("first connection closes");
    let initial = fixture
        .state
        .cursors
        .lock()
        .expect("cursors lock")
        .get(fixture.url.as_str())
        .cloned()
        .expect("initial cursor stores");
    assert!(initial.exhausted);
    assert_eq!(initial.covered_through_millis, Some(200_000_000));

    fixture.clock.set(20_000_000_000);
    fixture.connection.extend([
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-retained-1".to_owned(),
            exact_event: vec![3, 40],
        }),
        RelayReceive::Frame(RelayFrame::EndOfStoredEvents("hq-retained-1".to_owned())),
        RelayReceive::Pending,
    ]);
    let mut restarted = fixture.session();
    restarted
        .tick()
        .expect("post-downtime overlap scan completes");
    assert_eq!(
        fixture
            .ingest
            .committed
            .lock()
            .expect("ingest locks")
            .as_slice(),
        &[vec![2], vec![3]]
    );
    let refreshed = fixture
        .state
        .cursors
        .lock()
        .expect("cursors lock")
        .get(fixture.url.as_str())
        .cloned()
        .expect("refreshed cursor stores");
    assert!(refreshed.exhausted);
    assert_eq!(refreshed.covered_through_millis, Some(20_000_000_000));
    assert!(refreshed.scan_started_at_millis > initial.scan_started_at_millis);
}

#[test]
fn transient_input_stages_then_recovers_and_permanent_input_quarantines() {
    let fixture = Fixture::new(RelayAccess::Read, RelayAuthentication::Disabled);
    fixture.ingest.failures.store(1, Ordering::Release);
    fixture.connection.extend([
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-retained-1".to_owned(),
            exact_event: vec![3, 40],
        }),
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-retained-1".to_owned(),
            exact_event: vec![0xff],
        }),
        RelayReceive::Frame(RelayFrame::EndOfStoredEvents("hq-retained-1".to_owned())),
        RelayReceive::Pending,
    ]);
    let mut session = fixture.session();
    let first = session.tick().expect("input classifications persist");
    assert_eq!(first.staged, 1);
    assert_eq!(first.quarantined, 1);
    assert_eq!(fixture.state.staged.lock().expect("staging locks").len(), 1);
    assert_eq!(
        fixture
            .state
            .quarantine
            .lock()
            .expect("quarantine locks")
            .len(),
        1
    );

    fixture.clock.set(2_000);
    let recovered = session.tick().expect("staging retry succeeds");
    assert_eq!(recovered.ingested, 1);
    assert!(
        fixture
            .state
            .staged
            .lock()
            .expect("staging locks")
            .is_empty()
    );
}

#[test]
fn full_equal_time_page_never_claims_exhaustion_and_retries_after_backoff() {
    let fixture = Fixture::new(RelayAccess::Read, RelayAuthentication::Disabled);
    fixture.state.cursors.lock().expect("cursors lock").insert(
        fixture.url.as_str().to_owned(),
        CatchupCursor {
            url: fixture.url.clone(),
            generation: NonZeroU64::MIN,
            scan_started_at_millis: 1_000,
            covered_through_millis: None,
            oldest_created_at: Some(50),
            oldest_wrapper_id: Some([2; 32]),
            exhausted: false,
        },
    );
    for _ in 0..8 {
        fixture
            .connection
            .push(RelayReceive::Frame(RelayFrame::SubscriptionEvent {
                subscription: "hq-retained-1".to_owned(),
                exact_event: vec![2, 50],
            }));
    }
    fixture
        .connection
        .push(RelayReceive::Frame(RelayFrame::EndOfStoredEvents(
            "hq-retained-1".to_owned(),
        )));
    let mut session = fixture.session();
    let stalled = session.tick().expect("full repeated page stays safe");
    assert_eq!(stalled.retry_at_millis, Some(1_010));
    assert!(
        !fixture
            .state
            .cursors
            .lock()
            .expect("cursors lock")
            .get(fixture.url.as_str())
            .expect("cursor remains")
            .exhausted
    );
    let retained_requests = || {
        fixture
            .connection
            .sent
            .lock()
            .expect("sent locks")
            .iter()
            .filter(|frame| {
                matches!(frame, RelayFrame::Request { subscription, .. } if subscription == "hq-retained-1")
            })
            .count()
    };
    assert_eq!(retained_requests(), 1);
    let waiting = session.tick().expect("backoff suppresses immediate repeat");
    assert_eq!(waiting.retry_at_millis, Some(1_010));
    assert_eq!(retained_requests(), 1);
    fixture.clock.set(1_010);
    session.tick().expect("due boundary retries inclusively");
    assert_eq!(retained_requests(), 2);
}

#[test]
fn negative_ok_is_redacted_and_rate_limited_retry_reuses_exact_bytes() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::OnChallenge);
    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .push(outbound(5));
    let mut session = fixture.session();
    session.tick().expect("first publish succeeds");
    fixture.connection.push(RelayReceive::Frame(RelayFrame::Ok {
        event_id: [5; 32],
        accepted: false,
        message: "rate-limited: free relay prose is not durable".to_owned(),
    }));
    session.tick().expect("negative acknowledgement persists");
    let rejected = fixture
        .state
        .attempts
        .lock()
        .expect("attempts lock")
        .get(&(fixture.url.as_str().to_owned(), [5; 32]))
        .cloned()
        .expect("attempt persists");
    assert_eq!(rejected.disposition, AttemptDisposition::Rejected);
    assert_eq!(rejected.failure, Some(RelayAttemptFailure::RateLimited));
    let retry_at = rejected.retry_at_millis.expect("rate limit retries");
    fixture.clock.set(retry_at);
    assert_eq!(session.tick().expect("due retry publishes").published, 1);
    let wires = fixture.connection.published();
    assert_eq!(wires[0], wires[1]);
}

#[test]
fn challenge_replacement_and_auth_required_retry_never_persist_relay_prose() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::OnChallenge);
    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .push(outbound(6));
    fixture.connection.extend([
        RelayReceive::Frame(RelayFrame::Auth("challenge-a".to_owned())),
        RelayReceive::Frame(RelayFrame::Auth("challenge-b".to_owned())),
        RelayReceive::Frame(RelayFrame::Ok {
            event_id: [6; 32],
            accepted: false,
            message: "auth-required: arbitrary relay words".to_owned(),
        }),
    ]);
    let mut session = fixture.session();
    session.tick().expect("challenge flow remains retryable");
    let sent_auth = fixture
        .connection
        .sent
        .lock()
        .expect("sent locks")
        .iter()
        .filter_map(|frame| match frame {
            RelayFrame::Auth(value) => Some(value.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sent_auth,
        vec![
            "challenge-a".to_owned(),
            "challenge-b".to_owned(),
            "challenge-b".to_owned(),
        ]
    );
    assert_eq!(
        fixture
            .state
            .attempts
            .lock()
            .expect("attempts lock")
            .get(&(fixture.url.as_str().to_owned(), [6; 32]))
            .expect("attempt stores")
            .failure,
        Some(RelayAttemptFailure::AuthenticationRequired)
    );
}

#[test]
fn route_exclusion_and_disabled_policy_prevent_session_effects() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .push(outbound(7));
    let mut dependencies = fixture.dependencies();
    dependencies.routes = Arc::new(FakeRoutes {
        url: RelayUrl::new("wss://other.example".to_owned()).expect("other URL validates"),
    });
    let mut session = RelaySession::new(fixture.policy.clone(), Fixture::config(), dependencies)
        .expect("enabled session constructs");
    assert_eq!(
        session.tick().expect("excluded route is quiet").published,
        0
    );
    assert!(
        fixture
            .state
            .prepared
            .lock()
            .expect("prepared locks")
            .is_empty()
    );

    let mut disabled = fixture.policy.clone();
    disabled.enabled = false;
    assert!(matches!(
        RelaySession::new(disabled, Fixture::config(), fixture.dependencies()),
        Err(RelayPortError::InvalidInput)
    ));
}

#[test]
fn transient_local_work_failure_does_not_tear_down_a_healthy_connection() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .push(outbound(8));
    let mut dependencies = fixture.dependencies();
    dependencies.routes = Arc::new(UnavailableRoutes);
    let mut session = RelaySession::new(fixture.policy.clone(), Fixture::config(), dependencies)
        .expect("session constructs");
    let progress = session.tick().expect("local outage is tolerated");
    assert_eq!(progress.published, 0);
    assert_eq!(progress.retry_at_millis, Some(1_010));
    assert_eq!(session.tick().expect("healthy socket remains").published, 0);
    assert_eq!(fixture.connection.connects.load(Ordering::Acquire), 1);
    assert_eq!(fixture.connection.closes.load(Ordering::Acquire), 0);
}

#[test]
fn send_failure_leaves_a_durable_uncertain_attempt_before_reconnect() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .push(outbound(9));
    fixture.connection.send_failures.store(1, Ordering::Release);

    let mut session = fixture.session();
    assert_eq!(session.tick(), Err(RelayPortError::Connection));
    let attempt = fixture
        .state
        .attempts
        .lock()
        .expect("attempts lock")
        .get(&(fixture.url.as_str().to_owned(), [9; 32]))
        .cloned()
        .expect("uncertain attempt commits before network send");
    assert_eq!(attempt.disposition, AttemptDisposition::Uncertain);
    assert!(fixture.connection.published().is_empty());
}

#[test]
fn live_buffer_overflow_stages_exact_input_without_losing_retained_progress() {
    let fixture = Fixture::new(RelayAccess::Read, RelayAuthentication::Disabled);
    fixture.connection.extend([
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-live-1".to_owned(),
            exact_event: vec![1, 60],
        }),
        RelayReceive::Frame(RelayFrame::SubscriptionEvent {
            subscription: "hq-live-1".to_owned(),
            exact_event: vec![2, 61],
        }),
        RelayReceive::Frame(RelayFrame::EndOfStoredEvents("hq-retained-1".to_owned())),
    ]);
    let mut config = Fixture::config();
    config.live_buffer_items = 1;
    let mut session = RelaySession::new(fixture.policy.clone(), config, fixture.dependencies())
        .expect("session constructs");
    let progress = session.tick().expect("bounded live edge persists overflow");
    assert_eq!(progress.staged, 1);
    assert_eq!(progress.ingested, 1);
    assert_eq!(fixture.state.staged.lock().expect("staging locks").len(), 1);
}

#[test]
fn manager_coalesces_wakes_refreshes_only_changed_generation_and_joins_every_owner() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .policies
        .lock()
        .expect("policies lock")
        .push(fixture.policy.clone());
    let manager = RelayManager::start(
        RelayManagerConfig {
            session: Fixture::config(),
            policy_page_items: 8,
            max_sessions: 8,
        },
        fixture.dependencies(),
    )
    .expect("manager starts");
    wait_for(|| fixture.connection.connects.load(Ordering::Acquire) == 1);
    for _ in 0..32 {
        manager.wake().expect("wake coalesces");
    }

    let mut refreshed = fixture.policy.clone();
    refreshed.generation = NonZeroU64::new(2).expect("generation is positive");
    *fixture.state.policies.lock().expect("policies lock") = vec![refreshed];
    manager.wake().expect("refresh wake succeeds");
    wait_for(|| fixture.connection.connects.load(Ordering::Acquire) == 2);
    assert!(fixture.connection.closes.load(Ordering::Acquire) >= 1);

    fixture.state.policies.lock().expect("policies lock")[0].enabled = false;
    manager.wake().expect("disable wake succeeds");
    wait_for(|| fixture.connection.closes.load(Ordering::Acquire) >= 2);
    let report = manager.shutdown().expect("manager joins");
    assert_eq!(report.sessions_started, 2);
    assert_eq!(report.sessions_joined, 2);
    assert!(report.failures.is_empty());
}

#[test]
fn manager_drains_bounded_outbound_pages_from_one_durable_wake() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .policies
        .lock()
        .expect("policies lock")
        .push(fixture.policy.clone());
    let manager = RelayManager::start(
        RelayManagerConfig {
            session: Fixture::config(),
            policy_page_items: 8,
            max_sessions: 8,
        },
        fixture.dependencies(),
    )
    .expect("manager starts");
    wait_for(|| fixture.connection.connects.load(Ordering::Acquire) == 1);

    fixture
        .state
        .outbound
        .lock()
        .expect("outbound locks")
        .extend((1..=17).map(outbound));
    manager.wake().expect("one durable wake succeeds");
    wait_for(|| fixture.connection.published().len() == 17);

    let report = manager.shutdown().expect("manager joins");
    assert_eq!(report.sessions_started, 1);
    assert_eq!(report.sessions_joined, 1);
    assert!(report.failures.is_empty());
}

#[test]
fn inbound_socket_readiness_wakes_an_idle_authenticated_session() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Required);
    fixture
        .state
        .policies
        .lock()
        .expect("policies lock")
        .push(fixture.policy.clone());
    let manager = RelayManager::start(
        RelayManagerConfig {
            session: Fixture::config(),
            policy_page_items: 8,
            max_sessions: 8,
        },
        fixture.dependencies(),
    )
    .expect("manager starts");
    wait_for(|| fixture.connection.connects.load(Ordering::Acquire) == 1);

    fixture
        .connection
        .push(RelayReceive::Frame(RelayFrame::Auth(
            "idle-challenge".to_owned(),
        )));
    wait_for(|| {
        fixture
            .connection
            .sent
            .lock()
            .expect("sent locks")
            .iter()
            .any(|frame| matches!(frame, RelayFrame::Auth(value) if value == "idle-challenge"))
    });

    let report = manager.shutdown().expect("manager joins");
    assert!(report.failures.is_empty());
}

#[test]
fn peer_closure_wakes_idle_session_and_reconnects_on_failure_deadline() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .policies
        .lock()
        .expect("policies lock")
        .push(fixture.policy.clone());
    let manager = RelayManager::start(
        RelayManagerConfig {
            session: Fixture::config(),
            policy_page_items: 8,
            max_sessions: 8,
        },
        fixture.dependencies(),
    )
    .expect("manager starts");
    wait_for(|| fixture.connection.connects.load(Ordering::Acquire) == 1);

    fixture.connection.push(RelayReceive::Closed);
    wait_for(|| fixture.connection.connects.load(Ordering::Acquire) == 2);

    let report = manager.shutdown().expect("manager joins");
    assert!(report.failures.is_empty());
}

#[test]
fn manager_reconciles_a_terminal_child_without_a_periodic_scan() {
    let fixture = Fixture::new(RelayAccess::Write, RelayAuthentication::Disabled);
    fixture
        .state
        .policies
        .lock()
        .expect("policies lock")
        .push(fixture.policy.clone());
    fixture.state.worker_failures.store(1, Ordering::Release);
    let manager = RelayManager::start(
        RelayManagerConfig {
            session: Fixture::config(),
            policy_page_items: 8,
            max_sessions: 8,
        },
        fixture.dependencies(),
    )
    .expect("manager starts");

    wait_for(|| fixture.connection.connects.load(Ordering::Acquire) == 2);
    let report = manager.shutdown().expect("manager joins");
    assert_eq!(report.sessions_started, 2);
    assert_eq!(report.sessions_joined, 2);
    assert_eq!(report.failures, vec![RelayPortError::Corrupt]);
}

struct Fixture {
    url: RelayUrl,
    policy: RelayPolicy,
    state: Arc<FakeState>,
    ingest: Arc<FakeIngest>,
    clock: Arc<FakeClock>,
    connection: Arc<ScriptedConnectionState>,
    dependencies: RelaySessionDependencies,
}

impl Fixture {
    fn new(access: RelayAccess, authentication: RelayAuthentication) -> Self {
        let url = RelayUrl::new("wss://relay.example".to_owned()).expect("URL validates");
        let policy = RelayPolicy {
            url: url.clone(),
            access,
            authentication,
            enabled: true,
            generation: NonZeroU64::MIN,
        };
        let state = Arc::new(FakeState::default());
        let ingest = Arc::new(FakeIngest::default());
        let clock = Arc::new(FakeClock::new(1_000));
        let connection = Arc::new(ScriptedConnectionState::default());
        let dependencies = RelaySessionDependencies {
            state: state.clone(),
            routes: Arc::new(FakeRoutes { url: url.clone() }),
            ingest: ingest.clone(),
            envelopes: Arc::new(FakeEnvelopes),
            clock: clock.clone(),
            connector: Arc::new(FakeConnector {
                state: connection.clone(),
            }),
            jitter: Arc::new(ZeroJitter),
        };
        Self {
            url,
            policy,
            state,
            ingest,
            clock,
            connection,
            dependencies,
        }
    }

    fn session(&self) -> RelaySession {
        RelaySession::new(self.policy.clone(), Self::config(), self.dependencies())
            .expect("session constructs")
    }

    fn config() -> RelaySessionConfig {
        RelaySessionConfig {
            state_page_items: 8,
            retained_page_items: 8,
            live_buffer_items: 8,
            live_buffer_bytes: 1_024,
            max_frames_per_tick: 16,
            recover_staging: true,
            retry_initial: Duration::from_millis(10),
            retry_max: Duration::from_millis(40),
        }
    }

    fn dependencies(&self) -> RelaySessionDependencies {
        self.dependencies.clone()
    }
}

#[derive(Default)]
struct FakeState {
    policies: Mutex<Vec<RelayPolicy>>,
    outbound: Mutex<Vec<OutboundIntent>>,
    prepared: Mutex<BTreeMap<OutboxKey, PreparedOutbound>>,
    attempts: Mutex<BTreeMap<(String, [u8; 32]), RelayAttempt>>,
    cursors: Mutex<BTreeMap<String, CatchupCursor>>,
    staged: Mutex<BTreeMap<[u8; 32], StagedInput>>,
    quarantine: Mutex<Vec<hq_relay::QuarantineEvidence>>,
    worker_failures: AtomicUsize,
}

impl RelayStatePort for FakeState {
    fn load_page(&self, query: RelayStateQuery) -> Result<RelayStatePage, RelayPortError> {
        if !matches!(query.outbound, RelayPagePosition::Done)
            && self
                .worker_failures
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_sub(1)
                })
                .is_ok()
        {
            return Err(RelayPortError::Corrupt);
        }
        let mut state = RelayStateSnapshot::default();
        if !matches!(query.policies, RelayPagePosition::Done) {
            state
                .policies
                .clone_from(&self.policies.lock().expect("policies lock"));
        }
        let mut next = None;
        if !matches!(query.outbound, RelayPagePosition::Done) {
            let mut outbound = self.outbound.lock().expect("outbound locks").clone();
            outbound.sort_by_key(|intent| (intent.revision, intent.key));
            if let RelayPagePosition::After(cursor) = &query.outbound {
                outbound
                    .retain(|intent| (intent.revision, intent.key) > (cursor.revision, cursor.key));
            }
            let has_more = outbound.len() > query.limit;
            outbound.truncate(query.limit);
            if has_more {
                let last = outbound.last().expect("nonempty page has a last row");
                let mut continuation = query.clone();
                continuation.outbound = RelayPagePosition::After(OutboundCursor {
                    revision: last.revision,
                    key: last.key,
                });
                next = Some(continuation);
            }
            state.outbound = outbound;
        }
        if !matches!(query.staged, RelayPagePosition::Done) {
            state.staged = self
                .staged
                .lock()
                .expect("staging locks")
                .values()
                .cloned()
                .collect();
        }
        Ok(RelayStatePage { state, next })
    }

    fn prepared(&self, key: OutboxKey) -> Result<Option<PreparedOutbound>, RelayPortError> {
        Ok(self
            .prepared
            .lock()
            .expect("prepared locks")
            .get(&key)
            .cloned())
    }

    fn attempt(
        &self,
        url: &RelayUrl,
        wrapper_id: [u8; 32],
    ) -> Result<Option<RelayAttempt>, RelayPortError> {
        Ok(self
            .attempts
            .lock()
            .expect("attempts lock")
            .get(&(url.as_str().to_owned(), wrapper_id))
            .cloned())
    }

    fn cursor(&self, url: &RelayUrl) -> Result<Option<CatchupCursor>, RelayPortError> {
        Ok(self
            .cursors
            .lock()
            .expect("cursors lock")
            .get(url.as_str())
            .cloned())
    }

    fn apply(&self, mutation: RelayStateMutation) -> Result<(), RelayPortError> {
        match mutation {
            RelayStateMutation::Prepare(prepared) => {
                let mut values = self.prepared.lock().expect("prepared locks");
                if values
                    .get(&prepared.key)
                    .is_some_and(|stored| stored != &prepared)
                {
                    return Err(RelayPortError::Conflict);
                }
                values.insert(prepared.key, prepared);
            }
            RelayStateMutation::Attempt(attempt) => {
                self.attempts.lock().expect("attempts lock").insert(
                    (attempt.url.as_str().to_owned(), attempt.wrapper_id),
                    attempt,
                );
            }
            RelayStateMutation::Cursor(cursor) => {
                self.cursors
                    .lock()
                    .expect("cursors lock")
                    .insert(cursor.url.as_str().to_owned(), cursor);
            }
            RelayStateMutation::ClaimInbound { remove_staged, .. } => {
                if let Some(digest) = remove_staged {
                    self.staged.lock().expect("staging locks").remove(&digest);
                }
            }
            RelayStateMutation::Stage(input) => {
                self.staged
                    .lock()
                    .expect("staging locks")
                    .insert(input.wrapper_sha256, input);
            }
            RelayStateMutation::Quarantine {
                evidence,
                remove_staged,
            } => {
                self.quarantine
                    .lock()
                    .expect("quarantine locks")
                    .push(evidence);
                if let Some(digest) = remove_staged {
                    self.staged.lock().expect("staging locks").remove(&digest);
                }
            }
            RelayStateMutation::Configure(_) => {}
        }
        Ok(())
    }
}

struct FakeRoutes {
    url: RelayUrl,
}

struct UnavailableRoutes;

impl RouteResolver for UnavailableRoutes {
    fn resolve(&self, _key: OutboxKey) -> Result<ResolvedRoute, RelayPortError> {
        Err(RelayPortError::Unavailable)
    }
}

impl RouteResolver for FakeRoutes {
    fn resolve(&self, _key: OutboxKey) -> Result<ResolvedRoute, RelayPortError> {
        Ok(ResolvedRoute {
            recipient_public_key: [8; 32],
            relays: vec![self.url.clone()],
        })
    }
}

#[derive(Default)]
struct FakeIngest {
    failures: AtomicUsize,
    committed: Mutex<Vec<Vec<u8>>>,
}

impl CanonicalIngest for FakeIngest {
    fn ingest(&self, exact_canonical_bytes: Vec<u8>) -> Result<(), RelayPortError> {
        if self
            .failures
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            return Err(RelayPortError::Unavailable);
        }
        self.committed
            .lock()
            .expect("ingest locks")
            .push(exact_canonical_bytes);
        Ok(())
    }
}

struct FakeEnvelopes;

impl RelayEnvelopePort for FakeEnvelopes {
    fn local_public_key(&self) -> [u8; 32] {
        [7; 32]
    }

    fn prepare(
        &self,
        intent: &OutboundIntent,
        recipient_public_key: [u8; 32],
        now_seconds: u64,
    ) -> Result<PreparedOutbound, RelayPortError> {
        let identity = intent.exact_canonical_bytes[0];
        let exact_wire = vec![identity, 0xaa];
        Ok(PreparedOutbound {
            key: intent.key,
            envelope: DurableEnvelope {
                metadata: PreparedEnvelopeMetadata {
                    wrapper_id: [identity; 32],
                    one_use_public_key: [identity.wrapping_add(1); 32],
                    recipient_public_key,
                    canonical_event_id: *intent.key.fact_id.as_bytes(),
                    canonical_sha256: Sha256::digest(&intent.exact_canonical_bytes).into(),
                    wrapper_sha256: Sha256::digest(&exact_wire).into(),
                    seal_created_at: now_seconds,
                    gift_wrap_created_at: now_seconds,
                    byte_len: exact_wire.len(),
                },
                exact_wire,
            },
        })
    }

    fn open(&self, exact_outer: &[u8]) -> Result<RelayOpenOutcome, RelayPortError> {
        if exact_outer == [0xff] {
            return Ok(RelayOpenOutcome::Rejected(RejectedRelayEnvelope {
                failure: FailureClass::Mac,
                wrapper_id: None,
            }));
        }
        let identity = exact_outer[0];
        Ok(RelayOpenOutcome::Opened(OpenedRelayEnvelope {
            exact_canonical_bytes: vec![identity],
            wrapper_id: [identity; 32],
            wrapper_created_at: u64::from(exact_outer[1]),
            logical_id: LogicalEnvelopeId {
                origin_installation_id: [6; 32],
                canonical_event_id: [identity.wrapping_add(10); 32],
            },
            canonical_sha256: Sha256::digest([identity]).into(),
        }))
    }

    fn authenticate(
        &self,
        _url: &RelayUrl,
        challenge: &str,
        _now_seconds: u64,
    ) -> Result<PreparedRelayAuthentication, RelayPortError> {
        Ok(PreparedRelayAuthentication {
            event_id: [9; 32],
            exact_event: challenge.as_bytes().to_vec(),
        })
    }
}

struct FakeClock {
    millis: AtomicU64,
}

impl FakeClock {
    fn new(millis: u64) -> Self {
        Self {
            millis: AtomicU64::new(millis),
        }
    }

    fn set(&self, millis: u64) {
        self.millis.store(millis, Ordering::Release);
    }
}

impl RelayClock for FakeClock {
    fn unix_millis(&self) -> u64 {
        self.millis.load(Ordering::Acquire)
    }

    fn monotonic_millis(&self) -> u64 {
        self.unix_millis()
    }
}

struct ZeroJitter;

impl RelayJitter for ZeroJitter {
    fn jitter_millis(
        &self,
        _url: &RelayUrl,
        _identity: [u8; 32],
        _attempt: u32,
        _inclusive_max_millis: u64,
    ) -> u64 {
        0
    }
}

struct ScriptedConnectionState {
    received: Mutex<VecDeque<RelayReceive>>,
    sent: Mutex<Vec<RelayFrame>>,
    send_failures: AtomicUsize,
    closes: AtomicUsize,
    connects: AtomicUsize,
    readiness_writer: Mutex<UnixStream>,
    readiness_reader: UnixStream,
}

impl Default for ScriptedConnectionState {
    fn default() -> Self {
        let (readiness_reader, readiness_writer) =
            UnixStream::pair().expect("readiness pair constructs");
        readiness_reader
            .set_nonblocking(true)
            .and_then(|()| readiness_writer.set_nonblocking(true))
            .expect("readiness pair becomes nonblocking");
        Self {
            received: Mutex::default(),
            sent: Mutex::default(),
            send_failures: AtomicUsize::default(),
            closes: AtomicUsize::default(),
            connects: AtomicUsize::default(),
            readiness_writer: Mutex::new(readiness_writer),
            readiness_reader,
        }
    }
}

impl ScriptedConnectionState {
    fn push(&self, receive: RelayReceive) {
        self.received
            .lock()
            .expect("receive queue locks")
            .push_back(receive);
        self.signal_readiness();
    }

    fn extend(&self, receive: impl IntoIterator<Item = RelayReceive>) {
        self.received
            .lock()
            .expect("receive queue locks")
            .extend(receive);
        self.signal_readiness();
    }

    fn published(&self) -> Vec<Vec<u8>> {
        self.sent
            .lock()
            .expect("sent locks")
            .iter()
            .filter_map(|frame| match frame {
                RelayFrame::Event(exact) => Some(exact.clone()),
                _ => None,
            })
            .collect()
    }

    fn signal_readiness(&self) {
        let _ = self
            .readiness_writer
            .lock()
            .expect("readiness writer locks")
            .write(&[1]);
    }
}

struct FakeConnector {
    state: Arc<ScriptedConnectionState>,
}

impl RelayConnector for FakeConnector {
    fn connect(&self, _url: &RelayUrl) -> Result<Box<dyn RelayConnection>, RelayPortError> {
        self.state.connects.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(ScriptedConnection {
            state: self.state.clone(),
            readiness: self
                .state
                .readiness_reader
                .try_clone()
                .map_err(|_| RelayPortError::Unavailable)?,
        }))
    }
}

struct ScriptedConnection {
    state: Arc<ScriptedConnectionState>,
    readiness: UnixStream,
}

impl RelayConnection for ScriptedConnection {
    fn readiness(&self) -> BorrowedFd<'_> {
        self.readiness.as_fd()
    }

    fn send(&mut self, frame: RelayFrame) -> Result<(), RelayPortError> {
        if self
            .state
            .send_failures
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            return Err(RelayPortError::Unavailable);
        }
        self.state.sent.lock().expect("sent locks").push(frame);
        Ok(())
    }

    fn receive(&mut self) -> Result<RelayReceive, RelayPortError> {
        let receive = self
            .state
            .received
            .lock()
            .expect("receive queue locks")
            .pop_front()
            .unwrap_or(RelayReceive::Pending);
        if !matches!(receive, RelayReceive::Pending) {
            let mut byte = [0_u8; 1];
            let _ = self.readiness.read(&mut byte);
        }
        Ok(receive)
    }

    fn close(&mut self) -> Result<(), RelayPortError> {
        self.state.closes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

fn outbound(identity: u8) -> OutboundIntent {
    OutboundIntent {
        key: OutboxKey {
            fact_id: FactId::from_bytes([identity; 32]),
            recipient: InstallationId::from_bytes([identity.wrapping_add(1); 32]),
        },
        exact_canonical_bytes: vec![identity],
        revision: Revision::new(u64::from(identity)),
    }
}

#[allow(clippy::panic)]
fn wait_for(condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("condition did not become true");
}
