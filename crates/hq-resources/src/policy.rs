//! Pure home-qualified path claim and release policy.

use hq_domain::{
    InstallationId, ProjectId, ProjectResource, ResourceId, ResourceLocator, ResourceScheme,
};

use crate::PathReleaseAssessment;

/// Component-aware relationship from the left canonical path to the right.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathRelation {
    /// Both canonical paths are equal.
    Equal,
    /// The left path is a strict ancestor of the right.
    Ancestor,
    /// The left path is a strict descendant of the right.
    Descendant,
    /// The paths do not overlap or are not both working-tree locators.
    Disjoint,
}

/// One durable advisory path claim from the canonical project projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathClaim {
    /// Project holding the claim.
    pub project_id: ProjectId,
    /// Immutable home whose filesystem namespace qualifies the path.
    pub home: InstallationId,
    /// Desired path resource carrying display and canonical identity.
    pub resource: ProjectResource,
}

/// Explainable conflicting claims from distinct projects in one home.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathClaimConflict {
    /// Home-local namespace shared by both conflicting claims.
    pub home: InstallationId,
    /// Project proposing or already holding the left claim.
    pub requested_project: ProjectId,
    /// Resource on the requested side.
    pub requested_resource: ResourceId,
    /// Human-selected spelling on the requested side.
    pub requested_display: ResourceLocator,
    /// Canonical identity on the requested side.
    pub requested_canonical: ResourceLocator,
    /// Other project holding the overlapping claim.
    pub conflicting_project: ProjectId,
    /// Resource on the conflicting side.
    pub conflicting_resource: ResourceId,
    /// Human-selected spelling on the conflicting side.
    pub conflicting_display: ResourceLocator,
    /// Canonical identity on the conflicting side.
    pub conflicting_canonical: ResourceLocator,
    /// Exact canonical relationship from requested to conflicting.
    pub relationship: PathRelation,
}

/// Reports the component-aware relationship between two canonical locators.
pub fn path_relation(left: &ResourceLocator, right: &ResourceLocator) -> PathRelation {
    if left.scheme() != ResourceScheme::WorkingTree || right.scheme() != ResourceScheme::WorkingTree
    {
        return PathRelation::Disjoint;
    }
    let Some(left) = components(left.value()) else {
        return PathRelation::Disjoint;
    };
    let Some(right) = components(right.value()) else {
        return PathRelation::Disjoint;
    };
    if left == right {
        PathRelation::Equal
    } else if left.len() < right.len() && right.starts_with(&left) {
        PathRelation::Ancestor
    } else if right.len() < left.len() && left.starts_with(&right) {
        PathRelation::Descendant
    } else {
        PathRelation::Disjoint
    }
}

fn components(value: &str) -> Option<Vec<&str>> {
    if !value.starts_with('/')
        || (value != "/" && (value.ends_with('/') || value.contains("//")))
        || value
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return None;
    }
    Some(
        value
            .split('/')
            .filter(|component| !component.is_empty())
            .collect(),
    )
}

/// Returns whether both locators form a normalized path-resource identity.
pub fn valid_path_resource(resource: &ProjectResource) -> bool {
    resource.display_locator.scheme() == ResourceScheme::WorkingTree
        && resource.canonical_locator.scheme() == ResourceScheme::WorkingTree
        && components(resource.display_locator.value()).is_some()
        && components(resource.canonical_locator.value()).is_some()
}

/// Returns a conflict only for overlapping claims from distinct projects in one home.
pub fn claim_conflict(left: &PathClaim, right: &PathClaim) -> Option<PathClaimConflict> {
    if left.home != right.home || left.project_id == right.project_id {
        return None;
    }
    let relationship = path_relation(
        &left.resource.canonical_locator,
        &right.resource.canonical_locator,
    );
    (relationship != PathRelation::Disjoint).then_some(PathClaimConflict {
        home: left.home,
        requested_project: left.project_id,
        requested_resource: left.resource.resource_id,
        requested_display: left.resource.display_locator.clone(),
        requested_canonical: left.resource.canonical_locator.clone(),
        conflicting_project: right.project_id,
        conflicting_resource: right.resource.resource_id,
        conflicting_display: right.resource.display_locator.clone(),
        conflicting_canonical: right.resource.canonical_locator.clone(),
        relationship,
    })
}

/// Closed primary-selection error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourcePolicyError {
    /// The same resource identity appeared more than once.
    DuplicateResource,
    /// The explicit primary identity is not in the selected resources.
    UnknownPrimary,
}

impl std::fmt::Display for ResourcePolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateResource => "resource selection contains a duplicate identity",
            Self::UnknownPrimary => "primary resource is not selected",
        })
    }
}

impl std::error::Error for ResourcePolicyError {}

/// Selects an explicit primary or defaults to the first human-selected resource.
pub fn select_primary(
    resources: &[ProjectResource],
    requested: Option<ResourceId>,
) -> Result<Option<ResourceId>, ResourcePolicyError> {
    let mut identities = std::collections::BTreeSet::new();
    if resources
        .iter()
        .any(|resource| !identities.insert(resource.resource_id))
    {
        return Err(ResourcePolicyError::DuplicateResource);
    }
    if let Some(requested) = requested {
        if !identities.contains(&requested) {
            return Err(ResourcePolicyError::UnknownPrimary);
        }
        Ok(Some(requested))
    } else {
        Ok(resources.first().map(|resource| resource.resource_id))
    }
}

/// Aggregated release policy decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseDecision {
    /// Every resource is clean or not applicable.
    Proceed,
    /// Dirty or unknown observations require an explicit force flag.
    ForceRequired {
        /// Number of dirty or unknown resource assessments.
        risky_resources: usize,
    },
    /// The caller explicitly accepted every dirty or unknown observation.
    Forced {
        /// Number of dirty or unknown resource assessments explicitly accepted.
        risky_resources: usize,
    },
}

/// Applies the generic force rule without interpreting adapter-specific evidence.
pub fn decide_release(assessments: &[PathReleaseAssessment], force: bool) -> ReleaseDecision {
    let risky_resources = assessments
        .iter()
        .filter(|assessment| assessment.state.requires_force())
        .count();
    match (risky_resources, force) {
        (0, _) => ReleaseDecision::Proceed,
        (_, true) => ReleaseDecision::Forced { risky_resources },
        (_, false) => ReleaseDecision::ForceRequired { risky_resources },
    }
}
