//! Path identity, conflict, health, primary, and launch behavior.

#![allow(clippy::expect_used)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use hq_domain::{
    BoundedText, InstallationId, ProjectId, ProjectResource, ResourceHealth, ResourceId,
    ResourceLocator, ResourceScheme,
};
use hq_resources::{
    GitCommandFailure, GitCommandOutput, GitRunner, LaunchClaimRelation, PathClaim, PathCondition,
    PathEntryKind, PathIdentityRequest, PathProbeError, PathRelation, PathResourceAdapter,
    PathResourceError, PathSystem, claim_conflict, path_relation, select_primary,
    valid_path_resource,
};

mod support;

use support::TestDirectory;

fn home(value: u8) -> InstallationId {
    InstallationId::from_bytes([value; 32])
}

fn resource(value: u8) -> ResourceId {
    ResourceId::from_bytes([value; 32])
}

fn project(value: u8) -> ProjectId {
    ProjectId::from_bytes([value; 32])
}

#[derive(Clone, Copy, Debug)]
struct NoGit;

impl GitRunner for NoGit {
    fn run(
        &self,
        _directory: &Path,
        _arguments: &[&str],
    ) -> Result<GitCommandOutput, GitCommandFailure> {
        Err(GitCommandFailure::Unavailable)
    }
}

#[derive(Clone, Debug)]
struct InaccessiblePathSystem {
    root: PathBuf,
    canonical_root: PathBuf,
}

impl PathSystem for InaccessiblePathSystem {
    fn symlink_entry(&self, path: &Path) -> Result<PathEntryKind, PathProbeError> {
        if path == self.root {
            Ok(PathEntryKind::Directory)
        } else if path.starts_with(self.root.join("denied")) {
            Err(PathProbeError::Inaccessible)
        } else {
            Err(PathProbeError::Missing)
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, PathProbeError> {
        if path == self.root {
            Ok(self.canonical_root.clone())
        } else {
            Err(PathProbeError::Inaccessible)
        }
    }

    fn followed_entry(&self, path: &Path) -> Result<PathEntryKind, PathProbeError> {
        self.symlink_entry(path)
    }
}

#[test]
fn missing_path_uses_the_nearest_existing_ancestor_without_losing_human_spelling() {
    let directory = TestDirectory::new();
    let display = directory.join("future/nested");
    let adapter = PathResourceAdapter::system();
    let resolved = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: display.clone(),
        })
        .expect("missing path resolves");

    assert_eq!(resolved.home, home(1));
    assert_eq!(resolved.condition, PathCondition::Missing);
    assert_eq!(resolved.resource.health, ResourceHealth::Unavailable);
    assert_eq!(
        resolved.resource.display_locator.scheme(),
        ResourceScheme::WorkingTree
    );
    assert_eq!(
        resolved.resource.display_locator.value(),
        display.to_str().expect("UTF-8")
    );
    let expected = fs::canonicalize(directory.path())
        .expect("ancestor canonicalizes")
        .join("future/nested");
    assert_eq!(
        resolved.resource.canonical_locator.value(),
        expected.to_str().expect("UTF-8")
    );
}

#[cfg(unix)]
#[test]
fn health_revalidation_detects_symlink_retargeting_without_changing_identity() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let first = directory.join("first");
    let second = directory.join("second");
    fs::create_dir(&first).expect("first creates");
    fs::create_dir(&second).expect("second creates");
    let selected = directory.join("selected");
    symlink(&first, &selected).expect("first link creates");
    let display = selected.join("future");
    let adapter = PathResourceAdapter::system();
    let resolved = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: display.clone(),
        })
        .expect("reservation resolves");
    let expected = fs::canonicalize(&first)
        .expect("first canonicalizes")
        .join("future");
    assert_eq!(
        resolved.resource.canonical_locator.value(),
        expected.to_str().expect("UTF-8")
    );

    fs::remove_file(&selected).expect("old link removes");
    symlink(&second, &selected).expect("second link creates");
    fs::create_dir(second.join("future")).expect("future creates");
    let observed = adapter.inspect(resolved.home, &resolved.resource);

    assert_eq!(observed.home, resolved.home);
    assert_eq!(observed.resource_id, resolved.resource.resource_id);
    assert_eq!(observed.condition, PathCondition::IdentityChanged);
    assert_eq!(observed.health, ResourceHealth::Degraded);
    assert_eq!(
        observed
            .observed_canonical
            .expect("observed identity")
            .value(),
        fs::canonicalize(second.join("future"))
            .expect("second future canonicalizes")
            .to_str()
            .expect("UTF-8")
    );
    assert_eq!(
        resolved.resource.canonical_locator.value(),
        expected.to_str().expect("UTF-8")
    );
}

#[test]
fn relative_paths_fail_before_observation() {
    let error = PathResourceAdapter::system()
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: PathBuf::from("relative/path"),
        })
        .expect_err("relative path rejects");
    assert_eq!(error, PathResourceError::NotAbsolute);
}

#[test]
fn nul_and_oversized_paths_fail_before_observation() {
    let adapter = PathResourceAdapter::system();
    let nul = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: PathBuf::from("/tmp/hq\0path"),
        })
        .expect_err("NUL path rejects");
    assert_eq!(nul, PathResourceError::InvalidPath);

    let oversized = format!("/{}", "x".repeat(4097));
    let oversized = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: PathBuf::from(oversized),
        })
        .expect_err("oversized path rejects");
    assert_eq!(oversized, PathResourceError::LocatorTooLong);
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_fail_before_observation() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let path = PathBuf::from(OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]));
    let error = PathResourceAdapter::system()
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: path,
        })
        .expect_err("non-UTF-8 path rejects");
    assert_eq!(error, PathResourceError::UnsupportedEncoding);
}

#[test]
fn inaccessible_ancestors_remain_reserved_but_never_look_healthy() {
    let directory = TestDirectory::new();
    let canonical_root = fs::canonicalize(directory.path()).expect("root canonicalizes");
    let adapter = PathResourceAdapter::new(
        InaccessiblePathSystem {
            root: directory.path().to_path_buf(),
            canonical_root: canonical_root.clone(),
        },
        NoGit,
    );
    let display = directory.join("denied/future");
    let resolved = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(3),
            display_path: display.clone(),
        })
        .expect("inaccessible path retains identity");

    assert_eq!(resolved.condition, PathCondition::Inaccessible);
    assert_eq!(resolved.resource.health, ResourceHealth::Unavailable);
    assert_eq!(
        resolved.resource.display_locator.value(),
        display.to_str().expect("UTF-8")
    );
    assert_eq!(
        resolved.resource.canonical_locator.value(),
        canonical_root
            .join("denied/future")
            .to_str()
            .expect("UTF-8")
    );
}

#[test]
fn existing_non_directory_is_explicitly_unavailable() {
    let directory = TestDirectory::new();
    let file = directory.join("file");
    fs::write(&file, b"not a directory").expect("file writes");
    let resolved = PathResourceAdapter::system()
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(4),
            display_path: file,
        })
        .expect("file observes");
    assert_eq!(resolved.condition, PathCondition::NotDirectory);
    assert_eq!(resolved.resource.health, ResourceHealth::Unavailable);
}

#[cfg(unix)]
#[test]
fn broken_symlink_is_malformed_instead_of_missing() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let link = directory.join("broken");
    symlink(directory.join("absent-target"), &link).expect("broken link creates");
    let resolved = PathResourceAdapter::system()
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(5),
            display_path: link,
        })
        .expect("broken link retains anchored identity");
    assert_eq!(resolved.condition, PathCondition::Malformed);
    assert_eq!(resolved.resource.health, ResourceHealth::Degraded);
}

#[test]
fn conflicts_are_component_aware_home_scoped_and_ignore_same_project_overlap() {
    let directory = TestDirectory::new();
    let adapter = PathResourceAdapter::system();
    let parent = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(1),
            display_path: directory.path().to_path_buf(),
        })
        .expect("parent resolves");
    let child_path = directory.join("child");
    let child = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: child_path,
        })
        .expect("child resolves");
    assert_eq!(
        path_relation(
            &parent.resource.canonical_locator,
            &child.resource.canonical_locator
        ),
        PathRelation::Ancestor
    );

    let first = PathClaim {
        project_id: project(1),
        home: home(1),
        resource: parent.resource.clone(),
    };
    let same_project = PathClaim {
        project_id: project(1),
        home: home(1),
        resource: child.resource.clone(),
    };
    let other_project = PathClaim {
        project_id: project(2),
        home: home(1),
        resource: child.resource.clone(),
    };
    let other_home = PathClaim {
        project_id: project(2),
        home: home(2),
        resource: child.resource.clone(),
    };
    assert!(claim_conflict(&first, &same_project).is_none());
    assert!(claim_conflict(&first, &other_home).is_none());
    let conflict = claim_conflict(&first, &other_project).expect("cross-project overlap conflicts");
    assert_eq!(conflict.home, home(1));
    assert_eq!(conflict.relationship, PathRelation::Ancestor);
    assert_eq!(conflict.requested_project, project(1));
    assert_eq!(conflict.conflicting_project, project(2));

    let malformed = ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new("/tmp//not-normalized").expect("bounded locator"),
    );
    assert_eq!(
        path_relation(&first.resource.canonical_locator, &malformed),
        PathRelation::Disjoint
    );
}

#[test]
fn path_resource_identity_requires_normalized_working_tree_locators() {
    let valid = ProjectResource {
        resource_id: resource(9),
        display_locator: ResourceLocator::new(
            ResourceScheme::WorkingTree,
            BoundedText::new("/human/repo").expect("bounded"),
        ),
        canonical_locator: ResourceLocator::new(
            ResourceScheme::WorkingTree,
            BoundedText::new("/canonical/repo").expect("bounded"),
        ),
        health: ResourceHealth::Unknown,
    };
    assert!(valid_path_resource(&valid));

    let mut malformed = valid.clone();
    malformed.canonical_locator = ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new("/canonical//repo").expect("bounded"),
    );
    assert!(!valid_path_resource(&malformed));

    let mut opaque = valid;
    opaque.display_locator = ResourceLocator::new(
        ResourceScheme::Opaque,
        BoundedText::new("/human/repo").expect("bounded"),
    );
    assert!(!valid_path_resource(&opaque));
}

#[test]
fn primary_selection_and_launch_validation_are_explicit_and_never_relocate() {
    let directory = TestDirectory::new();
    let claimed = directory.join("claimed");
    let outside = directory.join("outside");
    fs::create_dir(&claimed).expect("claimed creates");
    fs::create_dir(&outside).expect("outside creates");
    let adapter = PathResourceAdapter::system();
    let first = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(1),
            display_path: claimed.clone(),
        })
        .expect("first resolves")
        .resource;
    let second = adapter
        .identify(&PathIdentityRequest {
            home: home(1),
            resource_id: resource(2),
            display_path: outside.clone(),
        })
        .expect("second resolves")
        .resource;

    assert_eq!(
        select_primary(&[first.clone(), second.clone()], None).expect("default primary"),
        Some(first.resource_id)
    );
    assert_eq!(
        select_primary(&[first.clone(), second.clone()], Some(second.resource_id))
            .expect("explicit primary"),
        Some(second.resource_id)
    );
    assert!(select_primary(std::slice::from_ref(&first), Some(resource(9))).is_err());

    let inside_report = adapter
        .validate_launch_directory(home(1), claimed.clone(), std::slice::from_ref(&first))
        .expect("inside validates");
    assert_eq!(inside_report.condition, PathCondition::Healthy);
    assert_eq!(inside_report.claim_relation, LaunchClaimRelation::Claimed);
    assert_eq!(
        inside_report.display_locator.value(),
        claimed.to_str().expect("UTF-8")
    );

    let outside_report = adapter
        .validate_launch_directory(home(1), outside.clone(), std::slice::from_ref(&first))
        .expect("outside validates with warning");
    assert_eq!(outside_report.condition, PathCondition::Healthy);
    assert_eq!(
        outside_report.claim_relation,
        LaunchClaimRelation::OutsideClaims
    );
    assert_eq!(
        outside_report.display_locator.value(),
        outside.to_str().expect("UTF-8")
    );

    let missing = directory.join("does-not-exist");
    let missing_report = adapter
        .validate_launch_directory(home(1), missing.clone(), std::slice::from_ref(&first))
        .expect("missing reports without substitution");
    assert_eq!(missing_report.condition, PathCondition::Missing);
    assert_eq!(
        missing_report.display_locator.value(),
        missing.to_str().expect("UTF-8")
    );
}
