//! End-to-end single-binary node lifecycle contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    num::NonZeroUsize,
    os::unix::net::UnixStream,
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use hq_local_api::{
    ClientEvent, InitialView,
    protocol::v1::{BuildMetadata, LifecycleRequest, LifecycleState, Request, ResponseResult},
};
use hq_node::{
    LifecycleClient, LifecycleClientConfig, LocalNodeClient, LocalNodeClientConfig,
    ProcessNodeLauncher, RuntimePaths, StateDirectoryOwner, StatePaths,
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
        .arg("--state-root")
        .arg(state_root)
        .arg("daemon")
        .arg(action);
    command
}

fn output(action: &str, state_root: &Path) -> Output {
    command(action, state_root)
        .output()
        .expect("CLI process runs")
}

fn machine_output(action: &str, state_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(state_root)
        .arg("daemon")
        .arg(action)
        .stdin(Stdio::null())
        .output()
        .expect("non-interactive CLI process runs")
}

fn offline_output(
    state_root: &Path,
    arguments: impl IntoIterator<Item = OsString>,
    input: Option<&[u8]>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(state_root)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn().expect("offline CLI process starts");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input)
            .expect("secret input writes");
    }
    child.wait_with_output().expect("offline CLI process exits")
}

fn initialize_identity(state_root: &Path) -> serde_json::Value {
    let initialized = offline_output(
        state_root,
        [OsString::from("identity"), OsString::from("init")],
        None,
    );
    assert!(
        initialized.status.success(),
        "identity init stderr: {:?}",
        initialized.stderr
    );
    let value: serde_json::Value =
        serde_json::from_slice(&initialized.stdout).expect("identity JSON");
    assert_eq!(value["kind"], "identity");
    assert_eq!(
        value["data"]["installation_id"].as_str().map(str::len),
        Some(64)
    );
    assert_eq!(
        value["data"]["signing_public_key"].as_str().map(str::len),
        Some(64)
    );
    value
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
    assert_eq!(first.status.revision, Some(1));
    let first_nonce = first.readiness.expect("first readiness").boot_nonce;

    let status = output("status", state.root());
    assert!(
        status.status.success(),
        "status stderr: {:?}",
        status.stderr
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("status=ready"));
    let machine = machine_output("status", state.root());
    assert!(
        machine.status.success(),
        "machine stderr: {:?}",
        machine.stderr
    );
    assert!(machine.stderr.is_empty());
    let record: serde_json::Value =
        serde_json::from_slice(&machine.stdout).expect("machine lifecycle record");
    assert_eq!(record["schema"], "hq-cli-output-v1");
    assert_eq!(record["ok"], true);
    assert_eq!(record["kind"], "lifecycle");
    assert_eq!(record["data"]["command"], "status");
    assert_eq!(record["data"]["state"], "ready");

    let mut local = LocalNodeClient::connect_with_launcher(
        LocalNodeClientConfig {
            state: state.clone(),
            build: BuildMetadata::new("hq-test", "0.1.0", Some("cli-e2e")).expect("build"),
            initial_view: InitialView::Snapshot,
            io_timeout: Duration::from_secs(2),
            command_deadline: Duration::from_secs(5),
            max_connection_attempts: NonZeroUsize::new(8).expect("positive attempts"),
            readiness_timeout: Duration::from_secs(5),
            readiness_retry_interval: Duration::from_millis(10),
            reconnect_initial: Duration::from_millis(10),
            reconnect_maximum: Duration::from_millis(40),
            completed_identity_capacity: NonZeroUsize::new(16).expect("positive history"),
        },
        ProcessNodeLauncher::new(env!("CARGO_BIN_EXE_hq").into()),
    )
    .expect("local command client");
    let local_status = local
        .request(Request::Lifecycle(LifecycleRequest::Status))
        .expect("local API status");
    assert!(
        matches!(
            local_status,
            ClientEvent::Response {
            result: ResponseResult::Lifecycle(ref status),
                ..
            } if status.state == LifecycleState::Ready
        ),
        "unexpected local status: {local_status:?}"
    );

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
    assert_eq!(second.status.revision, Some(1));
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
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("stopped intent=stopped"));
    wait_child(&mut child.0);
    assert!(!runtime.socket_file().exists());
    assert!(!runtime.readiness_file().exists());
}

#[test]
fn identity_backup_restore_is_noninteractive_redacted_and_does_not_copy_configuration() {
    let directory = TestDirectory::new();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let backup = directory.path().join("identity-backup.json");
    let unused_backup = directory.path().join("unused-backup.json");
    let password = b"correct horse battery staple\n";

    let initialized = initialize_identity(&source);

    let configured = offline_output(
        &source,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("default-provider"),
            OsString::from("codex"),
        ],
        None,
    );
    assert!(configured.status.success());

    let exported = offline_output(
        &source,
        [
            OsString::from("identity"),
            OsString::from("export"),
            backup.clone().into_os_string(),
            OsString::from("--password-stdin"),
        ],
        Some(password),
    );
    assert!(
        exported.status.success(),
        "export stderr: {:?}",
        exported.stderr
    );
    assert!(!String::from_utf8_lossy(&exported.stdout).contains("correct horse"));
    assert!(!String::from_utf8_lossy(&exported.stderr).contains("correct horse"));

    let closed = offline_output(
        &source,
        [
            OsString::from("identity"),
            OsString::from("export"),
            unused_backup.into_os_string(),
            OsString::from("--password-stdin"),
        ],
        None,
    );
    assert_eq!(closed.status.code(), Some(2));
    let closed_error: serde_json::Value =
        serde_json::from_slice(&closed.stderr).expect("closed-stdin error JSON");
    assert_eq!(closed_error["data"]["code"], "identity.secret_input");

    let wrong = offline_output(
        &target,
        [
            OsString::from("identity"),
            OsString::from("import"),
            backup.clone().into_os_string(),
            OsString::from("--password-stdin"),
        ],
        Some(b"wrong password\n"),
    );
    assert!(!wrong.status.success());
    assert!(!String::from_utf8_lossy(&wrong.stderr).contains("wrong password"));

    let imported = offline_output(
        &target,
        [
            OsString::from("identity"),
            OsString::from("import"),
            backup.into_os_string(),
            OsString::from("--password-stdin"),
        ],
        Some(password),
    );
    assert!(
        imported.status.success(),
        "import stderr: {:?}",
        imported.stderr
    );
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.stdout).expect("import identity JSON");
    assert_eq!(imported["data"], initialized["data"]);

    let overwrite = offline_output(
        &target,
        [OsString::from("identity"), OsString::from("init")],
        None,
    );
    assert!(!overwrite.status.success());
    let target_config = offline_output(
        &target,
        [OsString::from("config"), OsString::from("get")],
        None,
    );
    let target_config: serde_json::Value =
        serde_json::from_slice(&target_config.stdout).expect("default configuration JSON");
    assert_eq!(
        target_config["data"]["default_provider"],
        serde_json::Value::Null
    );
    assert_eq!(target_config["data"]["relays"], serde_json::json!([]));
}

#[test]
fn typed_configuration_is_canonical_revalidated_and_refuses_a_live_owner() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let _ = initialize_identity(&state_root);
    let provider = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("default-provider"),
            OsString::from("codex"),
        ],
        None,
    );
    assert!(
        provider.status.success(),
        "provider stderr: {:?}",
        provider.stderr
    );
    let relays = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("relays"),
            OsString::from("wss://z.example"),
            OsString::from("wss://a.example"),
        ],
        None,
    );
    assert!(
        relays.status.success(),
        "relays stderr: {:?}",
        relays.stderr
    );
    let relays: serde_json::Value =
        serde_json::from_slice(&relays.stdout).expect("configuration JSON");
    assert_eq!(relays["kind"], "configuration");
    assert_eq!(relays["data"]["default_provider"], "codex");
    assert_eq!(
        relays["data"]["relays"],
        serde_json::json!(["wss://a.example", "wss://z.example"])
    );

    let duplicate = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("relays"),
            OsString::from("wss://a.example"),
            OsString::from("wss://a.example"),
        ],
        None,
    );
    assert!(!duplicate.status.success());
    let preserved = offline_output(
        &state_root,
        [OsString::from("config"), OsString::from("get")],
        None,
    );
    let preserved: serde_json::Value =
        serde_json::from_slice(&preserved.stdout).expect("preserved configuration JSON");
    assert_eq!(preserved["data"], relays["data"]);

    let paths = StatePaths::new(state_root.clone()).expect("state paths");
    let live_owner = StateDirectoryOwner::acquire(paths).expect("test owns state");
    let refused = offline_output(
        &state_root,
        [OsString::from("identity"), OsString::from("show")],
        None,
    );
    assert!(!refused.status.success());
    drop(live_owner);
}

#[test]
fn startup_refuses_a_persisted_root_that_disagrees_with_the_owned_identity() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let other_root = directory.path().join("other");
    for root in [&state_root, &other_root] {
        let initialized = offline_output(
            root,
            [OsString::from("identity"), OsString::from("init")],
            None,
        );
        assert!(
            initialized.status.success(),
            "init stderr: {:?}",
            initialized.stderr
        );
    }

    let ready = output("readiness", &state_root);
    assert!(
        ready.status.success(),
        "readiness stderr: {:?}",
        ready.stderr
    );
    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );

    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(owner) = StateDirectoryOwner::acquire(state.clone()) {
            drop(owner);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stopped node did not release state ownership"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let other = StatePaths::new(other_root).expect("other state paths");
    let replacement = fs::read(other.identity_file()).expect("other identity reads");
    fs::write(state.identity_file(), replacement).expect("identity fixture is replaced");

    let mismatch = command("run", &state_root)
        .stdin(Stdio::null())
        .output()
        .expect("mismatched foreground exits");
    assert!(!mismatch.status.success());
    let stderr = String::from_utf8_lossy(&mismatch.stderr);
    assert!(stderr.contains("node.foreground_failed"));
    assert!(!stderr.contains("signing"));
    assert!(!stderr.contains("secret"));
    let owner = StateDirectoryOwner::acquire(state).expect("failed startup releases ownership");
    drop(owner);
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
