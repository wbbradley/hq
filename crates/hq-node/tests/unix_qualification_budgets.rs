//! Native foreground-node readiness, memory, and shutdown regression budgets.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use hq_local_api::protocol::v1::{BuildMetadata, LifecycleRequest, LifecycleState};
use hq_node::{
    LifecycleClient, LifecycleClientConfig, RuntimePaths, StateDirectoryOwner, StatePaths,
};

use support::TestDirectory;

struct ForegroundNode {
    child: Option<Child>,
    state: StatePaths,
}

impl Drop for ForegroundNode {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_hq"))
            .arg("--state-root")
            .arg(self.state.root())
            .args(["daemon", "stop"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn duration_budget(name: &str, fallback_milliseconds: u64) -> Duration {
    let milliseconds = std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback_milliseconds);
    Duration::from_millis(milliseconds)
}

fn memory_budget(name: &str, fallback_kibibytes: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback_kibibytes)
}

fn resident_memory_kibibytes(process_id: u32) -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &process_id.to_string()])
        .output()
        .expect("resident-memory query runs");
    assert!(output.status.success(), "resident-memory query succeeds");
    String::from_utf8(output.stdout)
        .expect("resident-memory output is UTF-8")
        .trim()
        .parse()
        .expect("resident-memory output is kibibytes")
}

fn lifecycle_client(runtime: RuntimePaths) -> LifecycleClient {
    LifecycleClient::new(LifecycleClientConfig {
        runtime,
        build: BuildMetadata::new("hq-qualification", "0.1.0", Some("native-budgets"))
            .expect("build metadata validates"),
        io_timeout: Duration::from_millis(500),
    })
    .expect("lifecycle client constructs")
}

fn wait_until_ready(client: &mut LifecycleClient, maximum: Duration) -> Duration {
    let started = Instant::now();
    loop {
        if let Ok(observation) = client.request(LifecycleRequest::Readiness)
            && observation.status.state == LifecycleState::Ready
        {
            return started.elapsed();
        }
        assert!(
            started.elapsed() <= maximum,
            "cold readiness exceeded {maximum:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_resident_memory(process_id: u32, variable: &str, fallback: u64, activity: &str) {
    let resident_memory = resident_memory_kibibytes(process_id);
    let maximum = memory_budget(variable, fallback);
    assert!(
        resident_memory <= maximum,
        "{activity} resident memory was {resident_memory} KiB, exceeding {maximum} KiB"
    );
}

fn stop_and_wait(node: &mut ForegroundNode, maximum: Duration) -> Duration {
    let started = Instant::now();
    let stop = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--state-root")
        .arg(node.state.root())
        .args(["daemon", "stop"])
        .stdin(Stdio::null())
        .output()
        .expect("stop command runs");
    assert!(
        stop.status.success(),
        "stop command failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
    loop {
        if let Some(status) = node
            .child
            .as_mut()
            .expect("child is retained")
            .try_wait()
            .expect("foreground status reads")
        {
            assert!(status.success(), "foreground node exited with {status}");
            return started.elapsed();
        }
        assert!(
            started.elapsed() <= maximum,
            "graceful shutdown exceeded {maximum:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn native_foreground_node_meets_readiness_memory_and_shutdown_budgets() {
    let directory = TestDirectory::new();
    let state = StatePaths::new(directory.path().join("state")).expect("state paths validate");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner acquires");
    owner.initialize().expect("identity initializes");
    drop(owner);
    let runtime = RuntimePaths::new(state.root().join("runtime")).expect("runtime paths validate");

    let child = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--state-root")
        .arg(state.root())
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("foreground node starts");
    let mut node = ForegroundNode {
        child: Some(child),
        state,
    };
    let mut client = lifecycle_client(runtime.clone());
    let readiness_maximum =
        duration_budget("HQ_QUALIFICATION_COLD_READINESS_MAX_MILLISECONDS", 5_000);
    let readiness_elapsed = wait_until_ready(&mut client, readiness_maximum);
    assert!(readiness_elapsed <= readiness_maximum);

    let process_id = node.child.as_ref().expect("child is retained").id();
    assert_resident_memory(
        process_id,
        "HQ_QUALIFICATION_IDLE_RESIDENT_MEMORY_MAX_KIBIBYTES",
        131_072,
        "idle",
    );

    let connections = (0..8)
        .map(|_| UnixStream::connect(runtime.socket_file()).expect("local connection opens"))
        .collect::<Vec<_>>();
    for _ in 0..16 {
        let observation = client
            .request(LifecycleRequest::Status)
            .expect("status work succeeds");
        assert_eq!(observation.status.state, LifecycleState::Ready);
    }
    thread::sleep(Duration::from_millis(50));
    assert_resident_memory(
        process_id,
        "HQ_QUALIFICATION_ACTIVE_RESIDENT_MEMORY_MAX_KIBIBYTES",
        196_608,
        "active",
    );
    drop(connections);

    let shutdown_maximum =
        duration_budget("HQ_QUALIFICATION_GRACEFUL_SHUTDOWN_MAX_MILLISECONDS", 5_000);
    let shutdown_elapsed = stop_and_wait(&mut node, shutdown_maximum);
    assert!(shutdown_elapsed <= shutdown_maximum);
    assert!(!runtime.socket_file().exists());
    assert!(!runtime.readiness_file().exists());

    let mut child = node.child.take().expect("completed child is retained");
    let status = child.wait().expect("completed child joins");
    assert!(status.success());
}
