//! Path identity, health, launch, and release capability composition.

use std::path::{Path, PathBuf};

use hq_domain::{InstallationId, ProjectResource, ResourceHealth, ResourceId, ResourceLocator};

use crate::{
    ExecGit, GitRunner, PathCondition, PathIdentityRequest, PathReleaseAssessment,
    PathReleaseState, PathResourceError, PathResourceResolution, PathSystem, StdPathSystem,
    git::parse_status,
    path::{git_locator, resolve_path},
    path_relation,
    policy::PathRelation,
};

/// Result of checking a recorded display spelling against immutable identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathResourceInspection {
    /// Home-local namespace qualifying the observation.
    pub home: InstallationId,
    /// Stable resource identity being inspected.
    pub resource_id: ResourceId,
    /// Coarse health suitable for a canonical observation fact.
    pub health: ResourceHealth,
    /// Exact adapter condition behind the coarse health.
    pub condition: PathCondition,
    /// Current canonical identity, when safely observable.
    pub observed_canonical: Option<ResourceLocator>,
}

/// Whether a healthy explicit launch directory is covered by a project claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchClaimRelation {
    /// The directory equals or descends from a desired path claim.
    Claimed,
    /// The directory is valid but outside every desired path claim.
    OutsideClaims,
    /// Claim relation is not meaningful because the directory is unhealthy.
    Unavailable,
}

/// Passive launch-directory validation report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchDirectoryAssessment {
    /// Home-local namespace qualifying the directory.
    pub home: InstallationId,
    /// Exact normalized display spelling requested by the human.
    pub display_locator: ResourceLocator,
    /// Current canonical identity, including a missing reservation identity.
    pub canonical_locator: ResourceLocator,
    /// Exact directory condition.
    pub condition: PathCondition,
    /// Advisory relationship to current desired resources.
    pub claim_relation: LaunchClaimRelation,
}

/// Owned path/Git observation capability with injectable effects.
#[derive(Clone, Debug)]
pub struct PathResourceAdapter<F = StdPathSystem, G = ExecGit> {
    filesystem: F,
    git: G,
}

impl PathResourceAdapter<StdPathSystem, ExecGit> {
    /// Creates the standard bounded filesystem and Git adapter.
    pub fn system() -> Self {
        Self {
            filesystem: StdPathSystem,
            git: ExecGit::system(),
        }
    }
}

impl<F, G> PathResourceAdapter<F, G> {
    /// Owns injected read-only filesystem and Git capabilities.
    pub const fn new(filesystem: F, git: G) -> Self {
        Self { filesystem, git }
    }
}

impl<F: PathSystem, G: GitRunner> PathResourceAdapter<F, G> {
    /// Resolves a human-selected absolute path to one home-local durable identity.
    pub fn identify(
        &self,
        request: &PathIdentityRequest,
    ) -> Result<PathResourceResolution, PathResourceError> {
        resolve_path(&self.filesystem, request)
    }

    /// Revalidates a display spelling without mutating the recorded identity.
    pub fn inspect(
        &self,
        home: InstallationId,
        resource: &ProjectResource,
    ) -> PathResourceInspection {
        let display_path = PathBuf::from(resource.display_locator.value());
        let observed = resolve_path(
            &self.filesystem,
            &PathIdentityRequest {
                home,
                resource_id: resource.resource_id,
                display_path,
            },
        );
        match observed {
            Ok(observed) if observed.resource.canonical_locator != resource.canonical_locator => {
                PathResourceInspection {
                    home,
                    resource_id: resource.resource_id,
                    health: ResourceHealth::Degraded,
                    condition: PathCondition::IdentityChanged,
                    observed_canonical: Some(observed.resource.canonical_locator),
                }
            }
            Ok(observed) => PathResourceInspection {
                home,
                resource_id: resource.resource_id,
                health: observed.resource.health,
                condition: observed.condition,
                observed_canonical: Some(observed.resource.canonical_locator),
            },
            Err(_) => PathResourceInspection {
                home,
                resource_id: resource.resource_id,
                health: ResourceHealth::Unknown,
                condition: PathCondition::Unknown,
                observed_canonical: None,
            },
        }
    }

    /// Validates an explicit launch directory without substituting a claimed path.
    pub fn validate_launch_directory(
        &self,
        home: InstallationId,
        display_path: PathBuf,
        claims: &[ProjectResource],
    ) -> Result<LaunchDirectoryAssessment, PathResourceError> {
        let resolved = resolve_path(
            &self.filesystem,
            &PathIdentityRequest {
                home,
                resource_id: ResourceId::from_bytes([0; 32]),
                display_path,
            },
        )?;
        let claim_relation = if resolved.condition != PathCondition::Healthy {
            LaunchClaimRelation::Unavailable
        } else if claims.iter().any(|claim| {
            matches!(
                path_relation(
                    &claim.canonical_locator,
                    &resolved.resource.canonical_locator
                ),
                PathRelation::Equal | PathRelation::Ancestor
            )
        }) {
            LaunchClaimRelation::Claimed
        } else {
            LaunchClaimRelation::OutsideClaims
        };
        Ok(LaunchDirectoryAssessment {
            home,
            display_locator: resolved.resource.display_locator,
            canonical_locator: resolved.resource.canonical_locator,
            condition: resolved.condition,
            claim_relation,
        })
    }

    /// Assesses Git cleanliness without retaining stderr, environment, file paths, or contents.
    pub fn assess_release(
        &self,
        home: InstallationId,
        resource: &ProjectResource,
    ) -> PathReleaseAssessment {
        let inspection = self.inspect(home, resource);
        if inspection.condition != PathCondition::Healthy {
            return PathReleaseAssessment::state(
                home,
                resource.resource_id,
                PathReleaseState::Unknown,
            );
        }
        let directory = Path::new(resource.display_locator.value());
        match self.has_git_marker(directory) {
            Ok(false) => {
                return PathReleaseAssessment::state(
                    home,
                    resource.resource_id,
                    PathReleaseState::NotApplicable,
                );
            }
            Err(()) => {
                return PathReleaseAssessment::state(
                    home,
                    resource.resource_id,
                    PathReleaseState::Unknown,
                );
            }
            Ok(true) => {}
        }
        let Some(worktree) = self.git_path(directory, &["rev-parse", "--show-toplevel"]) else {
            return PathReleaseAssessment::state(
                home,
                resource.resource_id,
                PathReleaseState::Unknown,
            );
        };
        let Some(common) = self.git_path(
            &worktree,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ) else {
            return PathReleaseAssessment::state(
                home,
                resource.resource_id,
                PathReleaseState::Unknown,
            );
        };
        let Ok(worktree_identity) = crate::path::working_tree_locator(&worktree) else {
            return PathReleaseAssessment::state(
                home,
                resource.resource_id,
                PathReleaseState::Unknown,
            );
        };
        let Ok(common_git_directory) = git_locator(&common) else {
            return PathReleaseAssessment::state(
                home,
                resource.resource_id,
                PathReleaseState::Unknown,
            );
        };
        let status = match self.git.run(
            &worktree,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        ) {
            Ok(output) if output.success => output.stdout,
            Ok(_) | Err(_) => {
                return PathReleaseAssessment::state(
                    home,
                    resource.resource_id,
                    PathReleaseState::Unknown,
                );
            }
        };
        let Ok((changes, changed_entries)) = parse_status(&status) else {
            return PathReleaseAssessment::state(
                home,
                resource.resource_id,
                PathReleaseState::Unknown,
            );
        };
        PathReleaseAssessment {
            home,
            resource_id: resource.resource_id,
            state: if changed_entries == 0 {
                PathReleaseState::Clean
            } else {
                PathReleaseState::Dirty
            },
            worktree_identity: Some(worktree_identity),
            common_git_directory: Some(common_git_directory),
            changes,
            changed_entries,
        }
    }

    fn has_git_marker(&self, directory: &Path) -> Result<bool, ()> {
        let mut candidate = directory.to_path_buf();
        loop {
            match self.filesystem.symlink_entry(&candidate.join(".git")) {
                Ok(_) => return Ok(true),
                Err(crate::PathProbeError::Missing) => {}
                Err(crate::PathProbeError::Inaccessible | crate::PathProbeError::Unknown) => {
                    return Err(());
                }
            }
            if !candidate.pop() {
                return Ok(false);
            }
        }
    }

    fn git_path(&self, directory: &Path, arguments: &[&str]) -> Option<PathBuf> {
        let output = self.git.run(directory, arguments).ok()?;
        if !output.success {
            return None;
        }
        let value = std::str::from_utf8(&output.stdout).ok()?;
        let value = value.strip_suffix('\n').unwrap_or(value);
        let value = value.strip_suffix('\r').unwrap_or(value);
        if value.is_empty() || value.contains(['\n', '\r', '\0']) {
            return None;
        }
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    }
}
