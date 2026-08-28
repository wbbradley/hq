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
    protocol::v1::{
        BuildMetadata, LifecycleRequest, LifecycleState, Request, ResponseResult, SnapshotItem,
    },
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

fn human_output(state_root: &Path, arguments: &[&str]) -> Output {
    offline_output(
        state_root,
        std::iter::once(OsString::from("human"))
            .chain(arguments.iter().copied().map(OsString::from)),
        None,
    )
}

fn admin_output(state_root: &Path, command: &str, arguments: &[&str]) -> Output {
    offline_output(
        state_root,
        std::iter::once(OsString::from(command))
            .chain(arguments.iter().copied().map(OsString::from)),
        None,
    )
}

fn admin_json(state_root: &Path, command: &str, arguments: &[&str]) -> serde_json::Value {
    let output = admin_output(state_root, command, arguments);
    assert!(
        output.status.success(),
        "{command} stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("admin JSON")
}

fn encode_hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .into_iter()
        .flat_map(|byte| {
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 0x0f)]),
            ]
        })
        .collect()
}

fn local_client(state: StatePaths, initial_view: InitialView) -> LocalNodeClient {
    LocalNodeClient::connect_with_launcher(
        LocalNodeClientConfig {
            state,
            build: BuildMetadata::new("hq-test", "0.1.0", Some("cli-e2e")).expect("build"),
            initial_view,
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
    .expect("local command client")
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

    let mut local = local_client(state.clone(), InitialView::Snapshot);
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
fn human_account_creation_reconciles_concurrent_callers_and_survives_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let identity = initialize_identity(&state_root);
    let installation = identity["data"]["installation_id"]
        .as_str()
        .expect("installation id")
        .to_owned();

    let first_root = state_root.clone();
    let second_root = state_root.clone();
    let first = std::thread::spawn(move || human_output(&first_root, &["create", "Personal"]));
    let second = std::thread::spawn(move || human_output(&second_root, &["create", "Personal"]));
    let first = first.join().expect("first creator");
    let second = second.join().expect("second creator");
    assert!(first.status.success(), "first stderr: {:?}", first.stderr);
    assert!(
        second.status.success(),
        "second stderr: {:?}",
        second.stderr
    );
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).expect("first human JSON");
    let second: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("second human JSON");
    assert_eq!(first["data"], second["data"]);
    let account = first["data"]["active_account"]
        .as_str()
        .expect("active account");
    assert_ne!(account, installation);
    assert_eq!(first["data"]["accounts"].as_array().map(Vec::len), Some(1));
    assert_eq!(first["data"]["accounts"][0]["account_id"], account);
    assert_eq!(
        first["data"]["accounts"][0]["creator_installation"],
        installation
    );
    assert_eq!(first["data"]["accounts"][0]["label"], "Personal");
    assert_eq!(first["data"]["accounts"][0]["selected"], true);

    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let mut client = local_client(state, InitialView::OnDemand);
    assert_eq!(
        client.snapshot().expect("authoritative snapshot").revision,
        4,
        "installation, mailbox, account, and selection are each authored exactly once"
    );

    let repeated = human_output(&state_root, &["create", "Personal"]);
    assert!(repeated.status.success());
    assert_eq!(
        client
            .snapshot()
            .expect("snapshot after reconcile")
            .revision,
        4
    );

    let changed_label = human_output(&state_root, &["create", "Work"]);
    assert!(!changed_label.status.success());
    let changed_error: serde_json::Value =
        serde_json::from_slice(&changed_label.stderr).expect("changed-label error JSON");
    assert_eq!(changed_error["data"]["code"], "human.state_unavailable");

    let unknown_id = "11".repeat(32);
    let unknown = human_output(&state_root, &["select", &unknown_id]);
    assert!(!unknown.status.success());
    assert_eq!(
        client.snapshot().expect("snapshot after refusals").revision,
        4
    );

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let shown = human_output(&state_root, &["show"]);
    assert!(shown.status.success(), "show stderr: {:?}", shown.stderr);
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).expect("shown human JSON");
    assert_eq!(shown["data"], first["data"]);

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn human_pairing_is_target_bound_replay_safe_and_survives_restart() {
    let directory = TestDirectory::new();
    let creator_root = directory.path().join("creator");
    let device_root = directory.path().join("device");
    let _creator_identity = initialize_identity(&creator_root);
    let device_identity = initialize_identity(&device_root);
    let device_id = device_identity["data"]["installation_id"]
        .as_str()
        .expect("device installation");
    let device_key = device_identity["data"]["signing_public_key"]
        .as_str()
        .expect("device signing key");
    let created = human_output(&creator_root, &["create", "Personal"]);
    assert!(
        created.status.success(),
        "create stderr: {:?}",
        created.stderr
    );
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).expect("creator JSON");
    let account = created["data"]["active_account"]
        .as_str()
        .expect("creator account")
        .to_owned();
    let invitation = directory.path().join("pairing-invitation.json");

    let invited = offline_output(
        &creator_root,
        [
            OsString::from("human"),
            OsString::from("invite"),
            OsString::from(device_id),
            OsString::from(device_key),
            invitation.clone().into_os_string(),
            OsString::from("--label"),
            OsString::from("laptop"),
            OsString::from("--relay"),
            OsString::from("wss://relay.example"),
        ],
        None,
    );
    assert!(
        invited.status.success(),
        "invite stderr: {:?}",
        invited.stderr
    );
    let invited_json: serde_json::Value =
        serde_json::from_slice(&invited.stdout).expect("invite JSON");
    assert_eq!(invited_json["kind"], "human_pairing");
    assert_eq!(invited_json["data"]["operation"], "invite");
    assert_eq!(invited_json["data"]["account_id"], account);
    assert_eq!(invited_json["data"]["device"], device_id);
    assert!(
        !String::from_utf8_lossy(&invited.stdout).contains(&invitation.display().to_string()),
        "output must not disclose the caller-selected path"
    );

    let creator_state = StatePaths::new(creator_root.clone()).expect("creator state");
    let mut creator_client = local_client(creator_state, InitialView::OnDemand);
    let invite_revision = creator_client.snapshot().expect("invite snapshot").revision;
    let repeated_invitation = directory.path().join("repeated-pairing-invitation.json");
    let repeated_invite = offline_output(
        &creator_root,
        [
            OsString::from("human"),
            OsString::from("invite"),
            OsString::from(device_id),
            OsString::from(device_key),
            repeated_invitation.clone().into_os_string(),
            OsString::from("--label"),
            OsString::from("laptop"),
            OsString::from("--relay"),
            OsString::from("wss://relay.example"),
        ],
        None,
    );
    assert!(
        repeated_invite.status.success(),
        "repeat invite stderr: {:?}",
        repeated_invite.stderr
    );
    assert_eq!(
        creator_client
            .snapshot()
            .expect("snapshot after repeat invite")
            .revision,
        invite_revision,
        "an unrevoked current grant is reused"
    );
    assert_eq!(
        fs::read(&repeated_invitation).expect("repeated invitation reads"),
        fs::read(&invitation).expect("original invitation reads")
    );

    let wrong_target = human_output(
        &creator_root,
        &["join", invitation.to_str().expect("UTF-8 path")],
    );
    assert!(!wrong_target.status.success());
    let wrong_error: serde_json::Value =
        serde_json::from_slice(&wrong_target.stderr).expect("wrong-target error JSON");
    assert_eq!(wrong_error["data"]["code"], "human.pairing_invalid");

    let joined = human_output(
        &device_root,
        &["join", invitation.to_str().expect("UTF-8 path")],
    );
    assert!(joined.status.success(), "join stderr: {:?}", joined.stderr);
    let joined_json: serde_json::Value = serde_json::from_slice(&joined.stdout).expect("join JSON");
    assert_eq!(joined_json["data"]["operation"], "join");
    assert_eq!(joined_json["data"]["account_id"], account);
    assert_eq!(joined_json["data"]["device"], device_id);

    let device_state = StatePaths::new(device_root.clone()).expect("device state");
    let mut device_client = local_client(device_state, InitialView::OnDemand);
    let joined_revision = device_client.snapshot().expect("joined snapshot").revision;
    let repeated = human_output(
        &device_root,
        &["join", invitation.to_str().expect("UTF-8 path")],
    );
    assert!(
        repeated.status.success(),
        "repeat join stderr: {:?}",
        repeated.stderr
    );
    assert_eq!(
        device_client
            .snapshot()
            .expect("snapshot after repeated join")
            .revision,
        joined_revision,
        "byte-identical evidence, acceptance, and selection are no-ops"
    );

    let device_listing = human_output(&device_root, &["devices"]);
    assert!(
        device_listing.status.success(),
        "device listing stderr: {:?}",
        device_listing.stderr
    );
    let device_listing: serde_json::Value =
        serde_json::from_slice(&device_listing.stdout).expect("device listing JSON");
    assert_eq!(device_listing["kind"], "human_devices");
    let joined_device = device_listing["data"]["devices"]
        .as_array()
        .expect("devices array")
        .iter()
        .find(|device| device["installation_id"] == device_id)
        .expect("joined device");
    assert_eq!(joined_device["state"], "active");
    assert_eq!(
        joined_device["acceptances"]
            .as_array()
            .expect("acceptances")
            .len(),
        1
    );

    let non_creator_revoke = human_output(&device_root, &["revoke", device_id]);
    assert!(!non_creator_revoke.status.success());
    let non_creator_error: serde_json::Value =
        serde_json::from_slice(&non_creator_revoke.stderr).expect("non-creator error JSON");
    assert_eq!(non_creator_error["data"]["code"], "human.state_unavailable");

    let before_revoke = creator_client
        .snapshot()
        .expect("snapshot before revoke")
        .revision;
    let revoked = human_output(&creator_root, &["revoke", device_id]);
    assert!(
        revoked.status.success(),
        "creator revoke stderr: {:?}",
        revoked.stderr
    );
    let revoked_json: serde_json::Value =
        serde_json::from_slice(&revoked.stdout).expect("revoked device JSON");
    let revoked_device = revoked_json["data"]["devices"]
        .as_array()
        .expect("devices array")
        .iter()
        .find(|device| device["installation_id"] == device_id)
        .expect("revoked device");
    assert_eq!(revoked_device["state"], "revoked");
    assert_eq!(
        revoked_device["revokes"].as_array().expect("revokes").len(),
        1
    );
    let revoked_revision = creator_client
        .snapshot()
        .expect("snapshot after revoke")
        .revision;
    assert_eq!(revoked_revision, before_revoke + 1);
    let repeated_revoke = human_output(&creator_root, &["revoke", device_id]);
    assert!(
        repeated_revoke.status.success(),
        "repeat revoke stderr: {:?}",
        repeated_revoke.stderr
    );
    assert_eq!(
        creator_client
            .snapshot()
            .expect("snapshot after repeat revoke")
            .revision,
        revoked_revision,
        "repeat revoke is a semantic no-op"
    );

    let tampered_path = directory.path().join("tampered-invitation.json");
    let mut tampered = fs::read(&invitation).expect("invitation reads");
    let byte = tampered
        .iter_mut()
        .find(|byte| **byte == b'a')
        .expect("fixture contains a mutable byte");
    *byte = b'b';
    fs::write(&tampered_path, tampered).expect("tampered fixture writes");
    let rejected = human_output(
        &device_root,
        &["join", tampered_path.to_str().expect("UTF-8 path")],
    );
    assert!(!rejected.status.success());
    assert_eq!(
        device_client
            .snapshot()
            .expect("snapshot after tamper")
            .revision,
        joined_revision
    );

    let restarted = output("restart", &device_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let shown = human_output(&device_root, &["show"]);
    assert!(shown.status.success(), "show stderr: {:?}", shown.stderr);
    let shown: serde_json::Value =
        serde_json::from_slice(&shown.stdout).expect("joined human JSON");
    assert_eq!(shown["data"]["active_account"], account);

    let creator_restarted = output("restart", &creator_root);
    assert!(
        creator_restarted.status.success(),
        "creator restart stderr: {:?}",
        creator_restarted.stderr
    );
    let persisted = human_output(&creator_root, &["devices"]);
    assert!(
        persisted.status.success(),
        "persisted devices stderr: {:?}",
        persisted.stderr
    );
    let persisted: serde_json::Value =
        serde_json::from_slice(&persisted.stdout).expect("persisted device JSON");
    assert!(
        persisted["data"]["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["installation_id"] == device_id && device["state"] == "revoked")
    );

    for root in [&creator_root, &device_root] {
        let stopped = output("stop", root);
        assert!(
            stopped.status.success(),
            "stop stderr: {:?}",
            stopped.stderr
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn directional_peer_and_mailbox_authority_is_replay_safe_and_recovers_after_distrust() {
    let directory = TestDirectory::new();
    let owner_root = directory.path().join("owner");
    let peer_root = directory.path().join("peer");
    let owner_identity = initialize_identity(&owner_root);
    let peer_identity = initialize_identity(&peer_root);
    let owner_id = owner_identity["data"]["installation_id"]
        .as_str()
        .expect("owner installation");
    let peer_id = peer_identity["data"]["installation_id"]
        .as_str()
        .expect("peer installation");
    let peer_signing_key = peer_identity["data"]["signing_public_key"]
        .as_str()
        .expect("peer signing key");

    let peer_state = StatePaths::new(peer_root.clone()).expect("peer state");
    let mut peer_client = local_client(peer_state, InitialView::OnDemand);
    let peer_snapshot = peer_client.snapshot().expect("peer snapshot");
    let peer_encryption_key = peer_snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Installation { encryption_key, .. } => {
                Some(encode_hex(encryption_key.bytes()))
            }
            _ => None,
        })
        .expect("peer encryption key");

    let created = human_output(&owner_root, &["create", "Personal"]);
    assert!(
        created.status.success(),
        "create stderr: {:?}",
        created.stderr
    );
    let mailboxes = admin_output(&owner_root, "mailbox", &["list"]);
    assert!(
        mailboxes.status.success(),
        "mailbox list stderr: {:?}",
        mailboxes.stderr
    );
    let mailboxes: serde_json::Value =
        serde_json::from_slice(&mailboxes.stdout).expect("mailbox JSON");
    let mailbox_id = mailboxes["data"]["mailboxes"][0]["mailbox_id"]
        .as_str()
        .expect("local mailbox");

    let added = admin_output(
        &owner_root,
        "peer",
        &[
            "add",
            peer_id,
            peer_signing_key,
            &peer_encryption_key,
            "--label",
            "collaborator",
            "--relay",
            "wss://relay.example",
        ],
    );
    assert!(
        added.status.success(),
        "peer add stderr: {:?}",
        added.stderr
    );
    let added: serde_json::Value = serde_json::from_slice(&added.stdout).expect("peer JSON");
    assert_eq!(added["kind"], "authority_admin");
    assert_eq!(added["data"]["peers"][0]["peer"], peer_id);
    assert_eq!(added["data"]["peers"][0]["state"], "routable");
    assert_eq!(
        added["data"]["peers"][0]["routes"][0]["relay_hints"][0]["value"],
        "wss://relay.example"
    );

    let owner_state = StatePaths::new(owner_root.clone()).expect("owner state");
    let mut owner_client = local_client(owner_state, InitialView::OnDemand);
    let added_revision = owner_client.snapshot().expect("added snapshot").revision;
    let repeated_add = admin_output(
        &owner_root,
        "peer",
        &[
            "add",
            peer_id,
            peer_signing_key,
            &peer_encryption_key,
            "--label",
            "collaborator",
            "--relay",
            "wss://relay.example",
        ],
    );
    assert!(repeated_add.status.success());
    assert_eq!(
        owner_client
            .snapshot()
            .expect("repeated add snapshot")
            .revision,
        added_revision,
        "an exact current route is reused"
    );

    let granted = admin_output(&owner_root, "mailbox", &["grant", mailbox_id, peer_id]);
    assert!(
        granted.status.success(),
        "mailbox grant stderr: {:?}",
        granted.stderr
    );
    let granted: serde_json::Value = serde_json::from_slice(&granted.stdout).expect("grant JSON");
    assert_eq!(granted["data"]["capabilities"][0]["active"], true);
    assert_eq!(
        granted["data"]["capabilities"][0]["grantee_installation"],
        peer_id
    );
    assert_eq!(
        granted["data"]["capabilities"][0]["grantee_signing_key"],
        peer_signing_key
    );
    let granted_revision = owner_client.snapshot().expect("grant snapshot").revision;
    let repeated_grant = admin_output(&owner_root, "mailbox", &["grant", mailbox_id, peer_id]);
    assert!(repeated_grant.status.success());
    assert_eq!(
        owner_client
            .snapshot()
            .expect("repeated grant snapshot")
            .revision,
        granted_revision,
        "an exact active capability is reused"
    );

    let distrusted = admin_output(&owner_root, "peer", &["distrust", peer_id]);
    assert!(
        distrusted.status.success(),
        "distrust stderr: {:?}",
        distrusted.stderr
    );
    let distrusted: serde_json::Value =
        serde_json::from_slice(&distrusted.stdout).expect("distrust JSON");
    assert_eq!(distrusted["data"]["peers"][0]["state"], "blocked");
    assert_eq!(distrusted["data"]["capabilities"][0]["active"], false);
    let distrusted_revision = owner_client.snapshot().expect("distrust snapshot").revision;
    assert_eq!(
        distrusted_revision,
        granted_revision + 2,
        "distrust revokes the capability before authoring the route block"
    );
    let repeated_distrust = admin_output(&owner_root, "peer", &["distrust", peer_id]);
    assert!(repeated_distrust.status.success());
    assert_eq!(
        owner_client
            .snapshot()
            .expect("repeated distrust snapshot")
            .revision,
        distrusted_revision
    );

    let recovered = admin_output(
        &owner_root,
        "peer",
        &[
            "add",
            peer_id,
            peer_signing_key,
            &peer_encryption_key,
            "--label",
            "collaborator",
            "--relay",
            "wss://relay.example",
        ],
    );
    assert!(
        recovered.status.success(),
        "recovery stderr: {:?}",
        recovered.stderr
    );
    let recovered: serde_json::Value =
        serde_json::from_slice(&recovered.stdout).expect("recovery JSON");
    assert_eq!(recovered["data"]["peers"][0]["state"], "routable");
    assert_eq!(
        recovered["data"]["peers"][0]["blocks"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        recovered["data"]["peers"][0]["routes"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let regranted = admin_output(&owner_root, "mailbox", &["grant", mailbox_id, peer_id]);
    assert!(
        regranted.status.success(),
        "regrant stderr: {:?}",
        regranted.stderr
    );
    let regranted: serde_json::Value =
        serde_json::from_slice(&regranted.stdout).expect("regrant JSON");
    let capabilities = regranted["data"]["capabilities"]
        .as_array()
        .expect("capabilities");
    assert_eq!(capabilities.len(), 2);
    assert_eq!(
        capabilities
            .iter()
            .filter(|capability| capability["active"] == true)
            .count(),
        1
    );

    let unauthorized = admin_output(&peer_root, "mailbox", &["grant", mailbox_id, owner_id]);
    assert!(!unauthorized.status.success());
    let unauthorized: serde_json::Value =
        serde_json::from_slice(&unauthorized.stderr).expect("authority error JSON");
    assert_eq!(unauthorized["data"]["code"], "authority.state_unavailable");

    let restarted = output("restart", &owner_root);
    assert!(
        restarted.status.success(),
        "owner restart stderr: {:?}",
        restarted.stderr
    );
    let persisted = admin_output(&owner_root, "mailbox", &["list"]);
    assert!(persisted.status.success());
    let persisted: serde_json::Value =
        serde_json::from_slice(&persisted.stdout).expect("persisted authority JSON");
    assert_eq!(
        persisted["data"]["capabilities"]
            .as_array()
            .expect("persisted capabilities")
            .iter()
            .filter(|capability| capability["active"] == true)
            .count(),
        1
    );

    for root in [&owner_root, &peer_root] {
        let stopped = output("stop", root);
        assert!(
            stopped.status.success(),
            "stop stderr: {:?}",
            stopped.stderr
        );
    }
}

#[test]
fn relay_administration_is_idempotent_redacted_and_restart_durable() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let _identity = initialize_identity(&state_root);
    let endpoint = "ws://127.0.0.1:9";

    let initial = admin_json(&state_root, "relay", &["status"]);
    assert_eq!(initial["kind"], "relay_admin");
    assert_eq!(initial["data"]["policies"], serde_json::json!([]));
    assert_eq!(initial["data"]["domains"].as_array().map(Vec::len), Some(4));

    let added = admin_json(
        &state_root,
        "relay",
        &["add", endpoint, "--access", "read", "--auth", "required"],
    );
    assert_eq!(added["data"]["outcome"], "accepted");
    assert_eq!(
        added["data"]["operation_id"].as_str().map(str::len),
        Some(64)
    );
    assert_eq!(added["data"]["policies"][0]["endpoint"], endpoint);
    assert_eq!(added["data"]["policies"][0]["access"], "read");
    assert_eq!(added["data"]["policies"][0]["authentication"], "required");
    assert_eq!(added["data"]["policies"][0]["enabled"], true);
    assert_eq!(added["data"]["policies"][0]["generation"], 1);

    let repeated = admin_json(
        &state_root,
        "relay",
        &["add", endpoint, "--access", "read", "--auth", "required"],
    );
    assert_eq!(repeated["data"]["outcome"], "unchanged");
    assert_eq!(repeated["data"]["policies"][0]["generation"], 1);

    let synchronized = admin_json(&state_root, "relay", &["sync", endpoint]);
    assert_eq!(synchronized["data"]["outcome"], "accepted");

    let repaired = admin_json(&state_root, "relay", &["repair"]);
    assert_eq!(repaired["data"]["outcome"], "repaired");
    assert_eq!(
        repaired["data"]["operation_id"].as_str().map(str::len),
        Some(64)
    );

    let removed = admin_json(&state_root, "relay", &["remove", endpoint]);
    assert_eq!(removed["data"]["policies"][0]["enabled"], false);
    assert_eq!(removed["data"]["policies"][0]["generation"], 2);

    let disabled_sync = admin_json(&state_root, "relay", &["sync", endpoint]);
    assert_eq!(disabled_sync["data"]["outcome"], "rejected");
    assert_eq!(
        disabled_sync["data"]["operation_id"].as_str().map(str::len),
        Some(64)
    );

    let repeated_remove = admin_json(&state_root, "relay", &["remove", endpoint]);
    assert_eq!(repeated_remove["data"]["outcome"], "unchanged");
    assert_eq!(repeated_remove["data"]["policies"][0]["generation"], 2);

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let persisted = admin_json(&state_root, "relay", &["list"]);
    assert_eq!(persisted["data"]["policies"][0]["endpoint"], endpoint);
    assert_eq!(persisted["data"]["policies"][0]["enabled"], false);
    assert_eq!(persisted["data"]["policies"][0]["generation"], 2);

    let secret_endpoint = "ws://secret@127.0.0.1:9";
    let invalid = admin_output(&state_root, "relay", &["add", secret_endpoint]);
    assert!(!invalid.status.success());
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains(secret_endpoint));

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
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
