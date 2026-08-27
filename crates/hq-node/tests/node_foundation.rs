//! Secure runtime-path, lifecycle, and RAII node-foundation contracts.

#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{fs, num::NonZeroUsize, path::PathBuf};

use hq_domain::InstallationId;
use hq_node::{
    NodeAdmission, NodeFoundation, NodeFoundationConfig, NodeLifecycle, NodePhase,
    NodeTransitionOutcome, OperatorAction, RuntimeDirectoryOwner, RuntimePathErrorClass,
    RuntimePaths, ShutdownIntent, StartupCause, StartupComponent, StartupDiagnostic,
    StateDirectoryOwner, StatePaths,
};

use support::{TestDirectory, assert_private_mode};

fn initialized_state(directory: &TestDirectory) -> StatePaths {
    let paths = StatePaths::new(directory.path().join("state")).expect("absolute state root");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity initializes");
    drop(owner);
    paths
}

fn runtime_paths(directory: &TestDirectory) -> RuntimePaths {
    RuntimePaths::new(directory.path().join("runtime")).expect("short absolute runtime root")
}

#[test]
fn runtime_paths_are_private_installation_qualified_and_portably_bounded() {
    let directory = TestDirectory::new();
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let installation = InstallationId::from_bytes([0xabu8; 32]);
    let derived = RuntimePaths::derive(Some(directory.path()), state.root(), installation)
        .expect("derived runtime paths");
    assert!(derived.root().starts_with(directory.path().join("hq")));
    assert!(derived.root().ends_with("abababababababababababab"));
    assert_eq!(derived.socket_file(), derived.root().join("node.sock"));
    assert_eq!(
        derived.readiness_file(),
        derived.root().join("node-ready.v1.json")
    );

    let fallback = RuntimePaths::derive(None, state.root(), installation)
        .expect("state-local runtime fallback");
    assert_eq!(fallback.root(), state.root().join("runtime"));

    let owner = RuntimeDirectoryOwner::prepare(derived.clone()).expect("runtime prepares");
    assert_eq!(owner.paths(), &derived);
    assert_private_mode(derived.root(), 0o700);

    let long = PathBuf::from("/").join("x".repeat(200));
    assert_eq!(
        RuntimePaths::new(long).map_err(hq_node::RuntimePathError::class),
        Err(RuntimePathErrorClass::SocketPathTooLong)
    );
}

#[cfg(unix)]
#[test]
fn runtime_path_security_rejects_symlinks_and_broad_permissions_without_cleanup() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = TestDirectory::new();
    let broad_root = directory.path().join("broad-runtime");
    fs::create_dir(&broad_root).expect("runtime fixture");
    fs::set_permissions(&broad_root, fs::Permissions::from_mode(0o755)).expect("broad mode");
    let broad = RuntimePaths::new(broad_root).expect("paths construct");
    assert_eq!(
        RuntimeDirectoryOwner::prepare(broad)
            .expect_err("broad permissions fail")
            .class(),
        RuntimePathErrorClass::UnsafePermissions
    );

    let target = directory.path().join("real-runtime");
    fs::create_dir(&target).expect("target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).expect("private target");
    let link = directory.path().join("linked-runtime");
    symlink(&target, &link).expect("runtime symlink");
    let linked = RuntimePaths::new(link).expect("paths construct");
    assert_eq!(
        RuntimeDirectoryOwner::prepare(linked)
            .expect_err("symlink fails")
            .class(),
        RuntimePathErrorClass::SymbolicLink
    );

    let private_root = directory.path().join("private-runtime");
    fs::create_dir(&private_root).expect("private runtime");
    fs::set_permissions(&private_root, fs::Permissions::from_mode(0o700)).expect("private mode");
    let stale_socket = private_root.join("node.sock");
    fs::write(&stale_socket, b"not-owned-by-this-foundation").expect("stale artifact");
    let paths = RuntimePaths::new(private_root).expect("runtime paths");
    let _ = RuntimeDirectoryOwner::prepare(paths).expect("ordinary stale artifact is preserved");
    assert_eq!(
        fs::read(stale_socket).expect("stale artifact remains"),
        b"not-owned-by-this-foundation"
    );
}

#[test]
fn lifecycle_admission_and_order_are_explicit_and_idempotent() {
    let mut lifecycle = NodeLifecycle::new();
    assert_eq!(lifecycle.phase(), NodePhase::Starting);
    assert!(!lifecycle.admits(NodeAdmission::Mutation));
    assert!(lifecycle.admits(NodeAdmission::Status));

    assert_eq!(
        lifecycle.mark_ready(7).expect("ready transition"),
        NodeTransitionOutcome::Changed
    );
    assert_eq!(lifecycle.phase(), NodePhase::Ready);
    assert_eq!(lifecycle.revision(), Some(7));
    assert!(lifecycle.admits(NodeAdmission::Mutation));
    assert!(lifecycle.admits(NodeAdmission::Launch));

    assert_eq!(
        lifecycle.begin_drain().expect("drain"),
        NodeTransitionOutcome::Changed
    );
    assert_eq!(
        lifecycle.begin_drain().expect("idempotent drain"),
        NodeTransitionOutcome::Unchanged
    );
    assert!(!lifecycle.admits(NodeAdmission::Mutation));
    assert!(!lifecycle.admits(NodeAdmission::Launch));
    assert!(lifecycle.admits(NodeAdmission::Status));
    assert!(lifecycle.mark_ready(8).is_err());

    assert_eq!(
        lifecycle
            .acknowledge_stopped()
            .expect("stop acknowledgement"),
        NodeTransitionOutcome::Changed
    );
    assert_eq!(lifecycle.phase(), NodePhase::Stopped);
    assert_eq!(
        lifecycle.acknowledge_stopped().expect("idempotent stop"),
        NodeTransitionOutcome::Unchanged
    );

    let mut restart = NodeLifecycle::new();
    assert!(restart.begin_restart().is_err());
    restart.mark_ready(9).expect("ready before restart");
    assert_eq!(
        restart.begin_restart().expect("restart begins drain"),
        NodeTransitionOutcome::Changed
    );
    assert_eq!(restart.phase(), NodePhase::Draining);
    assert_eq!(restart.shutdown_intent(), Some(ShutdownIntent::Restart));
    assert_eq!(
        restart.begin_restart().expect("restart is idempotent"),
        NodeTransitionOutcome::Unchanged
    );

    let mut failed = NodeLifecycle::new();
    let diagnostic = StartupDiagnostic::new(
        StartupComponent::Runtime,
        StartupCause::Unavailable,
        PathBuf::from("/state"),
        PathBuf::from("/runtime"),
    );
    assert_eq!(diagnostic.action(), OperatorAction::Retry);
    failed
        .mark_failed(diagnostic.clone())
        .expect("failure records");
    assert_eq!(failed.phase(), NodePhase::Failed);
    assert_eq!(failed.failure(), Some(&diagnostic));
    assert!(!failed.admits(NodeAdmission::Query));
    assert!(failed.admits(NodeAdmission::Status));
    assert!(failed.begin_restart().is_err());
}

#[test]
fn concurrent_foundations_fail_with_actionable_redacted_ownership_and_release_exactly() {
    let directory = TestDirectory::new();
    let state = initialized_state(&directory);
    let runtime = runtime_paths(&directory);
    let config = NodeFoundationConfig::new(
        state.clone(),
        runtime.clone(),
        NonZeroUsize::new(8).expect("capacity"),
    );
    let mut first = NodeFoundation::open(config.clone()).expect("first node owns state");
    first.mark_ready().expect("store-backed readiness");
    assert_eq!(first.lifecycle().phase(), NodePhase::Ready);
    assert!(first.admits(NodeAdmission::Mutation));

    let conflict = NodeFoundation::open(config.clone()).expect_err("second owner is rejected");
    assert_eq!(
        conflict.diagnostic().component(),
        StartupComponent::StateOwnership
    );
    assert_eq!(conflict.diagnostic().cause(), StartupCause::AlreadyOwned);
    assert!(!format!("{conflict:?}").contains("secret"));

    first.begin_drain().expect("drain begins");
    assert!(!first.admits(NodeAdmission::Mutation));
    first.shutdown().expect("checked shutdown");

    let second = NodeFoundation::open(config).expect("ownership released immediately");
    second.shutdown().expect("second shutdown");
    let state_owner = StateDirectoryOwner::acquire(state).expect("raw owner reacquires after node");
    drop(state_owner);
    assert_private_mode(runtime.root(), 0o700);
}

#[test]
fn every_startup_failure_unwinds_the_state_lock_for_immediate_retry() {
    let directory = TestDirectory::new();
    let missing_state =
        StatePaths::new(directory.path().join("missing-identity")).expect("state paths");
    let runtime = runtime_paths(&directory);
    let missing = NodeFoundationConfig::new(
        missing_state.clone(),
        runtime.clone(),
        NonZeroUsize::new(1).expect("capacity"),
    );
    let error = NodeFoundation::open(missing).expect_err("identity is required");
    assert_eq!(error.diagnostic().component(), StartupComponent::Identity);
    assert_eq!(error.diagnostic().cause(), StartupCause::Missing);
    let owner = StateDirectoryOwner::acquire(missing_state).expect("failed startup released lock");
    drop(owner);

    let state = initialized_state(&directory);
    fs::create_dir_all(runtime.root()).expect("runtime fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(runtime.root(), fs::Permissions::from_mode(0o755))
            .expect("unsafe runtime mode");
    }
    let unsafe_runtime = NodeFoundationConfig::new(
        state.clone(),
        runtime,
        NonZeroUsize::new(1).expect("capacity"),
    );
    let error = NodeFoundation::open(unsafe_runtime).expect_err("unsafe runtime rejected");
    assert_eq!(error.diagnostic().component(), StartupComponent::Runtime);
    let owner = StateDirectoryOwner::acquire(state).expect("runtime failure released lock");
    drop(owner);

    let store_directory = TestDirectory::new();
    let store_state = initialized_state(&store_directory);
    fs::create_dir(store_state.database_file()).expect("invalid database artifact");
    let store_failure = NodeFoundationConfig::new(
        store_state.clone(),
        runtime_paths(&store_directory),
        NonZeroUsize::new(1).expect("capacity"),
    );
    let error = NodeFoundation::open(store_failure).expect_err("store open fails");
    assert_eq!(error.diagnostic().component(), StartupComponent::Store);
    let owner =
        StateDirectoryOwner::acquire(store_state).expect("store failure released state lock");
    drop(owner);
}
