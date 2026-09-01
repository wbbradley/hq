//! Async listener and bounded central session-pump contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{num::NonZeroUsize, os::unix::net::UnixStream};

use hq_application::{ObserveRevisions, SubscriptionRequest, SubscriptionTopic};
use hq_domain::{InstallationId, MailboxId, OperationId, Revision};
use hq_local_api::{
    RevisionHub,
    protocol::v1::{BuildMetadata, ClientHello, Id32, V1, VersionRange, WireMessage},
};
use hq_node::{
    LocalSessionAdmissionError, LocalSessionDispatch, LocalSessionPump, LocalSessionPumpConfig,
    LocalSessionPumpEvent, LocalSessionPumpStartError, LocalSessionRegistryConfig, NodeFoundation,
    NodeFoundationConfig, RuntimeArtifactErrorClass, RuntimePaths, StateDirectoryOwner, StatePaths,
};
use hq_protocol::{Bip340Signer, DispatchOutcome, VerifiedSemanticFact};
use hq_reducer::AuthorityPolicy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use support::{TestDirectory, UnavailableLifecycle, unavailable_application};

fn foundation(directory: &TestDirectory) -> (NodeFoundation, RuntimePaths) {
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity");
    drop(owner);
    let runtime = RuntimePaths::new(directory.path().join("runtime")).expect("runtime paths");
    let mut foundation = NodeFoundation::open(NodeFoundationConfig::new(
        state,
        runtime.clone(),
        NonZeroUsize::new(4).expect("store capacity"),
    ))
    .expect("foundation opens");
    foundation.bind_local_listener().expect("listener binds");
    (foundation, runtime)
}

fn build() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("pump")).expect("build")
}

fn hello() -> WireMessage {
    WireMessage::ClientHello(ClientHello::new(
        VersionRange::new(V1, V1).expect("v1 range"),
        build(),
    ))
}

fn connect(runtime: &RuntimePaths) -> UnixStream {
    let client = UnixStream::connect(runtime.socket_file()).expect("client connects");
    client.set_nonblocking(true).expect("nonblocking client");
    client
}

async fn read_message(client: &mut tokio::net::UnixStream) -> WireMessage {
    let mut prefix = [0_u8; 4];
    client.read_exact(&mut prefix).await.expect("frame prefix");
    let body_len = usize::try_from(u32::from_be_bytes(prefix)).expect("frame length");
    let mut frame = prefix.to_vec();
    frame.resize(body_len + prefix.len(), 0);
    client
        .read_exact(&mut frame[prefix.len()..])
        .await
        .expect("frame body");
    WireMessage::decode_frame(&frame).expect("valid server frame")
}

fn config(session_capacity: usize) -> LocalSessionPumpConfig {
    LocalSessionPumpConfig {
        registry: LocalSessionRegistryConfig {
            session_capacity: NonZeroUsize::new(session_capacity).expect("session capacity"),
            event_capacity: NonZeroUsize::new(4).expect("event capacity"),
            write_capacity: NonZeroUsize::new(1).expect("write capacity"),
        },
        boot_nonce: Id32::new([121; 32]),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn committed_revision_wins_one_dispatch_then_listener_pressure_progresses() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(2).expect("hub capacity");
    let subscription_id = OperationId::from_bytes([91; 32]);
    hub.register_subscription(
        &SubscriptionRequest::new(subscription_id, [SubscriptionTopic::All])
            .expect("subscription request"),
    )
    .expect("subscription registers");
    hub.activate_subscription(subscription_id)
        .expect("subscription activates");
    let application = unavailable_application(hub.clone());
    let mut pump = LocalSessionPump::start(&mut foundation, config(1), hub.clone(), build())
        .expect("session pump starts");

    let drive = pump.drive_next(&application, &UnavailableLifecycle);
    let publish_and_connect = async {
        tokio::task::yield_now().await;
        let outcome = foundation
            .store()
            .expect("store remains owned")
            .ingest_verified(store_wake_fixture(), store_wake_policy())
            .expect("fixture commits");
        (outcome.revision(), connect(&runtime))
    };
    let (event, (revision, client)) = tokio::join!(drive, publish_and_connect);
    assert!(matches!(
        event,
        LocalSessionPumpEvent::StoreInvalidated {
            revision: observed,
            ..
        } if observed == revision && observed == Revision::new(1)
    ));
    let notice = hub
        .take(subscription_id)
        .expect("subscription remains known")
        .expect("revision reaches active subscriber");
    assert_eq!(notice.revision(), Revision::new(1));
    assert_eq!(notice.topics().len(), 1);
    assert!(notice.topics().contains(&SubscriptionTopic::All));
    assert!(notice.full_snapshot());
    assert!(matches!(
        pump.drive_next(&application, &UnavailableLifecycle).await,
        LocalSessionPumpEvent::Accepted { .. }
    ));

    drop(client);
    let report = pump.shutdown().await;
    assert_eq!(report.sessions.closed_sessions, 1);
    assert_eq!(report.sessions.joined_tasks, 1);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn listener_admission_is_bounded_and_session_progress_wins_under_accept_pressure() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(2).expect("hub capacity");
    let application = unavailable_application(hub.clone());
    let mut pump = LocalSessionPump::start(&mut foundation, config(1), hub, build())
        .expect("session pump starts");

    let first_client = connect(&runtime);
    let first_id = match pump.drive_next(&application, &UnavailableLifecycle).await {
        LocalSessionPumpEvent::Accepted { session_id } => session_id,
        other => panic!("unexpected first progress: {other:?}"),
    };
    let excess_client = connect(&runtime);
    let mut first_client =
        tokio::net::UnixStream::from_std(first_client).expect("first Tokio client");
    first_client
        .write_all(&hello().encode_frame().expect("hello frame"))
        .await
        .expect("hello writes");
    let first_progress = pump.drive_next(&application, &UnavailableLifecycle).await;
    let pressure_client = if matches!(first_progress, LocalSessionPumpEvent::Rejected { .. }) {
        Some(connect(&runtime))
    } else {
        None
    };
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let second_progress = pump.drive_next(&application, &UnavailableLifecycle).await;
    assert!(
        [&first_progress, &second_progress].iter().any(|event| {
            matches!(
                event,
                LocalSessionPumpEvent::Session {
                    dispatch: LocalSessionDispatch::MessageHandled { session_id },
                    invalidations,
                } if *session_id == first_id
                    && invalidations.delivered == 0
                    && invalidations.failures.is_empty()
            )
        }),
        "ready session work must progress within one competing listener event: {first_progress:?}, {second_progress:?}",
    );
    assert!(
        [&first_progress, &second_progress]
            .iter()
            .any(|event| matches!(
                event,
                LocalSessionPumpEvent::Rejected {
                    error: LocalSessionAdmissionError::Full,
                    ..
                }
            )),
        "listener pressure remains bounded by registry capacity",
    );
    assert!(matches!(
        read_message(&mut first_client).await,
        WireMessage::ServerHello(_)
    ));
    drop((excess_client, pressure_client));

    let report = pump.shutdown().await;
    assert!(report.listener_closed);
    assert_eq!(report.sessions.closed_sessions, 1);
    assert_eq!(report.sessions.joined_tasks, 1);
    assert_eq!(report.sessions.retained_sessions, 0);
    assert_eq!(report.sessions.retained_tasks, 0);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn joined_disconnect_releases_capacity_and_closed_intake_drops_the_listener() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(2).expect("hub capacity");
    let application = unavailable_application(hub.clone());
    let mut pump = LocalSessionPump::start(&mut foundation, config(1), hub, build())
        .expect("session pump starts");

    let first_client = connect(&runtime);
    let first_id = match pump.drive_next(&application, &UnavailableLifecycle).await {
        LocalSessionPumpEvent::Accepted { session_id } => session_id,
        other => panic!("unexpected first progress: {other:?}"),
    };
    drop(first_client);
    let mut joined = false;
    for _ in 0..2 {
        if matches!(
            pump.drive_next(&application, &UnavailableLifecycle).await,
            LocalSessionPumpEvent::Session {
                dispatch: LocalSessionDispatch::TaskJoined { session_id, .. },
                ..
            } if session_id == first_id
        ) {
            joined = true;
            break;
        }
    }
    assert!(joined, "closed session task is reaped in bounded progress");

    let second_client = connect(&runtime);
    assert!(matches!(
        pump.drive_next(&application, &UnavailableLifecycle).await,
        LocalSessionPumpEvent::Accepted { .. }
    ));
    pump.close_intake();
    assert!(UnixStream::connect(runtime.socket_file()).is_err());
    drop(second_client);

    let report = pump.shutdown().await;
    assert!(report.listener_closed);
    assert_eq!(report.sessions.closed_sessions, 1);
    assert_eq!(report.sessions.joined_tasks, 1);
    foundation.shutdown().expect("foundation cleanup");

    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let mut rebound = NodeFoundation::open(NodeFoundationConfig::new(
        state,
        runtime,
        NonZeroUsize::new(4).expect("store capacity"),
    ))
    .expect("foundation reopens");
    rebound
        .bind_local_listener()
        .expect("listener immediately rebinds");
    rebound.shutdown().expect("rebound cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn several_clients_receive_distinct_boot_local_ids_and_route_independently() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(2).expect("hub capacity");
    let application = unavailable_application(hub.clone());
    let mut pump = LocalSessionPump::start(&mut foundation, config(2), hub, build())
        .expect("session pump starts");
    let first_client = connect(&runtime);
    let second_client = connect(&runtime);

    let first_id = match pump.drive_next(&application, &UnavailableLifecycle).await {
        LocalSessionPumpEvent::Accepted { session_id } => session_id,
        other => panic!("unexpected first progress: {other:?}"),
    };
    let second_id = match pump.drive_next(&application, &UnavailableLifecycle).await {
        LocalSessionPumpEvent::Accepted { session_id } => session_id,
        other => panic!("unexpected second progress: {other:?}"),
    };
    assert_ne!(first_id, second_id);
    assert_ne!(first_id, Id32::new([0; 32]));
    assert_ne!(second_id, Id32::new([0; 32]));

    let mut first_client =
        tokio::net::UnixStream::from_std(first_client).expect("first Tokio client");
    let mut second_client =
        tokio::net::UnixStream::from_std(second_client).expect("second Tokio client");
    let frame = hello().encode_frame().expect("hello frame");
    first_client.write_all(&frame).await.expect("first hello");
    second_client.write_all(&frame).await.expect("second hello");

    let mut handled = Vec::new();
    while handled.len() < 2 {
        match pump.drive_next(&application, &UnavailableLifecycle).await {
            LocalSessionPumpEvent::Session {
                dispatch: LocalSessionDispatch::MessageHandled { session_id },
                ..
            } => handled.push(session_id),
            LocalSessionPumpEvent::Session {
                dispatch: LocalSessionDispatch::WriteConfirmed { .. },
                ..
            } => {}
            other => panic!("unexpected session progress: {other:?}"),
        }
    }
    handled.sort_unstable();
    let mut expected = vec![first_id, second_id];
    expected.sort_unstable();
    assert_eq!(handled, expected);
    assert!(matches!(
        read_message(&mut first_client).await,
        WireMessage::ServerHello(_)
    ));
    assert!(matches!(
        read_message(&mut second_client).await,
        WireMessage::ServerHello(_)
    ));

    let report = pump.shutdown().await;
    assert_eq!(report.sessions.closed_sessions, 2);
    assert_eq!(report.sessions.joined_tasks, 2);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn listener_transfer_is_once_only_and_invalid_nonce_does_not_consume_it() {
    let directory = TestDirectory::new();
    let (mut foundation, _runtime) = foundation(&directory);
    let hub = RevisionHub::new(1).expect("hub capacity");
    let mut invalid = config(1);
    invalid.boot_nonce = Id32::new([0; 32]);
    assert!(matches!(
        LocalSessionPump::start(&mut foundation, invalid, hub.clone(), build()),
        Err(LocalSessionPumpStartError::InvalidBootNonce)
    ));

    let pump = LocalSessionPump::start(&mut foundation, config(1), hub.clone(), build())
        .expect("valid transfer succeeds");
    assert!(matches!(
        LocalSessionPump::start(&mut foundation, config(1), hub, build()),
        Err(LocalSessionPumpStartError::Listener(
            RuntimeArtifactErrorClass::NotBound
        ))
    ));
    let report = pump.shutdown().await;
    assert!(report.listener_closed);
    foundation.shutdown().expect("foundation cleanup");
}

fn store_wake_policy() -> AuthorityPolicy {
    AuthorityPolicy::new(
        InstallationId::from_bytes([0x11; 32]),
        MailboxId::from_bytes([0x33; 32]),
    )
}

fn store_wake_fixture() -> VerifiedSemanticFact {
    const CONTENT: &str = r#"{"p":"hq/canonical","v":1,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[],"auth":[],"body":{"installation":"1111111111111111111111111111111111111111111111111111111111111111","signing":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","encryption":"2222222222222222222222222222222222222222222222222222222222222222","label":"wake-test"}}"#;
    let signer = Bip340Signer::from_secret_bytes({
        let mut secret = [0_u8; 32];
        secret[31] = 1;
        secret
    })
    .expect("fixture signer");
    let event = signer
        .sign(0, CONTENT.as_bytes(), [7; 32])
        .expect("fixture signs");
    let DispatchOutcome::Supported(supported) = event.dispatch().expect("fixture dispatches")
    else {
        panic!("fixture protocol is supported");
    };
    supported
        .decode_v1()
        .expect("fixture DTO verifies")
        .into_semantic_fact()
        .expect("fixture converts")
}
