//! Read-only project path-resource composition.

use std::path::PathBuf;

use hq_application::{
    ApplicationError, EffectOutcome, EffectRequest, InspectResource, ResourceInspectionRequest,
    ResourceInspectionResult,
};
use hq_domain::{
    DomainError, ErrorCategory, ErrorCode, InstallationId, ProjectResource, RepositoryContext,
    ResourceHealth,
};
use hq_projects::{
    ProjectLaunchObservation, ProjectLaunchValidationRequest, ProjectReleaseAssessmentRequest,
    ProjectResourceIdentificationRequest, ProjectResourceObservation, ProjectResourcePort,
    ProjectResourceValidationRequest,
};
use hq_resources::{LaunchClaimRelation, PathIdentityRequest, PathResourceAdapter as PathAdapter};

/// Home-qualified read-only resource capability shared by project workflows and local inspection.
#[derive(Clone, Debug)]
pub struct ProjectResourceAdapter {
    home: InstallationId,
    paths: PathAdapter,
}

impl ProjectResourceAdapter {
    /// Composes standard bounded filesystem and read-only Git observation for one installation.
    pub fn system(home: InstallationId) -> Self {
        Self {
            home,
            paths: PathAdapter::system(),
        }
    }

    /// Observes one canonical directory and its read-only Git repository context.
    pub fn repository_context(
        &self,
        directory: PathBuf,
    ) -> Result<RepositoryContext, ApplicationError> {
        self.paths
            .repository_context(self.home, directory)
            .map_err(|_| {
                ApplicationError::new(hq_application::ApplicationErrorCode::InvalidRequest)
            })
    }

    fn same_home(&self, home: InstallationId) -> Result<(), ApplicationError> {
        if home == self.home {
            Ok(())
        } else {
            Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::InvalidRequest,
            ))
        }
    }
}

impl ProjectResourcePort for ProjectResourceAdapter {
    fn identify_resource(
        &self,
        request: &EffectRequest<ProjectResourceIdentificationRequest>,
    ) -> Result<EffectOutcome<ProjectResource>, ApplicationError> {
        self.same_home(request.body.home)?;
        match self.paths.identify(&PathIdentityRequest {
            home: request.body.home,
            resource_id: request.body.resource_id,
            display_path: PathBuf::from(request.body.destination.value()),
        }) {
            Ok(resolution) => Ok(EffectOutcome::Accepted(resolution.resource)),
            Err(_) => Ok(EffectOutcome::Rejected(resource_error(
                ErrorCategory::InvalidInput,
                "project_resource_identification_rejected",
            ))),
        }
    }

    fn validate_resources(
        &self,
        request: &EffectRequest<ProjectResourceValidationRequest>,
    ) -> Result<EffectOutcome<Vec<ProjectResourceObservation>>, ApplicationError> {
        self.same_home(request.body.home)?;
        Ok(EffectOutcome::Accepted(
            request
                .body
                .resources
                .iter()
                .map(|resource| {
                    let inspection = self.paths.inspect(request.body.home, resource);
                    ProjectResourceObservation {
                        resource_id: resource.resource_id,
                        observed_canonical: inspection.observed_canonical,
                        health: inspection.health,
                    }
                })
                .collect(),
        ))
    }

    fn assess_release(
        &self,
        request: &EffectRequest<ProjectReleaseAssessmentRequest>,
    ) -> Result<EffectOutcome<Vec<hq_resources::PathReleaseAssessment>>, ApplicationError> {
        self.same_home(request.body.home)?;
        Ok(EffectOutcome::Accepted(
            request
                .body
                .resources
                .iter()
                .map(|resource| self.paths.assess_release(request.body.home, resource))
                .collect(),
        ))
    }

    fn validate_launch_directory(
        &self,
        request: &EffectRequest<ProjectLaunchValidationRequest>,
    ) -> Result<EffectOutcome<ProjectLaunchObservation>, ApplicationError> {
        self.same_home(request.body.home)?;
        match self.paths.validate_launch_directory(
            request.body.home,
            PathBuf::from(request.body.launch_directory.value()),
            &request.body.resources,
        ) {
            Ok(assessment) => Ok(EffectOutcome::Accepted(ProjectLaunchObservation {
                observed_canonical: assessment.canonical_locator,
                health: condition_health(assessment.condition),
                within_claims: assessment.claim_relation == LaunchClaimRelation::Claimed,
            })),
            Err(_) => Ok(EffectOutcome::Rejected(resource_error(
                ErrorCategory::InvalidInput,
                "project_launch_directory_rejected",
            ))),
        }
    }
}

impl InspectResource for ProjectResourceAdapter {
    fn inspect_resource(
        &self,
        request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        let inspection = self.paths.inspect(
            self.home,
            &ProjectResource {
                resource_id: request.body.resource_id,
                display_locator: request.body.display_locator.clone(),
                canonical_locator: request.body.canonical_locator.clone(),
                health: ResourceHealth::Unknown,
            },
        );
        Ok(EffectOutcome::Accepted(ResourceInspectionResult {
            health: inspection.health,
            observed_canonical: inspection.observed_canonical,
            details: None,
            checked_at: request.issued_at,
        }))
    }
}

const fn condition_health(condition: hq_resources::PathCondition) -> ResourceHealth {
    match condition {
        hq_resources::PathCondition::Healthy => ResourceHealth::Healthy,
        hq_resources::PathCondition::Malformed | hq_resources::PathCondition::IdentityChanged => {
            ResourceHealth::Degraded
        }
        hq_resources::PathCondition::Missing
        | hq_resources::PathCondition::Inaccessible
        | hq_resources::PathCondition::NotDirectory => ResourceHealth::Unavailable,
        hq_resources::PathCondition::Unknown => ResourceHealth::Unknown,
    }
}

#[allow(clippy::expect_used, reason = "all callers pass reviewed static codes")]
fn resource_error(category: ErrorCategory, code: &'static str) -> DomainError {
    DomainError::new(
        category,
        ErrorCode::new(code).expect("static resource error code"),
    )
}
