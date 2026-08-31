//! Project worker startup, admission, repair, and shutdown contracts.

#![allow(clippy::expect_used)]

use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use hq_application::{
    ApplicationError, ControlProjects, EffectOutcome, EffectRequest, InspectResource,
    ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
    ResourceInspectionRequest, ResourceInspectionResult, ResourceReleaseState,
};
use hq_domain::{
    AccountId, CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode, FactId,
    InstallationId, OperationId, ProjectId, Timestamp,
};
use hq_node::{
    CancellationToken, ComponentDrain, NodeComponent, ProjectNodeComponent, ProjectNodeConfig,
    ReconcileProjectMessages,
};
use hq_projects::{
    AutomaticProjectCommandPlan, PlanAutomaticProjectCommands, ProjectInputReconciliation,
    ProjectWorkerPort, ReconcileProjectInputs,
};

#[derive(Clone, Default)]
struct FakeWorker {
    trace: Arc<Mutex<Vec<&'static str>>>,
    requests: Arc<Mutex<Vec<ProjectCommandRequest>>>,
    reject: Arc<AtomicBool>,
}

impl ControlProjects for FakeWorker {
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.trace.lock().expect("trace").push("control");
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        if self.reject.load(Ordering::SeqCst) {
            return Ok(ProjectCommandOutcome::Rejected {
                operation_id: request.operation_id,
                error: DomainError::new(
                    ErrorCategory::Conflict,
                    ErrorCode::new("project_busy").expect("error code"),
                ),
                runtime: None,
                external_state_warning: None,
            });
        }
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
    planned: Arc<Mutex<Vec<ProjectCommandRequest>>>,
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

impl PlanAutomaticProjectCommands for FakeInputs {
    fn plan_automatic_project_commands(
        &self,
        _limit: usize,
    ) -> Result<AutomaticProjectCommandPlan, ApplicationError> {
        self.trace.lock().expect("trace").push("plan");
        Ok(AutomaticProjectCommandPlan {
            requests: self.planned.lock().expect("planned").clone(),
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
            release: ResourceReleaseState::Clean,
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

fn dispatch_request() -> ProjectCommandRequest {
    let mut request = request();
    request.command_id = CommandId::from_bytes([11; 32]);
    request.operation_id = OperationId::from_bytes([12; 32]);
    request.request_digest = CommandDigest::from_bytes([13; 32]);
    request.action = ProjectCommandAction::DispatchPending;
    request
}

#[test]
fn startup_repairs_before_admission_and_drain_checkpoints_after_intake_closes() {
    let worker = FakeWorker::default();
    let trace = Arc::clone(&worker.trace);
    let planned = Arc::new(Mutex::new(Vec::new()));
    let mut component = ProjectNodeComponent::new(
        ProjectNodeConfig {
            recovery_limit: NonZeroUsize::new(16).expect("nonzero"),
            recovery_time: Timestamp::from_unix_millis(10),
        },
        worker,
        FakeResources,
        FakeInputs {
            trace: Arc::clone(&trace),
            planned,
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
        [
            "inputs", "repair", "plan", "control", "inputs", "plan", "inputs", "repair", "plan"
        ]
    );
}

#[test]
fn message_reconciliation_submits_the_planned_durable_dispatch_command() {
    let worker = FakeWorker::default();
    let trace = Arc::clone(&worker.trace);
    let submitted = Arc::clone(&worker.requests);
    let planned = Arc::new(Mutex::new(Vec::new()));
    let mut component = ProjectNodeComponent::new(
        ProjectNodeConfig {
            recovery_limit: NonZeroUsize::new(16).expect("nonzero"),
            recovery_time: Timestamp::from_unix_millis(10),
        },
        worker,
        FakeResources,
        FakeInputs {
            trace: Arc::clone(&trace),
            planned: Arc::clone(&planned),
        },
    );
    component
        .start(CancellationToken::new())
        .expect("startup repair succeeds");
    trace.lock().expect("trace").clear();
    planned.lock().expect("planned").push(dispatch_request());

    let reconciliation = component
        .reconcile_project_messages(16)
        .expect("message reconciliation succeeds");

    assert_eq!(reconciliation.inputs.accepted, 0);
    assert_eq!(reconciliation.automatic_commands, 1);
    assert!(!reconciliation.truncated);
    assert_eq!(
        trace.lock().expect("trace").as_slice(),
        ["inputs", "plan", "control"]
    );
    assert_eq!(
        submitted.lock().expect("submitted").as_slice(),
        [dispatch_request()]
    );
}

#[test]
fn rejected_automatic_dispatch_remains_stably_retryable() {
    let worker = FakeWorker::default();
    let trace = Arc::clone(&worker.trace);
    let submitted = Arc::clone(&worker.requests);
    worker.reject.store(true, Ordering::SeqCst);
    let planned = Arc::new(Mutex::new(vec![dispatch_request()]));
    let mut component = ProjectNodeComponent::new(
        ProjectNodeConfig {
            recovery_limit: NonZeroUsize::new(16).expect("nonzero"),
            recovery_time: Timestamp::from_unix_millis(10),
        },
        worker,
        FakeResources,
        FakeInputs { trace, planned },
    );
    component
        .start(CancellationToken::new())
        .expect("startup repair succeeds");
    submitted.lock().expect("submitted").clear();

    let first = component
        .reconcile_project_messages(16)
        .expect("first reconciliation succeeds");
    let second = component
        .reconcile_project_messages(16)
        .expect("retry reconciliation succeeds");

    assert_eq!(first.automatic_commands, 0);
    assert_eq!(second.automatic_commands, 0);
    assert_eq!(
        submitted.lock().expect("submitted").as_slice(),
        [dispatch_request(), dispatch_request()]
    );
}
