//! Ordered local lifecycle and signal drain contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{num::NonZeroUsize, os::unix::net::UnixStream, time::Duration};

use hq_domain::MailboxId;
use hq_local_api::protocol::v1::{
    BuildMetadata, ClientHello, Id32, LifecycleRequest, LifecycleState, Request, RequestEnvelope,
    RequestId, Response, ResponseResult, V1, VersionRange, WireMessage,
};
use hq_node::{
    CancellationToken, ComponentDrain, ComponentError, ComponentKind, LocalNodeRuntime,
    LocalNodeRuntimeConfig, LocalNodeRuntimeStartError, LocalSessionPumpConfig,
    LocalSessionRegistryConfig, NodeComponent, NodeComponents, NodeFoundation,
    NodeFoundationConfig, NodeOwner, RuntimePaths, ShutdownIntent, ShutdownStage,
    StateDirectoryOwner, StatePaths, UnixShutdownSignals,
};
use hq_reducer::AuthorityPolicy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use support::{TestDirectory, UnavailableNodeComponent};

fn build() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("graceful")).expect("build")
}

fn owner(
    directory: &TestDirectory,
) -> (
    NodeOwner<
        UnavailableNodeComponent,
        UnavailableNodeComponent,
        UnavailableNodeComponent,
        UnavailableNodeComponent,
    >,
    RuntimePaths,
    AuthorityPolicy,
) {
    let (foundation, runtime, policy) = foundation(directory);
    let components = NodeComponents::new(
        UnavailableNodeComponent,
        UnavailableNodeComponent,
        UnavailableNodeComponent,
        UnavailableNodeComponent,
    );
    let owner = start_owner(foundation, components);
    (owner, runtime, policy)
}

fn foundation(directory: &TestDirectory) -> (NodeFoundation, RuntimePaths, AuthorityPolicy) {
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let initializer = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = initializer.initialize().expect("identity");
    drop(initializer);
    let runtime = RuntimePaths::new(directory.path().join("runtime")).expect("runtime paths");
    let foundation = NodeFoundation::open(NodeFoundationConfig::new(
        state,
        runtime.clone(),
        NonZeroUsize::new(4).expect("store capacity"),
    ))
    .expect("foundation opens");
    let policy = AuthorityPolicy::new(
        foundation.public_identity().installation_id,
        MailboxId::from_bytes([44; 32]),
    );
    (foundation, runtime, policy)
}

fn start_owner<L, R, H, P>(
    foundation: NodeFoundation,
    components: NodeComponents<L, R, H, P>,
) -> NodeOwner<L, R, H, P>
where
    L: NodeComponent,
    R: NodeComponent,
    H: NodeComponent,
    P: NodeComponent,
{
    NodeOwner::start(
        foundation,
        components,
        NonZeroUsize::new(4).expect("task capacity"),
        NonZeroUsize::new(4).expect("subscription capacity"),
    )
    .expect("node starts")
}

#[derive(Debug)]
struct FailingCleanupComponent;

impl NodeComponent for FailingCleanupComponent {
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        Err(ComponentError::unavailable())
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        Err(ComponentError::unavailable())
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        Err(ComponentError::unavailable())
    }
}

fn config(authority_policy: AuthorityPolicy) -> LocalNodeRuntimeConfig {
    LocalNodeRuntimeConfig {
        pump: LocalSessionPumpConfig {
            registry: LocalSessionRegistryConfig {
                session_capacity: NonZeroUsize::new(2).expect("session capacity"),
                event_capacity: NonZeroUsize::new(4).expect("event capacity"),
                write_capacity: NonZeroUsize::new(2).expect("write capacity"),
            },
            boot_nonce: Id32::new([151; 32]),
        },
        build: build(),
        authority_policy,
        response_drain_timeout: Duration::from_secs(1),
    }
}

async fn write(client: &mut tokio::net::UnixStream, message: WireMessage) {
    client
        .write_all(&message.encode_frame().expect("frame"))
        .await
        .expect("message writes");
}

async fn read(client: &mut tokio::net::UnixStream) -> WireMessage {
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

fn request(id: u64, lifecycle: LifecycleRequest) -> WireMessage {
    WireMessage::Request(RequestEnvelope::new(
        RequestId::new(id).expect("request id"),
        Request::Lifecycle(lifecycle),
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn protocol_stop_returns_the_draining_ack_before_ordered_cleanup_and_immediate_rebind() {
    let directory = TestDirectory::new();
    let (owner, runtime_paths, policy) = owner(&directory);
    let (runtime, readiness) =
        LocalNodeRuntime::start(owner, config(policy)).expect("local runtime starts");
    assert_eq!(readiness.build, build());
    assert!(runtime_paths.readiness_file().exists());

    let client = UnixStream::connect(runtime_paths.socket_file()).expect("client connects");
    client.set_nonblocking(true).expect("client nonblocking");
    let client = tokio::net::UnixStream::from_std(client).expect("Tokio client");
    let client_task = async move {
        let mut client = client;
        write(
            &mut client,
            WireMessage::ClientHello(ClientHello::new(
                VersionRange::new(V1, V1).expect("v1 range"),
                build(),
            )),
        )
        .await;
        assert!(matches!(
            read(&mut client).await,
            WireMessage::ServerHello(_)
        ));

        write(&mut client, request(1, LifecycleRequest::Status)).await;
        assert!(matches!(
            read(&mut client).await,
            WireMessage::Response(response)
                if matches!(
                    response.response,
                    Response::Success(ResponseResult::Lifecycle(ref status))
                        if status.state == LifecycleState::Ready && status.revision == Some(0)
                )
        ));

        write(&mut client, request(2, LifecycleRequest::Stop)).await;
        assert!(matches!(
            read(&mut client).await,
            WireMessage::Response(response)
                if matches!(
                    response.response,
                    Response::Success(ResponseResult::Lifecycle(ref status))
                        if status.state == LifecycleState::Draining
                )
        ));
        let mut terminal = [0_u8; 1];
        assert_eq!(client.read(&mut terminal).await.expect("terminal read"), 0);
    };

    let (report, ()) = tokio::join!(
        runtime.run_until(std::future::pending::<ShutdownIntent>()),
        client_task,
    );
    let report = report.expect("runtime drains");
    assert_eq!(report.intent, ShutdownIntent::Stop);
    assert!(!report.response_drain_timed_out);
    assert_eq!(report.local_sessions.sessions.retained_sessions, 0);
    assert_eq!(report.local_sessions.sessions.retained_tasks, 0);
    assert!(report.node.issues.is_empty());
    assert!(!runtime_paths.socket_file().exists());
    assert!(!runtime_paths.readiness_file().exists());

    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let mut rebound = NodeFoundation::open(NodeFoundationConfig::new(
        state,
        runtime_paths,
        NonZeroUsize::new(4).expect("store capacity"),
    ))
    .expect("state lock reacquired");
    rebound.bind_local_listener().expect("listener rebinds");
    rebound.shutdown().expect("rebound cleanup");
}

#[tokio::test(flavor = "current_thread")]
async fn external_shutdown_future_and_real_unix_signal_registration_share_stop_semantics() {
    let directory = TestDirectory::new();
    let (owner, _runtime_paths, policy) = owner(&directory);
    let (runtime, _) = LocalNodeRuntime::start(owner, config(policy)).expect("runtime starts");
    let report = runtime
        .run_until(std::future::ready(ShutdownIntent::Stop))
        .await
        .expect("signal drain");
    assert_eq!(report.intent, ShutdownIntent::Stop);
    assert_eq!(report.local_sessions.sessions.retained_tasks, 0);

    let _signals = UnixShutdownSignals::register().expect("Unix signals register");
}

#[tokio::test(flavor = "current_thread")]
async fn protocol_restart_retains_restart_intent_even_when_the_client_drops_its_ack() {
    let directory = TestDirectory::new();
    let (owner, runtime_paths, policy) = owner(&directory);
    let (runtime, _) = LocalNodeRuntime::start(owner, config(policy)).expect("runtime starts");
    let client = UnixStream::connect(runtime_paths.socket_file()).expect("client connects");
    client.set_nonblocking(true).expect("client nonblocking");
    let client = tokio::net::UnixStream::from_std(client).expect("Tokio client");
    let client_task = async move {
        let mut client = client;
        write(
            &mut client,
            WireMessage::ClientHello(ClientHello::new(
                VersionRange::new(V1, V1).expect("v1 range"),
                build(),
            )),
        )
        .await;
        assert!(matches!(
            read(&mut client).await,
            WireMessage::ServerHello(_)
        ));
        write(&mut client, request(1, LifecycleRequest::Restart)).await;
        drop(client);
    };
    let (report, ()) = tokio::join!(
        runtime.run_until(std::future::pending::<ShutdownIntent>()),
        client_task,
    );
    let report = report.expect("restart drains after response loss");
    assert_eq!(report.intent, ShutdownIntent::Restart);
    assert!(!report.response_drain_timed_out);
    assert_eq!(report.local_sessions.sessions.retained_sessions, 0);
    assert_eq!(report.local_sessions.sessions.retained_tasks, 0);
    assert!(!runtime_paths.socket_file().exists());
    assert!(!runtime_paths.readiness_file().exists());
}

#[test]
fn zero_response_drain_timeout_is_rejected_before_runtime_artifacts_open() {
    let directory = TestDirectory::new();
    let (owner, runtime_paths, policy) = owner(&directory);
    let mut invalid = config(policy);
    invalid.response_drain_timeout = Duration::ZERO;
    let error = LocalNodeRuntime::start(owner, invalid)
        .err()
        .expect("zero timeout is rejected");
    assert_eq!(
        error,
        LocalNodeRuntimeStartError::InvalidResponseDrainTimeout
    );
    assert!(!runtime_paths.socket_file().exists());
    assert!(!runtime_paths.readiness_file().exists());
}

#[tokio::test(flavor = "current_thread")]
async fn component_cleanup_failures_are_accumulated_without_skipping_foundation_release() {
    let directory = TestDirectory::new();
    let (foundation, runtime_paths, policy) = foundation(&directory);
    let owner = start_owner(
        foundation,
        NodeComponents::new(
            FailingCleanupComponent,
            UnavailableNodeComponent,
            UnavailableNodeComponent,
            UnavailableNodeComponent,
        ),
    );
    let (runtime, _) = LocalNodeRuntime::start(owner, config(policy)).expect("runtime starts");
    let report = runtime
        .run_until(std::future::ready(ShutdownIntent::Stop))
        .await
        .expect("runtime completes cleanup");
    assert_eq!(report.node.escalated, vec![ComponentKind::LocalSessions]);
    assert_eq!(report.node.issues.len(), 3);
    assert!(report.node.issues.iter().any(|issue| {
        issue.component == Some(ComponentKind::LocalSessions)
            && issue.stage == ShutdownStage::StopIntake
    }));
    assert!(report.node.issues.iter().any(|issue| {
        issue.component == Some(ComponentKind::LocalSessions) && issue.stage == ShutdownStage::Drain
    }));
    assert!(report.node.issues.iter().any(|issue| {
        issue.component == Some(ComponentKind::LocalSessions)
            && issue.stage == ShutdownStage::ForceStop
    }));
    assert!(!runtime_paths.socket_file().exists());
    assert!(!runtime_paths.readiness_file().exists());

    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let rebound = NodeFoundation::open(NodeFoundationConfig::new(
        state,
        runtime_paths,
        NonZeroUsize::new(4).expect("store capacity"),
    ))
    .expect("state lock released despite cleanup issues");
    rebound.shutdown().expect("rebound cleanup");
}
