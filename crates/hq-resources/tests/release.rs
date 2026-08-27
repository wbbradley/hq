//! Git cleanliness, worktree identity, and explicit release force behavior.

#![allow(clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use hq_domain::{InstallationId, ProjectId, ResourceId};
use hq_resources::{
    ExecGit, GitChangeKind, GitCommandConfig, GitCommandFailure, GitRunner, PathClaim,
    PathIdentityRequest, PathRelation, PathReleaseState, PathResourceAdapter, ReleaseDecision,
    claim_conflict, decide_release,
};

mod support;

use support::{TestDirectory, git, initialize_repository};

fn adapter() -> PathResourceAdapter {
    PathResourceAdapter::system()
}

fn identity(directory: PathBuf, id: u8) -> hq_resources::PathResourceResolution {
    adapter()
        .identify(&PathIdentityRequest {
            home: InstallationId::from_bytes([1; 32]),
            resource_id: ResourceId::from_bytes([id; 32]),
            display_path: directory,
        })
        .expect("resource identifies")
}

#[test]
fn clean_non_git_dirty_and_unknown_release_are_closed_and_force_gated() {
    let repository = TestDirectory::new();
    initialize_repository(repository.path());
    let resource = identity(repository.path().to_path_buf(), 1);
    let clean = adapter().assess_release(resource.home, &resource.resource);
    assert_eq!(clean.home, resource.home);
    assert_eq!(clean.state, PathReleaseState::Clean);
    assert!(clean.worktree_identity.is_some());
    assert!(clean.common_git_directory.is_some());
    assert!(clean.changes.is_empty());

    fs::write(repository.join("untracked.txt"), b"new\n").expect("untracked writes");
    let untracked = adapter().assess_release(resource.home, &resource.resource);
    assert_eq!(untracked.state, PathReleaseState::Dirty);
    assert!(untracked.changes.contains(&GitChangeKind::Untracked));

    fs::write(repository.join("tracked.txt"), b"changed\n").expect("tracked changes");
    let unstaged = adapter().assess_release(resource.home, &resource.resource);
    assert!(unstaged.changes.contains(&GitChangeKind::Unstaged));
    git(repository.path(), &["add", "tracked.txt"]);
    let staged = adapter().assess_release(resource.home, &resource.resource);
    assert!(staged.changes.contains(&GitChangeKind::Staged));
    fs::remove_file(repository.join("tracked.txt")).expect("tracked removes");
    let deleted = adapter().assess_release(resource.home, &resource.resource);
    assert!(deleted.changes.contains(&GitChangeKind::Deleted));

    git(repository.path(), &["reset", "--hard", "-q", "HEAD"]);
    git(repository.path(), &["clean", "-fdq"]);
    git(repository.path(), &["mv", "tracked.txt", "renamed.txt"]);
    let renamed = adapter().assess_release(resource.home, &resource.resource);
    assert!(renamed.changes.contains(&GitChangeKind::Renamed));

    assert_eq!(
        decide_release(std::slice::from_ref(&deleted), false),
        ReleaseDecision::ForceRequired { risky_resources: 1 }
    );
    assert_eq!(
        decide_release(std::slice::from_ref(&deleted), true),
        ReleaseDecision::Forced { risky_resources: 1 }
    );

    let plain = TestDirectory::new();
    let plain_resource = identity(plain.path().to_path_buf(), 2);
    let non_git = adapter().assess_release(plain_resource.home, &plain_resource.resource);
    assert_eq!(non_git.state, PathReleaseState::NotApplicable);

    let unavailable_git = PathResourceAdapter::new(
        hq_resources::StdPathSystem,
        ExecGit::new(GitCommandConfig {
            executable: PathBuf::from("definitely-not-an-hq-git-binary"),
            timeout: Duration::from_secs(1),
            max_output_bytes: 64 * 1024,
        })
        .expect("configuration validates"),
    );
    let unknown = unavailable_git.assess_release(resource.home, &resource.resource);
    assert_eq!(unknown.state, PathReleaseState::Unknown);
    assert_eq!(
        decide_release(&[clean, non_git, unknown], false),
        ReleaseDecision::ForceRequired { risky_resources: 1 }
    );
}

#[cfg(unix)]
#[test]
fn git_transport_enforces_inclusive_output_and_wall_time_bounds() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new();
    let executable = directory.join("bounded-git");
    fs::write(&executable, b"#!/bin/sh\nprintf 1234\n").expect("script writes");
    let mut permissions = fs::metadata(&executable)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("script becomes executable");

    let exact = ExecGit::new(GitCommandConfig {
        executable: executable.clone(),
        timeout: Duration::from_secs(1),
        max_output_bytes: 4,
    })
    .expect("exact runner validates")
    .run(directory.path(), &[])
    .expect("inclusive output succeeds");
    assert_eq!(exact.stdout, b"1234");

    let oversized = ExecGit::new(GitCommandConfig {
        executable: executable.clone(),
        timeout: Duration::from_secs(1),
        max_output_bytes: 3,
    })
    .expect("oversized runner validates")
    .run(directory.path(), &[])
    .expect_err("one byte beyond the bound rejects");
    assert_eq!(oversized, GitCommandFailure::OutputTooLarge);

    fs::write(&executable, b"#!/bin/sh\nsleep 5\n").expect("slow script writes");
    let started = Instant::now();
    let timed_out = ExecGit::new(GitCommandConfig {
        executable,
        timeout: Duration::from_millis(20),
        max_output_bytes: 4,
    })
    .expect("timeout runner validates")
    .run(directory.path(), &[])
    .expect_err("stalled command times out");
    assert_eq!(timed_out, GitCommandFailure::TimedOut);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "timeout must include inherited stdout cleanup"
    );
}

#[test]
fn linked_worktrees_share_repository_maintenance_identity_without_claim_conflict() {
    let repository = TestDirectory::new();
    initialize_repository(repository.path());
    let linked_parent = TestDirectory::new();
    let linked = linked_parent.join("linked");
    let linked_text = linked.to_str().expect("UTF-8");
    git(
        repository.path(),
        &["worktree", "add", "-q", "-b", "linked-test", linked_text],
    );

    let first = identity(repository.path().to_path_buf(), 1);
    let second = identity(linked.clone(), 2);
    let first_release = adapter().assess_release(first.home, &first.resource);
    let second_release = adapter().assess_release(second.home, &second.resource);
    assert_ne!(
        first_release.worktree_identity,
        second_release.worktree_identity
    );
    assert_eq!(
        first_release.common_git_directory,
        second_release.common_git_directory
    );

    let first_claim = PathClaim {
        project_id: ProjectId::from_bytes([1; 32]),
        home: first.home,
        resource: first.resource,
    };
    let second_claim = PathClaim {
        project_id: ProjectId::from_bytes([2; 32]),
        home: second.home,
        resource: second.resource,
    };
    assert!(claim_conflict(&first_claim, &second_claim).is_none());
    assert_eq!(
        hq_resources::path_relation(
            &first_claim.resource.canonical_locator,
            &second_claim.resource.canonical_locator
        ),
        PathRelation::Disjoint
    );
}
