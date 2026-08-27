//! Blocking lifecycle probe contracts over the owned local Unix socket.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{num::NonZeroUsize, sync::mpsc, time::Duration};

use hq_domain::MailboxId;
use hq_local_api::protocol::v1::{BuildMetadata, Id32, LifecycleRequest, LifecycleState};
use hq_node::{
    LifecycleClient, LifecycleClientConfig, LocalNodeRuntime, LocalNodeRuntimeConfig,
    LocalSessionPumpConfig, LocalSessionRegistryConfig, NodeComponents, NodeFoundation,
    NodeFoundationConfig, NodeOwner, RuntimePaths, ShutdownIntent, StateDirectoryOwner, StatePaths,
};
use hq_reducer::AuthorityPolicy;

use support::{TestDirectory, UnavailableNodeComponent};

fn build() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("lifecycle-client")).expect("build")
}

#[test]
fn one_shot_client_negotiates_status_and_preserves_the_stop_acknowledgement() {
    let directory = TestDirectory::new();
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let initializer = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = initializer.initialize().expect("identity");
    drop(initializer);
    let runtime_paths = RuntimePaths::new(directory.path().join("runtime")).expect("runtime paths");
    let server_state = state.clone();
    let server_runtime = runtime_paths.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Tokio runtime");
        runtime.block_on(async move {
            let foundation = NodeFoundation::open(NodeFoundationConfig::new(
                server_state,
                server_runtime,
                NonZeroUsize::new(4).expect("store capacity"),
            ))
            .expect("foundation");
            let policy = AuthorityPolicy::new(
                foundation.public_identity().installation_id(),
                MailboxId::from_bytes([61; 32]),
            );
            let owner = NodeOwner::start(
                foundation,
                NodeComponents::new(
                    UnavailableNodeComponent,
                    UnavailableNodeComponent,
                    UnavailableNodeComponent,
                    UnavailableNodeComponent,
                ),
                NonZeroUsize::new(4).expect("task capacity"),
                NonZeroUsize::new(4).expect("subscription capacity"),
            )
            .expect("owner");
            let (runtime, _) = LocalNodeRuntime::start(
                owner,
                LocalNodeRuntimeConfig {
                    pump: LocalSessionPumpConfig {
                        registry: LocalSessionRegistryConfig {
                            session_capacity: NonZeroUsize::new(2).expect("session capacity"),
                            event_capacity: NonZeroUsize::new(4).expect("event capacity"),
                            write_capacity: NonZeroUsize::new(2).expect("write capacity"),
                        },
                        boot_nonce: Id32::new([171; 32]),
                    },
                    build: build(),
                    authority_policy: policy,
                    response_drain_timeout: Duration::from_secs(1),
                },
            )
            .expect("runtime starts");
            ready_tx.send(()).expect("ready signal");
            runtime
                .run_until(std::future::pending::<ShutdownIntent>())
                .await
                .expect("runtime drains")
        })
    });
    ready_rx.recv().expect("server ready");

    let mut client = LifecycleClient::new(LifecycleClientConfig {
        runtime: runtime_paths,
        build: build(),
        io_timeout: Duration::from_secs(1),
    })
    .expect("client");
    let status = client
        .request(LifecycleRequest::Status)
        .expect("status response");
    assert_eq!(status.status.state, LifecycleState::Ready);
    assert_eq!(
        status
            .readiness
            .expect("readiness after protocol")
            .boot_nonce,
        Id32::new([171; 32])
    );
    let stopping = client
        .request(LifecycleRequest::Stop)
        .expect("stop acknowledgement");
    assert_eq!(stopping.status.state, LifecycleState::Draining);
    let report = server.join().expect("server joins");
    assert_eq!(report.intent, ShutdownIntent::Stop);
    assert_eq!(report.local_sessions.sessions.retained_tasks, 0);
}
