//! Narrow project-domain desired-resource conflict decisions.

use hq_domain::{InstallationId, ProjectId, ProjectResource, ResourceId, ResourceLocator};
use hq_resources::{PathClaim, PathRelation, claim_conflict};

/// Component relationship from a proposed canonical resource to an active conflicting resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectResourceRelationship {
    /// Both canonical paths are equal.
    Equal,
    /// The proposed path is a strict ancestor of the active path.
    Ancestor,
    /// The proposed path is a strict descendant of the active path.
    Descendant,
}

/// Passive domain-selected conflict for one proposed desired resource.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceConflict {
    /// Stable conflicting project identity.
    pub project_id: ProjectId,
    /// Stable conflicting resource identity.
    pub resource_id: ResourceId,
    /// Human-selected spelling of the conflicting resource.
    pub display_locator: ResourceLocator,
    /// Canonical identity of the conflicting resource.
    pub canonical_locator: ResourceLocator,
    /// Exact component relationship from proposed to conflicting canonical identity.
    pub relationship: ProjectResourceRelationship,
}

/// Applies the canonical project claim policy to a proposed and one active desired resource.
pub fn desired_resource_conflict(
    requested_project: ProjectId,
    requested_home: InstallationId,
    requested_resource: &ProjectResource,
    conflicting_project: ProjectId,
    conflicting_home: InstallationId,
    conflicting_resource: &ProjectResource,
) -> Option<ProjectResourceConflict> {
    let requested = PathClaim {
        project_id: requested_project,
        home: requested_home,
        resource: requested_resource.clone(),
    };
    let conflicting = PathClaim {
        project_id: conflicting_project,
        home: conflicting_home,
        resource: conflicting_resource.clone(),
    };
    claim_conflict(&requested, &conflicting).map(|conflict| ProjectResourceConflict {
        project_id: conflict.conflicting_project,
        resource_id: conflict.conflicting_resource,
        display_locator: conflict.conflicting_display,
        canonical_locator: conflict.conflicting_canonical,
        relationship: match conflict.relationship {
            PathRelation::Equal => ProjectResourceRelationship::Equal,
            PathRelation::Ancestor => ProjectResourceRelationship::Ancestor,
            PathRelation::Descendant => ProjectResourceRelationship::Descendant,
            PathRelation::Disjoint => unreachable!("claim_conflict excludes disjoint resources"),
        },
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use hq_domain::{BoundedText, ResourceHealth, ResourceLocator, ResourceScheme};

    use super::*;

    #[test]
    fn passive_conflict_preserves_domain_selected_relationship_and_identity() {
        let requested = resource(1, "/work");
        let conflicting = resource(2, "/work/child");
        let conflict = desired_resource_conflict(
            ProjectId::from_bytes([3; 32]),
            InstallationId::from_bytes([4; 32]),
            &requested,
            ProjectId::from_bytes([5; 32]),
            InstallationId::from_bytes([4; 32]),
            &conflicting,
        )
        .expect("overlap conflicts");
        assert_eq!(conflict.project_id, ProjectId::from_bytes([5; 32]));
        assert_eq!(conflict.resource_id, ResourceId::from_bytes([2; 32]));
        assert_eq!(conflict.relationship, ProjectResourceRelationship::Ancestor);
        assert_eq!(conflict.canonical_locator.value(), "/work/child");
    }

    fn resource(id: u8, path: &str) -> ProjectResource {
        let locator = ResourceLocator::new(
            ResourceScheme::WorkingTree,
            BoundedText::new(path).expect("path"),
        );
        ProjectResource {
            resource_id: ResourceId::from_bytes([id; 32]),
            display_locator: locator.clone(),
            canonical_locator: locator,
            health: ResourceHealth::Unknown,
        }
    }
}
