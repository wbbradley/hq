//! Bounded central local-session ownership contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{num::NonZeroUsize, os::unix::net::UnixStream};

use hq_application::{Application, ApplicationPorts, SubscriptionTopic};
use hq_domain::Revision;
use hq_local_api::{
    RevisionHub,
    protocol::v1::{
        BuildMetadata, ClientHello, Id32, InvalidationTopic, MAX_FRAME_BYTES, Request,
        RequestEnvelope, RequestId, Response, SubscriptionRequestDto, V1, VersionRange,
        WireMessage,
    },
};
use hq_node::{
    LocalSessionAdmissionError, LocalSessionClose, LocalSessionDisconnectCause,
    LocalSessionDispatch, LocalSessionRegistry, LocalSessionRegistryConfig, LocalSessionSendError,
    NodeFoundation, NodeFoundationConfig, RuntimePaths, StateDirectoryOwner, StatePaths,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use support::{TestDirectory, UnavailableLifecycle, snapshot_application, unavailable_application};

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

fn accepted(
    foundation: &NodeFoundation,
    runtime: &RuntimePaths,
) -> (hq_node::AcceptedLocalStream, UnixStream) {
    let client = UnixStream::connect(runtime.socket_file()).expect("client connects");
    client.set_nonblocking(true).expect("nonblocking client");
    loop {
        match foundation.accept_local() {
            Ok(stream) => return (stream, client),
            Err(error) if error.class() == hq_node::RuntimeArtifactErrorClass::WouldBlock => {
                std::thread::yield_now();
            }
            Err(error) => panic!("unexpected accept failure: {error:?}"),
        }
    }
}

fn build() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("registry")).expect("build")
}

fn hello() -> WireMessage {
    WireMessage::ClientHello(ClientHello::new(
        VersionRange::new(V1, V1).expect("v1 range"),
        build(),
    ))
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

async fn negotiate<P: ApplicationPorts>(
    registry: &mut LocalSessionRegistry,
    application: &Application<P>,
    client: &mut tokio::net::UnixStream,
    session_id: Id32,
) {
    client
        .write_all(&hello().encode_frame().expect("hello frame"))
        .await
        .expect("hello writes");
    assert_eq!(
        registry
            .dispatch_next(application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::MessageHandled { session_id })
    );
    assert!(matches!(
        read_message(client).await,
        WireMessage::ServerHello(_)
    ));
    assert_eq!(
        registry
            .dispatch_next(application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::WriteConfirmed { session_id })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_and_capacity_rejection_spawn_nothing_and_shutdown_joins_the_admitted_task() {
    let directory = TestDirectory::new();
    let (foundation, runtime) = foundation(&directory);
    let config = LocalSessionRegistryConfig {
        session_capacity: NonZeroUsize::new(1).expect("session capacity"),
        event_capacity: NonZeroUsize::new(2).expect("event capacity"),
        write_capacity: NonZeroUsize::new(1).expect("write capacity"),
    };
    let mut registry =
        LocalSessionRegistry::new(config, RevisionHub::new(2).expect("hub capacity"), build());
    let first_id = Id32::new([101; 32]);
    let (first_stream, _first_client) = accepted(&foundation, &runtime);
    registry
        .admit(first_id, first_stream)
        .expect("first session admitted");
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.task_count(), 1);
    let (duplicate_stream, _duplicate_client) = accepted(&foundation, &runtime);
    assert_eq!(
        registry.admit(first_id, duplicate_stream),
        Err(LocalSessionAdmissionError::Duplicate)
    );
    let (excess_stream, _excess_client) = accepted(&foundation, &runtime);
    assert_eq!(
        registry.admit(Id32::new([102; 32]), excess_stream),
        Err(LocalSessionAdmissionError::Full)
    );
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.task_count(), 1);

    registry.close_intake();
    let (closed_stream, _closed_client) = accepted(&foundation, &runtime);
    assert_eq!(
        registry.admit(Id32::new([108; 32]), closed_stream),
        Err(LocalSessionAdmissionError::Closed)
    );
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.task_count(), 1);

    let report = registry.shutdown().await;
    assert_eq!(report.closed_sessions, 1);
    assert_eq!(report.joined_tasks, 1);
    assert!(report.task_failures.is_empty());
    assert_eq!(report.retained_sessions, 0);
    assert_eq!(report.retained_tasks, 0);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn decoded_messages_route_through_call_scoped_capabilities_and_exact_writes_confirm() {
    let directory = TestDirectory::new();
    let (foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(2).expect("hub capacity");
    let application = unavailable_application(hub.clone());
    let mut registry = LocalSessionRegistry::new(
        LocalSessionRegistryConfig {
            session_capacity: NonZeroUsize::new(1).expect("session capacity"),
            event_capacity: NonZeroUsize::new(2).expect("event capacity"),
            write_capacity: NonZeroUsize::new(1).expect("write capacity"),
        },
        hub,
        build(),
    );
    let session_id = Id32::new([103; 32]);
    let (stream, client) = accepted(&foundation, &runtime);
    registry
        .admit(session_id, stream)
        .expect("session admitted");
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");

    negotiate(&mut registry, &application, &mut client, session_id).await;

    let request = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(1).expect("request id"),
        Request::AuthoritativeSnapshot,
    ));
    client
        .write_all(&request.encode_frame().expect("request frame"))
        .await
        .expect("request writes");
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::MessageHandled { session_id })
    );
    assert!(matches!(
        read_message(&mut client).await,
        WireMessage::Response(response) if matches!(response.response, Response::Error(_))
    ));
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::WriteConfirmed { session_id })
    );

    let report = registry.shutdown().await;
    assert_eq!(report.closed_sessions, 1);
    assert_eq!(report.joined_tasks, 1);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn closing_request_intake_preserves_an_accepted_response_until_exact_write_confirmation() {
    let directory = TestDirectory::new();
    let (foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(2).expect("hub capacity");
    let application = unavailable_application(hub.clone());
    let mut registry = LocalSessionRegistry::new(
        LocalSessionRegistryConfig {
            session_capacity: NonZeroUsize::new(1).expect("session capacity"),
            event_capacity: NonZeroUsize::new(2).expect("event capacity"),
            write_capacity: NonZeroUsize::new(1).expect("write capacity"),
        },
        hub,
        build(),
    );
    let session_id = Id32::new([109; 32]);
    let (stream, client) = accepted(&foundation, &runtime);
    registry
        .admit(session_id, stream)
        .expect("session admitted");
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");

    client
        .write_all(&hello().encode_frame().expect("hello frame"))
        .await
        .expect("hello writes");
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::MessageHandled { session_id })
    );
    assert_eq!(registry.pending_response_count(), 1);
    registry.close_request_intake();
    assert!(matches!(
        read_message(&mut client).await,
        WireMessage::ServerHello(_)
    ));
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::WriteConfirmed { session_id })
    );
    assert_eq!(registry.pending_response_count(), 0);

    let request = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(1).expect("request id"),
        Request::AuthoritativeSnapshot,
    ));
    client
        .write_all(&request.encode_frame().expect("request frame"))
        .await
        .expect("request writes");
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::SessionClosing {
            session_id,
            cause: LocalSessionDisconnectCause::RequestIntakeClosed,
        })
    );

    let report = registry.shutdown().await;
    assert_eq!(report.joined_tasks, 1);
    assert_eq!(report.retained_sessions, 0);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_peer_is_removed_without_interrupting_its_negotiated_sibling() {
    let directory = TestDirectory::new();
    let (foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(2).expect("hub capacity");
    let application = unavailable_application(hub.clone());
    let mut registry = LocalSessionRegistry::new(
        LocalSessionRegistryConfig {
            session_capacity: NonZeroUsize::new(2).expect("session capacity"),
            event_capacity: NonZeroUsize::new(4).expect("event capacity"),
            write_capacity: NonZeroUsize::new(1).expect("write capacity"),
        },
        hub,
        build(),
    );
    let malformed_id = Id32::new([104; 32]);
    let sibling_id = Id32::new([105; 32]);
    let (malformed_stream, malformed_client) = accepted(&foundation, &runtime);
    let (sibling_stream, sibling_client) = accepted(&foundation, &runtime);
    registry
        .admit(malformed_id, malformed_stream)
        .expect("malformed peer admitted");
    registry
        .admit(sibling_id, sibling_stream)
        .expect("sibling admitted");
    let mut malformed_client =
        tokio::net::UnixStream::from_std(malformed_client).expect("Tokio malformed client");
    let mut sibling_client =
        tokio::net::UnixStream::from_std(sibling_client).expect("Tokio sibling client");

    malformed_client
        .write_all(
            &u32::try_from(MAX_FRAME_BYTES + 1)
                .expect("frame bound")
                .to_be_bytes(),
        )
        .await
        .expect("malformed prefix writes");
    sibling_client
        .write_all(&hello().encode_frame().expect("hello frame"))
        .await
        .expect("sibling hello writes");

    let mut malformed_closed = false;
    let mut malformed_joined = false;
    let mut sibling_handled = false;
    for _ in 0..6 {
        match registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await
            .expect("bounded progress")
        {
            LocalSessionDispatch::SessionClosing {
                session_id,
                cause: LocalSessionDisconnectCause::Transport(LocalSessionClose::Protocol),
            } if session_id == malformed_id => malformed_closed = true,
            LocalSessionDispatch::TaskJoined { session_id, cause }
                if session_id == malformed_id && cause == LocalSessionClose::Protocol =>
            {
                malformed_closed = true;
                malformed_joined = true;
            }
            LocalSessionDispatch::MessageHandled { session_id } if session_id == sibling_id => {
                sibling_handled = true;
            }
            LocalSessionDispatch::WriteConfirmed { session_id } if session_id == sibling_id => {}
            LocalSessionDispatch::StaleEvent { session_id }
                if session_id == malformed_id && malformed_joined => {}
            other => panic!("unexpected dispatch: {other:?}"),
        }
        if malformed_closed && malformed_joined && sibling_handled {
            break;
        }
    }
    assert!(malformed_closed && malformed_joined && sibling_handled);
    assert!(matches!(
        read_message(&mut sibling_client).await,
        WireMessage::ServerHello(_)
    ));
    assert_eq!(registry.len(), 1);

    let report = registry.shutdown().await;
    assert_eq!(report.closed_sessions, 1);
    assert_eq!(report.joined_tasks, 1);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn invalidation_queue_pressure_closes_only_the_slow_subscriber_and_cancels_registration() {
    let directory = TestDirectory::new();
    let (foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(1).expect("hub capacity");
    let application = snapshot_application(hub.clone());
    let mut registry = LocalSessionRegistry::new(
        LocalSessionRegistryConfig {
            session_capacity: NonZeroUsize::new(1).expect("session capacity"),
            event_capacity: NonZeroUsize::new(2).expect("event capacity"),
            write_capacity: NonZeroUsize::new(1).expect("write capacity"),
        },
        hub.clone(),
        build(),
    );
    let session_id = Id32::new([106; 32]);
    let subscription_id = Id32::new([107; 32]);
    let (stream, client) = accepted(&foundation, &runtime);
    registry
        .admit(session_id, stream)
        .expect("session admitted");
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");
    negotiate(&mut registry, &application, &mut client, session_id).await;

    let subscribe = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(1).expect("request id"),
        Request::Subscribe(
            SubscriptionRequestDto::new(subscription_id, vec![InvalidationTopic::Conversation])
                .expect("subscription"),
        ),
    ));
    client
        .write_all(&subscribe.encode_frame().expect("subscribe frame"))
        .await
        .expect("subscribe writes");
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::MessageHandled { session_id })
    );
    assert!(matches!(
        read_message(&mut client).await,
        WireMessage::Response(response) if matches!(response.response, Response::Success(_))
    ));
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::WriteConfirmed { session_id })
    );
    assert_eq!(hub.len(), 1);

    let _ = hub.publish(Revision::new(8), [SubscriptionTopic::Conversation], false);
    let first = registry.flush_invalidations();
    assert_eq!(first.delivered, 1);
    assert!(first.failures.is_empty());
    let _ = hub.publish(Revision::new(9), [SubscriptionTopic::Conversation], false);
    let second = registry.flush_invalidations();
    assert_eq!(second.delivered, 0);
    assert_eq!(second.failures.len(), 1);
    assert_eq!(second.failures[0].session_id, session_id);
    assert_eq!(second.failures[0].error, LocalSessionSendError::Full);
    assert!(hub.is_empty());

    let report = registry.shutdown().await;
    assert_eq!(report.closed_sessions, 1);
    assert_eq!(report.joined_tasks, 1);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn drain_cancels_a_subscription_whose_response_was_never_confirmed() {
    let directory = TestDirectory::new();
    let (foundation, runtime) = foundation(&directory);
    let hub = RevisionHub::new(1).expect("hub capacity");
    let application = snapshot_application(hub.clone());
    let mut registry = LocalSessionRegistry::new(
        LocalSessionRegistryConfig {
            session_capacity: NonZeroUsize::new(1).expect("session capacity"),
            event_capacity: NonZeroUsize::new(2).expect("event capacity"),
            write_capacity: NonZeroUsize::new(1).expect("write capacity"),
        },
        hub.clone(),
        build(),
    );
    let session_id = Id32::new([109; 32]);
    let (stream, client) = accepted(&foundation, &runtime);
    registry
        .admit(session_id, stream)
        .expect("session admitted");
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");
    negotiate(&mut registry, &application, &mut client, session_id).await;

    let subscribe = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(1).expect("request id"),
        Request::Subscribe(
            SubscriptionRequestDto::new(
                Id32::new([110; 32]),
                vec![InvalidationTopic::Conversation],
            )
            .expect("subscription"),
        ),
    ));
    client
        .write_all(&subscribe.encode_frame().expect("subscribe frame"))
        .await
        .expect("subscribe writes");
    assert_eq!(
        registry
            .dispatch_next(&application, &UnavailableLifecycle)
            .await,
        Some(LocalSessionDispatch::MessageHandled { session_id })
    );
    assert_eq!(
        hub.len(),
        1,
        "pending registration exists before ack confirmation"
    );

    let report = registry.shutdown().await;
    assert!(
        hub.is_empty(),
        "lost acknowledgement cannot retain registration"
    );
    assert_eq!(report.closed_sessions, 1);
    assert_eq!(report.joined_tasks, 1);
    assert_eq!(report.retained_sessions, 0);
    assert_eq!(report.retained_tasks, 0);
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn drain_completes_while_the_shared_event_queue_is_saturated() {
    let directory = TestDirectory::new();
    let (foundation, runtime) = foundation(&directory);
    let mut registry = LocalSessionRegistry::new(
        LocalSessionRegistryConfig {
            session_capacity: NonZeroUsize::new(2).expect("session capacity"),
            event_capacity: NonZeroUsize::new(1).expect("event capacity"),
            write_capacity: NonZeroUsize::new(1).expect("write capacity"),
        },
        RevisionHub::new(2).expect("hub capacity"),
        build(),
    );
    let (first_stream, first_client) = accepted(&foundation, &runtime);
    let (second_stream, second_client) = accepted(&foundation, &runtime);
    registry
        .admit(Id32::new([111; 32]), first_stream)
        .expect("first admitted");
    registry
        .admit(Id32::new([112; 32]), second_stream)
        .expect("second admitted");
    let mut first_client =
        tokio::net::UnixStream::from_std(first_client).expect("first Tokio client");
    let mut second_client =
        tokio::net::UnixStream::from_std(second_client).expect("second Tokio client");
    let frame = hello().encode_frame().expect("hello frame");
    first_client.write_all(&frame).await.expect("first hello");
    second_client.write_all(&frame).await.expect("second hello");

    let report = registry.shutdown().await;
    assert_eq!(report.closed_sessions, 2);
    assert_eq!(report.joined_tasks, 2);
    assert!(report.task_failures.is_empty());
    assert_eq!(report.retained_sessions, 0);
    assert_eq!(report.retained_tasks, 0);
    foundation.shutdown().expect("foundation cleanup");
}
