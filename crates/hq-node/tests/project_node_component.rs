//! Project worker startup, admission, repair, and shutdown contracts.

#![allow(clippy::expect_used)]

use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use hq_application::{
    ApplicationError, ControlProjects, EffectOutcome, EffectRequest, InspectResource,
    ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
    ResourceInspectionRequest, ResourceInspectionResult,
};
use hq_domain::{
    AccountId, CommandDigest, CommandId, FactId, InstallationId, OperationId, ProjectId, Timestamp,
};
use hq_node::{
    CancellationToken, ComponentDrain, NodeComponent, ProjectNodeComponent, ProjectNodeConfig,
};
use hq_projects::{ProjectInputReconciliation, ProjectWorkerPort, ReconcileProjectInputs};

#[derive(Clone, Default)]
struct FakeWorker {
    trace: Arc<Mutex<Vec<&'static str>>>,
}

impl ControlProjects for FakeWorker {
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.trace.lock().expect("trace").push("control");
        Ok(ProjectCommandOutcome::Accepted {
            operation_id: request.operation_id,
            stage: ProjectCommandStage::Accepted,
        })
    }
}

impl ProjectWorkerPort for FakeWorker {
    fn repair_pending(
        &self,
        _received_at: Timestamp,
        _limit: usize,
    ) -> Result<Vec<ProjectCommandOutcome>, ApplicationError> {
        self.trace.lock().expect("trace").push("repair");
        Ok(Vec::new())
    }
}

#[derive(Clone, Copy)]
struct FakeResources;

#[derive(Clone)]
struct FakeInputs {
    trace: Arc<Mutex<Vec<&'static str>>>,
}

impl ReconcileProjectInputs for FakeInputs {
    fn reconcile_project_inputs(
        &self,
        _limit: usize,
    ) -> Result<ProjectInputReconciliation, ApplicationError> {
        self.trace.lock().expect("trace").push("inputs");
        Ok(ProjectInputReconciliation {
            accepted: 0,
            truncated: false,
        })
    }
}

impl InspectResource for FakeResources {
    fn inspect_resource(
        &self,
        request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        Ok(EffectOutcome::Accepted(ResourceInspectionResult {
            health: hq_domain::ResourceHealth::Healthy,
            observed_canonical: Some(request.body.canonical_locator.clone()),
            details: None,
            checked_at: request.issued_at,
        }))
    }
}

fn request() -> ProjectCommandRequest {
    ProjectCommandRequest {
        command_id: CommandId::from_bytes([1; 32]),
        operation_id: OperationId::from_bytes([2; 32]),
        request_digest: CommandDigest::from_bytes([3; 32]),
        account_id: AccountId::from_bytes([4; 32]),
        project_id: ProjectId::from_bytes([5; 32]),
        home: InstallationId::from_bytes([6; 32]),
        expected_head: Some(FactId::from_bytes([7; 32])),
        issued_at: Timestamp::from_unix_millis(8),
        action: ProjectCommandAction::Open,
    }
}

#[test]
fn startup_repairs_before_admission_and_drain_checkpoints_after_intake_closes() {
    let worker = FakeWorker::default();
    let trace = Arc::clone(&worker.trace);
    let mut component = ProjectNodeComponent::new(
        ProjectNodeConfig {
            recovery_limit: NonZeroUsize::new(16).expect("nonzero"),
            recovery_time: Timestamp::from_unix_millis(10),
        },
        worker,
        FakeResources,
        FakeInputs {
            trace: Arc::clone(&trace),
        },
    );

    assert!(component.control_project(request()).is_err());
    component
        .start(CancellationToken::new())
        .expect("startup repair succeeds");
    assert!(matches!(
        component
            .control_project(request())
            .expect("intake is open"),
        ProjectCommandOutcome::Accepted { .. }
    ));
    component.stop_intake().expect("intake closes");
    assert!(component.control_project(request()).is_err());
    assert_eq!(
        component.drain().expect("drain checkpoints"),
        ComponentDrain::Complete
    );
    assert_eq!(
        trace.lock().expect("trace").as_slice(),
        ["inputs", "repair", "control", "inputs", "repair"]
    );
}
