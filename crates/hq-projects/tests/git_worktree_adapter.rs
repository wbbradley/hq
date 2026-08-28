//! Real-Git contracts for exact worktree lookup and creation.

#![allow(clippy::expect_used)]

use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use hq_application::{EffectOutcome, EffectRequest};
use hq_domain::{
    BoundedText, CommandDigest, OperationId, ResourceLocator, ResourceScheme, ShortText, Timestamp,
};
use hq_projects::{
    GitWorktreeAdapter, GitWorktreeAdapterConfig, GitWorktreePort, GitWorktreeRequest,
    GitWorktreeState,
};
use hq_resources::{ExecGit, GitCommandConfig};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hq-project-worktree-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("test directory creates");
        Self(path)
    }

    fn join(&self, child: &str) -> PathBuf {
        self.0.join(child)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .expect("git starts");
    assert!(
        output.status.success(),
        "git {arguments:?} failed with {:?}",
        output.status.code()
    );
}

fn initialize_repository(directory: &Path) {
    fs::create_dir(directory).expect("repository directory creates");
    git(directory, &["init", "-q"]);
    git(directory, &["config", "user.email", "hq@example.invalid"]);
    git(directory, &["config", "user.name", "HQ Test"]);
    fs::write(directory.join("tracked.txt"), b"initial\n").expect("tracked file writes");
    git(directory, &["add", "tracked.txt"]);
    git(directory, &["commit", "-qm", "initial"]);
}

fn locator(path: &Path) -> ResourceLocator {
    ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new(path.to_string_lossy().into_owned()).expect("bounded path"),
    )
}

fn request(source: &Path, destination: &Path, branch: &str) -> EffectRequest<GitWorktreeRequest> {
    EffectRequest::new(
        OperationId::from_bytes([1; 32]),
        CommandDigest::from_bytes([2; 32]),
        Timestamp::from_unix_millis(3),
        GitWorktreeRequest {
            source: locator(source),
            destination: locator(destination),
            branch: ShortText::new(branch).expect("bounded branch"),
            create_branch: true,
        },
    )
}

fn adapter() -> GitWorktreeAdapter<ExecGit> {
    GitWorktreeAdapter::new(
        GitWorktreeAdapterConfig {
            max_repository_locks: NonZeroUsize::new(4).expect("nonzero"),
        },
        ExecGit::new(GitCommandConfig {
            executable: PathBuf::from("git"),
            timeout: Duration::from_secs(5),
            max_output_bytes: 1024 * 1024,
        })
        .expect("bounded git runner"),
    )
}

#[test]
fn real_git_create_is_exact_and_replay_safe() {
    let root = TestDirectory::new();
    let repository = root.join("repository");
    let destination = root.join("worktree");
    initialize_repository(&repository);
    let request = request(&repository, &destination, "feature");
    let adapter = adapter();

    assert_eq!(
        adapter.lookup(&request).expect("lookup succeeds"),
        EffectOutcome::Accepted(GitWorktreeState::ReadyToCreate)
    );
    assert_eq!(
        adapter.create(&request).expect("create succeeds"),
        EffectOutcome::Accepted(())
    );
    assert_eq!(
        adapter.lookup(&request).expect("created state reconciles"),
        EffectOutcome::Accepted(GitWorktreeState::Created)
    );
    assert_eq!(
        adapter.create(&request).expect("exact replay succeeds"),
        EffectOutcome::Accepted(())
    );
}

#[test]
fn competing_destinations_in_one_repository_are_serialized_and_both_reconcile() {
    let root = TestDirectory::new();
    let repository = root.join("repository");
    let first_destination = root.join("first-worktree");
    let second_destination = root.join("second-worktree");
    initialize_repository(&repository);
    let adapter = Arc::new(adapter());
    let barrier = Arc::new(Barrier::new(2));

    let first = {
        let adapter = Arc::clone(&adapter);
        let barrier = Arc::clone(&barrier);
        let request = request(&repository, &first_destination, "feature/first");
        std::thread::spawn(move || {
            barrier.wait();
            (request.clone(), adapter.create(&request))
        })
    };
    let second = {
        let adapter = Arc::clone(&adapter);
        let barrier = Arc::clone(&barrier);
        let request = request(&repository, &second_destination, "feature/second");
        std::thread::spawn(move || {
            barrier.wait();
            (request.clone(), adapter.create(&request))
        })
    };
    let (first_request, first_outcome) = first.join().expect("first worker joins");
    let (second_request, second_outcome) = second.join().expect("second worker joins");

    assert_eq!(
        first_outcome.expect("first create"),
        EffectOutcome::Accepted(())
    );
    assert_eq!(
        second_outcome.expect("second create"),
        EffectOutcome::Accepted(())
    );
    assert_eq!(
        adapter.lookup(&first_request).expect("first lookup"),
        EffectOutcome::Accepted(GitWorktreeState::Created)
    );
    assert_eq!(
        adapter.lookup(&second_request).expect("second lookup"),
        EffectOutcome::Accepted(GitWorktreeState::Created)
    );
}

#[test]
fn real_git_rejects_changed_branch_and_non_directory_destination() {
    let root = TestDirectory::new();
    let repository = root.join("repository");
    let destination = root.join("worktree");
    initialize_repository(&repository);
    let adapter = adapter();
    let original = request(&repository, &destination, "feature");
    assert_eq!(
        adapter.create(&original).expect("create succeeds"),
        EffectOutcome::Accepted(())
    );

    assert!(matches!(
        adapter
            .lookup(&request(&repository, &destination, "other"))
            .expect("conflict is a domain disposition"),
        EffectOutcome::Rejected(_)
    ));

    let file_destination = root.join("plain-file");
    fs::write(&file_destination, b"not a worktree").expect("plain file writes");
    assert!(matches!(
        adapter
            .lookup(&request(&repository, &file_destination, "second"))
            .expect("conflict is a domain disposition"),
        EffectOutcome::Rejected(_)
    ));
}

#[test]
fn real_git_rejects_partial_branch_and_stale_worktree_registration_state() {
    let root = TestDirectory::new();
    let repository = root.join("repository");
    initialize_repository(&repository);
    let adapter = adapter();

    git(&repository, &["branch", "feature/existing"]);
    assert!(matches!(
        adapter
            .lookup(&request(
                &repository,
                &root.join("existing-branch-worktree"),
                "feature/existing",
            ))
            .expect("branch conflict is a domain disposition"),
        EffectOutcome::Rejected(_)
    ));

    let stale_destination = root.join("stale-worktree");
    git(
        &repository,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature/stale",
            stale_destination.to_str().expect("UTF-8 test path"),
        ],
    );
    fs::remove_dir_all(&stale_destination).expect("simulate interrupted external cleanup");
    assert!(matches!(
        adapter
            .lookup(&request(&repository, &stale_destination, "feature/stale",))
            .expect("stale registration is a domain disposition"),
        EffectOutcome::Rejected(_)
    ));
}

#[cfg(unix)]
#[test]
fn real_git_rejects_a_symlink_destination_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let root = TestDirectory::new();
    let repository = root.join("repository");
    let target = root.join("target");
    let destination = root.join("linked-worktree");
    initialize_repository(&repository);
    fs::create_dir(&target).expect("target creates");
    symlink(&target, &destination).expect("destination symlink creates");

    assert!(matches!(
        adapter()
            .lookup(&request(&repository, &destination, "feature"))
            .expect("symlink conflict is a domain disposition"),
        EffectOutcome::Rejected(_)
    ));
    assert!(
        fs::read_dir(&target)
            .expect("target remains")
            .next()
            .is_none()
    );
}
