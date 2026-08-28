//! External path-resource and Git observation adapter.

mod adapter;
mod git;
mod path;
mod policy;

pub use adapter::{
    LaunchClaimRelation, LaunchDirectoryAssessment, PathResourceAdapter, PathResourceInspection,
};
pub use git::{
    ExecGit, GitChangeKind, GitCommandConfig, GitCommandFailure, GitCommandOutput, GitRunner,
    PathReleaseAssessment, PathReleaseState,
};
pub use path::{
    PathCondition, PathEntryKind, PathIdentityRequest, PathProbeError, PathResourceError,
    PathResourceResolution, PathSystem, StdPathSystem, normalize_absolute_path,
};
pub use policy::{
    PathClaim, PathClaimConflict, PathRelation, ReleaseDecision, ResourcePolicyError,
    claim_conflict, decide_release, path_relation, select_primary, valid_path_resource,
};
