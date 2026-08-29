//! Private Unix listener, peer, readiness, and identity-guarded cleanup contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    fs,
    num::NonZeroUsize,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    sync::Arc,
};

use hq_local_api::protocol::v1::{BuildMetadata, Id32, LifecycleState};
use hq_node::{
    CancellationToken, ComponentDrain, ComponentError, MAX_READINESS_BYTES, NodeComponent,
    NodeComponents, NodeFoundation, NodeFoundationConfig, NodeOwner, ReadinessRecord,
    RuntimeArtifactErrorClass, RuntimePaths, StateDirectoryOwner, StatePaths,
};

use support::{TestDirectory, assert_private_mode};

#[derive(Debug)]
struct ReadyComponent {
    fail_start: bool,
}

impl NodeComponent for ReadyComponent {
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        if self.fail_start {
            Err(ComponentError::unavailable())
        } else {
            Ok(())
        }
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        Ok(())
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        Ok(ComponentDrain::Complete)
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        Ok(())
    }
}

fn initialized_paths(directory: &TestDirectory) -> (StatePaths, RuntimePaths) {
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity");
    drop(owner);
    let runtime = RuntimePaths::new(directory.path().join("runtime")).expect("runtime paths");
    (state, runtime)
}

fn foundation(state: StatePaths, runtime: RuntimePaths) -> NodeFoundation {
    NodeFoundation::open(NodeFoundationConfig::new(
        state,
        runtime,
        NonZeroUsize::new(4).expect("store capacity"),
    ))
    .expect("foundation opens")
}

fn components(
    fail_first: bool,
) -> NodeComponents<ReadyComponent, ReadyComponent, ReadyComponent, ReadyComponent> {
    NodeComponents::new(
        ReadyComponent {
            fail_start: fail_first,
        },
        ReadyComponent { fail_start: false },
        ReadyComponent { fail_start: false },
        ReadyComponent { fail_start: false },
    )
}

fn build() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("runtime-tests")).expect("build metadata")
}

#[test]
fn bind_rejects_unsafe_artifacts_live_owner_and_replaces_only_proven_stale_socket() {
    let regular_directory = TestDirectory::new();
    let (regular_state, regular_runtime) = initialized_paths(&regular_directory);
    fs::create_dir_all(regular_runtime.root()).expect("runtime directory");
    fs::set_permissions(regular_runtime.root(), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    fs::write(regular_runtime.socket_file(), b"not-a-socket").expect("regular attack fixture");
    let mut regular = foundation(regular_state, regular_runtime);
    assert_eq!(
        regular
            .bind_local_listener()
            .expect_err("ordinary file is preserved")
            .class(),
        RuntimeArtifactErrorClass::UnsafeArtifact
    );

    let live_directory = TestDirectory::new();
    let (live_state, live_runtime) = initialized_paths(&live_directory);
    fs::create_dir_all(live_runtime.root()).expect("runtime directory");
    fs::set_permissions(live_runtime.root(), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    let live_listener = UnixListener::bind(live_runtime.socket_file()).expect("live listener");
    fs::set_permissions(
        live_runtime.socket_file(),
        fs::Permissions::from_mode(0o600),
    )
    .expect("private socket");
    let mut live = foundation(live_state, live_runtime);
    assert_eq!(
        live.bind_local_listener()
            .expect_err("connected listener remains owned")
            .class(),
        RuntimeArtifactErrorClass::LiveListener
    );
    drop(live_listener);

    let broad_directory = TestDirectory::new();
    let (broad_state, broad_runtime) = initialized_paths(&broad_directory);
    fs::create_dir_all(broad_runtime.root()).expect("runtime directory");
    fs::set_permissions(broad_runtime.root(), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    let broad_socket = UnixListener::bind(broad_runtime.socket_file()).expect("socket fixture");
    fs::set_permissions(
        broad_runtime.socket_file(),
        fs::Permissions::from_mode(0o666),
    )
    .expect("broad socket mode");
    drop(broad_socket);
    let mut broad = foundation(broad_state, broad_runtime.clone());
    assert_eq!(
        broad
            .bind_local_listener()
            .expect_err("broad stale socket is preserved")
            .class(),
        RuntimeArtifactErrorClass::UnsafePermissions
    );
    assert!(broad_runtime.socket_file().exists());

    let stale_directory = TestDirectory::new();
    let (stale_state, stale_runtime) = initialized_paths(&stale_directory);
    fs::create_dir_all(stale_runtime.root()).expect("runtime directory");
    fs::set_permissions(stale_runtime.root(), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    let stale_listener = UnixListener::bind(stale_runtime.socket_file()).expect("stale fixture");
    fs::set_permissions(
        stale_runtime.socket_file(),
        fs::Permissions::from_mode(0o600),
    )
    .expect("private stale socket");
    drop(stale_listener);
    let mut replacement = foundation(stale_state.clone(), stale_runtime.clone());
    replacement
        .bind_local_listener()
        .expect("proven stale socket is replaced");
    let replacement_metadata =
        fs::symlink_metadata(stale_runtime.socket_file()).expect("replacement metadata");
    assert!(replacement_metadata.file_type().is_socket());
    assert_private_mode(stale_runtime.socket_file(), 0o600);
    let replacement_client =
        UnixStream::connect(stale_runtime.socket_file()).expect("replacement accepts connections");
    let replacement_stream = replacement
        .accept_local()
        .expect("replacement owns the accepted stream");
    drop((replacement_client, replacement_stream));
    replacement.shutdown().expect("replacement cleans up");
    assert!(!stale_runtime.socket_file().exists());
    let mut rebound = foundation(stale_state, stale_runtime.clone());
    rebound
        .bind_local_listener()
        .expect("clean shutdown permits immediate rebind");
    rebound.shutdown().expect("rebound listener cleans up");
}

#[test]
fn accepted_streams_require_same_effective_user_before_protocol_bytes() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialized_paths(&directory);
    let mut foundation = foundation(state, runtime.clone());
    foundation.bind_local_listener().expect("listener binds");

    let same_user_client = UnixStream::connect(runtime.socket_file()).expect("client connects");
    let accepted = foundation.accept_local().expect("kernel reports same user");
    drop((same_user_client, accepted));
    foundation.shutdown().expect("foundation cleans up");
}

#[test]
fn readiness_is_strict_bounded_private_and_published_only_by_a_ready_node() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialized_paths(&directory);
    let foundation = foundation(state, runtime.clone());
    let mut node = NodeOwner::start(
        foundation,
        components(false),
        NonZeroUsize::new(1).expect("task capacity"),
        NonZeroUsize::new(1).expect("subscription capacity"),
    )
    .expect("node starts");
    assert_eq!(
        node.publish_readiness(build(), Id32::new([0x70; 32]))
            .expect_err("readiness requires an owned listener")
            .class(),
        RuntimeArtifactErrorClass::NotBound
    );
    assert!(!runtime.readiness_file().exists());
    node.bind_local_listener().expect("listener binds");
    let record = node
        .publish_readiness(build(), Id32::new([0x71; 32]))
        .expect("ready node publishes atomically");
    assert_eq!(record.version, 1);
    assert_eq!(record.state, LifecycleState::Ready);
    assert_eq!(record.process_id, std::process::id());
    assert_eq!(record.revision, 0);
    assert_eq!(record.boot_nonce, Id32::new([0x71; 32]));
    let bytes = fs::read(runtime.readiness_file()).expect("readiness is visible");
    assert_eq!(ReadinessRecord::decode(&bytes), Ok(record));
    assert_eq!(
        ReadinessRecord::read_from(runtime.readiness_file()),
        ReadinessRecord::decode(&bytes)
    );
    assert_private_mode(runtime.readiness_file(), 0o600);
    assert_eq!(
        ReadinessRecord::decode(&vec![b'x'; MAX_READINESS_BYTES + 1])
            .expect_err("oversize input rejected before decode")
            .class(),
        RuntimeArtifactErrorClass::ReadinessTooLarge
    );
    let oversized_file = runtime.root().join("oversized-ready.json");
    fs::write(&oversized_file, vec![b'x'; MAX_READINESS_BYTES + 1]).expect("oversized fixture");
    fs::set_permissions(&oversized_file, fs::Permissions::from_mode(0o600))
        .expect("private oversized fixture");
    assert_eq!(
        ReadinessRecord::read_from(&oversized_file)
            .expect_err("file length is rejected before allocating its body")
            .class(),
        RuntimeArtifactErrorClass::ReadinessTooLarge
    );
    let mut trailing = bytes;
    trailing.push(b' ');
    assert_eq!(
        ReadinessRecord::decode(&trailing)
            .expect_err("noncanonical trailing input fails")
            .class(),
        RuntimeArtifactErrorClass::ReadinessInvalid
    );
    node.request_restart().expect("node enters drain");
    assert_eq!(
        node.publish_readiness(build(), Id32::new([0x72; 32]))
            .expect_err("draining node cannot republish ready state")
            .class(),
        RuntimeArtifactErrorClass::ReadinessInvalid
    );
    let report = node.shutdown();
    assert!(report.issues.is_empty());
    assert!(!runtime.socket_file().exists());
    assert!(!runtime.readiness_file().exists());
}

#[test]
fn startup_rollback_and_checked_shutdown_clean_exact_owned_identities_only() {
    let rollback_directory = TestDirectory::new();
    let (rollback_state, rollback_runtime) = initialized_paths(&rollback_directory);
    let mut rollback_foundation = foundation(rollback_state.clone(), rollback_runtime.clone());
    rollback_foundation
        .bind_local_listener()
        .expect("listener binds before components");
    NodeOwner::start(
        rollback_foundation,
        components(true),
        NonZeroUsize::new(1).expect("task capacity"),
        NonZeroUsize::new(1).expect("subscription capacity"),
    )
    .expect_err("component startup fails after bind");
    assert!(!rollback_runtime.socket_file().exists());
    let rollback_owner =
        StateDirectoryOwner::acquire(rollback_state).expect("rollback releases state lock");
    drop(rollback_owner);

    let race_directory = TestDirectory::new();
    let (race_state, race_runtime) = initialized_paths(&race_directory);
    let foundation = foundation(race_state.clone(), race_runtime.clone());
    let mut node = NodeOwner::start(
        foundation,
        components(false),
        NonZeroUsize::new(1).expect("task capacity"),
        NonZeroUsize::new(1).expect("subscription capacity"),
    )
    .expect("node starts");
    node.bind_local_listener().expect("listener binds");
    let _ = node
        .publish_readiness(build(), Id32::new([0x81; 32]))
        .expect("readiness publishes");

    let moved_socket = race_runtime.root().join("moved-owned.sock");
    fs::rename(race_runtime.socket_file(), &moved_socket).expect("move owned socket");
    fs::write(race_runtime.socket_file(), b"replacement-socket-path")
        .expect("substitute socket path");
    let moved_readiness = race_runtime.root().join("moved-owned-ready.json");
    fs::rename(race_runtime.readiness_file(), &moved_readiness).expect("move owned readiness");
    fs::write(race_runtime.readiness_file(), b"replacement-readiness-path")
        .expect("substitute readiness path");
    fs::set_permissions(
        race_runtime.readiness_file(),
        fs::Permissions::from_mode(0o600),
    )
    .expect("private readiness substitute");

    assert_eq!(
        node.publish_readiness(build(), Id32::new([0x82; 32]))
            .expect_err("publication refuses a substituted owned target")
            .class(),
        RuntimeArtifactErrorClass::ArtifactChanged
    );

    let report = node.shutdown();
    assert_eq!(report.issues.len(), 1);
    assert_eq!(
        fs::read(race_runtime.socket_file()).expect("socket substitute preserved"),
        b"replacement-socket-path"
    );
    assert_eq!(
        fs::read(race_runtime.readiness_file()).expect("readiness substitute preserved"),
        b"replacement-readiness-path"
    );
    assert!(moved_socket.exists());
    assert!(moved_readiness.exists());
    let race_owner =
        StateDirectoryOwner::acquire(race_state).expect("cleanup race still releases state lock");
    drop(race_owner);
}

#[test]
fn symlink_artifacts_are_never_followed_or_removed() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let (state, runtime) = initialized_paths(&directory);
    fs::create_dir_all(runtime.root()).expect("runtime directory");
    fs::set_permissions(runtime.root(), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    let target = directory.path().join("target");
    fs::write(&target, b"preserved").expect("target fixture");
    symlink(&target, runtime.socket_file()).expect("socket symlink fixture");
    NodeFoundation::open(NodeFoundationConfig::new(
        state.clone(),
        runtime.clone(),
        NonZeroUsize::new(1).expect("store capacity"),
    ))
    .expect_err("foundation rejects reserved symlink before bind");
    assert!(
        fs::symlink_metadata(runtime.socket_file())
            .expect("symlink remains")
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(target).expect("target remains"), b"preserved");
    let state_owner =
        StateDirectoryOwner::acquire(state).expect("symlink failure releases state ownership");
    drop(state_owner);
}

#[test]
fn readiness_replacement_is_whole_and_leaves_no_temporary_file() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialized_paths(&directory);
    let foundation = foundation(state, runtime.clone());
    let mut node = NodeOwner::start(
        foundation,
        components(false),
        NonZeroUsize::new(1).expect("task capacity"),
        NonZeroUsize::new(1).expect("subscription capacity"),
    )
    .expect("node starts");
    node.bind_local_listener().expect("listener binds");
    let first = node
        .publish_readiness(build(), Id32::new([0x91; 32]))
        .expect("first readiness");
    assert_eq!(
        ReadinessRecord::decode(&fs::read(runtime.readiness_file()).expect("first file")),
        Ok(first)
    );
    let second = node
        .publish_readiness(build(), Id32::new([0x92; 32]))
        .expect("atomic replacement");
    assert_eq!(
        ReadinessRecord::decode(&fs::read(runtime.readiness_file()).expect("second file")),
        Ok(second)
    );
    assert_eq!(
        node.publish_readiness(build(), Id32::new([0x92; 32]))
            .expect_err("one boot nonce cannot name two publications")
            .class(),
        RuntimeArtifactErrorClass::ReadinessInvalid
    );
    let runtime_entries = fs::read_dir(runtime.root())
        .expect("runtime entries")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(runtime_entries.len(), 2);
    assert!(runtime_entries.iter().all(|name| {
        let name = name.to_string_lossy();
        name == "node.sock" || name == "node-ready.v1.json"
    }));
    let _ = node.shutdown();
}

#[test]
fn readiness_constructor_rejects_nonready_zero_pid_and_zero_nonce() {
    let installation_id = Id32::new([0x31; 32]);
    assert_eq!(
        ReadinessRecord::new(
            LifecycleState::Starting,
            1,
            build(),
            installation_id,
            0,
            Id32::new([0x32; 32]),
        )
        .expect_err("only acknowledged ready state publishes")
        .class(),
        RuntimeArtifactErrorClass::ReadinessInvalid
    );
    assert_eq!(
        ReadinessRecord::new(
            LifecycleState::Ready,
            0,
            build(),
            installation_id,
            0,
            Id32::new([0x32; 32]),
        )
        .expect_err("zero process identity is invalid")
        .class(),
        RuntimeArtifactErrorClass::ReadinessInvalid
    );
    assert_eq!(
        ReadinessRecord::new(
            LifecycleState::Ready,
            1,
            build(),
            installation_id,
            0,
            Id32::new([0; 32]),
        )
        .expect_err("zero boot nonce is invalid")
        .class(),
        RuntimeArtifactErrorClass::ReadinessInvalid
    );
}

#[test]
fn readiness_file_is_not_an_authority_or_listener_substitute() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialized_paths(&directory);
    fs::create_dir_all(runtime.root()).expect("runtime directory");
    fs::set_permissions(runtime.root(), fs::Permissions::from_mode(0o700))
        .expect("private runtime");
    fs::write(runtime.readiness_file(), b"{} ").expect("stale readiness fixture");
    fs::set_permissions(runtime.readiness_file(), fs::Permissions::from_mode(0o600))
        .expect("private stale readiness");
    let mut foundation = foundation(state, runtime.clone());
    foundation
        .bind_local_listener()
        .expect("stale readiness cannot block listener ownership");
    let metadata = fs::symlink_metadata(runtime.readiness_file()).expect("readiness remains");
    assert!(metadata.file_type().is_file());
    foundation.shutdown().expect("listener cleanup succeeds");
    assert!(runtime.readiness_file().exists());
}

#[test]
fn listener_owner_is_safe_to_accept_after_configuration_borrow_ends() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialized_paths(&directory);
    let mut foundation = foundation(state, runtime.clone());
    foundation.bind_local_listener().expect("listener binds");
    let shared = Arc::new(foundation);
    let client_path = runtime.socket_file().to_path_buf();
    let client = std::thread::spawn(move || UnixStream::connect(client_path).expect("connect"));
    let accepted = loop {
        match shared.accept_local() {
            Ok(stream) => break stream,
            Err(error) if error.class() == RuntimeArtifactErrorClass::WouldBlock => {
                std::thread::yield_now();
            }
            Err(error) => panic!("unexpected accept error: {error:?}"),
        }
    };
    let connected = client.join().expect("client joins");
    drop((accepted, connected));
    let foundation = Arc::try_unwrap(shared).expect("sole foundation owner");
    foundation.shutdown().expect("foundation cleanup");
}
