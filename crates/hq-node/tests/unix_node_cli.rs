//! End-to-end single-binary node lifecycle contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    io::Read,
    os::unix::net::UnixStream,
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use hq_local_api::protocol::v1::{BuildMetadata, LifecycleRequest, LifecycleState};
use hq_node::{
    LifecycleClient, LifecycleClientConfig, RuntimePaths, StateDirectoryOwner, StatePaths,
};

use support::TestDirectory;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn initialize(directory: &TestDirectory) -> (StatePaths, RuntimePaths) {
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity");
    drop(owner);
    let runtime = RuntimePaths::new(state.root().join("runtime")).expect("runtime paths");
    (state, runtime)
}

fn command(action: &str, state_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command
        .arg("node")
        .arg(action)
        .arg("--state-root")
        .arg(state_root);
    command
}

fn output(action: &str, state_root: &Path) -> Output {
    command(action, state_root)
        .output()
        .expect("CLI process runs")
}

fn client(runtime: RuntimePaths) -> LifecycleClient {
    LifecycleClient::new(LifecycleClientConfig {
        runtime,
        build: BuildMetadata::new("hq-test", "0.1.0", Some("cli-e2e")).expect("build"),
        io_timeout: Duration::from_millis(500),
    })
    .expect("client")
}

fn wait_ready(client: &mut LifecycleClient) -> hq_node::LifecycleObservation {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(observation) = client.request(LifecycleRequest::Readiness)
            && observation.status.state == LifecycleState::Ready
        {
            return observation;
        }
        assert!(Instant::now() < deadline, "node readiness timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_child(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().expect("child status") {
            assert!(status.success(), "foreground node exit: {status}");
            return;
        }
        assert!(Instant::now() < deadline, "foreground node exit timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn foreground_status_restart_and_stop_converge_across_a_fresh_generation() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialize(&directory);
    let child = command("run", state.root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("foreground starts");
    let mut child = ChildGuard(child);
    let mut probe = client(runtime.clone());
    let first = wait_ready(&mut probe);
    let first_nonce = first.readiness.expect("first readiness").boot_nonce;

    let status = output("status", state.root());
    assert!(
        status.status.success(),
        "status stderr: {:?}",
        status.stderr
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("status=ready"));

    let mut old_connection = UnixStream::connect(runtime.socket_file()).expect("old connection");
    old_connection
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let restarted = output("restart", state.root());
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    assert!(String::from_utf8_lossy(&restarted.stdout).contains("restart=ready"));
    let second = wait_ready(&mut probe);
    assert_ne!(
        second.readiness.expect("second readiness").boot_nonce,
        first_nonce
    );
    let mut terminal = [0_u8; 1];
    assert_eq!(
        old_connection
            .read(&mut terminal)
            .expect("old connection closes"),
        0
    );

    let stopped = output("stop", state.root());
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("stop=stopped"));
    wait_child(&mut child.0);
    assert!(!runtime.socket_file().exists());
    assert!(!runtime.readiness_file().exists());
}

#[test]
fn concurrent_readiness_callers_spawn_candidates_but_converge_on_one_owner() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialize(&directory);
    let first_root = state.root().to_path_buf();
    let second_root = first_root.clone();
    let first = std::thread::spawn(move || output("readiness", &first_root));
    let second = std::thread::spawn(move || output("readiness", &second_root));
    let first = first.join().expect("first caller");
    let second = second.join().expect("second caller");
    assert!(first.status.success(), "first stderr: {:?}", first.stderr);
    assert!(
        second.status.success(),
        "second stderr: {:?}",
        second.stderr
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("readiness=ready"));
    assert!(String::from_utf8_lossy(&second.stdout).contains("readiness=ready"));

    let stopped = output("stop", state.root());
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.socket_file().exists() || runtime.readiness_file().exists() {
        assert!(
            Instant::now() < deadline,
            "autostart artifacts did not clean"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
