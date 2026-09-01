//! Bounded asynchronous Unix session I/O contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{num::NonZeroUsize, os::unix::net::UnixStream};

use hq_local_api::protocol::v1::{
    BuildMetadata, ClientHello, Id32, InvalidationTopic, MAX_FRAME_BYTES, RevisionInvalidation, V1,
    VersionRange, WireMessage,
};
use hq_local_api::{RevisionHub, ServerSession};
use hq_node::{
    AcceptedLocalStream, LocalSessionClose, LocalSessionEvent, LocalSessionSendError,
    NodeFoundation, NodeFoundationConfig, RuntimePaths, StateDirectoryOwner, StatePaths,
    prepare_local_session_io,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use support::{TestDirectory, UnavailableLifecycle, unavailable_application};

fn foundation(directory: &TestDirectory) -> (NodeFoundation, RuntimePaths) {
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity");
    drop(owner);
    let runtime = RuntimePaths::new(directory.path().join("runtime")).expect("runtime paths");
    let foundation = NodeFoundation::open(NodeFoundationConfig::new(
        state,
        runtime.clone(),
        NonZeroUsize::new(4).expect("store capacity"),
    ))
    .expect("foundation opens");
    (foundation, runtime)
}

fn hello(commit: &'static str) -> WireMessage {
    WireMessage::ClientHello(ClientHello::new(
        VersionRange::new(V1, V1).expect("version range"),
        BuildMetadata::new("hq", "0.1.0", Some(commit)).expect("build"),
    ))
}

fn connect_and_accept(
    foundation: &NodeFoundation,
    runtime: &RuntimePaths,
) -> (AcceptedLocalStream, UnixStream) {
    let client = UnixStream::connect(runtime.socket_file()).expect("client connects");
    client.set_nonblocking(true).expect("nonblocking client");
    let accepted = loop {
        match foundation.accept_local() {
            Ok(stream) => break stream,
            Err(error) if error.class() == hq_node::RuntimeArtifactErrorClass::WouldBlock => {
                std::thread::yield_now();
            }
            Err(error) => panic!("unexpected accept failure: {error:?}"),
        }
    };
    (accepted, client)
}

#[tokio::test(flavor = "current_thread")]
async fn partial_and_multiple_frames_are_decoded_in_order_and_close_exactly_once() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    foundation.bind_local_listener().expect("listener binds");
    let (accepted, client) = connect_and_accept(&foundation, &runtime);

    let session_id = Id32::new([81; 32]);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(4);
    let (handle, driver) = prepare_local_session_io(
        accepted,
        session_id,
        NonZeroUsize::new(2).expect("write capacity"),
        events_tx,
    )
    .expect("session I/O prepares");
    let driver = tokio::spawn(driver);
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");

    let first = hello("first").encode_frame().expect("first frame");
    let second = hello("second").encode_frame().expect("second frame");
    client.write_all(&first[..3]).await.expect("partial prefix");
    let mut remainder = first[3..].to_vec();
    remainder.extend_from_slice(&second);
    client
        .write_all(&remainder)
        .await
        .expect("remaining frames");

    assert_eq!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Message {
            session_id,
            message: Box::new(hello("first")),
        })
    );
    assert_eq!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Message {
            session_id,
            message: Box::new(hello("second")),
        })
    );

    handle.close();
    assert_eq!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Closed {
            session_id,
            cause: LocalSessionClose::Requested,
        })
    );
    driver.await.expect("driver joins");
    assert!(events_rx.try_recv().is_err());
    drop((client, handle));
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_and_truncated_frames_close_without_emitting_a_message() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    foundation.bind_local_listener().expect("listener binds");
    let session_id = Id32::new([82; 32]);

    let (accepted, client) = connect_and_accept(&foundation, &runtime);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(2);
    let (_handle, driver) = prepare_local_session_io(
        accepted,
        session_id,
        NonZeroUsize::new(1).expect("write capacity"),
        events_tx,
    )
    .expect("session I/O prepares");
    let driver = tokio::spawn(driver);
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");
    let oversized = u32::try_from(MAX_FRAME_BYTES + 1)
        .expect("frame bound fits")
        .to_be_bytes();
    client
        .write_all(&oversized)
        .await
        .expect("oversized prefix");
    assert_eq!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Closed {
            session_id,
            cause: LocalSessionClose::Protocol,
        })
    );
    driver.await.expect("oversized driver joins");

    let (accepted, client) = connect_and_accept(&foundation, &runtime);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(2);
    let (_handle, driver) = prepare_local_session_io(
        accepted,
        session_id,
        NonZeroUsize::new(1).expect("write capacity"),
        events_tx,
    )
    .expect("session I/O prepares");
    let driver = tokio::spawn(driver);
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");
    let frame = hello("truncated").encode_frame().expect("frame");
    client
        .write_all(&frame[..frame.len() - 1])
        .await
        .expect("truncated bytes");
    client.shutdown().await.expect("client write shutdown");
    assert_eq!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Closed {
            session_id,
            cause: LocalSessionClose::Protocol,
        })
    );
    driver.await.expect("truncated driver joins");
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn encoded_write_queue_is_fixed_and_close_bypasses_its_capacity() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    foundation.bind_local_listener().expect("listener binds");
    let (accepted, client) = connect_and_accept(&foundation, &runtime);
    let session_id = Id32::new([83; 32]);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(2);
    let (handle, driver) = prepare_local_session_io(
        accepted,
        session_id,
        NonZeroUsize::new(1).expect("write capacity"),
        events_tx,
    )
    .expect("session I/O prepares");
    let invalidation = WireMessage::Invalidation(
        RevisionInvalidation::new(Id32::new([84; 32]), 7, vec![InvalidationTopic::All], false)
            .expect("invalidation"),
    );

    assert_eq!(handle.try_send_invalidation(&invalidation), Ok(()));
    assert_eq!(
        handle.try_send_invalidation(&invalidation),
        Err(LocalSessionSendError::Full)
    );
    assert_eq!(
        handle.try_send_invalidation(&hello("not-invalidation")),
        Err(LocalSessionSendError::InvalidMessage)
    );
    handle.close();
    let driver = tokio::spawn(driver);
    assert_eq!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Closed {
            session_id,
            cause: LocalSessionClose::Requested,
        })
    );
    driver.await.expect("driver joins");
    assert_eq!(
        handle.try_send_invalidation(&invalidation),
        Err(LocalSessionSendError::Closed)
    );
    drop((client, handle));
    foundation.shutdown().expect("foundation cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn tracked_ticket_is_emitted_only_after_the_exact_complete_frame_is_readable() {
    let directory = TestDirectory::new();
    let (mut foundation, runtime) = foundation(&directory);
    foundation.bind_local_listener().expect("listener binds");
    let (accepted, client) = connect_and_accept(&foundation, &runtime);
    let session_id = Id32::new([85; 32]);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(2);
    let (handle, driver) = prepare_local_session_io(
        accepted,
        session_id,
        NonZeroUsize::new(1).expect("write capacity"),
        events_tx,
    )
    .expect("session I/O prepares");
    let driver = tokio::spawn(driver);
    let mut client = tokio::net::UnixStream::from_std(client).expect("Tokio client");
    let hub = RevisionHub::new(1).expect("hub capacity");
    let application = unavailable_application(hub.clone());
    let mut server = ServerSession::new(hub, build_for_server(), session_id);
    let outbound = server
        .receive(hello("server-write"), &application, &UnavailableLifecycle)
        .expect("server hello prepared");
    let ticket = outbound.ticket();
    handle
        .try_send_response(outbound)
        .expect("response enters bounded queue");

    let mut prefix = [0_u8; 4];
    client.read_exact(&mut prefix).await.expect("length prefix");
    let length = usize::try_from(u32::from_be_bytes(prefix)).expect("frame length");
    let mut frame = prefix.to_vec();
    frame.resize(length + 4, 0);
    client
        .read_exact(&mut frame[4..])
        .await
        .expect("complete frame body");
    assert!(matches!(
        WireMessage::decode_frame(&frame),
        Ok(WireMessage::ServerHello(_))
    ));
    assert_eq!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Written { session_id, ticket })
    );
    server
        .confirm_written(ticket)
        .expect("exact ticket confirms after complete write");

    handle.close();
    assert!(matches!(
        events_rx.recv().await,
        Some(LocalSessionEvent::Closed {
            session_id: closed,
            cause: LocalSessionClose::Requested,
        }) if closed == session_id
    ));
    driver.await.expect("driver joins");
    foundation.shutdown().expect("foundation cleanup");
}

fn build_for_server() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("server")).expect("server build")
}
