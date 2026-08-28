//! Explicit activation and at-most-once project-input dispatch workflows.

use std::{collections::BTreeSet, num::NonZeroU64, path::Path};

use hq_application::{
    AgentRetirementOutcome, AgentRetirementRequest, EffectOutcome, EffectRequest, MutationAttempt,
    MutationOutcome, ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest,
    ProjectCommandStage, WorktreeProvisioningRequest,
};
use hq_domain::{
    AccountId, AgentId, AssignmentBinding, AssignmentId, AssignmentIntent, CommandDigest,
    CommandId, ContentText, DispatchId, DomainError, ErrorCategory, ErrorCode, FactId,
    InstallationId, MailboxId, MessageId, OperationCorrelation, OperationId, ProjectId,
    ProjectResource, ProviderId, ProviderSessionId, ResourceHealth, ResourceId, ResourceLocator,
    ResourceScheme, RuntimeObservation, ShortText, ThreadId, Timestamp,
};
use hq_resources::{
    PathReleaseAssessment, ReleaseDecision, decide_release, normalize_absolute_path,
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
    /// Whether failed graceful quiescence requires explicit human resolution.
    pub blocked: bool,
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

/// One current project assignment found while coordinating a named-agent retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentRetirementAssignment {
    /// Owning project.
    pub project_id: ProjectId,
    /// Exact project head that still contains the assignment.
    pub project_head: FactId,
}

/// One serialized global observation used to choose the safe retirement path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRetirementSnapshot {
    /// Whether the requested account is currently active at this installation.
    pub active_human: bool,
    /// Whether the named agent is uniquely active and installation-local.
    pub agent_active: bool,
    /// Unique compatible permanent claim when active.
    pub claim_fact: Option<FactId>,
    /// Whether project assignment state is globally inconsistent or forked.
    pub conflicted: bool,
    /// Every current assignment for the agent; more than one is a conflict.
    pub assignments: Vec<AgentRetirementAssignment>,
}

/// Closed canonical project mutations used by this workflow package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalProjectMutationAction {
    /// Create one initially open project over an exactly identified worktree.
    Create {
        /// Caller-allocated project mailbox.
        mailbox_id: MailboxId,
        /// Human-visible project name.
        name: ShortText,
        /// Optional project brief.
        brief: Option<ContentText>,
        /// Sole initial desired and primary resource.
        resource: ProjectResource,
    },
    /// Conditionally open a closed, unarchived, claimable project.
    Open,
    /// Add one observed desired resource.
    AddResource {
        /// Stable desired resource identity.
        resource: ProjectResource,
        /// Whether this resource becomes the explicit primary.
        make_primary: bool,
    },
    /// Remove one desired resource without touching external state.
    RemoveResource {
        /// Stable desired resource identity.
        resource_id: ResourceId,
        /// Explicit authority to mutate resources while assigned.
        force: bool,
    },
    /// Atomically replace one desired resource.
    ReplaceResource {
        /// Existing desired resource identity.
        old_resource_id: ResourceId,
        /// Fully observed replacement resource.
        new_resource: ProjectResource,
    },
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
        /// Whether HQ authority was explicitly revoked without clean external proof.
        forced: bool,
        /// Typed runtime observation, when this transition follows quiescence.
        runtime: Option<RuntimeObservation>,
    },
    /// Keep the exact assignment owned but non-runnable pending explicit resolution.
    BlockAssignment {
        /// Assignment epoch that could not be quiesced gracefully.
        assignment_id: AssignmentId,
        /// Stable typed reason requiring human resolution.
        cause: ErrorCode,
    },
    /// Enter canonical closing while retaining the current assignment and claims.
    BeginClosing,
    /// Finish canonical close after the assignment has ended.
    FinishClosing {
        /// Whether release proceeded under explicit force policy.
        forced: bool,
        /// Last truthful runtime observation, when a runtime was assigned.
        runtime: Option<RuntimeObservation>,
    },
    /// Hide one closed project without deleting history.
    Archive,
    /// Restore one archived project as visible and closed.
    Unarchive,
    /// Permanently retire one unassigned local named agent.
    RetireAgent {
        /// Durable named agent identity.
        agent_id: AgentId,
    },
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
    /// Exact head checked inside the commit transaction, absent only for creation.
    pub expected_head: Option<FactId>,
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
    /// Loads one serialized global agent/assignment observation for retirement routing.
    fn agent_retirement_snapshot(
        &self,
        _request: &AgentRetirementRequest,
    ) -> Result<AgentRetirementSnapshot, hq_application::ApplicationError> {
        Err(hq_application::ApplicationError::new(
            hq_application::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Atomically validates and retires an agent that remains globally unassigned.
    fn retire_idle_agent(
        &self,
        _request: &AgentRetirementRequest,
    ) -> Result<MutationAttempt, hq_application::ApplicationError> {
        Err(hq_application::ApplicationError::new(
            hq_application::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

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

/// Read-only identification request for one newly created worktree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectResourceIdentificationRequest {
    /// Immutable home namespace.
    pub home: InstallationId,
    /// Project that will own the resulting desired resource.
    pub project_id: ProjectId,
    /// Stable resource identity derived from the provisioning operation.
    pub resource_id: ResourceId,
    /// Exact reserved worktree destination.
    pub destination: ResourceLocator,
}

/// Batched read-only release assessment request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectReleaseAssessmentRequest {
    /// Immutable home namespace.
    pub home: InstallationId,
    /// Target project.
    pub project_id: ProjectId,
    /// Exact desired resources to assess without modifying them.
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
    /// Identifies one exact created path without mutating it.
    fn identify_resource(
        &self,
        _request: &EffectRequest<ProjectResourceIdentificationRequest>,
    ) -> Result<EffectOutcome<ProjectResource>, hq_application::ApplicationError> {
        Err(hq_application::ApplicationError::new(
            hq_application::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    /// Revalidates every desired resource as one stable read-only operation.
    fn validate_resources(
        &self,
        request: &EffectRequest<ProjectResourceValidationRequest>,
    ) -> Result<EffectOutcome<Vec<ProjectResourceObservation>>, hq_application::ApplicationError>;

    /// Assesses whether desired resources can release advisory claims gracefully.
    fn assess_release(
        &self,
        request: &EffectRequest<ProjectReleaseAssessmentRequest>,
    ) -> Result<EffectOutcome<Vec<PathReleaseAssessment>>, hq_application::ApplicationError>;

    /// Revalidates the exact launch directory after runtime readiness.
    fn validate_launch_directory(
        &self,
        request: &EffectRequest<ProjectLaunchValidationRequest>,
    ) -> Result<EffectOutcome<ProjectLaunchObservation>, hq_application::ApplicationError>;
}

/// Exact declarative Git worktree operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitWorktreeRequest {
    /// Existing repository or worktree used by Git.
    pub source: ResourceLocator,
    /// Exact reserved absolute destination.
    pub destination: ResourceLocator,
    /// Exact validated branch spelling.
    pub branch: hq_domain::ShortText,
    /// Whether the branch should be created from the source's current head.
    pub create_branch: bool,
}

impl From<&WorktreeProvisioningRequest> for GitWorktreeRequest {
    fn from(request: &WorktreeProvisioningRequest) -> Self {
        Self {
            source: request.source.clone(),
            destination: request.destination.clone(),
            branch: request.branch.clone(),
            create_branch: request.create_branch,
        }
    }
}

/// Exact state observed before or after one Git worktree create attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitWorktreeState {
    /// No destination, conflicting registration, or unsafe branch state exists.
    ReadyToCreate,
    /// The destination is the exact requested repository worktree and branch.
    Created,
}

/// Bounded mutating Git capability kept separate from read-only resource identification.
pub trait GitWorktreePort {
    /// Reconciles the exact destination, repository, registration, and branch.
    fn lookup(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<GitWorktreeState>, hq_application::ApplicationError>;

    /// Creates the exact worktree, returning accepted only after exact post-create lookup.
    fn create(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<()>, hq_application::ApplicationError>;
}

/// Closed unavailable Git port used by workflows that cannot provision worktrees.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableGitWorktreePort;

impl GitWorktreePort for UnavailableGitWorktreePort {
    fn lookup(
        &self,
        _request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<GitWorktreeState>, hq_application::ApplicationError> {
        Err(hq_application::ApplicationError::new(
            hq_application::ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    fn create(
        &self,
        _request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<()>, hq_application::ApplicationError> {
        Err(hq_application::ApplicationError::new(
            hq_application::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
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
    /// Human-selected launch directory for start/resume; absent for stop-only requests.
    pub launch_directory: Option<ResourceLocator>,
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
pub struct ProjectWorkflowManager<S, C, R, F, G = UnavailableGitWorktreePort> {
    store: S,
    canonical: C,
    runtime: R,
    resources: F,
    git: G,
}

impl<S, C, R, F> ProjectWorkflowManager<S, C, R, F, UnavailableGitWorktreePort>
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
            git: UnavailableGitWorktreePort,
        }
    }
}

impl<S, C, R, F, G> ProjectWorkflowManager<S, C, R, F, G>
where
    S: ProjectSagaStore,
    C: CanonicalProjectPort,
    R: ProjectRuntimePort,
    F: ProjectResourcePort,
    G: GitWorktreePort,
{
    /// Owns every capability required by existing-project and provisioning workflows.
    pub const fn with_git(store: S, canonical: C, runtime: R, resources: F, git: G) -> Self {
        Self {
            store,
            canonical,
            runtime,
            resources,
            git,
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
                    runtime: None,
                });
            }
            BeginSagaOutcome::ProjectBusy => {
                return Ok(ProjectCommandOutcome::Rejected {
                    operation_id,
                    error: error(ErrorCategory::Conflict, "project_command_in_progress"),
                    runtime: None,
                });
            }
        };
        self.run(record)
    }

    /// Coordinates one exact idle or assigned named-agent retirement.
    pub fn retire_agent(
        &self,
        request: AgentRetirementRequest,
    ) -> Result<AgentRetirementOutcome, hq_application::ApplicationError> {
        if crate::agent_retirement_request_digest(&request) != request.request_digest {
            return Ok(retirement_rejected(
                request.operation_id,
                "agent_retirement_digest_mismatch",
            ));
        }
        if let Some(existing) = self.store.find(request.operation_id).map_err(store_error)? {
            if !retirement_record_matches(&existing, &request) {
                return Ok(retirement_rejected(
                    request.operation_id,
                    "agent_retirement_identity_conflict",
                ));
            }
            let project_id = existing.project_id;
            return self
                .run(existing)
                .map(|outcome| retirement_from_project(outcome, project_id));
        }

        let snapshot = self.canonical.agent_retirement_snapshot(&request)?;
        if !snapshot.active_human {
            return Ok(retirement_rejected(
                request.operation_id,
                "agent_retirement_inactive_human",
            ));
        }
        if snapshot.conflicted
            || !snapshot.agent_active
            || snapshot.claim_fact != Some(request.expected_claim)
        {
            return Ok(retirement_rejected(
                request.operation_id,
                "agent_retirement_agent_unavailable",
            ));
        }
        match snapshot.assignments.as_slice() {
            [] => self
                .canonical
                .retire_idle_agent(&request)
                .map(|attempt| retirement_from_mutation(&request, attempt)),
            [assignment] => {
                let project_id = assignment.project_id;
                self.control(ProjectCommandRequest {
                    command_id: request.command_id,
                    operation_id: request.operation_id,
                    request_digest: request.request_digest,
                    account_id: request.account_id,
                    project_id,
                    home: request.home,
                    expected_head: Some(assignment.project_head),
                    issued_at: request.issued_at,
                    action: ProjectCommandAction::RetireAgent {
                        agent_id: request.agent_id,
                        force: request.force,
                    },
                })
                .map(|outcome| retirement_from_project(outcome, project_id))
            }
            [_, _, ..] => Ok(retirement_rejected(
                request.operation_id,
                "agent_retirement_assignment_conflict",
            )),
        }
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
        let provisioning = matches!(record.action, ProjectCommandAction::ProvisionWorktree(_));
        if provisioning == record.expected_head.is_some() {
            reject(
                &self.store,
                &mut record,
                error(
                    ErrorCategory::InvalidInput,
                    "project_command_head_precondition_invalid",
                ),
            )?;
            return Ok(progress_outcome(&record));
        }
        if let ProjectCommandAction::ProvisionWorktree(request) = &record.action
            && !valid_provisioning_paths(request)
        {
            reject(
                &self.store,
                &mut record,
                error(
                    ErrorCategory::InvalidInput,
                    "project_worktree_locator_not_normalized",
                ),
            )?;
            return Ok(progress_outcome(&record));
        }
        for _ in 0..MAX_PROJECT_WORKFLOW_ADVANCES {
            match record.action.clone() {
                ProjectCommandAction::Open
                | ProjectCommandAction::AddResource { .. }
                | ProjectCommandAction::RemoveResource { .. }
                | ProjectCommandAction::ReplaceResource { .. } => {
                    self.advance_resource_mutation(&mut record)?;
                }
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
                ProjectCommandAction::Close { force } => {
                    self.advance_close(&mut record, force, false)?;
                }
                ProjectCommandAction::SetArchived { archived: true } => {
                    self.advance_close(&mut record, false, true)?;
                }
                ProjectCommandAction::SetArchived { archived: false } => {
                    self.advance_unarchive(&mut record)?;
                }
                ProjectCommandAction::Handoff {
                    agent_id,
                    provider,
                    resume_session,
                    thread_id,
                    launch_directory,
                    force_takeover,
                } => self.advance_handoff(
                    &mut record,
                    agent_id,
                    provider,
                    resume_session,
                    thread_id,
                    launch_directory,
                    force_takeover,
                )?,
                ProjectCommandAction::RetireAgent { agent_id, force } => {
                    self.advance_retirement(&mut record, agent_id, force)?;
                }
                ProjectCommandAction::ProvisionWorktree(request) => {
                    self.advance_provisioning(&mut record, &request)?;
                }
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

    fn advance_provisioning(
        &self,
        record: &mut ProjectSagaRecord,
        request: &WorktreeProvisioningRequest,
    ) -> Result<(), hq_application::ApplicationError> {
        match current_stage(record) {
            ProjectCommandStage::Accepted => checkpoint(
                &self.store,
                record,
                ProjectCommandStage::ReservingDestination,
            ),
            ProjectCommandStage::ReservingDestination => {
                checkpoint(&self.store, record, ProjectCommandStage::ReconcilingGit)
            }
            ProjectCommandStage::ReconcilingGit
            | ProjectCommandStage::CreatingWorktree
            | ProjectCommandStage::ReconciliationRequired => {
                self.reconcile_or_create_worktree(record, request)
            }
            ProjectCommandStage::IdentifyingResource => {
                self.identify_provisioned_resource(record, request)
            }
            ProjectCommandStage::CreatingProject => self.commit_provisioned_project(record),
            _ => Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )),
        }
    }

    fn reconcile_or_create_worktree(
        &self,
        record: &mut ProjectSagaRecord,
        request: &WorktreeProvisioningRequest,
    ) -> Result<(), hq_application::ApplicationError> {
        let operation_id = record.git_operation_id.unwrap_or_else(|| {
            derived_operation(
                record.operation_id,
                b"git-worktree",
                request.destination.value().as_bytes(),
            )
        });
        if record.git_operation_id.is_none() {
            record.git_operation_id = Some(operation_id);
            persist(&self.store, record)?;
        }
        let effect = git_effect_request(record, operation_id, request);
        match self.git.lookup(&effect)? {
            EffectOutcome::Accepted(GitWorktreeState::Created) => {
                record.git_effect = SagaEffectState::Accepted;
                checkpoint(
                    &self.store,
                    record,
                    ProjectCommandStage::IdentifyingResource,
                )
            }
            EffectOutcome::Accepted(GitWorktreeState::ReadyToCreate) => {
                if matches!(record.git_effect, SagaEffectState::NotStarted) {
                    record.git_effect = SagaEffectState::Pending;
                    checkpoint(&self.store, record, ProjectCommandStage::CreatingWorktree)?;
                }
                match self.git.create(&effect)? {
                    EffectOutcome::Accepted(()) => {
                        record.git_effect = SagaEffectState::Accepted;
                        checkpoint(
                            &self.store,
                            record,
                            ProjectCommandStage::IdentifyingResource,
                        )
                    }
                    EffectOutcome::Rejected(error) => {
                        record.git_effect = SagaEffectState::Rejected(error.clone());
                        reject(&self.store, record, error)
                    }
                    EffectOutcome::Uncertain(returned) if returned == operation_id => reconcile(
                        &self.store,
                        record,
                        ProjectCommandStage::CreatingWorktree,
                        effect_error("project_git_create_unknown"),
                        EffectKind::Git,
                    ),
                    EffectOutcome::Uncertain(_) => Err(hq_application::ApplicationError::new(
                        hq_application::ApplicationErrorCode::StateCorrupt,
                    )),
                }
            }
            EffectOutcome::Rejected(error)
                if matches!(
                    record.git_effect,
                    SagaEffectState::Accepted | SagaEffectState::Uncertain(_)
                ) =>
            {
                reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::ReconcilingGit,
                    error,
                    EffectKind::Git,
                )
            }
            EffectOutcome::Rejected(error) => {
                record.git_effect = SagaEffectState::Rejected(error.clone());
                reject(&self.store, record, error)
            }
            EffectOutcome::Uncertain(returned) if returned == operation_id => reconcile(
                &self.store,
                record,
                ProjectCommandStage::ReconcilingGit,
                effect_error("project_git_lookup_unknown"),
                EffectKind::Git,
            ),
            EffectOutcome::Uncertain(_) => Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )),
        }
    }

    fn identify_provisioned_resource(
        &self,
        record: &mut ProjectSagaRecord,
        request: &WorktreeProvisioningRequest,
    ) -> Result<(), hq_application::ApplicationError> {
        if record.pending_canonical_mutation.is_some() {
            return checkpoint(&self.store, record, ProjectCommandStage::CreatingProject);
        }
        let resource_id = ResourceId::from_bytes(hash(&[
            b"hq-project-provisioned-resource-v1",
            record.operation_id.as_bytes(),
        ]));
        let operation_id = record.resource_operation_id.unwrap_or_else(|| {
            derived_operation(
                record.operation_id,
                b"identify-worktree",
                resource_id.as_bytes(),
            )
        });
        if record.resource_operation_id.is_none() {
            record.resource_operation_id = Some(operation_id);
            record.resource_effect = SagaEffectState::Pending;
            persist(&self.store, record)?;
        }
        let effect = EffectRequest::new(
            operation_id,
            derived_digest(
                record.operation_id,
                b"identify-worktree",
                request.destination.value().as_bytes(),
            ),
            record.issued_at,
            ProjectResourceIdentificationRequest {
                home: record.home,
                project_id: record.project_id,
                resource_id,
                destination: request.destination.clone(),
            },
        );
        match self.resources.identify_resource(&effect)? {
            EffectOutcome::Accepted(resource)
                if resource.resource_id == resource_id
                    && resource.display_locator == request.destination
                    && resource.health == ResourceHealth::Healthy =>
            {
                record.resource_effect = SagaEffectState::Accepted;
                record.pending_canonical_mutation =
                    Some(provisioning_creation_mutation(record, request, resource));
                checkpoint(&self.store, record, ProjectCommandStage::CreatingProject)
            }
            EffectOutcome::Accepted(_) => reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_worktree_identity_changed"),
            ),
            EffectOutcome::Rejected(error) => {
                record.resource_effect = SagaEffectState::Rejected(error.clone());
                reject(&self.store, record, error)
            }
            EffectOutcome::Uncertain(returned) if returned == operation_id => reconcile(
                &self.store,
                record,
                ProjectCommandStage::IdentifyingResource,
                effect_error("project_resource_identification_unknown"),
                EffectKind::Resource,
            ),
            EffectOutcome::Uncertain(_) => Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )),
        }
    }

    fn commit_provisioned_project(
        &self,
        record: &mut ProjectSagaRecord,
    ) -> Result<(), hq_application::ApplicationError> {
        let Some(pending) = record.pending_canonical_mutation.as_ref() else {
            return Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            ));
        };
        if !matches!(
            pending.action,
            CanonicalProjectMutationAction::Create { .. }
        ) {
            return Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            ));
        }
        match self.replay_canonical_mutation(record)? {
            CanonicalProjectMutationOutcome::Committed { project_head } => {
                record.state = ProjectSagaState::Completed { project_head };
                persist(&self.store, record)
            }
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                ProjectCommandStage::CreatingProject,
                effect_error("project_creation_commit_unknown"),
                EffectKind::None,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn advance_handoff(
        &self,
        record: &mut ProjectSagaRecord,
        target_agent: AgentId,
        provider: ProviderId,
        resume_session: Option<ProviderSessionId>,
        thread_id: ThreadId,
        launch_directory: ResourceLocator,
        force_takeover: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        let stage = current_stage(record);
        if !matches!(
            stage,
            ProjectCommandStage::Accepted
                | ProjectCommandStage::QuiescingRuntime
                | ProjectCommandStage::EndingAssignment
        ) {
            return self.advance_activation(
                record,
                target_agent,
                provider,
                resume_session,
                Some(thread_id),
                launch_directory,
            );
        }
        let snapshot =
            self.canonical
                .snapshot(record.project_id, record.account_id, Some(target_agent))?;
        if snapshot.home != record.home || snapshot.project_id != record.project_id {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Unauthorized, "project_wrong_home"),
            );
        }
        if let Some(pending) = record.pending_canonical_mutation.clone() {
            return match self.replay_canonical_mutation(record)? {
                CanonicalProjectMutationOutcome::Committed { .. } => {
                    self.finish_handoff_mutation(record, &pending.action)
                }
                CanonicalProjectMutationOutcome::Rejected(error) => {
                    reject(&self.store, record, error)
                }
                CanonicalProjectMutationOutcome::Uncertain => reconcile(
                    &self.store,
                    record,
                    stage,
                    effect_error("project_handoff_commit_unknown"),
                    EffectKind::None,
                ),
            };
        }
        match stage {
            ProjectCommandStage::Accepted => {
                self.accept_handoff(record, &snapshot, target_agent, thread_id, force_takeover)
            }
            ProjectCommandStage::QuiescingRuntime => {
                let Some(assignment) = snapshot.assignment.as_ref() else {
                    return reject(
                        &self.store,
                        record,
                        error(ErrorCategory::Conflict, "project_not_assigned"),
                    );
                };
                self.observe_assignment_stop(record, assignment, force_takeover)?;
                checkpoint(&self.store, record, ProjectCommandStage::EndingAssignment)
            }
            ProjectCommandStage::EndingAssignment => {
                self.commit_handoff_resolution(record, &snapshot, force_takeover)
            }
            _ => Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )),
        }
    }

    fn accept_handoff(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        target_agent: AgentId,
        thread_id: ThreadId,
        force_takeover: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        if Some(snapshot.head) != record.expected_head {
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
        if snapshot.archived
            || snapshot.lifecycle != CanonicalProjectLifecycle::Open
            || !snapshot.claimable
        {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_invalid_transition"),
            );
        }
        let Some(assignment) = snapshot.assignment.as_ref() else {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_not_assigned"),
            );
        };
        if assignment.intent.agent_id == target_agent {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_handoff_same_agent"),
            );
        }
        if assignment.blocked && !force_takeover {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_handoff_force_required"),
            );
        }
        if !snapshot.requested_agent_available {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_agent_unavailable"),
            );
        }
        if !snapshot.historical_threads.contains(&thread_id) {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_activation_thread_missing"),
            );
        }
        checkpoint(&self.store, record, ProjectCommandStage::QuiescingRuntime)
    }

    fn observe_assignment_stop(
        &self,
        record: &mut ProjectSagaRecord,
        assignment: &CanonicalProjectAssignment,
        force: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        let operation = derived_operation(record.operation_id, b"assignment-quiescence", &[]);
        record.runtime_operation_id = Some(operation);
        if matches!(record.runtime_effect, SagaEffectState::NotStarted) {
            record.runtime_effect = SagaEffectState::Pending;
            persist(&self.store, record)?;
        }
        let request = EffectRequest {
            operation_id: operation,
            request_digest: close_runtime_digest(record.project_id, assignment),
            issued_at: record.issued_at,
            body: ProjectRuntimeRequest {
                project_id: record.project_id,
                agent_id: assignment.intent.agent_id,
                provider: assignment.intent.provider.clone(),
                resume_session: assignment
                    .binding
                    .as_ref()
                    .map(|binding| binding.session.clone()),
                launch_directory: None,
            },
        };
        match self.runtime.stop(&request)? {
            EffectOutcome::Accepted(()) => {
                record.runtime_effect = SagaEffectState::Accepted;
            }
            EffectOutcome::Rejected(error) => {
                record.runtime_effect = SagaEffectState::Rejected(error.clone());
                if !force {
                    record.failure = Some(error);
                }
            }
            EffectOutcome::Uncertain(_) => {
                let error = effect_error("project_runtime_stop_unknown");
                record.runtime_effect = SagaEffectState::Uncertain(error.clone());
                if !force {
                    record.failure = Some(error);
                }
            }
        }
        persist(&self.store, record)
    }

    fn commit_handoff_resolution(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        force: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        let assignment = snapshot.assignment.as_ref().ok_or_else(|| {
            hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )
        })?;
        let action = if let Some(failure) = record.failure.as_ref() {
            CanonicalProjectMutationAction::BlockAssignment {
                assignment_id: assignment.intent.assignment_id,
                cause: failure.code().clone(),
            }
        } else {
            CanonicalProjectMutationAction::EndAssignment {
                assignment_id: assignment.intent.assignment_id,
                forced: force && !matches!(record.runtime_effect, SagaEffectState::Accepted),
                runtime: close_runtime_observation(&record.runtime_effect)?,
            }
        };
        match self.mutate(
            record,
            snapshot.head,
            b"handoff-old-assignment",
            action.clone(),
        )? {
            CanonicalProjectMutationOutcome::Committed { .. } => {
                self.finish_handoff_mutation(record, &action)
            }
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                ProjectCommandStage::EndingAssignment,
                effect_error("project_handoff_commit_unknown"),
                EffectKind::None,
            ),
        }
    }

    fn finish_handoff_mutation(
        &self,
        record: &mut ProjectSagaRecord,
        action: &CanonicalProjectMutationAction,
    ) -> Result<(), hq_application::ApplicationError> {
        match action {
            CanonicalProjectMutationAction::BlockAssignment { .. } => {
                let failure = record.failure.clone().ok_or_else(|| {
                    hq_application::ApplicationError::new(
                        hq_application::ApplicationErrorCode::StateCorrupt,
                    )
                })?;
                reject(&self.store, record, failure)
            }
            CanonicalProjectMutationAction::EndAssignment { .. } => {
                record.runtime_operation_id = None;
                record.runtime_effect = SagaEffectState::NotStarted;
                record.runtime_session = None;
                checkpoint(
                    &self.store,
                    record,
                    ProjectCommandStage::ValidatingResources,
                )
            }
            _ => Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )),
        }
    }

    fn advance_retirement(
        &self,
        record: &mut ProjectSagaRecord,
        agent_id: AgentId,
        force: bool,
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
        if record.pending_canonical_mutation.is_some() {
            return self.replay_retirement_mutation(record, stage);
        }
        match stage {
            ProjectCommandStage::Accepted => {
                self.accept_retirement(record, &snapshot, agent_id, force)
            }
            ProjectCommandStage::QuiescingRuntime => {
                let assignment = snapshot.assignment.as_ref().ok_or_else(|| {
                    hq_application::ApplicationError::new(
                        hq_application::ApplicationErrorCode::StateCorrupt,
                    )
                })?;
                self.observe_assignment_stop(record, assignment, force)?;
                checkpoint(&self.store, record, ProjectCommandStage::EndingAssignment)
            }
            ProjectCommandStage::EndingAssignment => {
                self.commit_retirement_assignment(record, &snapshot, force)
            }
            ProjectCommandStage::UpdatingProject => self.commit_terminal_mutation(
                record,
                snapshot.head,
                b"retire-agent",
                CanonicalProjectMutationAction::RetireAgent { agent_id },
                "project_retirement_commit_unknown",
            ),
            _ => Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )),
        }
    }

    fn replay_retirement_mutation(
        &self,
        record: &mut ProjectSagaRecord,
        stage: ProjectCommandStage,
    ) -> Result<(), hq_application::ApplicationError> {
        let pending = record.pending_canonical_mutation.clone().ok_or_else(|| {
            hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )
        })?;
        match self.replay_canonical_mutation(record)? {
            CanonicalProjectMutationOutcome::Committed { project_head } => match pending.action {
                CanonicalProjectMutationAction::BlockAssignment { .. } => {
                    let failure = record.failure.clone().ok_or_else(|| {
                        hq_application::ApplicationError::new(
                            hq_application::ApplicationErrorCode::StateCorrupt,
                        )
                    })?;
                    reject(&self.store, record, failure)
                }
                CanonicalProjectMutationAction::EndAssignment { .. } => {
                    checkpoint(&self.store, record, ProjectCommandStage::UpdatingProject)
                }
                CanonicalProjectMutationAction::RetireAgent { .. } => {
                    record.state = ProjectSagaState::Completed { project_head };
                    persist(&self.store, record)
                }
                _ => Err(hq_application::ApplicationError::new(
                    hq_application::ApplicationErrorCode::StateCorrupt,
                )),
            },
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                stage,
                effect_error("project_retirement_commit_unknown"),
                EffectKind::None,
            ),
        }
    }

    fn accept_retirement(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        agent_id: AgentId,
        force: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        if Some(snapshot.head) != record.expected_head {
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
        if !snapshot.requested_agent_available {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_agent_unavailable"),
            );
        }
        match snapshot.assignment.as_ref() {
            Some(assignment) if assignment.intent.agent_id == agent_id => {
                if snapshot.lifecycle != CanonicalProjectLifecycle::Open {
                    return reject(
                        &self.store,
                        record,
                        error(ErrorCategory::Conflict, "project_invalid_transition"),
                    );
                }
                if assignment.blocked && !force {
                    return reject(
                        &self.store,
                        record,
                        error(ErrorCategory::Conflict, "project_retirement_force_required"),
                    );
                }
                checkpoint(&self.store, record, ProjectCommandStage::QuiescingRuntime)
            }
            Some(_) | None => checkpoint(&self.store, record, ProjectCommandStage::UpdatingProject),
        }
    }

    fn commit_retirement_assignment(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        force: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        let assignment = snapshot.assignment.as_ref().ok_or_else(|| {
            hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )
        })?;
        let action = if let Some(failure) = record.failure.as_ref() {
            CanonicalProjectMutationAction::BlockAssignment {
                assignment_id: assignment.intent.assignment_id,
                cause: failure.code().clone(),
            }
        } else {
            CanonicalProjectMutationAction::EndAssignment {
                assignment_id: assignment.intent.assignment_id,
                forced: force && !matches!(record.runtime_effect, SagaEffectState::Accepted),
                runtime: close_runtime_observation(&record.runtime_effect)?,
            }
        };
        match self.mutate(
            record,
            snapshot.head,
            b"retirement-end-assignment",
            action.clone(),
        )? {
            CanonicalProjectMutationOutcome::Committed { .. } => match action {
                CanonicalProjectMutationAction::BlockAssignment { .. } => {
                    let failure = record.failure.clone().ok_or_else(|| {
                        hq_application::ApplicationError::new(
                            hq_application::ApplicationErrorCode::StateCorrupt,
                        )
                    })?;
                    reject(&self.store, record, failure)
                }
                CanonicalProjectMutationAction::EndAssignment { .. } => {
                    checkpoint(&self.store, record, ProjectCommandStage::UpdatingProject)
                }
                _ => unreachable!(),
            },
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                ProjectCommandStage::EndingAssignment,
                effect_error("project_retirement_assignment_unknown"),
                EffectKind::None,
            ),
        }
    }

    fn advance_close(
        &self,
        record: &mut ProjectSagaRecord,
        force: bool,
        archive_after_close: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        let snapshot = self
            .canonical
            .snapshot(record.project_id, record.account_id, None)?;
        if snapshot.home != record.home || snapshot.project_id != record.project_id {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Unauthorized, "project_wrong_home"),
            );
        }
        let stage = current_stage(record);
        if let Some(pending) = record.pending_canonical_mutation.clone() {
            return match self.replay_canonical_mutation(record)? {
                CanonicalProjectMutationOutcome::Committed { project_head } => self
                    .advance_after_close_mutation(
                        record,
                        &pending.action,
                        project_head,
                        archive_after_close,
                    ),
                CanonicalProjectMutationOutcome::Rejected(error) => {
                    reject(&self.store, record, error)
                }
                CanonicalProjectMutationOutcome::Uncertain => reconcile(
                    &self.store,
                    record,
                    stage,
                    effect_error("project_canonical_commit_unknown"),
                    EffectKind::None,
                ),
            };
        }
        match stage {
            ProjectCommandStage::Accepted => {
                self.accept_close(record, &snapshot, archive_after_close)
            }
            ProjectCommandStage::AssessingRelease => {
                self.assess_close_release(record, &snapshot, force)
            }
            ProjectCommandStage::QuiescingRuntime => {
                self.quiesce_close_runtime(record, &snapshot, force)
            }
            ProjectCommandStage::Closing => {
                let action = CanonicalProjectMutationAction::FinishClosing {
                    forced: force,
                    runtime: close_runtime_observation(&record.runtime_effect)?,
                };
                match self.mutate(record, snapshot.head, b"finish-close", action.clone())? {
                    CanonicalProjectMutationOutcome::Committed { project_head } => self
                        .advance_after_close_mutation(
                            record,
                            &action,
                            project_head,
                            archive_after_close,
                        ),
                    CanonicalProjectMutationOutcome::Rejected(error) => {
                        reject(&self.store, record, error)
                    }
                    CanonicalProjectMutationOutcome::Uncertain => reconcile(
                        &self.store,
                        record,
                        stage,
                        effect_error("project_close_commit_unknown"),
                        EffectKind::None,
                    ),
                }
            }
            ProjectCommandStage::UpdatingProject if archive_after_close => self
                .commit_terminal_mutation(
                    record,
                    snapshot.head,
                    b"archive",
                    CanonicalProjectMutationAction::Archive,
                    "project_archive_commit_unknown",
                ),
            _ => reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_invalid_transition"),
            ),
        }
    }

    fn accept_close(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        archive_after_close: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        if Some(snapshot.head) != record.expected_head {
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
        match snapshot.lifecycle {
            CanonicalProjectLifecycle::Closed if archive_after_close => {
                checkpoint(&self.store, record, ProjectCommandStage::UpdatingProject)
            }
            CanonicalProjectLifecycle::Open | CanonicalProjectLifecycle::Closing => {
                checkpoint(&self.store, record, ProjectCommandStage::AssessingRelease)
            }
            CanonicalProjectLifecycle::Closed => reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_invalid_transition"),
            ),
        }
    }

    fn assess_close_release(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        force: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        let operation = derived_operation(record.operation_id, b"release-assessment", &[]);
        record.resource_operation_id = Some(operation);
        if matches!(record.resource_effect, SagaEffectState::NotStarted) {
            record.resource_effect = SagaEffectState::Pending;
            persist(&self.store, record)?;
        }
        let request = EffectRequest {
            operation_id: operation,
            request_digest: release_assessment_digest(record, &snapshot.resources),
            issued_at: record.issued_at,
            body: ProjectReleaseAssessmentRequest {
                home: record.home,
                project_id: record.project_id,
                resources: snapshot.resources.clone(),
            },
        };
        match self.resources.assess_release(&request)? {
            EffectOutcome::Accepted(assessments)
                if release_assessments_match(record.home, &snapshot.resources, &assessments) =>
            {
                match decide_release(&assessments, force) {
                    ReleaseDecision::Proceed | ReleaseDecision::Forced { .. } => {
                        record.resource_effect = SagaEffectState::Accepted;
                        persist(&self.store, record)?;
                        self.begin_or_resume_closing(record, snapshot)
                    }
                    ReleaseDecision::ForceRequired { .. } => reject(
                        &self.store,
                        record,
                        error(ErrorCategory::Conflict, "project_release_force_required"),
                    ),
                }
            }
            EffectOutcome::Accepted(_) => reject(
                &self.store,
                record,
                error(
                    ErrorCategory::Conflict,
                    "project_release_assessment_changed",
                ),
            ),
            EffectOutcome::Rejected(error) if force => {
                record.resource_effect = SagaEffectState::Rejected(error);
                persist(&self.store, record)?;
                self.begin_or_resume_closing(record, snapshot)
            }
            EffectOutcome::Rejected(error) => reject(&self.store, record, error),
            EffectOutcome::Uncertain(_) if force => {
                record.resource_effect =
                    SagaEffectState::Uncertain(effect_error("project_release_assessment_unknown"));
                persist(&self.store, record)?;
                self.begin_or_resume_closing(record, snapshot)
            }
            EffectOutcome::Uncertain(_) => reconcile(
                &self.store,
                record,
                ProjectCommandStage::AssessingRelease,
                effect_error("project_release_assessment_unknown"),
                EffectKind::Resource,
            ),
        }
    }

    fn begin_or_resume_closing(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
    ) -> Result<(), hq_application::ApplicationError> {
        if snapshot.lifecycle == CanonicalProjectLifecycle::Closing {
            return checkpoint(&self.store, record, ProjectCommandStage::QuiescingRuntime);
        }
        match self.mutate(
            record,
            snapshot.head,
            b"begin-close",
            CanonicalProjectMutationAction::BeginClosing,
        )? {
            CanonicalProjectMutationOutcome::Committed { .. } => {
                checkpoint(&self.store, record, ProjectCommandStage::QuiescingRuntime)
            }
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                ProjectCommandStage::AssessingRelease,
                effect_error("project_begin_close_unknown"),
                EffectKind::None,
            ),
        }
    }

    fn quiesce_close_runtime(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        force: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        if snapshot.lifecycle != CanonicalProjectLifecycle::Closing {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Conflict, "project_invalid_transition"),
            );
        }
        let Some(assignment) = snapshot.assignment.as_ref() else {
            return checkpoint(&self.store, record, ProjectCommandStage::Closing);
        };
        let operation = derived_operation(record.operation_id, b"close-runtime", &[]);
        record.runtime_operation_id = Some(operation);
        if matches!(record.runtime_effect, SagaEffectState::NotStarted) {
            record.runtime_effect = SagaEffectState::Pending;
            persist(&self.store, record)?;
        }
        let request = EffectRequest {
            operation_id: operation,
            request_digest: close_runtime_digest(record.project_id, assignment),
            issued_at: record.issued_at,
            body: ProjectRuntimeRequest {
                project_id: record.project_id,
                agent_id: assignment.intent.agent_id,
                provider: assignment.intent.provider.clone(),
                resume_session: assignment
                    .binding
                    .as_ref()
                    .map(|binding| binding.session.clone()),
                launch_directory: None,
            },
        };
        let (runtime, forced) = match self.runtime.stop(&request)? {
            EffectOutcome::Accepted(()) => {
                record.runtime_effect = SagaEffectState::Accepted;
                (RuntimeObservation::Succeeded, false)
            }
            EffectOutcome::Rejected(error) if force => {
                let runtime = RuntimeObservation::Failed(error.code().clone());
                record.runtime_effect = SagaEffectState::Rejected(error);
                (runtime, true)
            }
            EffectOutcome::Rejected(error) => return reject(&self.store, record, error),
            EffectOutcome::Uncertain(_) if force => {
                let error = effect_error("project_runtime_stop_unknown");
                let runtime = RuntimeObservation::Uncertain(error.code().clone());
                record.runtime_effect = SagaEffectState::Uncertain(error);
                (runtime, true)
            }
            EffectOutcome::Uncertain(_) => {
                return reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::QuiescingRuntime,
                    effect_error("project_runtime_stop_unknown"),
                    EffectKind::Runtime,
                );
            }
        };
        persist(&self.store, record)?;
        match self.mutate(
            record,
            snapshot.head,
            b"close-end-assignment",
            CanonicalProjectMutationAction::EndAssignment {
                assignment_id: assignment.intent.assignment_id,
                forced,
                runtime: Some(runtime),
            },
        )? {
            CanonicalProjectMutationOutcome::Committed { .. } => {
                checkpoint(&self.store, record, ProjectCommandStage::Closing)
            }
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                ProjectCommandStage::QuiescingRuntime,
                effect_error("project_assignment_end_unknown"),
                EffectKind::None,
            ),
        }
    }

    fn advance_after_close_mutation(
        &self,
        record: &mut ProjectSagaRecord,
        action: &CanonicalProjectMutationAction,
        project_head: FactId,
        archive_after_close: bool,
    ) -> Result<(), hq_application::ApplicationError> {
        match action {
            CanonicalProjectMutationAction::BeginClosing => {
                checkpoint(&self.store, record, ProjectCommandStage::QuiescingRuntime)
            }
            CanonicalProjectMutationAction::EndAssignment { .. } => {
                checkpoint(&self.store, record, ProjectCommandStage::Closing)
            }
            CanonicalProjectMutationAction::FinishClosing { .. } if archive_after_close => {
                checkpoint(&self.store, record, ProjectCommandStage::UpdatingProject)
            }
            CanonicalProjectMutationAction::FinishClosing { .. }
            | CanonicalProjectMutationAction::Archive => {
                record.state = ProjectSagaState::Completed { project_head };
                persist(&self.store, record)
            }
            _ => Err(hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )),
        }
    }

    fn advance_unarchive(
        &self,
        record: &mut ProjectSagaRecord,
    ) -> Result<(), hq_application::ApplicationError> {
        let snapshot = self
            .canonical
            .snapshot(record.project_id, record.account_id, None)?;
        if snapshot.home != record.home || snapshot.project_id != record.project_id {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Unauthorized, "project_wrong_home"),
            );
        }
        if current_stage(record) == ProjectCommandStage::Accepted {
            if Some(snapshot.head) != record.expected_head {
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
            checkpoint(&self.store, record, ProjectCommandStage::UpdatingProject)?;
            return Ok(());
        }
        self.commit_terminal_mutation(
            record,
            snapshot.head,
            b"unarchive",
            CanonicalProjectMutationAction::Unarchive,
            "project_unarchive_commit_unknown",
        )
    }

    fn commit_terminal_mutation(
        &self,
        record: &mut ProjectSagaRecord,
        expected_head: FactId,
        tag: &[u8],
        action: CanonicalProjectMutationAction,
        uncertain_code: &'static str,
    ) -> Result<(), hq_application::ApplicationError> {
        let stage = current_stage(record);
        let outcome = if record.pending_canonical_mutation.is_some() {
            self.replay_canonical_mutation(record)?
        } else {
            self.mutate(record, expected_head, tag, action)?
        };
        match outcome {
            CanonicalProjectMutationOutcome::Committed { project_head } => {
                record.state = ProjectSagaState::Completed { project_head };
                persist(&self.store, record)
            }
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                stage,
                effect_error(uncertain_code),
                EffectKind::None,
            ),
        }
    }

    fn advance_resource_mutation(
        &self,
        record: &mut ProjectSagaRecord,
    ) -> Result<(), hq_application::ApplicationError> {
        let snapshot = self
            .canonical
            .snapshot(record.project_id, record.account_id, None)?;
        if snapshot.home != record.home || snapshot.project_id != record.project_id {
            return reject(
                &self.store,
                record,
                error(ErrorCategory::Unauthorized, "project_wrong_home"),
            );
        }
        let stage = current_stage(record);
        if record.pending_canonical_mutation.is_some() {
            return match self.replay_canonical_mutation(record)? {
                CanonicalProjectMutationOutcome::Committed { project_head } => {
                    record.state = ProjectSagaState::Completed { project_head };
                    persist(&self.store, record)
                }
                CanonicalProjectMutationOutcome::Rejected(error) => {
                    reject(&self.store, record, error)
                }
                CanonicalProjectMutationOutcome::Uncertain => reconcile(
                    &self.store,
                    record,
                    stage,
                    effect_error("project_canonical_commit_unknown"),
                    EffectKind::None,
                ),
            };
        }
        if stage == ProjectCommandStage::Accepted {
            return self.accept_resource_mutation(record, &snapshot);
        }

        let Some(action) = self.prepare_resource_mutation(record, &snapshot, stage)? else {
            return Ok(());
        };
        let tag = direct_mutation_tag(&action).ok_or_else(|| {
            hq_application::ApplicationError::new(
                hq_application::ApplicationErrorCode::StateCorrupt,
            )
        })?;
        match self.mutate(record, snapshot.head, tag, action)? {
            CanonicalProjectMutationOutcome::Committed { project_head } => {
                record.state = ProjectSagaState::Completed { project_head };
                persist(&self.store, record)
            }
            CanonicalProjectMutationOutcome::Rejected(error) => reject(&self.store, record, error),
            CanonicalProjectMutationOutcome::Uncertain => reconcile(
                &self.store,
                record,
                stage,
                effect_error("project_canonical_commit_unknown"),
                EffectKind::None,
            ),
        }
    }

    fn accept_resource_mutation(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
    ) -> Result<(), hq_application::ApplicationError> {
        if let Some(error) = direct_precondition_error(record, snapshot) {
            return reject(&self.store, record, error);
        }
        let next_stage = match record.action {
            ProjectCommandAction::RemoveResource { .. } => ProjectCommandStage::UpdatingProject,
            _ => ProjectCommandStage::ValidatingResources,
        };
        checkpoint(&self.store, record, next_stage)
    }

    fn prepare_resource_mutation(
        &self,
        record: &mut ProjectSagaRecord,
        snapshot: &ProjectWorkflowSnapshot,
        stage: ProjectCommandStage,
    ) -> Result<Option<CanonicalProjectMutationAction>, hq_application::ApplicationError> {
        let requested_action = record.action.clone();
        let action = match &requested_action {
            ProjectCommandAction::Open if stage == ProjectCommandStage::ValidatingResources => {
                if !self.validate_resource_observation(record, &snapshot.resources)? {
                    return Ok(None);
                }
                CanonicalProjectMutationAction::Open
            }
            ProjectCommandAction::AddResource {
                resource,
                make_primary,
            } if stage == ProjectCommandStage::ValidatingResources => {
                if !self.validate_resource_observation(record, std::slice::from_ref(resource))? {
                    return Ok(None);
                }
                CanonicalProjectMutationAction::AddResource {
                    resource: resource.clone(),
                    make_primary: *make_primary,
                }
            }
            ProjectCommandAction::ReplaceResource {
                old_resource_id,
                new_resource,
            } if stage == ProjectCommandStage::ValidatingResources => {
                if !self
                    .validate_resource_observation(record, std::slice::from_ref(new_resource))?
                {
                    return Ok(None);
                }
                CanonicalProjectMutationAction::ReplaceResource {
                    old_resource_id: *old_resource_id,
                    new_resource: new_resource.clone(),
                }
            }
            ProjectCommandAction::RemoveResource { resource_id, force }
                if stage == ProjectCommandStage::UpdatingProject =>
            {
                CanonicalProjectMutationAction::RemoveResource {
                    resource_id: *resource_id,
                    force: *force,
                }
            }
            _ => {
                reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_invalid_transition"),
                )?;
                return Ok(None);
            }
        };
        Ok(Some(action))
    }

    fn validate_resource_observation(
        &self,
        record: &mut ProjectSagaRecord,
        resources: &[ProjectResource],
    ) -> Result<bool, hq_application::ApplicationError> {
        let operation = derived_operation(record.operation_id, b"resource-validation", &[]);
        record.resource_operation_id = Some(operation);
        if matches!(record.resource_effect, SagaEffectState::NotStarted) {
            record.resource_effect = SagaEffectState::Pending;
            persist(&self.store, record)?;
        }
        let request = EffectRequest {
            operation_id: operation,
            request_digest: resource_validation_digest(record, resources),
            issued_at: record.issued_at,
            body: ProjectResourceValidationRequest {
                home: record.home,
                project_id: record.project_id,
                resources: resources.to_vec(),
            },
        };
        match self.resources.validate_resources(&request)? {
            EffectOutcome::Accepted(observations)
                if resources_match_observations(resources, &observations) =>
            {
                record.resource_effect = SagaEffectState::Accepted;
                persist(&self.store, record)?;
                Ok(true)
            }
            EffectOutcome::Accepted(_) => {
                reject(
                    &self.store,
                    record,
                    error(ErrorCategory::Conflict, "project_resource_identity_changed"),
                )?;
                Ok(false)
            }
            EffectOutcome::Rejected(error) => {
                reject(&self.store, record, error)?;
                Ok(false)
            }
            EffectOutcome::Uncertain(_) => {
                reconcile(
                    &self.store,
                    record,
                    ProjectCommandStage::ValidatingResources,
                    effect_error("project_resource_validation_unknown"),
                    EffectKind::Resource,
                )?;
                Ok(false)
            }
        }
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
            if Some(snapshot.head) != record.expected_head {
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
                    launch_directory: Some(launch_directory.clone()),
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
            if Some(snapshot.head) != record.expected_head {
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
                    launch_directory: None,
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
                        forced: false,
                        runtime: None,
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
                        CanonicalProjectMutationAction::FinishClosing {
                            forced: false,
                            runtime: None,
                        },
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
            request_digest: mutation_digest(record.operation_id, tag, Some(expected_head), &action),
            account_id: record.account_id,
            project_id: record.project_id,
            home: record.home,
            expected_head: Some(expected_head),
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

fn valid_provisioning_paths(request: &WorktreeProvisioningRequest) -> bool {
    exact_normalized_path(
        &request.source,
        &[ResourceScheme::GitRepository, ResourceScheme::WorkingTree],
    ) && exact_normalized_path(&request.destination, &[ResourceScheme::WorkingTree])
}

fn exact_normalized_path(locator: &ResourceLocator, schemes: &[ResourceScheme]) -> bool {
    let original = Path::new(locator.value());
    schemes.contains(&locator.scheme())
        && normalize_absolute_path(original)
            .is_ok_and(|normalized| normalized.as_os_str() == original.as_os_str())
}

impl<S, C, R, F, G> hq_application::ControlProjects for ProjectWorkflowManager<S, C, R, F, G>
where
    S: ProjectSagaStore,
    C: CanonicalProjectPort,
    R: ProjectRuntimePort,
    F: ProjectResourcePort,
    G: GitWorktreePort,
{
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, hq_application::ApplicationError> {
        self.control(request)
    }
}

impl<S, C, R, F, G> hq_application::RetireAgents for ProjectWorkflowManager<S, C, R, F, G>
where
    S: ProjectSagaStore,
    C: CanonicalProjectPort,
    R: ProjectRuntimePort,
    F: ProjectResourcePort,
    G: GitWorktreePort,
{
    fn retire_agent(
        &self,
        request: AgentRetirementRequest,
    ) -> Result<AgentRetirementOutcome, hq_application::ApplicationError> {
        ProjectWorkflowManager::retire_agent(self, request)
    }
}

fn retirement_record_matches(record: &ProjectSagaRecord, request: &AgentRetirementRequest) -> bool {
    record.command_id == request.command_id
        && record.operation_id == request.operation_id
        && record.request_digest == request.request_digest
        && record.account_id == request.account_id
        && record.home == request.home
        && record.issued_at == request.issued_at
        && record.action
            == (ProjectCommandAction::RetireAgent {
                agent_id: request.agent_id,
                force: request.force,
            })
}

fn retirement_from_project(
    outcome: ProjectCommandOutcome,
    project_id: ProjectId,
) -> AgentRetirementOutcome {
    match outcome {
        ProjectCommandOutcome::Accepted {
            operation_id,
            stage,
        }
        | ProjectCommandOutcome::Running {
            operation_id,
            stage,
        } => AgentRetirementOutcome::Running {
            operation_id,
            stage,
        },
        ProjectCommandOutcome::Completed {
            operation_id,
            runtime,
            ..
        } => AgentRetirementOutcome::Completed {
            operation_id,
            project_id: Some(project_id),
            runtime,
        },
        ProjectCommandOutcome::Rejected {
            operation_id,
            error,
            runtime,
        } => AgentRetirementOutcome::Rejected {
            operation_id,
            error,
            runtime,
        },
        ProjectCommandOutcome::Reconcilable {
            operation_id,
            stage,
            error,
        } => AgentRetirementOutcome::Reconcilable {
            operation_id,
            stage,
            error,
        },
    }
}

fn retirement_from_mutation(
    request: &AgentRetirementRequest,
    attempt: MutationAttempt,
) -> AgentRetirementOutcome {
    match attempt {
        MutationAttempt::Completed(receipt) => match receipt.outcome() {
            MutationOutcome::Committed => AgentRetirementOutcome::Completed {
                operation_id: request.operation_id,
                project_id: None,
                runtime: None,
            },
            MutationOutcome::Rejected(error) => AgentRetirementOutcome::Rejected {
                operation_id: request.operation_id,
                error: error.clone(),
                runtime: None,
            },
        },
        MutationAttempt::Uncertain { .. } => AgentRetirementOutcome::Reconcilable {
            operation_id: request.operation_id,
            stage: ProjectCommandStage::ReconciliationRequired,
            error: effect_error("agent_retirement_commit_unknown"),
        },
    }
}

fn retirement_rejected(operation_id: OperationId, code: &'static str) -> AgentRetirementOutcome {
    AgentRetirementOutcome::Rejected {
        operation_id,
        error: error(ErrorCategory::Conflict, code),
        runtime: None,
    }
}

impl<S, C, R, F, G> crate::RepairLocalProjectWorkflows for ProjectWorkflowManager<S, C, R, F, G>
where
    S: ProjectSagaStore,
    C: CanonicalProjectPort,
    R: ProjectRuntimePort,
    F: ProjectResourcePort,
    G: GitWorktreePort,
{
    fn repair_local(
        &self,
        limit: usize,
    ) -> Result<Vec<ProjectCommandOutcome>, hq_application::ApplicationError> {
        self.repair(limit)
    }
}

#[derive(Clone, Copy)]
enum EffectKind {
    None,
    Runtime,
    Dispatch,
    Resource,
    Git,
}

fn direct_precondition_error(
    record: &ProjectSagaRecord,
    snapshot: &ProjectWorkflowSnapshot,
) -> Option<DomainError> {
    if Some(snapshot.head) != record.expected_head {
        return Some(error(ErrorCategory::Conflict, "project_stale_head"));
    }
    if !snapshot.active_human {
        return Some(error(ErrorCategory::Unauthorized, "project_inactive_human"));
    }
    if snapshot.archived {
        return Some(error(ErrorCategory::Conflict, "project_archived"));
    }
    if snapshot.lifecycle == CanonicalProjectLifecycle::Closing {
        return Some(error(ErrorCategory::Conflict, "project_invalid_transition"));
    }
    let resource_exists = |resource_id| {
        snapshot
            .resources
            .iter()
            .any(|resource| resource.resource_id == resource_id)
    };
    match &record.action {
        ProjectCommandAction::Open if snapshot.lifecycle != CanonicalProjectLifecycle::Closed => {
            Some(error(ErrorCategory::Conflict, "project_invalid_transition"))
        }
        ProjectCommandAction::Open if !snapshot.claimable => Some(error(
            ErrorCategory::Conflict,
            "project_resource_claim_conflict",
        )),
        ProjectCommandAction::AddResource { resource, .. }
            if resource_exists(resource.resource_id) =>
        {
            Some(error(ErrorCategory::Conflict, "project_resource_exists"))
        }
        ProjectCommandAction::RemoveResource { resource_id, .. }
            if !resource_exists(*resource_id) =>
        {
            Some(error(ErrorCategory::Conflict, "project_resource_missing"))
        }
        ProjectCommandAction::RemoveResource { force: false, .. }
            if snapshot.assignment.is_some() =>
        {
            Some(error(
                ErrorCategory::Conflict,
                "project_resource_force_required",
            ))
        }
        ProjectCommandAction::ReplaceResource {
            old_resource_id,
            new_resource,
        } if !resource_exists(*old_resource_id)
            || (*old_resource_id != new_resource.resource_id
                && resource_exists(new_resource.resource_id)) =>
        {
            Some(error(
                ErrorCategory::Conflict,
                "project_resource_invalid_replace",
            ))
        }
        _ => None,
    }
}

fn direct_mutation_tag(action: &CanonicalProjectMutationAction) -> Option<&'static [u8]> {
    match action {
        CanonicalProjectMutationAction::Open => Some(b"open"),
        CanonicalProjectMutationAction::AddResource { .. } => Some(b"add-resource"),
        CanonicalProjectMutationAction::RemoveResource { .. } => Some(b"remove-resource"),
        CanonicalProjectMutationAction::ReplaceResource { .. } => Some(b"replace-resource"),
        _ => None,
    }
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

fn resources_match_observations(
    desired: &[ProjectResource],
    observations: &[ProjectResourceObservation],
) -> bool {
    desired.len() == observations.len()
        && desired.iter().all(|resource| {
            observations.iter().any(|observation| {
                observation.resource_id == resource.resource_id
                    && observation.observed_canonical.as_ref() == Some(&resource.canonical_locator)
            })
        })
}

fn release_assessments_match(
    home: InstallationId,
    resources: &[ProjectResource],
    assessments: &[PathReleaseAssessment],
) -> bool {
    resources.len() == assessments.len()
        && resources.iter().all(|resource| {
            assessments.iter().any(|assessment| {
                assessment.home == home && assessment.resource_id == resource.resource_id
            })
        })
}

fn resource_validation_digest(
    record: &ProjectSagaRecord,
    resources: &[ProjectResource],
) -> CommandDigest {
    let mut digest = Sha256::new();
    put(&mut digest, b"hq-project-resource-validation-v1");
    put(&mut digest, record.operation_id.as_bytes());
    for resource in resources {
        put_resource(&mut digest, resource);
    }
    CommandDigest::from_bytes(digest.finalize().into())
}

fn release_assessment_digest(
    record: &ProjectSagaRecord,
    resources: &[ProjectResource],
) -> CommandDigest {
    let mut digest = Sha256::new();
    put(&mut digest, b"hq-project-release-assessment-v1");
    put(&mut digest, record.operation_id.as_bytes());
    for resource in resources {
        put_resource(&mut digest, resource);
    }
    CommandDigest::from_bytes(digest.finalize().into())
}

fn close_runtime_digest(
    project_id: ProjectId,
    assignment: &CanonicalProjectAssignment,
) -> CommandDigest {
    let mut digest = Sha256::new();
    put(&mut digest, b"hq-project-close-runtime-v1");
    put(&mut digest, project_id.as_bytes());
    put(&mut digest, assignment.intent.assignment_id.as_bytes());
    put(&mut digest, assignment.intent.agent_id.as_bytes());
    put(&mut digest, assignment.intent.provider.as_str().as_bytes());
    if let Some(binding) = &assignment.binding {
        put_binding(&mut digest, binding);
    }
    CommandDigest::from_bytes(digest.finalize().into())
}

fn close_runtime_observation(
    effect: &SagaEffectState,
) -> Result<Option<RuntimeObservation>, hq_application::ApplicationError> {
    match effect {
        SagaEffectState::NotStarted => Ok(None),
        SagaEffectState::Accepted => Ok(Some(RuntimeObservation::Succeeded)),
        SagaEffectState::Rejected(error) => {
            Ok(Some(RuntimeObservation::Failed(error.code().clone())))
        }
        SagaEffectState::Uncertain(error) => {
            Ok(Some(RuntimeObservation::Uncertain(error.code().clone())))
        }
        SagaEffectState::Pending => Err(hq_application::ApplicationError::new(
            hq_application::ApplicationErrorCode::StateCorrupt,
        )),
    }
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
        EffectKind::Git => record.git_effect = SagaEffectState::Uncertain(error.clone()),
    }
    record.state = ProjectSagaState::Reconcilable { stage, error };
    persist(store, record)
}

fn terminal_outcome(record: &ProjectSagaRecord) -> Option<ProjectCommandOutcome> {
    match &record.state {
        ProjectSagaState::Completed { project_head } => Some(ProjectCommandOutcome::Completed {
            operation_id: record.operation_id,
            project_head: *project_head,
            runtime: reported_runtime(record),
        }),
        ProjectSagaState::Rejected(error) => Some(ProjectCommandOutcome::Rejected {
            operation_id: record.operation_id,
            error: error.clone(),
            runtime: reported_runtime(record),
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
            runtime: reported_runtime(record),
        },
        ProjectSagaState::Rejected(error) => ProjectCommandOutcome::Rejected {
            operation_id: record.operation_id,
            error: error.clone(),
            runtime: reported_runtime(record),
        },
    }
}

fn reported_runtime(record: &ProjectSagaRecord) -> Option<RuntimeObservation> {
    match &record.runtime_effect {
        SagaEffectState::Accepted => Some(RuntimeObservation::Succeeded),
        SagaEffectState::Rejected(error) => Some(RuntimeObservation::Failed(error.code().clone())),
        SagaEffectState::Uncertain(error) => {
            Some(RuntimeObservation::Uncertain(error.code().clone()))
        }
        SagaEffectState::NotStarted | SagaEffectState::Pending => None,
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

fn git_effect_request(
    record: &ProjectSagaRecord,
    operation_id: OperationId,
    request: &WorktreeProvisioningRequest,
) -> EffectRequest<GitWorktreeRequest> {
    let mut digest = Sha256::new();
    put(&mut digest, b"hq-project-git-worktree-v1");
    put(&mut digest, record.operation_id.as_bytes());
    put_locator(&mut digest, &request.source);
    put_locator(&mut digest, &request.destination);
    put(&mut digest, request.branch.as_str().as_bytes());
    put(&mut digest, &[u8::from(request.create_branch)]);
    EffectRequest::new(
        operation_id,
        CommandDigest::from_bytes(digest.finalize().into()),
        record.issued_at,
        GitWorktreeRequest::from(request),
    )
}

fn provisioning_creation_mutation(
    record: &ProjectSagaRecord,
    request: &WorktreeProvisioningRequest,
    resource: ProjectResource,
) -> CanonicalProjectMutation {
    let action = CanonicalProjectMutationAction::Create {
        mailbox_id: request.mailbox_id,
        name: request.project_name.clone(),
        brief: request.brief.clone(),
        resource,
    };
    CanonicalProjectMutation {
        command_id: derived_command(
            record.operation_id,
            b"create-project",
            record.project_id.as_bytes(),
        ),
        request_digest: mutation_digest(record.operation_id, b"create-project", None, &action),
        account_id: record.account_id,
        project_id: record.project_id,
        home: record.home,
        expected_head: None,
        issued_at: record.issued_at,
        action,
    }
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

#[allow(
    clippy::too_many_lines,
    reason = "closed canonical mutation digest table"
)]
fn mutation_digest(
    operation: OperationId,
    tag: &[u8],
    expected_head: Option<FactId>,
    action: &CanonicalProjectMutationAction,
) -> CommandDigest {
    let mut digest = Sha256::new();
    put(&mut digest, b"hq-project-canonical-mutation-v1");
    put(&mut digest, operation.as_bytes());
    put(&mut digest, tag);
    match expected_head {
        Some(expected_head) => {
            put(&mut digest, &[1]);
            put(&mut digest, expected_head.as_bytes());
        }
        None => put(&mut digest, &[0]),
    }
    match action {
        CanonicalProjectMutationAction::Create {
            mailbox_id,
            name,
            brief,
            resource,
        } => {
            put(&mut digest, b"create");
            put(&mut digest, mailbox_id.as_bytes());
            put(&mut digest, name.as_str().as_bytes());
            put_optional_text(&mut digest, brief.as_ref().map(ContentText::as_str));
            put_resource(&mut digest, resource);
        }
        CanonicalProjectMutationAction::Open => put(&mut digest, b"open"),
        CanonicalProjectMutationAction::AddResource {
            resource,
            make_primary,
        } => {
            put(&mut digest, b"add-resource");
            put_resource(&mut digest, resource);
            put(&mut digest, &[u8::from(*make_primary)]);
        }
        CanonicalProjectMutationAction::RemoveResource { resource_id, force } => {
            put(&mut digest, b"remove-resource");
            put(&mut digest, resource_id.as_bytes());
            put(&mut digest, &[u8::from(*force)]);
        }
        CanonicalProjectMutationAction::ReplaceResource {
            old_resource_id,
            new_resource,
        } => {
            put(&mut digest, b"replace-resource");
            put(&mut digest, old_resource_id.as_bytes());
            put_resource(&mut digest, new_resource);
        }
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
        CanonicalProjectMutationAction::EndAssignment {
            assignment_id,
            forced,
            runtime,
        } => {
            put(&mut digest, b"end-assignment");
            put(&mut digest, assignment_id.as_bytes());
            put(&mut digest, &[u8::from(*forced)]);
            put_runtime_observation(&mut digest, runtime.as_ref());
        }
        CanonicalProjectMutationAction::BlockAssignment {
            assignment_id,
            cause,
        } => {
            put(&mut digest, b"block-assignment");
            put(&mut digest, assignment_id.as_bytes());
            put(&mut digest, cause.as_str().as_bytes());
        }
        CanonicalProjectMutationAction::BeginClosing => put(&mut digest, b"begin-closing"),
        CanonicalProjectMutationAction::FinishClosing { forced, runtime } => {
            put(&mut digest, b"finish-closing");
            put(&mut digest, &[u8::from(*forced)]);
            put_runtime_observation(&mut digest, runtime.as_ref());
        }
        CanonicalProjectMutationAction::Archive => put(&mut digest, b"archive"),
        CanonicalProjectMutationAction::Unarchive => put(&mut digest, b"unarchive"),
        CanonicalProjectMutationAction::RetireAgent { agent_id } => {
            put(&mut digest, b"retire-agent");
            put(&mut digest, agent_id.as_bytes());
        }
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

fn put_resource(digest: &mut Sha256, resource: &ProjectResource) {
    put(digest, resource.resource_id.as_bytes());
    put_locator(digest, &resource.display_locator);
    put_locator(digest, &resource.canonical_locator);
    let health = match resource.health {
        ResourceHealth::Unknown => 1_u8,
        ResourceHealth::Healthy => 2,
        ResourceHealth::Degraded => 3,
        ResourceHealth::Unavailable => 4,
    };
    put(digest, &[health]);
}

fn put_optional_text(digest: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            put(digest, &[1]);
            put(digest, value.as_bytes());
        }
        None => put(digest, &[0]),
    }
}

fn put_runtime_observation(digest: &mut Sha256, observation: Option<&RuntimeObservation>) {
    match observation {
        None => put(digest, &[0]),
        Some(RuntimeObservation::Succeeded) => put(digest, &[1]),
        Some(RuntimeObservation::Failed(code)) => {
            put(digest, &[2]);
            put(digest, code.as_str().as_bytes());
        }
        Some(RuntimeObservation::Uncertain(code)) => {
            put(digest, &[3]);
            put(digest, code.as_str().as_bytes());
        }
    }
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
