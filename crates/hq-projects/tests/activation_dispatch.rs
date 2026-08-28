//! Activation compensation and at-most-once dispatch contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
    sync::{Arc, Mutex},
};

use hq_application::{
    ApplicationError, EffectOutcome, EffectRequest, ProjectCommandAction, ProjectCommandOutcome,
    ProjectCommandRequest,
};
use hq_domain::{
    AccountId, AgentId, AssignmentBinding, BoundedText, CommandDigest, CommandId, DomainError,
    ErrorCategory, ErrorCode, FactId, InstallationId, MessageId, OperationId, ProjectId,
    ProjectResource, ProviderId, ProviderSessionId, ResourceHealth, ResourceId, ResourceLocator,
    ResourceScheme, ThreadId, Timestamp,
};
use hq_projects::{
    BeginSagaOutcome, CanonicalProjectAssignment, CanonicalProjectLifecycle,
    CanonicalProjectMutation, CanonicalProjectMutationAction, CanonicalProjectMutationOutcome,
    CanonicalProjectPort, PendingProjectInput, ProjectLaunchObservation,
    ProjectLaunchValidationRequest, ProjectResourceObservation, ProjectResourcePort,
    ProjectResourceValidationRequest, ProjectRuntimeDelivery, ProjectRuntimePort,
    ProjectRuntimeRequest, ProjectSagaRecord, ProjectSagaStore, ProjectWorkflowManager,
    ProjectWorkflowSnapshot, SagaStoreError,
};

#[derive(Clone, Default)]
struct MemorySagaStore(Arc<Mutex<Option<ProjectSagaRecord>>>);

impl ProjectSagaStore for MemorySagaStore {
    fn begin(&self, record: ProjectSagaRecord) -> Result<BeginSagaOutcome, SagaStoreError> {
        let mut retained = self.0.lock().map_err(|_| SagaStoreError::Unavailable)?;
        if let Some(existing) = retained.as_ref() {
            return Ok(
                if existing.operation_id == record.operation_id
                    && existing.command_id == record.command_id
                    && existing.request_digest == record.request_digest
                    && existing.action == record.action
                {
                    BeginSagaOutcome::Existing(existing.clone())
                } else {
                    BeginSagaOutcome::IdentityConflict
                },
            );
        }
        *retained = Some(record.clone());
        Ok(BeginSagaOutcome::Inserted(record))
    }

    fn replace(&self, record: ProjectSagaRecord) -> Result<(), SagaStoreError> {
        *self.0.lock().map_err(|_| SagaStoreError::Unavailable)? = Some(record);
        Ok(())
    }

    fn runnable(&self, limit: usize) -> Result<Vec<ProjectSagaRecord>, SagaStoreError> {
        if limit == 0 {
            return Err(SagaStoreError::Conflict);
        }
        Ok(self
            .0
            .lock()
            .map_err(|_| SagaStoreError::Unavailable)?
            .iter()
            .filter(|record| !record.state.is_terminal())
            .cloned()
            .collect())
    }
}

#[derive(Clone)]
struct ScriptedCanonical(Arc<Mutex<CanonicalState>>);

struct CanonicalState {
    snapshot: ProjectWorkflowSnapshot,
    mutations: Vec<CanonicalProjectMutationAction>,
    receipts: BTreeMap<CommandId, (CommandDigest, CanonicalProjectMutationOutcome)>,
    next_head: u8,
    uncertain_once: Option<MutationBoundary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationBoundary {
    Open,
    Configure,
    MakeRunnable,
    BeginClosing,
    RecordDispatch,
}

impl ScriptedCanonical {
    fn new(snapshot: ProjectWorkflowSnapshot) -> Self {
        Self(Arc::new(Mutex::new(CanonicalState {
            snapshot,
            mutations: Vec::new(),
            receipts: BTreeMap::new(),
            next_head: 40,
            uncertain_once: None,
        })))
    }

    fn uncertain_during_compensation(snapshot: ProjectWorkflowSnapshot) -> Self {
        Self::uncertain_once(snapshot, MutationBoundary::BeginClosing)
    }

    fn uncertain_once(snapshot: ProjectWorkflowSnapshot, boundary: MutationBoundary) -> Self {
        let canonical = Self::new(snapshot);
        canonical.0.lock().expect("canonical lock").uncertain_once = Some(boundary);
        canonical
    }

    fn snapshot_value(&self) -> ProjectWorkflowSnapshot {
        self.0.lock().expect("canonical lock").snapshot.clone()
    }

    fn mutations(&self) -> Vec<CanonicalProjectMutationAction> {
        self.0.lock().expect("canonical lock").mutations.clone()
    }
}

impl CanonicalProjectPort for ScriptedCanonical {
    fn snapshot(
        &self,
        _project_id: ProjectId,
        _account_id: AccountId,
        _requested_agent: Option<AgentId>,
    ) -> Result<ProjectWorkflowSnapshot, ApplicationError> {
        Ok(self.snapshot_value())
    }

    fn mutate(
        &self,
        mutation: CanonicalProjectMutation,
    ) -> Result<CanonicalProjectMutationOutcome, ApplicationError> {
        let mut state = self.0.lock().expect("canonical lock");
        if let Some((digest, outcome)) = state.receipts.get(&mutation.command_id) {
            return Ok(if digest == &mutation.request_digest {
                outcome.clone()
            } else {
                CanonicalProjectMutationOutcome::Rejected(domain_error(
                    ErrorCategory::Conflict,
                    "canonical-command-collision",
                ))
            });
        }
        if mutation.expected_head != state.snapshot.head {
            return Ok(CanonicalProjectMutationOutcome::Rejected(domain_error(
                ErrorCategory::Conflict,
                "stale-head",
            )));
        }
        let action = mutation.action;
        state.mutations.push(action.clone());
        match action.clone() {
            CanonicalProjectMutationAction::Open => {
                state.snapshot.lifecycle = CanonicalProjectLifecycle::Open;
            }
            CanonicalProjectMutationAction::Configure(intent) => {
                state.snapshot.assignment = Some(CanonicalProjectAssignment {
                    intent,
                    binding: None,
                    thread_id: None,
                    runnable: false,
                });
            }
            CanonicalProjectMutationAction::MakeRunnable {
                binding, thread_id, ..
            } => {
                let assignment = state.snapshot.assignment.as_mut().expect("configuring");
                assignment.binding = Some(binding);
                assignment.thread_id = Some(thread_id);
                assignment.runnable = true;
            }
            CanonicalProjectMutationAction::EndAssignment { .. } => {
                state.snapshot.assignment = None;
            }
            CanonicalProjectMutationAction::BeginClosing => {
                state.snapshot.lifecycle = CanonicalProjectLifecycle::Closing;
            }
            CanonicalProjectMutationAction::FinishClosing => {
                state.snapshot.lifecycle = CanonicalProjectLifecycle::Closed;
            }
            CanonicalProjectMutationAction::RecordDispatch { input, .. } => {
                state
                    .snapshot
                    .pending_inputs
                    .retain(|pending| pending.message_id != input.message_id);
            }
        }
        let head = FactId::from_bytes([state.next_head; 32]);
        state.next_head = state.next_head.saturating_add(1);
        state.snapshot.head = head;
        let boundary = match action {
            CanonicalProjectMutationAction::Open => Some(MutationBoundary::Open),
            CanonicalProjectMutationAction::Configure(_) => Some(MutationBoundary::Configure),
            CanonicalProjectMutationAction::MakeRunnable { .. } => {
                Some(MutationBoundary::MakeRunnable)
            }
            CanonicalProjectMutationAction::BeginClosing => Some(MutationBoundary::BeginClosing),
            CanonicalProjectMutationAction::RecordDispatch { .. } => {
                Some(MutationBoundary::RecordDispatch)
            }
            CanonicalProjectMutationAction::EndAssignment { .. }
            | CanonicalProjectMutationAction::FinishClosing => None,
        };
        if boundary.is_some() && state.uncertain_once == boundary {
            state.uncertain_once = None;
            state.receipts.insert(
                mutation.command_id,
                (
                    mutation.request_digest,
                    CanonicalProjectMutationOutcome::Committed { project_head: head },
                ),
            );
            return Ok(CanonicalProjectMutationOutcome::Uncertain);
        }
        let outcome = CanonicalProjectMutationOutcome::Committed { project_head: head };
        state.receipts.insert(
            mutation.command_id,
            (mutation.request_digest, outcome.clone()),
        );
        Ok(outcome)
    }
}

#[derive(Clone)]
struct HealthyResources;

impl ProjectResourcePort for HealthyResources {
    fn validate_resources(
        &self,
        request: &EffectRequest<ProjectResourceValidationRequest>,
    ) -> Result<EffectOutcome<Vec<ProjectResourceObservation>>, ApplicationError> {
        Ok(EffectOutcome::Accepted(
            request
                .body
                .resources
                .iter()
                .map(|resource| ProjectResourceObservation {
                    resource_id: resource.resource_id,
                    observed_canonical: Some(resource.canonical_locator.clone()),
                    health: ResourceHealth::Healthy,
                })
                .collect(),
        ))
    }

    fn validate_launch_directory(
        &self,
        request: &EffectRequest<ProjectLaunchValidationRequest>,
    ) -> Result<EffectOutcome<ProjectLaunchObservation>, ApplicationError> {
        Ok(EffectOutcome::Accepted(ProjectLaunchObservation {
            observed_canonical: request.body.launch_directory.clone(),
            health: ResourceHealth::Healthy,
            within_claims: true,
        }))
    }
}

#[derive(Clone, Copy)]
enum ResourceBehavior {
    UncertainResources,
    UncertainLaunch,
    RejectLaunch,
}

#[derive(Clone)]
struct ScriptedResources {
    behavior: ResourceBehavior,
    first_call: Arc<Mutex<bool>>,
}

impl ScriptedResources {
    fn new(behavior: ResourceBehavior) -> Self {
        Self {
            behavior,
            first_call: Arc::new(Mutex::new(true)),
        }
    }

    fn take_first_call(&self) -> bool {
        let mut first = self.first_call.lock().expect("resource lock");
        let result = *first;
        *first = false;
        result
    }
}

impl ProjectResourcePort for ScriptedResources {
    fn validate_resources(
        &self,
        request: &EffectRequest<ProjectResourceValidationRequest>,
    ) -> Result<EffectOutcome<Vec<ProjectResourceObservation>>, ApplicationError> {
        if matches!(self.behavior, ResourceBehavior::UncertainResources) && self.take_first_call() {
            return Ok(EffectOutcome::Uncertain(request.operation_id));
        }
        HealthyResources.validate_resources(request)
    }

    fn validate_launch_directory(
        &self,
        request: &EffectRequest<ProjectLaunchValidationRequest>,
    ) -> Result<EffectOutcome<ProjectLaunchObservation>, ApplicationError> {
        if matches!(self.behavior, ResourceBehavior::UncertainLaunch) && self.take_first_call() {
            return Ok(EffectOutcome::Uncertain(request.operation_id));
        }
        if matches!(self.behavior, ResourceBehavior::RejectLaunch) {
            return Ok(EffectOutcome::Rejected(domain_error(
                ErrorCategory::Unresolved,
                "launch-rejected",
            )));
        }
        HealthyResources.validate_launch_directory(request)
    }
}

#[derive(Clone, Default)]
struct ScriptedRuntime(Arc<Mutex<RuntimeState>>);

#[derive(Default)]
struct RuntimeState {
    reject_start: bool,
    uncertain_start_once: bool,
    uncertain_delivery_once: bool,
    deliveries: BTreeMap<MessageId, CommandDigest>,
    delivery_order: Vec<MessageId>,
    delivery_calls: usize,
    starts: usize,
    stops: usize,
}

impl ScriptedRuntime {
    fn rejecting_start() -> Self {
        let runtime = Self::default();
        runtime.0.lock().expect("runtime lock").reject_start = true;
        runtime
    }

    fn uncertain_delivery_once() -> Self {
        let runtime = Self::default();
        runtime
            .0
            .lock()
            .expect("runtime lock")
            .uncertain_delivery_once = true;
        runtime
    }

    fn uncertain_start_once() -> Self {
        let runtime = Self::default();
        runtime.0.lock().expect("runtime lock").uncertain_start_once = true;
        runtime
    }
}

impl ProjectRuntimePort for ScriptedRuntime {
    fn start_or_resume(
        &self,
        request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<ProviderSessionId>, ApplicationError> {
        let mut state = self.0.lock().expect("runtime lock");
        state.starts += 1;
        if state.reject_start {
            Ok(EffectOutcome::Rejected(domain_error(
                ErrorCategory::Unresolved,
                "start-rejected",
            )))
        } else if state.uncertain_start_once {
            state.uncertain_start_once = false;
            Ok(EffectOutcome::Uncertain(request.operation_id))
        } else {
            Ok(EffectOutcome::Accepted(
                ProviderSessionId::new("session-ready").expect("session"),
            ))
        }
    }

    fn deliver(
        &self,
        request: &EffectRequest<ProjectRuntimeDelivery>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        let mut state = self.0.lock().expect("runtime lock");
        state.delivery_calls += 1;
        if let Some(retained) = state.deliveries.get(&request.body.submission_id) {
            return Ok(if retained == &request.request_digest {
                EffectOutcome::Accepted(())
            } else {
                EffectOutcome::Rejected(domain_error(ErrorCategory::Conflict, "delivery-collision"))
            });
        }
        state
            .deliveries
            .insert(request.body.submission_id, request.request_digest);
        state.delivery_order.push(request.body.submission_id);
        if state.uncertain_delivery_once {
            state.uncertain_delivery_once = false;
            Ok(EffectOutcome::Uncertain(request.operation_id))
        } else {
            Ok(EffectOutcome::Accepted(()))
        }
    }

    fn stop(
        &self,
        _request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.0.lock().expect("runtime lock").stops += 1;
        Ok(EffectOutcome::Accepted(()))
    }
}

#[test]
fn closed_activation_binds_readiness_then_dispatches_each_pending_input_once() {
    let snapshot = snapshot(CanonicalProjectLifecycle::Closed);
    let expected_final_message = snapshot.pending_inputs[0].message_id;
    let canonical = ScriptedCanonical::new(snapshot);
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );

    let outcome = manager.control(activation_request()).expect("activation");
    assert!(matches!(outcome, ProjectCommandOutcome::Completed { .. }));
    let final_snapshot = canonical.snapshot_value();
    assert_eq!(final_snapshot.lifecycle, CanonicalProjectLifecycle::Open);
    assert!(final_snapshot.pending_inputs.is_empty());
    let assignment = final_snapshot.assignment.expect("runnable assignment");
    assert!(assignment.runnable);
    assert_eq!(
        assignment.binding.expect("binding").session,
        ProviderSessionId::new("session-ready").expect("session")
    );
    let runtime_state = runtime.0.lock().expect("runtime lock");
    assert_eq!(runtime_state.starts, 1);
    assert_eq!(runtime_state.deliveries.len(), 1);
    assert!(
        runtime_state
            .deliveries
            .contains_key(&expected_final_message)
    );
    drop(runtime_state);
    assert!(matches!(
        canonical.mutations().as_slice(),
        [
            CanonicalProjectMutationAction::Open,
            CanonicalProjectMutationAction::Configure(_),
            CanonicalProjectMutationAction::MakeRunnable { .. },
            CanonicalProjectMutationAction::RecordDispatch { .. }
        ]
    ));
}

#[test]
fn accepted_response_loss_reconciles_without_a_second_runtime_acceptance() {
    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let runtime = ScriptedRuntime::uncertain_delivery_once();
    let store = MemorySagaStore::default();
    let manager =
        ProjectWorkflowManager::new(store, canonical.clone(), runtime.clone(), HealthyResources);
    let request = dispatch_request();

    let first = manager.control(request.clone()).expect("first attempt");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    assert_eq!(canonical.snapshot_value().pending_inputs.len(), 1);

    let replay = manager.control(request).expect("exact replay");
    assert!(
        matches!(replay, ProjectCommandOutcome::Completed { .. }),
        "{replay:?}"
    );
    let runtime_state = runtime.0.lock().expect("runtime lock");
    assert_eq!(runtime_state.deliveries.len(), 1);
    assert_eq!(runtime_state.delivery_calls, 2);
    drop(runtime_state);
    assert_eq!(
        canonical
            .mutations()
            .iter()
            .filter(|mutation| matches!(
                mutation,
                CanonicalProjectMutationAction::RecordDispatch { .. }
            ))
            .count(),
        1
    );
}

#[test]
fn failed_start_compensates_assignment_and_newly_acquired_open_state() {
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let runtime = ScriptedRuntime::rejecting_start();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime,
        HealthyResources,
    );

    let outcome = manager
        .control(activation_request())
        .expect("activation failure");
    assert!(matches!(outcome, ProjectCommandOutcome::Rejected { .. }));
    let restored = canonical.snapshot_value();
    assert_eq!(restored.lifecycle, CanonicalProjectLifecycle::Closed);
    assert!(restored.assignment.is_none());
    assert_eq!(restored.pending_inputs.len(), 1);
    assert!(matches!(
        canonical.mutations().as_slice(),
        [
            CanonicalProjectMutationAction::Open,
            CanonicalProjectMutationAction::Configure(_),
            CanonicalProjectMutationAction::EndAssignment { .. },
            CanonicalProjectMutationAction::BeginClosing,
            CanonicalProjectMutationAction::FinishClosing
        ]
    ));
}

#[test]
fn stale_head_rejects_before_resource_or_runtime_effects() {
    let mut request = activation_request();
    request.expected_head = FactId::from_bytes([99; 32]);
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );

    let outcome = manager.control(request).expect("stale rejection");
    assert!(matches!(outcome, ProjectCommandOutcome::Rejected { .. }));
    assert!(canonical.mutations().is_empty());
    assert_eq!(runtime.0.lock().expect("runtime lock").starts, 0);
}

#[test]
fn compensation_response_loss_repairs_with_the_original_failure_and_finishes_closed() {
    let canonical = ScriptedCanonical::uncertain_during_compensation(snapshot(
        CanonicalProjectLifecycle::Closed,
    ));
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        ScriptedRuntime::rejecting_start(),
        HealthyResources,
    );
    let request = activation_request();

    let first = manager
        .control(request.clone())
        .expect("uncertain compensation");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    assert_eq!(
        canonical.snapshot_value().lifecycle,
        CanonicalProjectLifecycle::Closing
    );
    assert_eq!(canonical.snapshot_value().pending_inputs.len(), 1);

    let replay = manager.control(request).expect("compensation repair");
    assert!(matches!(replay, ProjectCommandOutcome::Rejected { .. }));
    let restored = canonical.snapshot_value();
    assert_eq!(restored.lifecycle, CanonicalProjectLifecycle::Closed);
    assert!(restored.assignment.is_none());
    assert_eq!(restored.pending_inputs.len(), 1);
}

#[test]
fn committed_canonical_response_loss_reconciles_at_every_activation_boundary() {
    for boundary in [
        MutationBoundary::Open,
        MutationBoundary::Configure,
        MutationBoundary::MakeRunnable,
        MutationBoundary::RecordDispatch,
    ] {
        let canonical = ScriptedCanonical::uncertain_once(
            snapshot(CanonicalProjectLifecycle::Closed),
            boundary,
        );
        let runtime = ScriptedRuntime::default();
        let manager = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            HealthyResources,
        );
        let request = activation_request();

        let first = manager.control(request.clone()).expect("lost response");
        assert!(
            matches!(first, ProjectCommandOutcome::Reconcilable { .. }),
            "boundary {boundary:?} returned {first:?}"
        );
        let replay = manager.control(request).expect("canonical reconciliation");
        assert!(
            matches!(replay, ProjectCommandOutcome::Completed { .. }),
            "boundary {boundary:?} returned {replay:?}"
        );
        assert!(canonical.snapshot_value().pending_inputs.is_empty());
        assert_eq!(runtime.0.lock().expect("runtime lock").deliveries.len(), 1);
    }
}

#[test]
fn unknown_resource_observation_retries_before_opening_or_starting_runtime() {
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        ScriptedResources::new(ResourceBehavior::UncertainResources),
    );
    let request = activation_request();

    let first = manager.control(request.clone()).expect("unknown resources");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    assert!(canonical.mutations().is_empty());
    assert_eq!(runtime.0.lock().expect("runtime lock").starts, 0);

    let replay = manager.control(request).expect("resource reconciliation");
    assert!(matches!(replay, ProjectCommandOutcome::Completed { .. }));
}

#[test]
fn unknown_runtime_start_and_launch_observation_resume_from_durable_boundaries() {
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let runtime = ScriptedRuntime::uncertain_start_once();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );
    let request = activation_request();

    let first = manager.control(request.clone()).expect("unknown start");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    assert_eq!(runtime.0.lock().expect("runtime lock").starts, 1);
    let replay = manager.control(request).expect("runtime reconciliation");
    assert!(matches!(replay, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(runtime.0.lock().expect("runtime lock").starts, 2);

    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical,
        runtime.clone(),
        ScriptedResources::new(ResourceBehavior::UncertainLaunch),
    );
    let request = activation_request();
    let first = manager.control(request.clone()).expect("unknown launch");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    assert_eq!(runtime.0.lock().expect("runtime lock").starts, 1);
    let replay = manager.control(request).expect("launch reconciliation");
    assert!(matches!(replay, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(runtime.0.lock().expect("runtime lock").starts, 1);
}

#[test]
fn definite_launch_failure_stops_runtime_and_restores_prior_stable_state() {
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        ScriptedResources::new(ResourceBehavior::RejectLaunch),
    );

    let outcome = manager
        .control(activation_request())
        .expect("launch rejection");

    assert!(matches!(outcome, ProjectCommandOutcome::Rejected { .. }));
    let restored = canonical.snapshot_value();
    assert_eq!(restored.lifecycle, CanonicalProjectLifecycle::Closed);
    assert!(restored.assignment.is_none());
    assert_eq!(restored.pending_inputs.len(), 1);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
}

#[test]
fn an_already_open_project_remains_open_when_runtime_start_fails() {
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Open));
    let runtime = ScriptedRuntime::rejecting_start();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime,
        HealthyResources,
    );

    let outcome = manager
        .control(activation_request())
        .expect("activation failure");

    assert!(matches!(outcome, ProjectCommandOutcome::Rejected { .. }));
    let restored = canonical.snapshot_value();
    assert_eq!(restored.lifecycle, CanonicalProjectLifecycle::Open);
    assert!(restored.assignment.is_none());
    assert_eq!(restored.pending_inputs.len(), 1);
    assert!(matches!(
        canonical.mutations().as_slice(),
        [
            CanonicalProjectMutationAction::Configure(_),
            CanonicalProjectMutationAction::EndAssignment { .. }
        ]
    ));
}

#[test]
fn explicit_resume_uses_the_requested_historical_thread_and_exact_session() {
    let mut initial = snapshot(CanonicalProjectLifecycle::Open);
    initial.pending_inputs.clear();
    let historical_thread = ThreadId::from_bytes([31; 32]);
    initial.historical_threads.insert(historical_thread);
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime,
        HealthyResources,
    );
    let mut request = activation_request();
    if let ProjectCommandAction::Activate {
        resume_session,
        resume_thread,
        ..
    } = &mut request.action
    {
        *resume_session = Some(ProviderSessionId::new("session-ready").expect("session"));
        *resume_thread = Some(historical_thread);
    }

    let outcome = manager.control(request).expect("historical resume");

    assert!(matches!(outcome, ProjectCommandOutcome::Completed { .. }));
    let assignment = canonical
        .snapshot_value()
        .assignment
        .expect("runnable assignment");
    assert_eq!(assignment.thread_id, Some(historical_thread));
    assert_eq!(
        assignment.binding.expect("runtime binding").session,
        ProviderSessionId::new("session-ready").expect("session")
    );
}

#[test]
fn missing_explicit_resume_thread_compensates_without_consuming_pending_input() {
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );
    let mut request = activation_request();
    if let ProjectCommandAction::Activate { resume_thread, .. } = &mut request.action {
        *resume_thread = Some(ThreadId::from_bytes([99; 32]));
    }

    let outcome = manager.control(request).expect("missing thread rejection");

    assert!(matches!(outcome, ProjectCommandOutcome::Rejected { .. }));
    let restored = canonical.snapshot_value();
    assert_eq!(restored.lifecycle, CanonicalProjectLifecycle::Closed);
    assert!(restored.assignment.is_none());
    assert_eq!(restored.pending_inputs.len(), 1);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
}

#[test]
fn authority_claim_and_agent_conflicts_reject_before_external_effects() {
    for (inactive_human, unclaimable, unavailable_agent, expected_code) in [
        (true, false, false, "project_inactive_human"),
        (false, true, false, "project_resource_claim_conflict"),
        (false, false, true, "project_agent_unavailable"),
    ] {
        let mut initial = snapshot(CanonicalProjectLifecycle::Closed);
        initial.active_human = !inactive_human;
        initial.claimable = !unclaimable;
        initial.requested_agent_available = !unavailable_agent;
        let canonical = ScriptedCanonical::new(initial);
        let runtime = ScriptedRuntime::default();
        let manager = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            HealthyResources,
        );

        let outcome = manager.control(activation_request()).expect("rejection");

        let ProjectCommandOutcome::Rejected { error, .. } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(error.code().as_str(), expected_code);
        assert!(canonical.mutations().is_empty());
        assert_eq!(runtime.0.lock().expect("runtime lock").starts, 0);
    }
}

#[test]
fn dispatch_drains_multiple_inputs_in_authoritative_sequence_order() {
    let mut initial = runnable_snapshot();
    let mut second = pending_input();
    second.message_id = MessageId::from_bytes([25; 32]);
    second.input_fact_id = FactId::from_bytes([26; 32]);
    second.accepted_fact = FactId::from_bytes([27; 32]);
    second.sequence = NonZeroU64::new(2).expect("nonzero");
    second.body = hq_domain::ContentText::new("then verify it").expect("body");
    initial.pending_inputs.push(second);
    let expected_order = initial
        .pending_inputs
        .iter()
        .map(|input| input.message_id)
        .collect::<Vec<_>>();
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );

    let outcome = manager
        .control(dispatch_request())
        .expect("ordered dispatch");

    assert!(matches!(outcome, ProjectCommandOutcome::Completed { .. }));
    assert!(canonical.snapshot_value().pending_inputs.is_empty());
    assert_eq!(
        runtime.0.lock().expect("runtime lock").delivery_order,
        expected_order
    );
}

#[test]
fn changed_input_under_one_submission_identity_is_rejected_and_remains_pending() {
    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let runtime = ScriptedRuntime::uncertain_delivery_once();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );
    let request = dispatch_request();

    let first = manager
        .control(request.clone())
        .expect("uncertain delivery");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    canonical
        .0
        .lock()
        .expect("canonical lock")
        .snapshot
        .pending_inputs[0]
        .body = hq_domain::ContentText::new("changed input").expect("body");

    let replay = manager.control(request).expect("changed replay");

    let ProjectCommandOutcome::Rejected { error, .. } = replay else {
        panic!("expected collision, got {replay:?}");
    };
    assert_eq!(error.code().as_str(), "delivery-collision");
    assert_eq!(canonical.snapshot_value().pending_inputs.len(), 1);
    assert_eq!(runtime.0.lock().expect("runtime lock").deliveries.len(), 1);
    assert!(canonical.mutations().is_empty());
}

fn activation_request() -> ProjectCommandRequest {
    ProjectCommandRequest {
        command_id: CommandId::from_bytes([1; 32]),
        operation_id: OperationId::from_bytes([2; 32]),
        request_digest: CommandDigest::from_bytes([3; 32]),
        account_id: AccountId::from_bytes([4; 32]),
        project_id: ProjectId::from_bytes([5; 32]),
        home: InstallationId::from_bytes([6; 32]),
        expected_head: FactId::from_bytes([7; 32]),
        issued_at: Timestamp::from_unix_millis(8),
        action: ProjectCommandAction::Activate {
            agent_id: AgentId::from_bytes([9; 32]),
            provider: ProviderId::new("provider").expect("provider"),
            resume_session: None,
            resume_thread: None,
            launch_directory: locator("/work/project"),
        },
    }
}

fn dispatch_request() -> ProjectCommandRequest {
    ProjectCommandRequest {
        command_id: CommandId::from_bytes([11; 32]),
        operation_id: OperationId::from_bytes([12; 32]),
        request_digest: CommandDigest::from_bytes([13; 32]),
        account_id: AccountId::from_bytes([4; 32]),
        project_id: ProjectId::from_bytes([5; 32]),
        home: InstallationId::from_bytes([6; 32]),
        expected_head: FactId::from_bytes([7; 32]),
        issued_at: Timestamp::from_unix_millis(8),
        action: ProjectCommandAction::DispatchPending,
    }
}

fn snapshot(lifecycle: CanonicalProjectLifecycle) -> ProjectWorkflowSnapshot {
    let input = pending_input();
    ProjectWorkflowSnapshot {
        project_id: ProjectId::from_bytes([5; 32]),
        home: InstallationId::from_bytes([6; 32]),
        head: FactId::from_bytes([7; 32]),
        lifecycle,
        archived: false,
        resources: vec![ProjectResource {
            resource_id: ResourceId::from_bytes([14; 32]),
            display_locator: locator("/work/project"),
            canonical_locator: locator("/work/project"),
            health: ResourceHealth::Unknown,
        }],
        claimable: true,
        assignment: None,
        active_human: true,
        requested_agent_available: true,
        pending_inputs: vec![input.clone()],
        historical_threads: BTreeSet::from([input.thread_id]),
    }
}

fn runnable_snapshot() -> ProjectWorkflowSnapshot {
    let mut snapshot = snapshot(CanonicalProjectLifecycle::Open);
    snapshot.assignment = Some(CanonicalProjectAssignment {
        intent: hq_domain::AssignmentIntent {
            assignment_id: hq_domain::AssignmentId::from_bytes([20; 32]),
            agent_id: AgentId::from_bytes([9; 32]),
            provider: ProviderId::new("provider").expect("provider"),
        },
        binding: Some(AssignmentBinding {
            assignment_id: hq_domain::AssignmentId::from_bytes([20; 32]),
            agent_id: AgentId::from_bytes([9; 32]),
            provider: ProviderId::new("provider").expect("provider"),
            session: ProviderSessionId::new("session-ready").expect("session"),
        }),
        thread_id: Some(ThreadId::from_bytes([18; 32])),
        runnable: true,
    });
    snapshot
}

fn pending_input() -> PendingProjectInput {
    PendingProjectInput {
        message_id: MessageId::from_bytes([15; 32]),
        input_fact_id: FactId::from_bytes([16; 32]),
        accepted_fact: FactId::from_bytes([17; 32]),
        sequence: NonZeroU64::new(1).expect("nonzero"),
        thread_id: ThreadId::from_bytes([18; 32]),
        body: hq_domain::ContentText::new("ship it").expect("body"),
    }
}

fn locator(path: &str) -> ResourceLocator {
    ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new(path).expect("locator"),
    )
}

fn domain_error(category: ErrorCategory, code: &str) -> DomainError {
    DomainError::new(category, ErrorCode::new(code).expect("error code"))
}
