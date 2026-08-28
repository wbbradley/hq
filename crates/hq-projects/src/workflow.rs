//! Explicit activation and at-most-once project-input dispatch workflows.

use std::{collections::BTreeSet, num::NonZeroU64};

use hq_application::{
    EffectOutcome, EffectRequest, ProjectCommandAction, ProjectCommandOutcome,
    ProjectCommandRequest, ProjectCommandStage,
};
use hq_domain::{
    AccountId, AgentId, AssignmentBinding, AssignmentId, AssignmentIntent, CommandDigest,
    CommandId, ContentText, DispatchId, DomainError, ErrorCategory, ErrorCode, FactId,
    InstallationId, MessageId, OperationCorrelation, OperationId, ProjectId, ProjectResource,
    ProviderId, ProviderSessionId, ResourceHealth, ResourceId, ResourceLocator, ThreadId,
    Timestamp,
};
use sha2::{Digest, Sha256};

use crate::{
    BeginSagaOutcome, ProjectSagaRecord, ProjectSagaState, ProjectSagaStore, SagaEffectState,
    SagaStoreError,
};

/// Maximum workflow boundaries crossed by one synchronous control call.
pub const MAX_PROJECT_WORKFLOW_ADVANCES: usize = 128;

/// Stable project lifecycle observed from one serialized canonical snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalProjectLifecycle {
    /// Desired claims may be held and an assignment may run.
    Open,
    /// New dispatch is stopped while assignment and claims remain.
    Closing,
    /// No assignment or active claim remains.
    Closed,
}

/// Current canonical assignment details required by activation and dispatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalProjectAssignment {
    /// Session-free intent committed before runtime startup.
    pub intent: AssignmentIntent,
    /// Acknowledged runtime binding, when runnable.
    pub binding: Option<AssignmentBinding>,
    /// Selected project thread, when runnable.
    pub thread_id: Option<ThreadId>,
    /// Whether canonical policy currently permits delivery.
    pub runnable: bool,
}

/// One accepted project input that has not yet received canonical dispatch attribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingProjectInput {
    /// Stable public message identity used by provider idempotency.
    pub message_id: MessageId,
    /// Original canonical message fact.
    pub input_fact_id: FactId,
    /// Home-authored acceptance fact.
    pub accepted_fact: FactId,
    /// Contiguous home sequence.
    pub sequence: NonZeroU64,
    /// Causal conversation thread containing the message.
    pub thread_id: ThreadId,
    /// Exact bounded runtime input.
    pub body: ContentText,
}

/// Complete workflow-facing canonical project snapshot from one serialized state point.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these are independent canonical observations, not one encoded state machine"
)]
pub struct ProjectWorkflowSnapshot {
    /// Target project.
    pub project_id: ProjectId,
    /// Immutable home.
    pub home: InstallationId,
    /// Current unique linear head.
    pub head: FactId,
    /// Current project lifecycle.
    pub lifecycle: CanonicalProjectLifecycle,
    /// Archived projects cannot be activated.
    pub archived: bool,
    /// All desired resources with their last canonical observations.
    pub resources: Vec<ProjectResource>,
    /// Whether the complete desired claim set is currently conflict-free.
    pub claimable: bool,
    /// Current assignment, when any.
    pub assignment: Option<CanonicalProjectAssignment>,
    /// Whether the command account is active on this exact home snapshot.
    pub active_human: bool,
    /// Whether the requested agent is active, local, idle, and cardinality-safe.
    pub requested_agent_available: bool,
    /// Accepted undispatched inputs in authoritative home order.
    pub pending_inputs: Vec<PendingProjectInput>,
    /// Historical project threads that may be explicitly resumed.
    pub historical_threads: BTreeSet<ThreadId>,
}

/// Closed canonical project mutations used by this workflow package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalProjectMutationAction {
    /// Conditionally open a closed, unarchived, claimable project.
    Open,
    /// Record session-free assignment intent.
    Configure(AssignmentIntent),
    /// Bind exact runtime readiness and the selected thread.
    MakeRunnable {
        /// Exact acknowledged runtime binding.
        binding: AssignmentBinding,
        /// Selected compatible project thread.
        thread_id: ThreadId,
        /// Revalidated explicit launch directory.
        launch_directory: ResourceLocator,
        /// Stable activation correlation.
        activation: OperationCorrelation,
    },
    /// End one exact configuring or runnable assignment.
    EndAssignment {
        /// Assignment epoch to end.
        assignment_id: AssignmentId,
    },
    /// Enter canonical closing during compensation.
    BeginClosing,
    /// Restore a project opened by this workflow to closed.
    FinishClosing,
    /// Author immutable attribution after definite provider acceptance.
    RecordDispatch {
        /// Accepted input.
        input: PendingProjectInput,
        /// Stable dispatch identity.
        dispatch_id: DispatchId,
        /// Exact current assignment binding.
        binding: AssignmentBinding,
        /// Exact current project thread.
        thread_id: ThreadId,
    },
}

/// Exact retryable canonical mutation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalProjectMutation {
    /// Stable per-boundary mutation identity.
    pub command_id: CommandId,
    /// Digest of the exact mutation input.
    pub request_digest: CommandDigest,
    /// Authorizing active human account.
    pub account_id: AccountId,
    /// Target project.
    pub project_id: ProjectId,
    /// Required immutable home.
    pub home: InstallationId,
    /// Exact head checked inside the commit transaction.
    pub expected_head: FactId,
    /// Caller-supplied semantic time.
    pub issued_at: Timestamp,
    /// Closed transition.
    pub action: CanonicalProjectMutationAction,
}

/// Transaction-consistent canonical mutation result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalProjectMutationOutcome {
    /// The exact transition committed or replayed at this head.
    Committed {
        /// Resulting unique canonical project head.
        project_head: FactId,
    },
    /// Pure snapshot policy definitely rejected the transition.
    Rejected(DomainError),
    /// Commit response was lost; the exact command must be reconciled.
    Uncertain,
}

/// Canonical project snapshot and compare-and-swap mutation capability.
pub trait CanonicalProjectPort {
    /// Loads one complete serialized workflow view.
    fn snapshot(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        requested_agent: Option<AgentId>,
    ) -> Result<ProjectWorkflowSnapshot, hq_application::ApplicationError>;

    /// Validates and commits against one transaction-consistent snapshot.
    fn mutate(
        &self,
        mutation: CanonicalProjectMutation,
    ) -> Result<CanonicalProjectMutationOutcome, hq_application::ApplicationError>;
}

/// Batched desired-resource revalidation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceValidationRequest {
    /// Immutable home namespace.
    pub home: InstallationId,
    /// Target project.
    pub project_id: ProjectId,
    /// Exact desired resources to re-resolve.
    pub resources: Vec<ProjectResource>,
}

/// One exact resource observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceObservation {
    /// Stable desired resource identity.
    pub resource_id: ResourceId,
    /// Current immutable canonical locator, when observable.
    pub observed_canonical: Option<ResourceLocator>,
    /// Current typed health.
    pub health: ResourceHealth,
}

/// Explicit launch-directory revalidation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLaunchValidationRequest {
    /// Immutable home namespace.
    pub home: InstallationId,
    /// Target project.
    pub project_id: ProjectId,
    /// Human-selected exact launch directory.
    pub launch_directory: ResourceLocator,
    /// Current desired resources that should cover the directory.
    pub resources: Vec<ProjectResource>,
}

/// Passive launch-directory observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLaunchObservation {
    /// Current canonical launch identity.
    pub observed_canonical: ResourceLocator,
    /// Current typed health.
    pub health: ResourceHealth,
    /// Whether one current desired claim equals or contains the directory.
    pub within_claims: bool,
}

/// Read-only resource observation capability used around external runtime startup.
pub trait ProjectResourcePort {
    /// Revalidates every desired resource as one stable read-only operation.
    fn validate_resources(
        &self,
        request: &EffectRequest<ProjectResourceValidationRequest>,
    ) -> Result<EffectOutcome<Vec<ProjectResourceObservation>>, hq_application::ApplicationError>;

    /// Revalidates the exact launch directory after runtime readiness.
    fn validate_launch_directory(
        &self,
        request: &EffectRequest<ProjectLaunchValidationRequest>,
    ) -> Result<EffectOutcome<ProjectLaunchObservation>, hq_application::ApplicationError>;
}

/// Project-bound runtime startup or exact-resume request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeRequest {
    /// Target project.
    pub project_id: ProjectId,
    /// Assigned durable agent.
    pub agent_id: AgentId,
    /// Selected provider namespace.
    pub provider: ProviderId,
    /// Exact durable session to resume, or absence for a fresh session.
    pub resume_session: Option<ProviderSessionId>,
}

/// One exact project input routed through the sole durable runtime delivery ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeDelivery {
    /// Target project.
    pub project_id: ProjectId,
    /// Exact active assignment binding.
    pub binding: AssignmentBinding,
    /// Exact selected project thread.
    pub thread_id: ThreadId,
    /// Stable provider submission identity.
    pub submission_id: MessageId,
    /// Authoritative home input sequence.
    pub sequence: NonZeroU64,
    /// Exact bounded input body.
    pub body: ContentText,
}

/// Project-bound runtime and durable exact-delivery capability.
pub trait ProjectRuntimePort {
    /// Starts or exactly resumes one project-bound logical worker.
    fn start_or_resume(
        &self,
        request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<ProviderSessionId>, hq_application::ApplicationError>;

    /// Reconciles before retry and reports acceptance only from the sole durable delivery ledger.
    fn deliver(
        &self,
        request: &EffectRequest<ProjectRuntimeDelivery>,
    ) -> Result<EffectOutcome<()>, hq_application::ApplicationError>;

    /// Stops a non-runnable project worker during compensation using a stable identity.
    fn stop(
        &self,
        request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<()>, hq_application::ApplicationError>;
}

/// Serialized bounded owner of activation, compensation, and pending-input dispatch.
pub struct ProjectWorkflowManager<S, C, R, F> {
    store: S,
    canonical: C,
    runtime: R,
    resources: F,
}

impl<S, C, R, F> ProjectWorkflowManager<S, C, R, F>
where
    S: ProjectSagaStore,
    C: CanonicalProjectPort,
    R: ProjectRuntimePort,
    F: ProjectResourcePort,
{
    /// Owns the four capabilities used by the explicit workflow state machines.
    pub const fn new(store: S, canonical: C, runtime: R, resources: F) -> Self {
        Self {
            store,
            canonical,
            runtime,
            resources,
        }
    }

    /// Accepts and executes a bounded amount of one exact activation or dispatch command.
    pub fn control(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, hq_application::ApplicationError> {
        let operation_id = request.operation_id;
        let proposed = ProjectSagaRecord::from_request(request);
        let record = match self.store.begin(proposed).map_err(store_error)? {
            BeginSagaOutcome::Inserted(record) | BeginSagaOutcome::Existing(record) => record,
            BeginSagaOutcome::IdentityConflict => {
                return Ok(ProjectCommandOutcome::Rejected {
                    operation_id,
                    error: error(ErrorCategory::Conflict, "project_command_identity_conflict"),
                });
            }
            BeginSagaOutcome::ProjectBusy => {
                return Ok(ProjectCommandOutcome::Rejected {
                    operation_id,
                    error: error(ErrorCategory::Conflict, "project_command_in_progress"),
                });
            }
        };
        self.run(record)
    }

    /// Repairs one bounded deterministic set of nonterminal workflows.
    pub fn repair(
        &self,
        limit: usize,
    ) -> Result<Vec<ProjectCommandOutcome>, hq_application::ApplicationError> {
        if limit == 0 || limit > crate::MAX_RUNNABLE_SAGAS {
            return Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::InvalidRequest,
            ));
        }
        self.store
            .runnable(limit)
            .map_err(store_error)?
            .into_iter()
            .map(|record| self.run(record))
            .collect()
    }

    fn run(
        &self,
        mut record: ProjectSagaRecord,
    ) -> Result<ProjectCommandOutcome, hq_application::ApplicationError> {
        if let Some(outcome) = terminal_outcome(&record) {
            return Ok(outcome);
        }
        for _ in 0..MAX_PROJECT_WORKFLOW_ADVANCES {
            match record.action.clone() {
                ProjectCommandAction::Activate {
                    agent_id,
                    provider,
                    resume_session,
                    resume_thread,
                    launch_directory,
                } => self.advance_activation(
                    &mut record,
                    agent_id,
                    provider,
                    resume_session,
                    resume_thread,
                    launch_directory,
                )?,
                ProjectCommandAction::DispatchPending => self.advance_dispatch(&mut record)?,
                _ => reject(
                    &self.store,
                    &mut record,
                    error(
                        ErrorCategory::InvalidInput,
                        "project_action_not_implemented",
                    ),
                )?,
            }
            if let Some(outcome) = terminal_outcome(&record) {
                return Ok(outcome);
            }
            if matches!(record.state, ProjectSagaState::Reconcilable { .. }) {
                return Ok(progress_outcome(&record));
            }
        }
        Ok(progress_outcome(&record))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn advance_activation(
        &self,
        record: &mut ProjectSagaRecord,
        agent_id: AgentId,
        provider: ProviderId,
        resume_session: Option<ProviderSessionId>,
        resume_thread: Option<ThreadId>,
        launch_directory: ResourceLocator,
    ) -> Result<(), hq_application::ApplicationError> {
        let snapshot =
            self.canonical
                .snapshot(record.project_id, record.account_id, Some(agent_id))?;
        if snapshot.home != record.home || snapshot.project_id != record.project_id {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Unauthorized, "project_wrong_home"),
            );
        }
        let stage = current_stage(record);
        if stage == ProjectCommandStage::Compensating {
            let cause = record.failure.clone().ok_or_else(|| {
                hq_application::ApplicationError::new(
                    hq_application::ApplicationErrorCode::StateCorrupt,
                )
            })?;
            return self.compensate(record, agent_id, provider, resume_session, cause);
        }
        if record.pending_canonical_mutation.is_some() {
            match self.replay_canonical_mutation(record)? {
                CanonicalProjectMutationOutcome::Committed { .. } => {
                    return checkpoint(&self.store, record, stage);
                }
                CanonicalProjectMutationOutcome::Rejected(error)
                    if stage == ProjectCommandStage::Opening =>
                {
                    return reject(&self.store, record, error);
                }
                CanonicalProjectMutationOutcome::Rejected(error) => {
                    return self.compensate(record, agent_id, provider, resume_session, error);
                }
                CanonicalProjectMutationOutcome::Uncertain => {
                    return reconcile(
                        &self.store,
                        record,
                        stage,
                        effect_error("project_canonical_commit_unknown"),
                        EffectKind::None,
                    );
                }
            }
        }
        if stage == ProjectCommandStage::Accepted {
            if snapshot.head != record.expected_head {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_stale_head"),
                );
            }
            if !snapshot.active_human {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Unauthorized, "project_inactive_human"),
                );
            }
            if snapshot.archived {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_archived"),
                );
            }
            if !snapshot.claimable {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_resource_claim_conflict"),
                );
            }
            if !snapshot.requested_agent_available {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_agent_unavailable"),
                );
            }
            if snapshot.lifecycle == CanonicalProjectLifecycle::Closing
                || snapshot.assignment.is_some()
            {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_assignment_conflict"),
                );
            }
            checkpoint(
                &self.store,
                record,
                ProjectCommandStage::ValidatingResources,
            )?;
            return Ok(());
        }

        if stage == ProjectCommandStage::ValidatingResources {
            let operation = derived_operation(record.operation_id, b"resource-validation", &[]);
            record.resource_operation_id = Some(operation);
            if matches!(record.resource_effect, SagaEffectState::NotStarted) {
                record.resource_effect = SagaEffectState::Pending;
                persist(&self.store, record)?;
            }
            let request = EffectRequest {
                operation_id: operation,
                request_digest: derived_digest(record.operation_id, b"resource-validation", &[]),
                issued_at: record.issued_at,
                body: ProjectResourceValidationRequest {
                    home: record.home,
                    project_id: record.project_id,
                    resources: snapshot.resources.clone(),
                },
            };
            match self.resources.validate_resources(&request)? {
                EffectOutcome::Accepted(observations)
                    if resources_are_healthy(&snapshot.resources, &observations) =>
                {
                    record.resource_effect = SagaEffectState::Accepted;
                    checkpoint(&self.store, record, ProjectCommandStage::Opening)
                }
                EffectOutcome::Accepted(_) | EffectOutcome::Rejected(_) => self.compensate(
                    record,
                    agent_id,
                    provider,
                    resume_session,
                    error(ErrorCategory::Unresolved, "project_resource_unavailable"),
                ),
                EffectOutcome::Uncertain(_) => reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::ValidatingResources,
                    effect_error("project_resource_observation_unknown"),
                    EffectKind::Resource,
                ),
            }?;
            return Ok(());
        }

        if stage == ProjectCommandStage::Opening {
            if snapshot.lifecycle == CanonicalProjectLifecycle::Closed {
                if !record.opened_by_workflow {
                    record.opened_by_workflow = true;
                    persist(&self.store, record)?;
                }
                match self.mutate(
                    record,
                    snapshot.head,
                    b"open",
                    CanonicalProjectMutationAction::Open,
                )? {
                    CanonicalProjectMutationOutcome::Committed { .. } => {}
                    CanonicalProjectMutationOutcome::Rejected(error) => {
                        return reject(&self.store, record, error);
                    }
                    CanonicalProjectMutationOutcome::Uncertain => {
                        return reconcile(
                            &self.store,
                            record,
                            ProjectCommandStage::Opening,
                            effect_error("project_open_commit_unknown"),
                            EffectKind::None,
                        );
                    }
                }
            }
            return checkpoint(
                &self.store,
                record,
                ProjectCommandStage::ConfiguringAssignment,
            );
        }

        let intent = AssignmentIntent {
            assignment_id: derived_assignment(record.operation_id),
            agent_id,
            provider: provider.clone(),
        };
        if stage == ProjectCommandStage::ConfiguringAssignment {
            if let Some(assignment) = &snapshot.assignment {
                if assignment.intent != intent {
                    return reject(
                        &self.store,
                        record,
                        error(ErrorCategory::Conflict, "project_assignment_conflict"),
                    );
                }
            } else {
                match self.mutate(
                    record,
                    snapshot.head,
                    b"configure-assignment",
                    CanonicalProjectMutationAction::Configure(intent),
                )? {
                    CanonicalProjectMutationOutcome::Committed { .. } => {}
                    CanonicalProjectMutationOutcome::Rejected(error) => {
                        return self.compensate(record, agent_id, provider, resume_session, error);
                    }
                    CanonicalProjectMutationOutcome::Uncertain => {
                        return reconcile(
                            &self.store,
                            record,
                            ProjectCommandStage::ConfiguringAssignment,
                            effect_error("project_configuring_commit_unknown"),
                            EffectKind::None,
                        );
                    }
                }
            }
            return checkpoint(&self.store, record, ProjectCommandStage::StartingRuntime);
        }

        if stage == ProjectCommandStage::StartingRuntime {
            let operation = derived_operation(record.operation_id, b"runtime", &[]);
            record.runtime_operation_id = Some(operation);
            if matches!(record.runtime_effect, SagaEffectState::NotStarted) {
                record.runtime_effect = SagaEffectState::Pending;
                persist(&self.store, record)?;
            }
            let request = EffectRequest {
                operation_id: operation,
                request_digest: derived_digest(record.operation_id, b"runtime", &[]),
                issued_at: record.issued_at,
                body: ProjectRuntimeRequest {
                    project_id: record.project_id,
                    agent_id,
                    provider: provider.clone(),
                    resume_session: resume_session.clone(),
                },
            };
            match self.runtime.start_or_resume(&request)? {
                EffectOutcome::Accepted(session)
                    if resume_session
                        .as_ref()
                        .is_none_or(|expected| expected == &session) =>
                {
                    record.runtime_session = Some(session);
                    record.runtime_effect = SagaEffectState::Accepted;
                    checkpoint(
                        &self.store,
                        record,
                        ProjectCommandStage::ValidatingLaunchDirectory,
                    )
                }
                EffectOutcome::Accepted(_) | EffectOutcome::Rejected(_) => self.compensate(
                    record,
                    agent_id,
                    provider,
                    resume_session,
                    error(ErrorCategory::Unresolved, "project_runtime_start_rejected"),
                ),
                EffectOutcome::Uncertain(_) => reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::StartingRuntime,
                    effect_error("project_runtime_start_unknown"),
                    EffectKind::Runtime,
                ),
            }?;
            return Ok(());
        }

        if stage == ProjectCommandStage::ValidatingLaunchDirectory {
            let operation = derived_operation(record.operation_id, b"launch-validation", &[]);
            let request = EffectRequest {
                operation_id: operation,
                request_digest: derived_digest(record.operation_id, b"launch-validation", &[]),
                issued_at: record.issued_at,
                body: ProjectLaunchValidationRequest {
                    home: record.home,
                    project_id: record.project_id,
                    launch_directory: launch_directory.clone(),
                    resources: snapshot.resources.clone(),
                },
            };
            match self.resources.validate_launch_directory(&request)? {
                EffectOutcome::Accepted(observation)
                    if observation.health == ResourceHealth::Healthy
                        && observation.within_claims
                        && observation.observed_canonical == launch_directory =>
                {
                    let Some(thread) = select_thread(&snapshot, resume_thread) else {
                        return self.compensate(
                            record,
                            agent_id,
                            provider,
                            resume_session,
                            error(ErrorCategory::Conflict, "project_activation_thread_missing"),
                        );
                    };
                    record.selected_thread = Some(thread);
                    checkpoint(&self.store, record, ProjectCommandStage::MakingRunnable)
                }
                EffectOutcome::Accepted(_) | EffectOutcome::Rejected(_) => self.compensate(
                    record,
                    agent_id,
                    provider,
                    resume_session,
                    error(
                        ErrorCategory::Unresolved,
                        "project_launch_directory_invalid",
                    ),
                ),
                EffectOutcome::Uncertain(_) => reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::ValidatingLaunchDirectory,
                    effect_error("project_launch_observation_unknown"),
                    EffectKind::None,
                ),
            }?;
            return Ok(());
        }

        if stage == ProjectCommandStage::MakingRunnable {
            let session = record.runtime_session.clone().ok_or_else(|| {
                hq_application::ApplicationError::new(
                    hq_application::ApplicationErrorCode::StateCorrupt,
                )
            })?;
            let thread_id = record.selected_thread.ok_or_else(|| {
                hq_application::ApplicationError::new(
                    hq_application::ApplicationErrorCode::StateCorrupt,
                )
            })?;
            let binding = AssignmentBinding {
                assignment_id: derived_assignment(record.operation_id),
                agent_id,
                provider: provider.clone(),
                session: session.clone(),
            };
            if snapshot.assignment.as_ref().is_some_and(|assignment| {
                assignment.binding.as_ref() == Some(&binding)
                    && assignment.thread_id == Some(thread_id)
                    && assignment.runnable
            }) {
                return checkpoint(&self.store, record, ProjectCommandStage::DispatchingInputs);
            }
            match self.mutate(
                record,
                snapshot.head,
                b"make-runnable",
                CanonicalProjectMutationAction::MakeRunnable {
                    binding,
                    thread_id,
                    launch_directory,
                    activation: OperationCorrelation::new(
                        provider.clone(),
                        session,
                        record.operation_id,
                    ),
                },
            )? {
                CanonicalProjectMutationOutcome::Committed { .. } => {
                    checkpoint(&self.store, record, ProjectCommandStage::DispatchingInputs)
                }
                CanonicalProjectMutationOutcome::Rejected(error) => {
                    self.compensate(record, agent_id, provider, resume_session, error)
                }
                CanonicalProjectMutationOutcome::Uncertain => reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::MakingRunnable,
                    effect_error("project_runnable_commit_unknown"),
                    EffectKind::None,
                ),
            }?;
            return Ok(());
        }

        if stage == ProjectCommandStage::DispatchingInputs {
            return self.advance_dispatch(record);
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn advance_dispatch(
        &self,
        record: &mut ProjectSagaRecord,
    ) -> Result<(), hq_application::ApplicationError> {
        let snapshot = self
            .canonical
            .snapshot(record.project_id, record.account_id, None)?;
        if snapshot.home != record.home {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Unauthorized, "project_wrong_home"),
            );
        }
        if record.pending_canonical_mutation.is_some() {
            return match self.replay_canonical_mutation(record)? {
                CanonicalProjectMutationOutcome::Committed { .. } => {
                    checkpoint(&self.store, record, ProjectCommandStage::DispatchingInputs)
                }
                CanonicalProjectMutationOutcome::Rejected(error) => {
                    reject(&self.store, record, error)
                }
                CanonicalProjectMutationOutcome::Uncertain => reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::DispatchingInputs,
                    effect_error("project_dispatch_commit_unknown"),
                    EffectKind::None,
                ),
            };
        }
        if current_stage(record) == ProjectCommandStage::Accepted {
            if snapshot.head != record.expected_head {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_stale_head"),
                );
            }
            if !snapshot.active_human {
                return reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Unauthorized, "project_inactive_human"),
                );
            }
            return checkpoint(&self.store, record, ProjectCommandStage::DispatchingInputs);
        }
        let Some(assignment) = snapshot.assignment.as_ref() else {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_not_assigned"),
            );
        };
        let (Some(binding), Some(thread_id)) = (assignment.binding.clone(), assignment.thread_id)
        else {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_not_runnable"),
            );
        };
        if snapshot.lifecycle != CanonicalProjectLifecycle::Open
            || !assignment.runnable
            || !snapshot.claimable
        {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_not_runnable"),
            );
        }
        if current_stage(record) != ProjectCommandStage::DispatchingInputs {
            checkpoint(&self.store, record, ProjectCommandStage::DispatchingInputs)?;
            return Ok(());
        }
        let Some(input) = snapshot.pending_inputs.first().cloned() else {
            record.state = ProjectSagaState::Completed {
                project_head: snapshot.head,
            };
            return persist(&self.store, record);
        };
        let dispatch_id = derived_dispatch(record.project_id, input.message_id, input.sequence);
        let operation = derived_operation(
            record.operation_id,
            b"dispatch",
            input.message_id.as_bytes(),
        );
        let request = EffectRequest {
            operation_id: operation,
            request_digest: delivery_digest(record.project_id, &binding, thread_id, &input),
            issued_at: record.issued_at,
            body: ProjectRuntimeDelivery {
                project_id: record.project_id,
                binding: binding.clone(),
                thread_id,
                submission_id: input.message_id,
                sequence: input.sequence,
                body: input.body.clone(),
            },
        };
        match self.runtime.deliver(&request)? {
            EffectOutcome::Accepted(()) => match self.mutate(
                record,
                snapshot.head,
                b"record-dispatch",
                CanonicalProjectMutationAction::RecordDispatch {
                    input,
                    dispatch_id,
                    binding,
                    thread_id,
                },
            )? {
                CanonicalProjectMutationOutcome::Committed { .. } => {
                    record.dispatch_operation_id.get_or_insert(operation);
                    record.dispatch_effect = SagaEffectState::Accepted;
                    checkpoint(&self.store, record, ProjectCommandStage::DispatchingInputs)
                }
                CanonicalProjectMutationOutcome::Rejected(error) => {
                    reject(&self.store, record, error)
                }
                CanonicalProjectMutationOutcome::Uncertain => reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::DispatchingInputs,
                    effect_error("project_dispatch_commit_unknown"),
                    EffectKind::None,
                ),
            },
            EffectOutcome::Rejected(error) => reject(&self.store, record, error),
            EffectOutcome::Uncertain(_) => {
                record.dispatch_operation_id.get_or_insert(operation);
                reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::DispatchingInputs,
                    effect_error("project_delivery_unknown"),
                    EffectKind::Dispatch,
                )
            }
        }
    }

    fn compensate(
        &self,
        record: &mut ProjectSagaRecord,
        agent_id: AgentId,
        provider: ProviderId,
        resume_session: Option<ProviderSessionId>,
        cause: DomainError,
    ) -> Result<(), hq_application::ApplicationError> {
        if record.failure.is_none() {
            record.failure = Some(cause.clone());
        }
        checkpoint(&self.store, record, ProjectCommandStage::Compensating)?;
        if record.runtime_session.is_some() {
            let operation = derived_operation(record.operation_id, b"compensate-runtime", &[]);
            let request = EffectRequest {
                operation_id: operation,
                request_digest: derived_digest(record.operation_id, b"compensate-runtime", &[]),
                issued_at: record.issued_at,
                body: ProjectRuntimeRequest {
                    project_id: record.project_id,
                    agent_id,
                    provider,
                    resume_session,
                },
            };
            if matches!(self.runtime.stop(&request)?, EffectOutcome::Uncertain(_)) {
                return reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::Compensating,
                    effect_error("project_compensation_runtime_unknown"),
                    EffectKind::None,
                );
            }
        }
        for _ in 0..4 {
            let snapshot = self
                .canonical
                .snapshot(record.project_id, record.account_id, None)?;
            let mutation = if let Some(assignment) = snapshot.assignment {
                Some((
                    b"compensate-assignment".as_slice(),
                    CanonicalProjectMutationAction::EndAssignment {
                        assignment_id: assignment.intent.assignment_id,
                    },
                ))
            } else if record.opened_by_workflow {
                match snapshot.lifecycle {
                    CanonicalProjectLifecycle::Open => Some((
                        b"compensate-begin-closing".as_slice(),
                        CanonicalProjectMutationAction::BeginClosing,
                    )),
                    CanonicalProjectLifecycle::Closing => Some((
                        b"compensate-finish-closing".as_slice(),
                        CanonicalProjectMutationAction::FinishClosing,
                    )),
                    CanonicalProjectLifecycle::Closed => None,
                }
            } else {
                None
            };
            let Some((tag, action)) = mutation else {
                return reject(&self.store, record, cause);
            };
            match self.mutate(record, snapshot.head, tag, action)? {
                CanonicalProjectMutationOutcome::Committed { .. } => {}
                CanonicalProjectMutationOutcome::Rejected(_) => {
                    return reconcile(
                        &self.store,
                        record,
                        ProjectCommandStage::Compensating,
                        effect_error("project_compensation_rejected"),
                        EffectKind::None,
                    );
                }
                CanonicalProjectMutationOutcome::Uncertain => {
                    return reconcile(
                        &self.store,
                        record,
                        ProjectCommandStage::Compensating,
                        effect_error("project_compensation_commit_unknown"),
                        EffectKind::None,
                    );
                }
            }
        }
        Err(hq_application::ApplicationError::new(
            hq_application::ApplicationErrorCode::InvariantViolation,
        ))
    }

    fn mutate(
        &self,
        record: &mut ProjectSagaRecord,
        expected_head: FactId,
        tag: &[u8],
        action: CanonicalProjectMutationAction,
    ) -> Result<CanonicalProjectMutationOutcome, hq_application::ApplicationError> {
        if record.pending_canonical_mutation.is_some() {
            return self.replay_canonical_mutation(record);
        }
        let mutation = CanonicalProjectMutation {
            command_id: derived_command(record.operation_id, tag, expected_head.as_bytes()),
            request_digest: mutation_digest(record.operation_id, tag, expected_head, &action),
            account_id: record.account_id,
            project_id: record.project_id,
            home: record.home,
            expected_head,
            issued_at: record.issued_at,
            action,
        };
        record.pending_canonical_mutation = Some(mutation.clone());
        persist(&self.store, record)?;
        self.finish_canonical_attempt(record, self.canonical.mutate(mutation)?)
    }

    fn replay_canonical_mutation(
        &self,
        record: &mut ProjectSagaRecord,
    ) -> Result<CanonicalProjectMutationOutcome, hq_application::ApplicationError> {
        let mutation = record.pending_canonical_mutation.clone().ok_or_else(|| {
            hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )
        })?;
        self.finish_canonical_attempt(record, self.canonical.mutate(mutation)?)
    }

    fn finish_canonical_attempt(
        &self,
        record: &mut ProjectSagaRecord,
        outcome: CanonicalProjectMutationOutcome,
    ) -> Result<CanonicalProjectMutationOutcome, hq_application::ApplicationError> {
        if !matches!(outcome, CanonicalProjectMutationOutcome::Uncertain) {
            record.pending_canonical_mutation = None;
            persist(&self.store, record)?;
        }
        Ok(outcome)
    }
}

#[derive(Clone, Copy)]
enum EffectKind {
    None,
    Runtime,
    Dispatch,
    Resource,
}

fn resources_are_healthy(
    desired: &[ProjectResource],
    observations: &[ProjectResourceObservation],
) -> bool {
    desired.len() == observations.len()
        && desired.iter().all(|resource| {
            observations.iter().any(|observation| {
                observation.resource_id == resource.resource_id
                    && observation.observed_canonical.as_ref() == Some(&resource.canonical_locator)
                    && observation.health == ResourceHealth::Healthy
            })
        })
}

fn select_thread(
    snapshot: &ProjectWorkflowSnapshot,
    requested: Option<ThreadId>,
) -> Option<ThreadId> {
    requested.map_or_else(
        || snapshot.pending_inputs.first().map(|input| input.thread_id),
        |thread| {
            snapshot
                .historical_threads
                .contains(&thread)
                .then_some(thread)
        },
    )
}

fn current_stage(record: &ProjectSagaRecord) -> ProjectCommandStage {
    match &record.state {
        ProjectSagaState::Running(stage) | ProjectSagaState::Reconcilable { stage, .. } => *stage,
        ProjectSagaState::Completed { .. } | ProjectSagaState::Rejected(_) => {
            ProjectCommandStage::Complete
        }
    }
}

fn checkpoint<S: ProjectSagaStore>(
    store: &S,
    record: &mut ProjectSagaRecord,
    stage: ProjectCommandStage,
) -> Result<(), hq_application::ApplicationError> {
    record.state = ProjectSagaState::Running(stage);
    persist(store, record)
}

fn persist<S: ProjectSagaStore>(
    store: &S,
    record: &mut ProjectSagaRecord,
) -> Result<(), hq_application::ApplicationError> {
    record.updated_at_millis = record.updated_at_millis.saturating_add(1);
    store.replace(record.clone()).map_err(store_error)
}

fn reject<S: ProjectSagaStore>(
    store: &S,
    record: &mut ProjectSagaRecord,
    error: DomainError,
) -> Result<(), hq_application::ApplicationError> {
    record.state = ProjectSagaState::Rejected(error);
    persist(store, record)
}

fn reconcile<S: ProjectSagaStore>(
    store: &S,
    record: &mut ProjectSagaRecord,
    stage: ProjectCommandStage,
    error: DomainError,
    effect: EffectKind,
) -> Result<(), hq_application::ApplicationError> {
    match effect {
        EffectKind::Runtime => record.runtime_effect = SagaEffectState::Uncertain(error.clone()),
        EffectKind::Dispatch if !matches!(record.dispatch_effect, SagaEffectState::Accepted) => {
            record.dispatch_effect = SagaEffectState::Uncertain(error.clone());
        }
        EffectKind::None | EffectKind::Dispatch => {}
        EffectKind::Resource => record.resource_effect = SagaEffectState::Uncertain(error.clone()),
    }
    record.state = ProjectSagaState::Reconcilable { stage, error };
    persist(store, record)
}

fn terminal_outcome(record: &ProjectSagaRecord) -> Option<ProjectCommandOutcome> {
    match &record.state {
        ProjectSagaState::Completed { project_head } => Some(ProjectCommandOutcome::Completed {
            operation_id: record.operation_id,
            project_head: *project_head,
        }),
        ProjectSagaState::Rejected(error) => Some(ProjectCommandOutcome::Rejected {
            operation_id: record.operation_id,
            error: error.clone(),
        }),
        ProjectSagaState::Running(_) | ProjectSagaState::Reconcilable { .. } => None,
    }
}

fn progress_outcome(record: &ProjectSagaRecord) -> ProjectCommandOutcome {
    match &record.state {
        ProjectSagaState::Reconcilable { stage, error } => ProjectCommandOutcome::Reconcilable {
            operation_id: record.operation_id,
            stage: *stage,
            error: error.clone(),
        },
        ProjectSagaState::Running(stage) => ProjectCommandOutcome::Running {
            operation_id: record.operation_id,
            stage: *stage,
        },
        ProjectSagaState::Completed { project_head } => ProjectCommandOutcome::Completed {
            operation_id: record.operation_id,
            project_head: *project_head,
        },
        ProjectSagaState::Rejected(error) => ProjectCommandOutcome::Rejected {
            operation_id: record.operation_id,
            error: error.clone(),
        },
    }
}

fn derived_assignment(operation: OperationId) -> AssignmentId {
    AssignmentId::from_bytes(hash(&[b"hq-project-assignment-v1", operation.as_bytes()]))
}

fn derived_operation(operation: OperationId, tag: &[u8], extra: &[u8]) -> OperationId {
    OperationId::from_bytes(hash(&[
        b"hq-project-effect-v1",
        operation.as_bytes(),
        tag,
        extra,
    ]))
}

fn derived_command(operation: OperationId, tag: &[u8], extra: &[u8]) -> CommandId {
    CommandId::from_bytes(hash(&[
        b"hq-project-mutation-v1",
        operation.as_bytes(),
        tag,
        extra,
    ]))
}

fn derived_digest(operation: OperationId, tag: &[u8], extra: &[u8]) -> CommandDigest {
    CommandDigest::from_bytes(hash(&[
        b"hq-project-effect-digest-v1",
        operation.as_bytes(),
        tag,
        extra,
    ]))
}

fn derived_dispatch(project: ProjectId, message: MessageId, sequence: NonZeroU64) -> DispatchId {
    DispatchId::from_bytes(hash(&[
        b"hq-project-dispatch-v1",
        project.as_bytes(),
        message.as_bytes(),
        &sequence.get().to_be_bytes(),
    ]))
}

fn delivery_digest(
    project_id: ProjectId,
    binding: &AssignmentBinding,
    thread: ThreadId,
    input: &PendingProjectInput,
) -> CommandDigest {
    CommandDigest::from_bytes(hash(&[
        b"hq-project-delivery-v1",
        project_id.as_bytes(),
        binding.assignment_id.as_bytes(),
        binding.agent_id.as_bytes(),
        binding.provider.as_str().as_bytes(),
        binding.session.as_str().as_bytes(),
        thread.as_bytes(),
        input.message_id.as_bytes(),
        input.input_fact_id.as_bytes(),
        input.accepted_fact.as_bytes(),
        &input.sequence.get().to_be_bytes(),
        input.thread_id.as_bytes(),
        input.body.as_str().as_bytes(),
    ]))
}

fn mutation_digest(
    operation: OperationId,
    tag: &[u8],
    expected_head: FactId,
    action: &CanonicalProjectMutationAction,
) -> CommandDigest {
    let mut digest = Sha256::new();
    put(&mut digest, b"hq-project-canonical-mutation-v1");
    put(&mut digest, operation.as_bytes());
    put(&mut digest, tag);
    put(&mut digest, expected_head.as_bytes());
    match action {
        CanonicalProjectMutationAction::Open => put(&mut digest, b"open"),
        CanonicalProjectMutationAction::Configure(intent) => {
            put(&mut digest, b"configure");
            put(&mut digest, intent.assignment_id.as_bytes());
            put(&mut digest, intent.agent_id.as_bytes());
            put(&mut digest, intent.provider.as_str().as_bytes());
        }
        CanonicalProjectMutationAction::MakeRunnable {
            binding,
            thread_id,
            launch_directory,
            activation,
        } => {
            put(&mut digest, b"runnable");
            put_binding(&mut digest, binding);
            put(&mut digest, thread_id.as_bytes());
            put_locator(&mut digest, launch_directory);
            put(&mut digest, activation.provider().as_str().as_bytes());
            put(&mut digest, activation.session().as_str().as_bytes());
            put(&mut digest, activation.operation().as_bytes());
        }
        CanonicalProjectMutationAction::EndAssignment { assignment_id } => {
            put(&mut digest, b"end-assignment");
            put(&mut digest, assignment_id.as_bytes());
        }
        CanonicalProjectMutationAction::BeginClosing => put(&mut digest, b"begin-closing"),
        CanonicalProjectMutationAction::FinishClosing => put(&mut digest, b"finish-closing"),
        CanonicalProjectMutationAction::RecordDispatch {
            input,
            dispatch_id,
            binding,
            thread_id,
        } => {
            put(&mut digest, b"dispatch");
            put(&mut digest, input.message_id.as_bytes());
            put(&mut digest, input.input_fact_id.as_bytes());
            put(&mut digest, input.accepted_fact.as_bytes());
            put(&mut digest, &input.sequence.get().to_be_bytes());
            put(&mut digest, input.thread_id.as_bytes());
            put(&mut digest, input.body.as_str().as_bytes());
            put(&mut digest, dispatch_id.as_bytes());
            put_binding(&mut digest, binding);
            put(&mut digest, thread_id.as_bytes());
        }
    }
    CommandDigest::from_bytes(digest.finalize().into())
}

fn put_binding(digest: &mut Sha256, binding: &AssignmentBinding) {
    put(digest, binding.assignment_id.as_bytes());
    put(digest, binding.agent_id.as_bytes());
    put(digest, binding.provider.as_str().as_bytes());
    put(digest, binding.session.as_str().as_bytes());
}

fn put_locator(digest: &mut Sha256, locator: &ResourceLocator) {
    let scheme = match locator.scheme() {
        hq_domain::ResourceScheme::GitRepository => 1_u8,
        hq_domain::ResourceScheme::WorkingTree => 2,
        hq_domain::ResourceScheme::Container => 3,
        hq_domain::ResourceScheme::Opaque => 4,
    };
    put(digest, &[scheme]);
    put(digest, locator.value().as_bytes());
}

fn put(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

fn hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().into()
}

fn effect_error(code: &'static str) -> DomainError {
    error(ErrorCategory::Unresolved, code)
}

#[allow(
    clippy::expect_used,
    reason = "all callers pass reviewed static error codes"
)]
fn error(category: ErrorCategory, code: &'static str) -> DomainError {
    DomainError::new(
        category,
        ErrorCode::new(code).expect("static project workflow error code"),
    )
}

const fn store_error(error: SagaStoreError) -> hq_application::ApplicationError {
    let code = match error {
        SagaStoreError::Conflict => hq_application::ApplicationErrorCode::StateIdentityConflict,
        SagaStoreError::Unavailable => hq_application::ApplicationErrorCode::AdapterUnavailable,
        SagaStoreError::Corrupt => hq_application::ApplicationErrorCode::StateCorrupt,
    };
    hq_application::ApplicationError::new(code)
}
