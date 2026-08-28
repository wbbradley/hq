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
    ResourceScheme, RuntimeObservation, ThreadId, Timestamp,
};
use hq_projects::{
    BeginSagaOutcome, CanonicalProjectAssignment, CanonicalProjectLifecycle,
    CanonicalProjectMutation, CanonicalProjectMutationAction, CanonicalProjectMutationOutcome,
    CanonicalProjectPort, PendingProjectInput, ProjectLaunchObservation,
    ProjectLaunchValidationRequest, ProjectReleaseAssessmentRequest, ProjectResourceObservation,
    ProjectResourcePort, ProjectResourceValidationRequest, ProjectRuntimeDelivery,
    ProjectRuntimePort, ProjectRuntimeRequest, ProjectSagaRecord, ProjectSagaStore,
    ProjectWorkflowManager, ProjectWorkflowSnapshot, SagaStoreError,
};
use hq_resources::{PathReleaseAssessment, PathReleaseState};

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
    retired_agents: BTreeSet<AgentId>,
    receipts: BTreeMap<CommandId, (CommandDigest, CanonicalProjectMutationOutcome)>,
    next_head: u8,
    uncertain_once: Option<MutationBoundary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationBoundary {
    Open,
    AddResource,
    ReplaceResource,
    Configure,
    MakeRunnable,
    BeginClosing,
    EndAssignment,
    BlockAssignment,
    FinishClosing,
    Archive,
    Unarchive,
    RetireAgent,
    RecordDispatch,
}

impl ScriptedCanonical {
    fn new(snapshot: ProjectWorkflowSnapshot) -> Self {
        Self(Arc::new(Mutex::new(CanonicalState {
            snapshot,
            mutations: Vec::new(),
            retired_agents: BTreeSet::new(),
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
        requested_agent: Option<AgentId>,
    ) -> Result<ProjectWorkflowSnapshot, ApplicationError> {
        let state = self.0.lock().expect("canonical lock");
        let mut snapshot = state.snapshot.clone();
        if requested_agent.is_some_and(|agent| state.retired_agents.contains(&agent)) {
            snapshot.requested_agent_available = false;
        }
        Ok(snapshot)
    }

    #[allow(clippy::too_many_lines, reason = "single closed fake transition table")]
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
        if matches!(
            action,
            CanonicalProjectMutationAction::RemoveResource { force: false, .. }
        ) && state.snapshot.assignment.is_some()
        {
            return Ok(CanonicalProjectMutationOutcome::Rejected(domain_error(
                ErrorCategory::Conflict,
                "resource-force-required",
            )));
        }
        state.mutations.push(action.clone());
        match action.clone() {
            CanonicalProjectMutationAction::Open => {
                state.snapshot.lifecycle = CanonicalProjectLifecycle::Open;
            }
            CanonicalProjectMutationAction::AddResource { resource, .. } => {
                state.snapshot.resources.push(resource);
            }
            CanonicalProjectMutationAction::RemoveResource { resource_id, .. } => {
                state
                    .snapshot
                    .resources
                    .retain(|resource| resource.resource_id != resource_id);
            }
            CanonicalProjectMutationAction::ReplaceResource {
                old_resource_id,
                new_resource,
            } => {
                state
                    .snapshot
                    .resources
                    .retain(|resource| resource.resource_id != old_resource_id);
                state.snapshot.resources.push(new_resource);
            }
            CanonicalProjectMutationAction::Configure(intent) => {
                state.snapshot.assignment = Some(CanonicalProjectAssignment {
                    intent,
                    binding: None,
                    thread_id: None,
                    runnable: false,
                    blocked: false,
                });
            }
            CanonicalProjectMutationAction::MakeRunnable {
                binding, thread_id, ..
            } => {
                let assignment = state.snapshot.assignment.as_mut().expect("configuring");
                assignment.binding = Some(binding);
                assignment.thread_id = Some(thread_id);
                assignment.runnable = true;
                assignment.blocked = false;
            }
            CanonicalProjectMutationAction::EndAssignment { .. } => {
                state.snapshot.assignment = None;
            }
            CanonicalProjectMutationAction::BlockAssignment { .. } => {
                let assignment = state.snapshot.assignment.as_mut().expect("assigned");
                assignment.runnable = false;
                assignment.blocked = true;
            }
            CanonicalProjectMutationAction::BeginClosing => {
                state.snapshot.lifecycle = CanonicalProjectLifecycle::Closing;
            }
            CanonicalProjectMutationAction::FinishClosing { .. } => {
                state.snapshot.lifecycle = CanonicalProjectLifecycle::Closed;
            }
            CanonicalProjectMutationAction::Archive => state.snapshot.archived = true,
            CanonicalProjectMutationAction::Unarchive => state.snapshot.archived = false,
            CanonicalProjectMutationAction::RetireAgent { agent_id } => {
                state.retired_agents.insert(agent_id);
            }
            CanonicalProjectMutationAction::RecordDispatch { input, .. } => {
                state
                    .snapshot
                    .pending_inputs
                    .retain(|pending| pending.message_id != input.message_id);
            }
        }
        let head = if matches!(action, CanonicalProjectMutationAction::RetireAgent { .. }) {
            state.snapshot.head
        } else {
            let head = FactId::from_bytes([state.next_head; 32]);
            state.next_head = state.next_head.saturating_add(1);
            state.snapshot.head = head;
            head
        };
        let boundary = match action {
            CanonicalProjectMutationAction::Open => Some(MutationBoundary::Open),
            CanonicalProjectMutationAction::AddResource { .. } => {
                Some(MutationBoundary::AddResource)
            }
            CanonicalProjectMutationAction::ReplaceResource { .. } => {
                Some(MutationBoundary::ReplaceResource)
            }
            CanonicalProjectMutationAction::Configure(_) => Some(MutationBoundary::Configure),
            CanonicalProjectMutationAction::MakeRunnable { .. } => {
                Some(MutationBoundary::MakeRunnable)
            }
            CanonicalProjectMutationAction::BeginClosing => Some(MutationBoundary::BeginClosing),
            CanonicalProjectMutationAction::EndAssignment { .. } => {
                Some(MutationBoundary::EndAssignment)
            }
            CanonicalProjectMutationAction::BlockAssignment { .. } => {
                Some(MutationBoundary::BlockAssignment)
            }
            CanonicalProjectMutationAction::RetireAgent { .. } => {
                Some(MutationBoundary::RetireAgent)
            }
            CanonicalProjectMutationAction::FinishClosing { .. } => {
                Some(MutationBoundary::FinishClosing)
            }
            CanonicalProjectMutationAction::Archive => Some(MutationBoundary::Archive),
            CanonicalProjectMutationAction::Unarchive => Some(MutationBoundary::Unarchive),
            CanonicalProjectMutationAction::RecordDispatch { .. } => {
                Some(MutationBoundary::RecordDispatch)
            }
            CanonicalProjectMutationAction::RemoveResource { .. } => None,
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

    fn assess_release(
        &self,
        request: &EffectRequest<ProjectReleaseAssessmentRequest>,
    ) -> Result<EffectOutcome<Vec<PathReleaseAssessment>>, ApplicationError> {
        Ok(EffectOutcome::Accepted(
            request
                .body
                .resources
                .iter()
                .map(|resource| PathReleaseAssessment {
                    home: request.body.home,
                    resource_id: resource.resource_id,
                    state: PathReleaseState::NotApplicable,
                    worktree_identity: None,
                    common_git_directory: None,
                    changes: BTreeSet::new(),
                    changed_entries: 0,
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
    ChangedResource,
    UncertainLaunch,
    RejectLaunch,
    DirtyRelease,
    UnknownRelease,
    RejectRelease,
    UncertainRelease,
    ChangedRelease,
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
        if matches!(self.behavior, ResourceBehavior::ChangedResource) {
            return Ok(EffectOutcome::Accepted(
                request
                    .body
                    .resources
                    .iter()
                    .map(|resource| ProjectResourceObservation {
                        resource_id: resource.resource_id,
                        observed_canonical: Some(locator("/observed/elsewhere")),
                        health: ResourceHealth::Healthy,
                    })
                    .collect(),
            ));
        }
        HealthyResources.validate_resources(request)
    }

    fn assess_release(
        &self,
        request: &EffectRequest<ProjectReleaseAssessmentRequest>,
    ) -> Result<EffectOutcome<Vec<PathReleaseAssessment>>, ApplicationError> {
        if matches!(self.behavior, ResourceBehavior::RejectRelease) {
            return Ok(EffectOutcome::Rejected(domain_error(
                ErrorCategory::Unresolved,
                "release-rejected",
            )));
        }
        if matches!(self.behavior, ResourceBehavior::UncertainRelease) && self.take_first_call() {
            return Ok(EffectOutcome::Uncertain(request.operation_id));
        }
        let mut outcome = HealthyResources.assess_release(request)?;
        let EffectOutcome::Accepted(assessments) = &mut outcome else {
            return Ok(outcome);
        };
        if let Some(first) = assessments.first_mut() {
            match self.behavior {
                ResourceBehavior::DirtyRelease => first.state = PathReleaseState::Dirty,
                ResourceBehavior::UnknownRelease => first.state = PathReleaseState::Unknown,
                ResourceBehavior::ChangedRelease => {
                    first.home = InstallationId::from_bytes([99; 32]);
                }
                ResourceBehavior::UncertainResources
                | ResourceBehavior::ChangedResource
                | ResourceBehavior::UncertainLaunch
                | ResourceBehavior::RejectLaunch
                | ResourceBehavior::RejectRelease
                | ResourceBehavior::UncertainRelease => {}
            }
        }
        Ok(outcome)
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent one-shot failure switches keep this workflow fake explicit"
)]
struct RuntimeState {
    reject_start: bool,
    uncertain_start_once: bool,
    uncertain_delivery_once: bool,
    reject_stop: bool,
    uncertain_stop_once: bool,
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

    fn rejecting_stop() -> Self {
        let runtime = Self::default();
        runtime.0.lock().expect("runtime lock").reject_stop = true;
        runtime
    }

    fn uncertain_stop_once() -> Self {
        let runtime = Self::default();
        runtime.0.lock().expect("runtime lock").uncertain_stop_once = true;
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
        request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        let mut state = self.0.lock().expect("runtime lock");
        state.stops += 1;
        if state.reject_stop {
            Ok(EffectOutcome::Rejected(domain_error(
                ErrorCategory::Unresolved,
                "stop-rejected",
            )))
        } else if state.uncertain_stop_once {
            state.uncertain_stop_once = false;
            Ok(EffectOutcome::Uncertain(request.operation_id))
        } else {
            Ok(EffectOutcome::Accepted(()))
        }
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
            CanonicalProjectMutationAction::FinishClosing { .. }
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

#[test]
fn explicit_open_and_resource_mutations_commit_one_canonical_transition_each() {
    let open_canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Closed));
    let open_manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        open_canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    );
    let open = open_manager
        .control(request_for(ProjectCommandAction::Open))
        .expect("open");
    assert!(matches!(open, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(
        open_canonical.mutations(),
        vec![CanonicalProjectMutationAction::Open]
    );

    let added = resource(30, "/work/added");
    let add_canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Open));
    let add_manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        add_canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    );
    let add = add_manager
        .control(request_for(ProjectCommandAction::AddResource {
            resource: added.clone(),
            make_primary: true,
        }))
        .expect("add");
    assert!(matches!(add, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(
        add_canonical.mutations(),
        vec![CanonicalProjectMutationAction::AddResource {
            resource: added.clone(),
            make_primary: true,
        }]
    );

    let replacement = resource(31, "/work/replacement");
    let replace_canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Open));
    let old_resource = replace_canonical.snapshot_value().resources[0].resource_id;
    let replace_manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        replace_canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    );
    let replace = replace_manager
        .control(request_for(ProjectCommandAction::ReplaceResource {
            old_resource_id: old_resource,
            new_resource: replacement.clone(),
        }))
        .expect("replace");
    assert!(matches!(replace, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(
        replace_canonical.mutations(),
        vec![CanonicalProjectMutationAction::ReplaceResource {
            old_resource_id: old_resource,
            new_resource: replacement,
        }]
    );
}

#[test]
fn assigned_resource_removal_requires_force_and_never_calls_runtime() {
    let initial = runnable_snapshot();
    let resource_id = initial.resources[0].resource_id;
    let canonical = ScriptedCanonical::new(initial.clone());
    let runtime = ScriptedRuntime::default();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );

    let rejected = manager
        .control(request_for(ProjectCommandAction::RemoveResource {
            resource_id,
            force: false,
        }))
        .expect("remove rejection");
    assert!(matches!(rejected, ProjectCommandOutcome::Rejected { .. }));
    assert_eq!(canonical.snapshot_value(), initial);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);

    let forced_canonical = ScriptedCanonical::new(runnable_snapshot());
    let forced = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        forced_canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::RemoveResource {
        resource_id,
        force: true,
    }))
    .expect("forced removal");
    assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
    assert!(forced_canonical.snapshot_value().resources.is_empty());
}

#[test]
fn direct_mutation_preconditions_reject_without_canonical_changes() {
    let mut archived = snapshot(CanonicalProjectLifecycle::Closed);
    archived.archived = true;
    let mut stale_request = request_for(ProjectCommandAction::Open);
    stale_request.expected_head = FactId::from_bytes([99; 32]);
    let mut inactive = snapshot(CanonicalProjectLifecycle::Closed);
    inactive.active_human = false;
    let mut conflicted = snapshot(CanonicalProjectLifecycle::Closed);
    conflicted.claimable = false;

    for (initial, request, expected_code) in [
        (
            archived,
            request_for(ProjectCommandAction::Open),
            "project_archived",
        ),
        (
            snapshot(CanonicalProjectLifecycle::Closed),
            stale_request,
            "project_stale_head",
        ),
        (
            inactive,
            request_for(ProjectCommandAction::Open),
            "project_inactive_human",
        ),
        (
            conflicted,
            request_for(ProjectCommandAction::Open),
            "project_resource_claim_conflict",
        ),
        (
            snapshot(CanonicalProjectLifecycle::Open),
            request_for(ProjectCommandAction::Open),
            "project_invalid_transition",
        ),
    ] {
        let canonical = ScriptedCanonical::new(initial);
        let outcome = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            ScriptedRuntime::default(),
            HealthyResources,
        )
        .control(request)
        .expect("definite rejection");
        let ProjectCommandOutcome::Rejected { error, .. } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(error.code().as_str(), expected_code);
        assert!(canonical.mutations().is_empty());
    }
}

#[test]
fn changed_resource_identity_rejects_before_canonical_mutation() {
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Open));
    let outcome = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        ScriptedRuntime::default(),
        ScriptedResources::new(ResourceBehavior::ChangedResource),
    )
    .control(request_for(ProjectCommandAction::AddResource {
        resource: resource(32, "/work/desired"),
        make_primary: false,
    }))
    .expect("changed identity rejection");

    let ProjectCommandOutcome::Rejected { error, .. } = outcome else {
        panic!("expected rejection, got {outcome:?}");
    };
    assert_eq!(error.code().as_str(), "project_resource_identity_changed");
    assert!(canonical.mutations().is_empty());
}

#[test]
fn resource_and_commit_uncertainty_repair_without_duplicate_mutation() {
    let requested = resource(33, "/work/retry");
    let request = request_for(ProjectCommandAction::AddResource {
        resource: requested.clone(),
        make_primary: false,
    });
    let store = MemorySagaStore::default();
    let canonical = ScriptedCanonical::new(snapshot(CanonicalProjectLifecycle::Open));
    let resources = ScriptedResources::new(ResourceBehavior::UncertainResources);
    let first = ProjectWorkflowManager::new(
        store.clone(),
        canonical.clone(),
        ScriptedRuntime::default(),
        resources.clone(),
    )
    .control(request.clone())
    .expect("unknown resource observation");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    assert!(canonical.mutations().is_empty());

    let repaired = ProjectWorkflowManager::new(
        store,
        canonical.clone(),
        ScriptedRuntime::default(),
        resources,
    )
    .control(request)
    .expect("resource repair");
    assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(canonical.mutations().len(), 1);

    let replacement = resource(34, "/work/atomic-retry");
    let canonical = ScriptedCanonical::uncertain_once(
        snapshot(CanonicalProjectLifecycle::Open),
        MutationBoundary::ReplaceResource,
    );
    let old_resource_id = canonical.snapshot_value().resources[0].resource_id;
    let request = request_for(ProjectCommandAction::ReplaceResource {
        old_resource_id,
        new_resource: replacement.clone(),
    });
    let store = MemorySagaStore::default();
    let first = ProjectWorkflowManager::new(
        store.clone(),
        canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    )
    .control(request.clone())
    .expect("unknown canonical response");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));

    let repaired = ProjectWorkflowManager::new(
        store,
        canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    )
    .control(request)
    .expect("canonical repair");
    assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(canonical.mutations().len(), 1);
    assert_eq!(canonical.snapshot_value().resources, vec![replacement]);
}

#[test]
fn graceful_close_quiesces_assignment_before_releasing_claims() {
    let initial = runnable_snapshot();
    let pending = initial.pending_inputs.clone();
    let resources = initial.resources.clone();
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let outcome = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::Close { force: false }))
    .expect("graceful close");

    assert!(matches!(outcome, ProjectCommandOutcome::Completed { .. }));
    let closed = canonical.snapshot_value();
    assert_eq!(closed.lifecycle, CanonicalProjectLifecycle::Closed);
    assert!(closed.assignment.is_none());
    assert_eq!(closed.resources, resources);
    assert_eq!(closed.pending_inputs, pending);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
}

#[test]
fn dirty_or_unknown_release_requires_force_before_closing() {
    for behavior in [
        ResourceBehavior::DirtyRelease,
        ResourceBehavior::UnknownRelease,
    ] {
        let initial = runnable_snapshot();
        let canonical = ScriptedCanonical::new(initial.clone());
        let runtime = ScriptedRuntime::default();
        let rejected = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            ScriptedResources::new(behavior),
        )
        .control(request_for(ProjectCommandAction::Close { force: false }))
        .expect("graceful release refusal");

        let ProjectCommandOutcome::Rejected { error, .. } = rejected else {
            panic!("expected force requirement, got {rejected:?}");
        };
        assert_eq!(error.code().as_str(), "project_release_force_required");
        assert_eq!(canonical.snapshot_value(), initial);
        assert!(canonical.mutations().is_empty());
        assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);

        let forced_canonical = ScriptedCanonical::new(runnable_snapshot());
        let forced = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            forced_canonical.clone(),
            ScriptedRuntime::default(),
            ScriptedResources::new(behavior),
        )
        .control(request_for(ProjectCommandAction::Close { force: true }))
        .expect("forced release");
        assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
        assert_eq!(
            forced_canonical.snapshot_value().lifecycle,
            CanonicalProjectLifecycle::Closed
        );
        assert!(matches!(
            forced_canonical.mutations().last(),
            Some(CanonicalProjectMutationAction::FinishClosing {
                forced: true,
                runtime: Some(RuntimeObservation::Succeeded),
            })
        ));
    }
}

#[test]
fn failed_or_unknown_release_can_only_be_overridden_by_force() {
    let rejected_canonical = ScriptedCanonical::new(runnable_snapshot());
    let rejected = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        rejected_canonical.clone(),
        ScriptedRuntime::default(),
        ScriptedResources::new(ResourceBehavior::RejectRelease),
    )
    .control(request_for(ProjectCommandAction::Close { force: false }))
    .expect("release rejection");
    assert!(matches!(rejected, ProjectCommandOutcome::Rejected { .. }));
    assert!(rejected_canonical.mutations().is_empty());

    let uncertain_store = MemorySagaStore::default();
    let uncertain_canonical = ScriptedCanonical::new(runnable_snapshot());
    let uncertain_resources = ScriptedResources::new(ResourceBehavior::UncertainRelease);
    let request = request_for(ProjectCommandAction::Close { force: false });
    let uncertain = ProjectWorkflowManager::new(
        uncertain_store.clone(),
        uncertain_canonical.clone(),
        ScriptedRuntime::default(),
        uncertain_resources.clone(),
    )
    .control(request.clone())
    .expect("unknown release");
    assert!(matches!(
        uncertain,
        ProjectCommandOutcome::Reconcilable { .. }
    ));
    assert!(uncertain_canonical.mutations().is_empty());

    let repaired = ProjectWorkflowManager::new(
        uncertain_store,
        uncertain_canonical.clone(),
        ScriptedRuntime::default(),
        uncertain_resources,
    )
    .control(request)
    .expect("release repair");
    assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));

    for behavior in [
        ResourceBehavior::RejectRelease,
        ResourceBehavior::UncertainRelease,
    ] {
        let canonical = ScriptedCanonical::new(runnable_snapshot());
        let forced = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            ScriptedRuntime::default(),
            ScriptedResources::new(behavior),
        )
        .control(request_for(ProjectCommandAction::Close { force: true }))
        .expect("forced release override");
        assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
        assert_eq!(
            canonical.snapshot_value().lifecycle,
            CanonicalProjectLifecycle::Closed
        );
    }
}

#[test]
fn changed_release_assessment_identity_is_a_definite_conflict_even_with_force() {
    for force in [false, true] {
        let canonical = ScriptedCanonical::new(runnable_snapshot());
        let outcome = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            ScriptedRuntime::default(),
            ScriptedResources::new(ResourceBehavior::ChangedRelease),
        )
        .control(request_for(ProjectCommandAction::Close { force }))
        .expect("changed release assessment");
        let ProjectCommandOutcome::Rejected { error, .. } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(error.code().as_str(), "project_release_assessment_changed");
        assert!(canonical.mutations().is_empty());
    }
}

#[test]
fn runtime_stop_failure_retains_assignment_until_a_forced_retry() {
    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let runtime = ScriptedRuntime::rejecting_stop();
    let rejected = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::Close { force: false }))
    .expect("runtime stop rejection");
    assert!(matches!(rejected, ProjectCommandOutcome::Rejected { .. }));
    let retained = canonical.snapshot_value();
    assert_eq!(retained.lifecycle, CanonicalProjectLifecycle::Closing);
    assert!(retained.assignment.is_some());
    assert!(matches!(
        canonical.mutations().as_slice(),
        [CanonicalProjectMutationAction::BeginClosing]
    ));

    let mut request = request_for(ProjectCommandAction::Close { force: true });
    request.expected_head = retained.head;
    let forced = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime,
        HealthyResources,
    )
    .control(request)
    .expect("forced runtime stop override");
    assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
    assert!(matches!(
        canonical.mutations().as_slice(),
        [
            CanonicalProjectMutationAction::BeginClosing,
            CanonicalProjectMutationAction::EndAssignment {
                forced: true,
                runtime: Some(RuntimeObservation::Failed(code)),
                ..
            },
            CanonicalProjectMutationAction::FinishClosing {
                forced: true,
                runtime: Some(RuntimeObservation::Failed(finish_code)),
            }
        ] if code.as_str() == "stop-rejected" && finish_code.as_str() == "stop-rejected"
    ));
}

#[test]
fn uncertain_runtime_stop_repairs_or_records_a_truthful_forced_close() {
    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let runtime = ScriptedRuntime::uncertain_stop_once();
    let store = MemorySagaStore::default();
    let request = request_for(ProjectCommandAction::Close { force: false });
    let manager =
        ProjectWorkflowManager::new(store, canonical.clone(), runtime.clone(), HealthyResources);
    let first = manager.control(request.clone()).expect("unknown stop");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    assert_eq!(
        canonical.snapshot_value().lifecycle,
        CanonicalProjectLifecycle::Closing
    );
    assert!(canonical.snapshot_value().assignment.is_some());
    let repaired = manager.control(request).expect("stop repair");
    assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 2);
    assert_eq!(
        canonical
            .mutations()
            .iter()
            .filter(|mutation| matches!(mutation, CanonicalProjectMutationAction::BeginClosing))
            .count(),
        1
    );

    let forced_canonical = ScriptedCanonical::new(runnable_snapshot());
    let forced = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        forced_canonical.clone(),
        ScriptedRuntime::uncertain_stop_once(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::Close { force: true }))
    .expect("forced unknown stop");
    assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
    assert!(matches!(
        forced_canonical.mutations().as_slice(),
        [
            CanonicalProjectMutationAction::BeginClosing,
            CanonicalProjectMutationAction::EndAssignment {
                forced: true,
                runtime: Some(RuntimeObservation::Uncertain(code)),
                ..
            },
            CanonicalProjectMutationAction::FinishClosing {
                forced: true,
                runtime: Some(RuntimeObservation::Uncertain(finish_code)),
            }
        ] if code.as_str() == "project_runtime_stop_unknown"
            && finish_code.as_str() == "project_runtime_stop_unknown"
    ));
}

#[test]
fn close_response_loss_replays_each_canonical_boundary_exactly_once() {
    for boundary in [
        MutationBoundary::BeginClosing,
        MutationBoundary::EndAssignment,
        MutationBoundary::FinishClosing,
    ] {
        let canonical = ScriptedCanonical::uncertain_once(runnable_snapshot(), boundary);
        let runtime = ScriptedRuntime::default();
        let manager = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            HealthyResources,
        );
        let request = request_for(ProjectCommandAction::Close { force: false });

        let first = manager
            .control(request.clone())
            .expect("lost close response");
        assert!(
            matches!(first, ProjectCommandOutcome::Reconcilable { .. }),
            "boundary {boundary:?} returned {first:?}"
        );
        let replay = manager.control(request).expect("close reconciliation");
        assert!(
            matches!(replay, ProjectCommandOutcome::Completed { .. }),
            "boundary {boundary:?} returned {replay:?}"
        );
        assert_eq!(
            canonical.snapshot_value().lifecycle,
            CanonicalProjectLifecycle::Closed
        );
        assert_eq!(canonical.mutations().len(), 3);
        assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
    }
}

#[test]
fn open_project_archive_gracefully_closes_then_hides_without_data_loss() {
    let initial = runnable_snapshot();
    let resources = initial.resources.clone();
    let pending = initial.pending_inputs.clone();
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let outcome = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::SetArchived {
        archived: true,
    }))
    .expect("open archive");

    assert!(matches!(outcome, ProjectCommandOutcome::Completed { .. }));
    let archived = canonical.snapshot_value();
    assert_eq!(archived.lifecycle, CanonicalProjectLifecycle::Closed);
    assert!(archived.archived);
    assert!(archived.assignment.is_none());
    assert_eq!(archived.resources, resources);
    assert_eq!(archived.pending_inputs, pending);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
    assert!(matches!(
        canonical.mutations().as_slice(),
        [
            CanonicalProjectMutationAction::BeginClosing,
            CanonicalProjectMutationAction::EndAssignment { .. },
            CanonicalProjectMutationAction::FinishClosing { forced: false, .. },
            CanonicalProjectMutationAction::Archive,
        ]
    ));
}

#[test]
fn archive_and_unarchive_response_loss_reconcile_exactly_once() {
    let canonical = ScriptedCanonical::uncertain_once(
        snapshot(CanonicalProjectLifecycle::Closed),
        MutationBoundary::Archive,
    );
    let store = MemorySagaStore::default();
    let manager = ProjectWorkflowManager::new(
        store,
        canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    );
    let request = request_for(ProjectCommandAction::SetArchived { archived: true });
    let first = manager
        .control(request.clone())
        .expect("lost archive response");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    let repaired = manager.control(request).expect("archive repair");
    assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(
        canonical.mutations(),
        vec![CanonicalProjectMutationAction::Archive]
    );

    let mut archived = snapshot(CanonicalProjectLifecycle::Closed);
    archived.archived = true;
    let canonical = ScriptedCanonical::uncertain_once(archived, MutationBoundary::Unarchive);
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        ScriptedRuntime::default(),
        HealthyResources,
    );
    let request = request_for(ProjectCommandAction::SetArchived { archived: false });
    let first = manager
        .control(request.clone())
        .expect("lost unarchive response");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    let repaired = manager.control(request).expect("unarchive repair");
    assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(
        canonical.mutations(),
        vec![CanonicalProjectMutationAction::Unarchive]
    );
}

#[test]
fn close_and_archive_preconditions_reject_before_external_effects() {
    let mut stale = request_for(ProjectCommandAction::Close { force: false });
    stale.expected_head = FactId::from_bytes([99; 32]);
    let mut inactive = snapshot(CanonicalProjectLifecycle::Open);
    inactive.active_human = false;
    let mut archived = snapshot(CanonicalProjectLifecycle::Closed);
    archived.archived = true;

    for (initial, request, expected_code) in [
        (
            snapshot(CanonicalProjectLifecycle::Open),
            stale,
            "project_stale_head",
        ),
        (
            inactive,
            request_for(ProjectCommandAction::Close { force: false }),
            "project_inactive_human",
        ),
        (
            snapshot(CanonicalProjectLifecycle::Closed),
            request_for(ProjectCommandAction::Close { force: false }),
            "project_invalid_transition",
        ),
        (
            archived,
            request_for(ProjectCommandAction::SetArchived { archived: true }),
            "project_archived",
        ),
    ] {
        let canonical = ScriptedCanonical::new(initial);
        let runtime = ScriptedRuntime::default();
        let outcome = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            HealthyResources,
        )
        .control(request)
        .expect("precondition rejection");
        let ProjectCommandOutcome::Rejected { error, .. } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(error.code().as_str(), expected_code);
        assert!(canonical.mutations().is_empty());
        assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);
    }
}

#[test]
fn closed_archive_and_unarchive_do_not_touch_runtime_or_resources() {
    let initial = snapshot(CanonicalProjectLifecycle::Closed);
    let resources = initial.resources.clone();
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let archived = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::SetArchived {
        archived: true,
    }))
    .expect("archive");
    assert!(matches!(archived, ProjectCommandOutcome::Completed { .. }));
    assert!(canonical.snapshot_value().archived);

    let mut unarchive_request = request_for(ProjectCommandAction::SetArchived { archived: false });
    unarchive_request.expected_head = canonical.snapshot_value().head;
    let unarchived = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(unarchive_request)
    .expect("unarchive");
    assert!(matches!(
        unarchived,
        ProjectCommandOutcome::Completed { .. }
    ));
    let restored = canonical.snapshot_value();
    assert!(!restored.archived);
    assert_eq!(restored.lifecycle, CanonicalProjectLifecycle::Closed);
    assert_eq!(restored.resources, resources);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);
}

#[test]
fn graceful_handoff_ends_the_old_assignment_before_starting_the_new_agent() {
    let initial = runnable_snapshot();
    let old_assignment = initial
        .assignment
        .as_ref()
        .expect("old assignment")
        .intent
        .assignment_id;
    let thread_id = initial.pending_inputs[0].thread_id;
    let target_agent = AgentId::from_bytes([51; 32]);
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let outcome = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::Handoff {
        agent_id: target_agent,
        provider: ProviderId::new("provider").expect("provider"),
        resume_session: Some(ProviderSessionId::new("session-ready").expect("session")),
        thread_id,
        launch_directory: locator("/work/project"),
        force_takeover: false,
    }))
    .expect("graceful handoff");

    assert!(matches!(outcome, ProjectCommandOutcome::Completed { .. }));
    let assignment = canonical
        .snapshot_value()
        .assignment
        .expect("new assignment");
    assert_eq!(assignment.intent.agent_id, target_agent);
    assert_eq!(assignment.thread_id, Some(thread_id));
    let mutations = canonical.mutations();
    assert!(matches!(
        mutations.first(),
        Some(CanonicalProjectMutationAction::EndAssignment {
            assignment_id,
            forced: false,
            runtime: Some(RuntimeObservation::Succeeded),
        }) if *assignment_id == old_assignment
    ));
    let runtime = runtime.0.lock().expect("runtime lock");
    assert_eq!(runtime.stops, 1);
    assert_eq!(runtime.starts, 1);
}

#[test]
fn graceful_handoff_failure_blocks_until_explicit_forced_takeover() {
    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let runtime = ScriptedRuntime::rejecting_stop();
    let rejected = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(handoff_action(false)))
    .expect("blocked handoff");
    let ProjectCommandOutcome::Rejected { error, .. } = rejected else {
        panic!("expected rejected handoff, got {rejected:?}");
    };
    assert_eq!(error.code().as_str(), "stop-rejected");
    let blocked = canonical.snapshot_value();
    assert!(
        blocked
            .assignment
            .as_ref()
            .is_some_and(|assignment| assignment.blocked)
    );
    assert!(matches!(
        canonical.mutations().as_slice(),
        [CanonicalProjectMutationAction::BlockAssignment { .. }]
    ));

    let mut graceful_retry = request_for(handoff_action(false));
    graceful_retry.expected_head = blocked.head;
    let still_blocked = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(graceful_retry)
    .expect("graceful retry");
    let ProjectCommandOutcome::Rejected { error, .. } = still_blocked else {
        panic!("expected force requirement, got {still_blocked:?}");
    };
    assert_eq!(error.code().as_str(), "project_handoff_force_required");
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);

    let mut forced_request = request_for(handoff_action(true));
    forced_request.expected_head = blocked.head;
    let forced = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime,
        HealthyResources,
    )
    .control(forced_request)
    .expect("forced takeover");
    assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
    assert!(matches!(
        canonical.mutations().get(1),
        Some(CanonicalProjectMutationAction::EndAssignment {
            forced: true,
            runtime: Some(RuntimeObservation::Failed(code)),
            ..
        }) if code.as_str() == "stop-rejected"
    ));
    assert_eq!(
        canonical
            .snapshot_value()
            .assignment
            .expect("replacement assignment")
            .intent
            .agent_id,
        AgentId::from_bytes([51; 32])
    );
}

#[test]
fn unknown_graceful_handoff_blocks_and_forced_unknown_records_uncertainty() {
    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let unknown = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        ScriptedRuntime::uncertain_stop_once(),
        HealthyResources,
    )
    .control(request_for(handoff_action(false)))
    .expect("unknown graceful stop");
    assert!(matches!(unknown, ProjectCommandOutcome::Rejected { .. }));
    assert!(
        canonical
            .snapshot_value()
            .assignment
            .expect("blocked")
            .blocked
    );

    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let forced = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        ScriptedRuntime::uncertain_stop_once(),
        HealthyResources,
    )
    .control(request_for(handoff_action(true)))
    .expect("forced unknown takeover");
    assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
    assert!(matches!(
        canonical.mutations().first(),
        Some(CanonicalProjectMutationAction::EndAssignment {
            forced: true,
            runtime: Some(RuntimeObservation::Uncertain(code)),
            ..
        }) if code.as_str() == "project_runtime_stop_unknown"
    ));
}

#[test]
fn failed_target_activation_after_handoff_leaves_the_project_open_and_unassigned() {
    let initial = runnable_snapshot();
    let pending = initial.pending_inputs.clone();
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::rejecting_start();
    let outcome = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(handoff_action(false)))
    .expect("target activation failure");

    assert!(matches!(outcome, ProjectCommandOutcome::Rejected { .. }));
    let project = canonical.snapshot_value();
    assert_eq!(project.lifecycle, CanonicalProjectLifecycle::Open);
    assert!(project.assignment.is_none());
    assert_eq!(project.pending_inputs, pending);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
    assert_eq!(
        canonical
            .mutations()
            .iter()
            .filter(|mutation| matches!(
                mutation,
                CanonicalProjectMutationAction::EndAssignment { .. }
            ))
            .count(),
        2
    );
}

#[test]
fn handoff_rejects_same_busy_or_threadless_target_before_runtime_effects() {
    let mut same = handoff_action(false);
    if let ProjectCommandAction::Handoff { agent_id, .. } = &mut same {
        *agent_id = AgentId::from_bytes([9; 32]);
    }
    let mut busy = runnable_snapshot();
    busy.requested_agent_available = false;
    let mut threadless = runnable_snapshot();
    threadless.historical_threads.clear();
    for (snapshot, action, code) in [
        (runnable_snapshot(), same, "project_handoff_same_agent"),
        (busy, handoff_action(false), "project_agent_unavailable"),
        (
            threadless,
            handoff_action(false),
            "project_activation_thread_missing",
        ),
    ] {
        let canonical = ScriptedCanonical::new(snapshot);
        let runtime = ScriptedRuntime::default();
        let outcome = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            HealthyResources,
        )
        .control(request_for(action))
        .expect("handoff precondition");
        let ProjectCommandOutcome::Rejected { error, .. } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert_eq!(error.code().as_str(), code);
        assert!(canonical.mutations().is_empty());
        assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);
    }
}

#[test]
fn handoff_response_loss_replays_every_authority_boundary_exactly_once() {
    for boundary in [
        MutationBoundary::EndAssignment,
        MutationBoundary::Configure,
        MutationBoundary::MakeRunnable,
        MutationBoundary::RecordDispatch,
    ] {
        let canonical = ScriptedCanonical::uncertain_once(runnable_snapshot(), boundary);
        let runtime = ScriptedRuntime::default();
        let manager = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            HealthyResources,
        );
        let request = request_for(handoff_action(false));
        let first = manager
            .control(request.clone())
            .expect("lost handoff response");
        assert!(
            matches!(first, ProjectCommandOutcome::Reconcilable { .. }),
            "boundary {boundary:?} returned {first:?}"
        );
        let repaired = manager.control(request).expect("handoff repair");
        assert!(
            matches!(repaired, ProjectCommandOutcome::Completed { .. }),
            "boundary {boundary:?} returned {repaired:?}"
        );
        assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
        assert_eq!(
            canonical
                .mutations()
                .iter()
                .filter(|mutation| matches!(
                    mutation,
                    CanonicalProjectMutationAction::EndAssignment { .. }
                ))
                .count(),
            1
        );
    }

    let canonical =
        ScriptedCanonical::uncertain_once(runnable_snapshot(), MutationBoundary::BlockAssignment);
    let runtime = ScriptedRuntime::rejecting_stop();
    let manager = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    );
    let request = request_for(handoff_action(false));
    let first = manager
        .control(request.clone())
        .expect("lost block response");
    assert!(matches!(first, ProjectCommandOutcome::Reconcilable { .. }));
    let repaired = manager.control(request).expect("block repair");
    assert!(matches!(repaired, ProjectCommandOutcome::Rejected { .. }));
    assert!(
        canonical
            .snapshot_value()
            .assignment
            .expect("blocked")
            .blocked
    );
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
    assert_eq!(canonical.mutations().len(), 1);
}

#[test]
fn assigned_agent_retirement_quiesces_without_closing_or_releasing_claims() {
    let initial = runnable_snapshot();
    let retiring_agent = initial
        .assignment
        .as_ref()
        .expect("assignment")
        .intent
        .agent_id;
    let resources = initial.resources.clone();
    let pending = initial.pending_inputs.clone();
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let outcome = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::RetireAgent {
        agent_id: retiring_agent,
        force: false,
    }))
    .expect("assigned retirement");

    assert!(matches!(outcome, ProjectCommandOutcome::Completed { .. }));
    let project = canonical.snapshot_value();
    assert_eq!(project.lifecycle, CanonicalProjectLifecycle::Open);
    assert!(project.assignment.is_none());
    assert_eq!(project.resources, resources);
    assert_eq!(project.pending_inputs, pending);
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
    assert!(matches!(
        canonical.mutations().last(),
        Some(CanonicalProjectMutationAction::RetireAgent { agent_id })
            if *agent_id == retiring_agent
    ));
}

#[test]
fn idle_retirement_skips_runtime_and_is_absorbing_for_future_selection() {
    let initial = snapshot(CanonicalProjectLifecycle::Open);
    let retiring_agent = AgentId::from_bytes([52; 32]);
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::default();
    let retired = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::RetireAgent {
        agent_id: retiring_agent,
        force: false,
    }))
    .expect("idle retirement");
    assert!(matches!(retired, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);

    let unavailable = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime,
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::Activate {
        agent_id: retiring_agent,
        provider: ProviderId::new("provider").expect("provider"),
        resume_session: None,
        resume_thread: None,
        launch_directory: locator("/work/project"),
    }))
    .expect("retired selection");
    let ProjectCommandOutcome::Rejected { error, .. } = unavailable else {
        panic!("expected retired agent rejection, got {unavailable:?}");
    };
    assert_eq!(error.code().as_str(), "project_agent_unavailable");
}

#[test]
fn handoff_rejects_a_retired_target_before_quiescing_the_current_assignment() {
    let retiring_agent = AgentId::from_bytes([51; 32]);
    let canonical = ScriptedCanonical::new(runnable_snapshot());
    let runtime = ScriptedRuntime::default();
    let retired = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::RetireAgent {
        agent_id: retiring_agent,
        force: false,
    }))
    .expect("idle target retirement");
    assert!(matches!(retired, ProjectCommandOutcome::Completed { .. }));

    let handoff = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(handoff_action(false)))
    .expect("retired target handoff");
    let ProjectCommandOutcome::Rejected { error, .. } = handoff else {
        panic!("expected retired target rejection, got {handoff:?}");
    };
    assert_eq!(error.code().as_str(), "project_agent_unavailable");
    assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);
    assert!(canonical.snapshot_value().assignment.is_some());
}

#[test]
fn assigned_retirement_blocks_on_stop_failure_then_force_retires() {
    let initial = runnable_snapshot();
    let retiring_agent = initial
        .assignment
        .as_ref()
        .expect("assignment")
        .intent
        .agent_id;
    let canonical = ScriptedCanonical::new(initial);
    let runtime = ScriptedRuntime::rejecting_stop();
    let rejected = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime.clone(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::RetireAgent {
        agent_id: retiring_agent,
        force: false,
    }))
    .expect("blocked retirement");
    assert!(matches!(rejected, ProjectCommandOutcome::Rejected { .. }));
    let blocked = canonical.snapshot_value();
    assert!(blocked.assignment.expect("blocked assignment").blocked);

    let mut request = request_for(ProjectCommandAction::RetireAgent {
        agent_id: retiring_agent,
        force: true,
    });
    request.expected_head = blocked.head;
    let forced = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        runtime,
        HealthyResources,
    )
    .control(request)
    .expect("forced retirement");
    assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
    assert!(canonical.snapshot_value().assignment.is_none());
    assert!(matches!(
        canonical.mutations().as_slice(),
        [
            CanonicalProjectMutationAction::BlockAssignment { .. },
            CanonicalProjectMutationAction::EndAssignment {
                forced: true,
                runtime: Some(RuntimeObservation::Failed(code)),
                ..
            },
            CanonicalProjectMutationAction::RetireAgent { .. },
        ] if code.as_str() == "stop-rejected"
    ));
}

#[test]
fn retirement_response_loss_replays_assignment_end_and_retirement_once() {
    for boundary in [
        MutationBoundary::EndAssignment,
        MutationBoundary::RetireAgent,
    ] {
        let initial = runnable_snapshot();
        let agent_id = initial
            .assignment
            .as_ref()
            .expect("assignment")
            .intent
            .agent_id;
        let canonical = ScriptedCanonical::uncertain_once(initial, boundary);
        let runtime = ScriptedRuntime::default();
        let manager = ProjectWorkflowManager::new(
            MemorySagaStore::default(),
            canonical.clone(),
            runtime.clone(),
            HealthyResources,
        );
        let request = request_for(ProjectCommandAction::RetireAgent {
            agent_id,
            force: false,
        });
        let first = manager
            .control(request.clone())
            .expect("lost retirement response");
        assert!(
            matches!(first, ProjectCommandOutcome::Reconcilable { .. }),
            "boundary {boundary:?} returned {first:?}"
        );
        let repaired = manager.control(request).expect("retirement repair");
        assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));
        assert_eq!(runtime.0.lock().expect("runtime lock").stops, 1);
        assert_eq!(
            canonical
                .mutations()
                .iter()
                .filter(|mutation| matches!(
                    mutation,
                    CanonicalProjectMutationAction::RetireAgent { .. }
                ))
                .count(),
            1
        );
    }
}

#[test]
fn uncertain_assigned_retirement_blocks_or_force_records_unknown_runtime() {
    let initial = runnable_snapshot();
    let agent_id = initial
        .assignment
        .as_ref()
        .expect("assignment")
        .intent
        .agent_id;
    let canonical = ScriptedCanonical::new(initial);
    let rejected = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        ScriptedRuntime::uncertain_stop_once(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::RetireAgent {
        agent_id,
        force: false,
    }))
    .expect("unknown retirement stop");
    assert!(matches!(rejected, ProjectCommandOutcome::Rejected { .. }));
    assert!(
        canonical
            .snapshot_value()
            .assignment
            .expect("blocked")
            .blocked
    );

    let initial = runnable_snapshot();
    let canonical = ScriptedCanonical::new(initial);
    let forced = ProjectWorkflowManager::new(
        MemorySagaStore::default(),
        canonical.clone(),
        ScriptedRuntime::uncertain_stop_once(),
        HealthyResources,
    )
    .control(request_for(ProjectCommandAction::RetireAgent {
        agent_id,
        force: true,
    }))
    .expect("forced unknown retirement");
    assert!(matches!(forced, ProjectCommandOutcome::Completed { .. }));
    assert!(matches!(
        canonical.mutations().first(),
        Some(CanonicalProjectMutationAction::EndAssignment {
            forced: true,
            runtime: Some(RuntimeObservation::Uncertain(code)),
            ..
        }) if code.as_str() == "project_runtime_stop_unknown"
    ));
}

#[test]
fn stale_or_inactive_handoff_and_retirement_stop_before_runtime_effects() {
    for action in [
        handoff_action(false),
        ProjectCommandAction::RetireAgent {
            agent_id: AgentId::from_bytes([9; 32]),
            force: false,
        },
    ] {
        let mut stale = request_for(action.clone());
        stale.expected_head = FactId::from_bytes([99; 32]);
        let mut inactive = runnable_snapshot();
        inactive.active_human = false;
        for (snapshot, request, code) in [
            (runnable_snapshot(), stale, "project_stale_head"),
            (
                inactive,
                request_for(action.clone()),
                "project_inactive_human",
            ),
        ] {
            let canonical = ScriptedCanonical::new(snapshot);
            let runtime = ScriptedRuntime::default();
            let outcome = ProjectWorkflowManager::new(
                MemorySagaStore::default(),
                canonical.clone(),
                runtime.clone(),
                HealthyResources,
            )
            .control(request)
            .expect("authority precondition");
            let ProjectCommandOutcome::Rejected { error, .. } = outcome else {
                panic!("expected rejection, got {outcome:?}");
            };
            assert_eq!(error.code().as_str(), code);
            assert!(canonical.mutations().is_empty());
            assert_eq!(runtime.0.lock().expect("runtime lock").stops, 0);
        }
    }
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

fn request_for(action: ProjectCommandAction) -> ProjectCommandRequest {
    let mut request = activation_request();
    request.action = action;
    request
}

fn handoff_action(force_takeover: bool) -> ProjectCommandAction {
    ProjectCommandAction::Handoff {
        agent_id: AgentId::from_bytes([51; 32]),
        provider: ProviderId::new("provider").expect("provider"),
        resume_session: Some(ProviderSessionId::new("session-ready").expect("session")),
        thread_id: ThreadId::from_bytes([18; 32]),
        launch_directory: locator("/work/project"),
        force_takeover,
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
        blocked: false,
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

fn resource(id: u8, path: &str) -> ProjectResource {
    ProjectResource {
        resource_id: ResourceId::from_bytes([id; 32]),
        display_locator: locator(path),
        canonical_locator: locator(path),
        health: ResourceHealth::Unknown,
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
