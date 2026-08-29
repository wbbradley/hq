//! Transport-independent project command values and capability boundary.

use hq_domain::{
    AccountId, AgentId, CommandDigest, CommandId, ContentText, DomainError, FactId, InstallationId,
    MailboxId, OperationId, ProjectId, ProviderId, ProviderSessionId, ResourceId, ResourceLocator,
    RuntimeObservation, ShortText, ThreadId, Timestamp,
};

use crate::ApplicationError;

/// Requested project operation. These variants are data; workflow policy lives in the owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectCommandAction {
    /// Identify one existing resource and create an initially open project over it.
    Create(ProjectCreationRequest),
    /// Open a closed project and acquire its advisory resource claims.
    Open,
    /// Activate one agent assignment, optionally resuming exact durable history.
    Activate {
        /// Durable named agent selected by the human.
        agent_id: AgentId,
        /// Neutral provider namespace.
        provider: ProviderId,
        /// Exact provider session to resume, or `None` to start one.
        resume_session: Option<ProviderSessionId>,
        /// Existing project thread to resume, or `None` to use the first pending input.
        resume_thread: Option<ThreadId>,
        /// Exact normalized directory in which the runtime must launch.
        launch_directory: ResourceLocator,
    },
    /// Reconcile and drain undispatched accepted project inputs in sequence.
    DispatchPending,
    /// Close a project, optionally revoking HQ authority despite dirty or uncertain effects.
    Close {
        /// Whether explicit force policy is authorized.
        force: bool,
    },
    /// Change presentation archive state without changing resource claims.
    SetArchived {
        /// Desired archive state.
        archived: bool,
    },
    /// Quiesce the current assignment and activate another agent.
    Handoff {
        /// New durable named agent.
        agent_id: AgentId,
        /// Neutral provider namespace.
        provider: ProviderId,
        /// Exact provider session to resume, or `None` to start one.
        resume_session: Option<ProviderSessionId>,
        /// Existing project thread to resume.
        thread_id: ThreadId,
        /// Exact normalized launch directory.
        launch_directory: ResourceLocator,
        /// Whether takeover may revoke the old HQ assignment without observed runtime cessation.
        force_takeover: bool,
    },
    /// End any assignment for one agent and author its retirement.
    RetireAgent {
        /// Durable named agent being retired.
        agent_id: AgentId,
        /// Whether unknown runtime cessation may revoke HQ authority.
        force: bool,
    },
    /// Add one desired resource.
    AddResource {
        /// Stable desired resource identity allocated by the caller.
        resource_id: ResourceId,
        /// Normalized home-local display locator to identify authoritatively.
        resource: ResourceLocator,
        /// Whether this resource becomes the launch primary.
        make_primary: bool,
    },
    /// Remove one desired resource.
    RemoveResource {
        /// Stable resource identity.
        resource_id: ResourceId,
        /// Whether dirty or unknown release may proceed.
        force: bool,
    },
    /// Atomically replace one desired resource identity.
    ReplaceResource {
        /// Existing resource identity.
        old_resource_id: ResourceId,
        /// Stable replacement identity allocated by the caller.
        new_resource_id: ResourceId,
        /// Normalized home-local replacement display locator.
        resource: ResourceLocator,
    },
    /// Select one existing desired resource as the launch primary.
    SetPrimaryResource {
        /// Stable desired resource identity.
        resource_id: ResourceId,
    },
    /// Provision a Git worktree and create its project exactly once.
    ProvisionWorktree(WorktreeProvisioningRequest),
}

/// Exact declarative input for existing-resource project creation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectCreationRequest {
    /// Project mailbox allocated deterministically by the caller.
    pub mailbox_id: MailboxId,
    /// Human-visible project name.
    pub project_name: ShortText,
    /// Optional project brief.
    pub brief: Option<ContentText>,
    /// Stable resource identity allocated deterministically by the caller.
    pub resource_id: ResourceId,
    /// Normalized existing resource spelling to identify on the selected home.
    pub resource: ResourceLocator,
}

/// Exact declarative input for Git worktree provisioning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeProvisioningRequest {
    /// Project mailbox allocated by the caller.
    pub mailbox_id: MailboxId,
    /// Human-visible project name.
    pub project_name: ShortText,
    /// Optional project brief.
    pub brief: Option<ContentText>,
    /// Existing repository or worktree from which Git will create the worktree.
    pub source: ResourceLocator,
    /// Exact normalized destination reserved by this command.
    pub destination: ResourceLocator,
    /// Exact branch spelling requested from Git.
    pub branch: ShortText,
    /// Exact revision from which a new branch is created, absent for an existing branch.
    pub base: Option<ShortText>,
    /// Whether Git should create the branch from `base` rather than require it to exist.
    pub create_branch: bool,
}

/// Stable, exact project command envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectCommandRequest {
    /// Stable command identity used for canonical replay.
    pub command_id: CommandId,
    /// Stable external workflow identity.
    pub operation_id: OperationId,
    /// Digest of every exact request field.
    pub request_digest: CommandDigest,
    /// Active human account authorizing the operation.
    pub account_id: AccountId,
    /// Target or to-be-created project identity.
    pub project_id: ProjectId,
    /// Immutable authoritative installation.
    pub home: InstallationId,
    /// Expected canonical project head, absent only when creating a new project.
    pub expected_head: Option<FactId>,
    /// Caller-supplied semantic time.
    pub issued_at: Timestamp,
    /// Closed requested behavior.
    pub action: ProjectCommandAction,
}

/// Stable exact request for node-owned named-agent retirement coordination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentRetirementRequest {
    /// Stable command identity used by fact and project-saga replay.
    pub command_id: CommandId,
    /// Stable external workflow identity.
    pub operation_id: OperationId,
    /// Digest of every caller-controlled request field.
    pub request_digest: CommandDigest,
    /// Active human account authorizing retirement.
    pub account_id: AccountId,
    /// Durable named agent being retired.
    pub agent_id: AgentId,
    /// Exact active name claim selected by the caller.
    pub expected_claim: FactId,
    /// Installation that owns the agent and retirement workflow.
    pub home: InstallationId,
    /// Caller-supplied semantic time.
    pub issued_at: Timestamp,
    /// Whether failed or uncertain runtime cessation may revoke HQ authority.
    pub force: bool,
}

/// Typed result of node-owned idle or assigned named-agent retirement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentRetirementOutcome {
    /// Bounded execution made progress but has not reached a terminal state.
    Running {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Current durable project-workflow checkpoint.
        stage: ProjectCommandStage,
    },
    /// Retirement reached a stable canonical state.
    Completed {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Project whose assignment was quiesced, absent for an idle agent.
        project_id: Option<ProjectId>,
        /// Definite or uncertain runtime truth retained by a forced assigned retirement.
        runtime: Option<RuntimeObservation>,
    },
    /// Retirement was definitely rejected before an unknown unrecorded effect.
    Rejected {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Stable typed reason.
        error: DomainError,
        /// Definite or uncertain runtime truth observed before rejection.
        runtime: Option<RuntimeObservation>,
    },
    /// External or commit truth is unknown and exact replay remains safe.
    Reconcilable {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Durable checkpoint at which replay resumes.
        stage: ProjectCommandStage,
        /// Stable typed reason.
        error: DomainError,
    },
}

/// Durable externally visible project workflow checkpoint.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectCommandStage {
    /// The exact request is durable but has not crossed another boundary.
    Accepted,
    /// A non-home request is waiting for durable routing to its home.
    AwaitingHome,
    /// The home has durably received a remote request.
    ReceivedAtHome,
    /// Desired resources and claim conflicts are being checked.
    ValidatingResources,
    /// A closed project is being opened.
    Opening,
    /// A canonical configuring assignment is being authored.
    ConfiguringAssignment,
    /// A provider runtime is being started or resumed.
    StartingRuntime,
    /// The exact launch directory is being re-resolved.
    ValidatingLaunchDirectory,
    /// The assignment is being made canonically runnable.
    MakingRunnable,
    /// Accepted pending input is being reconciled and dispatched.
    DispatchingInputs,
    /// Resource cleanliness and runtime state are being assessed.
    AssessingRelease,
    /// A runtime is being asked to quiesce.
    QuiescingRuntime,
    /// Canonical assignment authority is being ended.
    EndingAssignment,
    /// The project lifecycle is being closed.
    Closing,
    /// A direct canonical project update is being committed.
    UpdatingProject,
    /// A normalized worktree destination is being reserved.
    ReservingDestination,
    /// Existing Git state is being reconciled before mutation.
    ReconcilingGit,
    /// Git worktree creation may be in progress.
    CreatingWorktree,
    /// The created path is being identified as a stable resource.
    IdentifyingResource,
    /// The provisioned project fact is being committed.
    CreatingProject,
    /// Observed partial work is being compensated.
    Compensating,
    /// An effect outcome is unknown and lookup is required before retry.
    ReconciliationRequired,
    /// The workflow has reached a terminal stable result.
    Complete,
}

/// Typed command submission or progress result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectCommandOutcome {
    /// The exact request is durable and scheduled.
    Accepted {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Current durable checkpoint.
        stage: ProjectCommandStage,
    },
    /// Bounded execution made progress but is not terminal.
    Running {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Current durable checkpoint.
        stage: ProjectCommandStage,
    },
    /// The command reached a canonical stable state.
    Completed {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Resulting canonical project head.
        project_head: FactId,
        /// Definite or uncertain runtime truth observed while reaching the result.
        runtime: Option<RuntimeObservation>,
    },
    /// The command was definitely rejected without an unknown external effect.
    Rejected {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Stable typed reason.
        error: DomainError,
        /// Definite or uncertain runtime truth observed before rejection.
        runtime: Option<RuntimeObservation>,
        /// External state deliberately retained for operator inspection.
        external_state_warning: Option<hq_domain::ProjectExternalStateWarning>,
    },
    /// External truth is unknown and the exact operation remains recoverable.
    Reconcilable {
        /// Stable workflow identity.
        operation_id: OperationId,
        /// Durable checkpoint at which reconciliation must resume.
        stage: ProjectCommandStage,
        /// Stable typed reason for surfacing the unknown outcome.
        error: DomainError,
        /// External state that may exist while reconciliation remains pending.
        external_state_warning: Option<hq_domain::ProjectExternalStateWarning>,
    },
}

/// Home-authoritative project workflow capability.
pub trait ControlProjects {
    /// Accepts, executes a bounded amount of, or reconciles one exact project command.
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError>;
}

/// Node-owned coordinator for safe named-agent retirement.
pub trait RetireAgents {
    /// Executes or reconciles one exact idle or assigned retirement request.
    fn retire_agent(
        &self,
        request: AgentRetirementRequest,
    ) -> Result<AgentRetirementOutcome, ApplicationError>;
}
