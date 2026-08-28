use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{
    AgentId, AssignmentBinding, AssignmentIntent, AuthorityRole, CommandDigest, CommandId,
    ContentText, DispatchId, ErrorCode, Fact, FactId, FactScope, InitialProjectState,
    InstallationId, MailboxAddress, MailboxId, MessageContent, MessageId, ProjectId,
    ProjectResource, RemoteCommandResult, ResourceId, ResourceLocator, ResourceScheme,
    RuntimeObservation, SemanticPayload, ShortText, ThreadId,
};

use crate::{
    AuthorityPolicy, AuthorityReason, ConflictObservation, ConflictReason, DecisionStatus,
    DomainDecision, DomainReducer, ProjectionContribution, ReductionContext,
};

/// Stable authoritative project lifecycle. Operational saga checkpoints remain separate facts.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectLifecycle {
    /// Resources may be claimed and work may be assigned.
    Open,
    /// New dispatch is stopped while claims and any assignment are retained.
    Closing,
    /// No claims or current assignment remain.
    Closed,
}

/// Current assignment phase retained by the pure project model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectAssignmentPhase {
    /// The assignment owns the project but is not runnable yet.
    Configuring,
    /// The exact provider session and project thread are runnable.
    Runnable {
        /// Immutable project-scoped thread.
        thread_id: ThreadId,
        /// Selected launch directory acknowledged by the home.
        launch_directory: ResourceLocator,
    },
    /// The assignment remains owned but cannot run until explicitly resolved.
    Blocked(ErrorCode),
}

/// One current assignment epoch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectAssignmentView {
    /// Exact immutable intent recorded before runtime startup.
    pub intent: AssignmentIntent,
    /// Acknowledged runtime binding, present only after startup reached runnable.
    pub binding: Option<AssignmentBinding>,
    /// Current phase.
    pub phase: ProjectAssignmentPhase,
    /// Whether global project/agent cardinality is singular.
    pub cardinality_conflicted: bool,
    /// Runnable only when phase, project, claims, and cardinality all permit it.
    pub runnable: bool,
    /// Facts supporting the epoch through its current phase.
    pub support: BTreeSet<FactId>,
}

/// Rebuildable authoritative project view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectView {
    /// Unique creation fact.
    pub root: FactId,
    /// Last unique admitted home-linear fact.
    pub head: FactId,
    /// Every unusable sibling immediately beyond the retained common head.
    pub fork_participants: BTreeSet<FactId>,
    /// Immutable project home.
    pub home: InstallationId,
    /// Immutable project mailbox.
    pub mailbox: MailboxAddress,
    /// Optional immutable predecessor.
    pub predecessor: Option<ProjectId>,
    /// Mutable bounded display name.
    pub name: ShortText,
    /// Mutable bounded brief.
    pub brief: Option<ContentText>,
    /// Durable desired resources, including latest typed health.
    pub resources: BTreeMap<ResourceId, ProjectResource>,
    /// Explicit primary desired resource.
    pub primary: Option<ResourceId>,
    /// Stable lifecycle.
    pub lifecycle: ProjectLifecycle,
    /// Presentation archive state; archived projects remain permanent and queryable.
    pub archived: bool,
    /// Active conflict-free resource IDs. No filesystem ownership is implied.
    pub active_claims: BTreeSet<ResourceId>,
    /// Conflicting projects per desired resource.
    pub claim_conflicts: BTreeMap<ResourceId, BTreeSet<ProjectId>>,
    /// True only when every desired claim is globally singular.
    pub claimable: bool,
    /// Current assignment, if any.
    pub assignment: Option<ProjectAssignmentView>,
    /// Last accepted contiguous input sequence, or zero before the first input.
    pub input_sequence: u64,
}

/// Immutable attribution of one accepted project input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectInputView {
    /// Project receiving the input.
    pub project_id: ProjectId,
    /// Stable public message identity.
    pub message_id: MessageId,
    /// Canonical message fact.
    pub input_fact_id: FactId,
    /// Home-assigned contiguous sequence.
    pub sequence: u64,
    /// Acceptance fact.
    pub accepted_fact: FactId,
}

/// Immutable at-most-once dispatch attribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectDispatchView {
    /// Stable dispatch identity.
    pub dispatch_id: DispatchId,
    /// Project input identity.
    pub message_id: MessageId,
    /// Home sequence.
    pub sequence: u64,
    /// Assignment/provider binding at dispatch.
    pub binding: AssignmentBinding,
    /// Immutable project thread.
    pub thread_id: ThreadId,
    /// Dispatch fact.
    pub fact_id: FactId,
    /// Changed duplicate dispatches make the identity unusable.
    pub conflicted: bool,
}

/// Whether retained output belongs to the current assignment or historical inactive work.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectOutputStatus {
    /// The captured assignment is still current.
    Current,
    /// The captured assignment has ended or been replaced.
    LateFromInactive,
    /// Same stable output identity carried unequal content or provenance.
    Conflicted,
}

/// One stable project output identity and complete provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectOutputView {
    /// Project output identity.
    pub output_id: MessageId,
    /// Originating dispatch.
    pub dispatch_id: DispatchId,
    /// Captured assignment/provider binding.
    pub binding: AssignmentBinding,
    /// Captured immutable thread.
    pub thread_id: ThreadId,
    /// Typed message content.
    pub message: MessageContent,
    /// Current, late, or collided classification.
    pub status: ProjectOutputStatus,
    /// Every identical or conflicting record fact.
    pub facts: BTreeSet<FactId>,
}

/// Normalized remote-control progress without project mutation authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteCommandStage {
    /// An active human requested the command; the home may still be offline.
    Queued,
    /// The immutable home acknowledged receipt at an observed head.
    Received {
        /// Canonical project head observed by the home.
        received_head: FactId,
    },
    /// The home reported a terminal canonical or rejected outcome.
    Terminal {
        /// Definite canonical commit or typed rejection.
        result: RemoteCommandResult,
        /// Runtime observation, including explicit uncertainty.
        runtime: Option<RuntimeObservation>,
    },
    /// Unequal request, receipt, or terminal values exist.
    Conflicted,
}

/// One stable remote command view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteCommandView {
    /// Exact request digest.
    pub digest: CommandDigest,
    /// Target project.
    pub project_id: ProjectId,
    /// Expected canonical project head.
    pub expected_head: FactId,
    /// Current control-plane stage.
    pub stage: RemoteCommandStage,
    /// Complete control record support.
    pub support: BTreeSet<FactId>,
}

/// Typed aggregate identities for project reduction.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectAggregateKey {
    /// Immutable project identity and home-linear history.
    Project(ProjectId),
    /// Home-qualified resource claim namespace.
    Resource {
        /// Immutable home namespace.
        home: InstallationId,
        /// Canonical semantic locator.
        locator: ResourceLocator,
    },
    /// Global assignment cardinality for one agent.
    AgentAssignment {
        /// Immutable project/agent home namespace.
        home: InstallationId,
        /// Durable agent identity within the home.
        agent: AgentId,
    },
    /// Stable input public identity.
    Input(MessageId),
    /// Stable dispatch identity.
    Dispatch(DispatchId),
    /// Stable output identity.
    Output(MessageId),
    /// Stable remote command identity.
    Command(CommandId),
}

/// Public projection keys for projects and related immutable histories.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectProjectionKey {
    /// Authoritative project state.
    Project(ProjectId),
    /// Accepted project input.
    Input(MessageId),
    /// Project dispatch.
    Dispatch(DispatchId),
    /// Project output.
    Output(MessageId),
    /// Remote command progress.
    Command(CommandId),
}

/// Public project projection values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectProjection {
    /// Authoritative project state.
    Project(Box<ProjectView>),
    /// Accepted input.
    Input(Box<ProjectInputView>),
    /// Dispatch attribution.
    Dispatch(Box<ProjectDispatchView>),
    /// Retained output.
    Output(Box<ProjectOutputView>),
    /// Remote-control visibility.
    Command(Box<RemoteCommandView>),
}

/// Closed project validation and conflict reasons.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectReason {
    /// Historical authority policy rejected the fact.
    Authority(AuthorityReason),
    /// Scope, home signer, project, mailbox, or embedded subject disagreed.
    SubjectMismatch,
    /// A required typed project authority was absent or of the wrong family.
    ProjectAuthorityMismatch,
    /// Unequal roots claim one project or home-qualified project mailbox.
    ProjectIdentityConflict,
    /// Sibling home-linear children cite one previous head.
    HomeLinearFork,
    /// Expected previous head is not the exact admitted project head.
    StaleHead,
    /// The typed lifecycle/resource/assignment transition is not admitted from this state.
    InvalidTransition,
    /// Desired resource identity or primary selection is inconsistent.
    ResourceInvariant,
    /// Cross-project home-qualified resources overlap.
    ResourceClaimConflict,
    /// One project or agent participates in multiple current assignment epochs.
    AssignmentCardinalityConflict,
    /// Project thread, assignment, provider session, or launch subject disagrees.
    AssignmentBindingMismatch,
    /// Input sequence, message identity, or project addressing is inconsistent.
    InputSequenceConflict,
    /// Dispatch identity or attribution is duplicated incompatibly.
    DispatchConflict,
    /// Output identity or immutable provenance is duplicated incompatibly.
    OutputConflict,
    /// Remote command identity, receipt, result, or canonical head is inconsistent.
    RemoteCommandConflict,
}

/// Pure policy for semantic resource overlap. It performs no adapter or filesystem I/O.
pub trait ResourceConflictPolicy: Clone + std::fmt::Debug + Eq {
    /// Reports whether two canonical resource locators overlap in one home namespace.
    fn conflicts(&self, left: &ResourceLocator, right: &ResourceLocator) -> bool;
}

/// First-release path-resource overlap policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PathResourcePolicy;

impl ResourceConflictPolicy for PathResourcePolicy {
    fn conflicts(&self, left: &ResourceLocator, right: &ResourceLocator) -> bool {
        if left.scheme() != ResourceScheme::WorkingTree
            || right.scheme() != ResourceScheme::WorkingTree
        {
            return left == right;
        }
        path_contains(left.value(), right.value()) || path_contains(right.value(), left.value())
    }
}

fn path_contains(parent: &str, candidate: &str) -> bool {
    let parent = parent.trim_end_matches('/');
    let candidate = candidate.trim_end_matches('/');
    parent == candidate
        || candidate
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Pure complete-batch project policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectReducer<P = PathResourcePolicy> {
    authority: AuthorityPolicy,
    resources: P,
}

/// Complete normalized first-release project report.
pub type ProjectReport = crate::DomainReductionReport<ProjectReducer<PathResourcePolicy>>;

impl ProjectReducer<PathResourcePolicy> {
    /// Creates the first-release project reducer with path-resource semantics.
    pub const fn new(authority: AuthorityPolicy) -> Self {
        Self {
            authority,
            resources: PathResourcePolicy,
        }
    }
}

impl<P> ProjectReducer<P> {
    /// Creates a reducer with an explicit pure resource conflict policy.
    pub const fn with_resource_policy(authority: AuthorityPolicy, resources: P) -> Self {
        Self {
            authority,
            resources,
        }
    }
}

impl<P: ResourceConflictPolicy> DomainReducer for ProjectReducer<P> {
    type AggregateKey = ProjectAggregateKey;
    type ProjectionKey = ProjectProjectionKey;
    type ProjectionValue = ProjectProjection;
    type Reason = ProjectReason;

    fn classify(
        &self,
        fact: &Fact,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        let authority = crate::authority::classify_fact(self.authority, fact, context);
        if !matches!(authority, DomainDecision::Projected) {
            return map_authority_decision(authority);
        }
        match classify_project_fact(fact, context, &self.resources) {
            Ok(()) => DomainDecision::Projected,
            Err((
                reason @ (ProjectReason::ProjectIdentityConflict
                | ProjectReason::HomeLinearFork
                | ProjectReason::ResourceClaimConflict
                | ProjectReason::AssignmentCardinalityConflict
                | ProjectReason::DispatchConflict
                | ProjectReason::OutputConflict
                | ProjectReason::RemoteCommandConflict),
                participants,
            )) => DomainDecision::Conflicted {
                reason,
                participants,
            },
            Err((reason, _)) => DomainDecision::Invalid { reason },
        }
    }

    fn aggregate_keys(
        &self,
        fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<Self::AggregateKey> {
        aggregate_keys(fact)
    }

    fn projections(
        &self,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ProjectionContribution<Self::ProjectionKey, Self::ProjectionValue>> {
        derive_projections(context, &self.resources)
    }

    fn conflicts(
        &self,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ConflictObservation<Self::Reason>> {
        derive_conflicts(context, &self.resources)
    }
}

fn map_authority_decision(
    decision: DomainDecision<AuthorityReason>,
) -> DomainDecision<ProjectReason> {
    match decision {
        DomainDecision::Projected => DomainDecision::Projected,
        DomainDecision::Unauthorized {
            reason,
            failed_authorities,
        } => DomainDecision::Unauthorized {
            reason: ProjectReason::Authority(reason),
            failed_authorities,
        },
        DomainDecision::Conflicted {
            reason,
            participants,
        } => DomainDecision::Conflicted {
            reason: ProjectReason::Authority(reason),
            participants,
        },
        DomainDecision::Invalid { reason } => DomainDecision::Invalid {
            reason: ProjectReason::Authority(reason),
        },
        DomainDecision::Unsupported { reason } => DomainDecision::Unsupported {
            reason: ProjectReason::Authority(reason),
        },
    }
}

type ClassificationError = (ProjectReason, BTreeSet<FactId>);

fn invalid(reason: ProjectReason) -> ClassificationError {
    (reason, BTreeSet::new())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InternalAssignment {
    intent: AssignmentIntent,
    binding: Option<AssignmentBinding>,
    phase: ProjectAssignmentPhase,
    support: BTreeSet<FactId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InternalProjectState {
    project_id: ProjectId,
    root: FactId,
    head: FactId,
    home: InstallationId,
    mailbox_id: MailboxId,
    predecessor: Option<ProjectId>,
    name: ShortText,
    brief: Option<ContentText>,
    resources: BTreeMap<ResourceId, ProjectResource>,
    primary: Option<ResourceId>,
    lifecycle: ProjectLifecycle,
    archived: bool,
    assignment: Option<InternalAssignment>,
    input_sequence: u64,
}

fn classify_project_fact<P: ResourceConflictPolicy>(
    fact: &Fact,
    context: &ReductionContext<'_, ProjectReason>,
    resources: &P,
) -> Result<(), ClassificationError> {
    match fact.payload() {
        SemanticPayload::ProjectCreated { .. } => validate_creation(fact, context),
        payload if project_id(payload).is_some() && !is_remote(payload) => {
            validate_home_transition(fact, context, resources)
        }
        SemanticPayload::RemoteProjectCommandRequested { .. }
        | SemanticPayload::RemoteProjectCommandReceipt { .. }
        | SemanticPayload::RemoteProjectCommandOutcome { .. } => {
            validate_remote_control(fact, context)
        }
        _ => Ok(()),
    }
}

fn validate_creation(
    fact: &Fact,
    context: &ReductionContext<'_, ProjectReason>,
) -> Result<(), ClassificationError> {
    let SemanticPayload::ProjectCreated {
        project_id,
        mailbox_id,
        home,
        name,
        predecessor,
        resources,
        primary,
        ..
    } = fact.payload()
    else {
        unreachable!()
    };
    if fact.author().installation_id() != *home
        || !matches!(fact.scope(), FactScope::AccountAddressed(_))
        || name.as_str().is_empty()
        || !valid_project_home(fact, *home, context)
        || !valid_active_human(fact, context)
    {
        return Err(invalid(ProjectReason::ProjectAuthorityMismatch));
    }
    let resource_ids = resources
        .as_slice()
        .iter()
        .map(|resource| resource.resource_id)
        .collect::<BTreeSet<_>>();
    if resource_ids.len() != resources.as_slice().len()
        || resources
            .as_slice()
            .iter()
            .any(|resource| !valid_path_resource(resource))
        || primary.is_some_and(|id| !resource_ids.contains(&id))
    {
        return Err(invalid(ProjectReason::ResourceInvariant));
    }
    if let Some(predecessor) = predecessor {
        let valid = fact.causal().parents().iter().any(|parent| context.facts().get(*parent).is_some_and(|candidate| {
            context.is_projected(candidate.id()) && matches!(candidate.payload(), SemanticPayload::ProjectCreated { project_id, .. } if project_id == predecessor)
        }));
        if !valid {
            return Err(invalid(ProjectReason::SubjectMismatch));
        }
    }
    let participants = context.facts().facts().filter(|candidate| {
        matches!(candidate.payload(), SemanticPayload::ProjectCreated { project_id: candidate_id, mailbox_id: candidate_mailbox, home: candidate_home, .. }
            if candidate_id == project_id || (*candidate_home == *home && *candidate_mailbox == *mailbox_id))
    }).map(Fact::id).collect::<BTreeSet<_>>();
    if participants.len() > 1 {
        return Err((ProjectReason::ProjectIdentityConflict, participants));
    }
    Ok(())
}

fn validate_home_transition<P: ResourceConflictPolicy>(
    fact: &Fact,
    context: &ReductionContext<'_, ProjectReason>,
    policy: &P,
) -> Result<(), ClassificationError> {
    let project_id =
        project_id(fact.payload()).ok_or_else(|| invalid(ProjectReason::SubjectMismatch))?;
    let previous_id = fact
        .causal()
        .authority(AuthorityRole::PreviousState)
        .ok_or_else(|| invalid(ProjectReason::ProjectAuthorityMismatch))?;
    if !fact.causal().parents().contains(&previous_id) || !context.is_projected(previous_id) {
        return Err(invalid(ProjectReason::StaleHead));
    }
    let mut state = state_at(previous_id, context)
        .filter(|state| state.project_id == project_id)
        .ok_or_else(|| invalid(ProjectReason::StaleHead))?;
    if fact.author().installation_id() != state.home
        || !valid_project_home(fact, state.home, context)
    {
        return Err(invalid(ProjectReason::ProjectAuthorityMismatch));
    }
    if let Some((reason, participants)) = stable_identity_conflict(fact, context) {
        return Err((reason, participants));
    }
    let participants = sibling_participants(fact, project_id, previous_id, context);
    if participants.len() > 1 {
        return Err((ProjectReason::HomeLinearFork, participants));
    }
    apply_payload(&mut state, fact, context)?;
    if matches!(fact.payload(), SemanticPayload::ProjectOpened { .. }) {
        let conflicts = claim_conflict_participants(&state, fact.id(), context, policy);
        if !conflicts.is_empty() {
            return Err((ProjectReason::ResourceClaimConflict, conflicts));
        }
    }
    Ok(())
}

fn stable_identity_conflict(
    fact: &Fact,
    context: &ReductionContext<'_, ProjectReason>,
) -> Option<(ProjectReason, BTreeSet<FactId>)> {
    let (reason, identity) = match fact.payload() {
        SemanticPayload::ProjectInputAccepted {
            project_id,
            message_id,
            input_fact_id,
            sequence,
        } => (
            ProjectReason::InputSequenceConflict,
            StableIdentity::Input {
                project_id: *project_id,
                message_id: *message_id,
                input_fact_id: *input_fact_id,
                sequence: sequence.get(),
            },
        ),
        SemanticPayload::ProjectInputDispatched {
            project_id,
            message_id,
            sequence,
            dispatch_id,
            ..
        } => (
            ProjectReason::DispatchConflict,
            StableIdentity::Dispatch {
                project_id: *project_id,
                message_id: *message_id,
                sequence: sequence.get(),
                dispatch_id: *dispatch_id,
            },
        ),
        SemanticPayload::ProjectOutputRecorded { output_id, .. } => (
            ProjectReason::OutputConflict,
            StableIdentity::Output(*output_id),
        ),
        _ => return None,
    };
    let participants = context
        .facts()
        .facts()
        .filter(|candidate| identity.matches(candidate) && candidate.payload() != fact.payload())
        .map(Fact::id)
        .chain([fact.id()])
        .collect::<BTreeSet<_>>();
    (participants.len() > 1).then_some((reason, participants))
}

enum StableIdentity {
    Input {
        project_id: ProjectId,
        message_id: MessageId,
        input_fact_id: FactId,
        sequence: u64,
    },
    Dispatch {
        project_id: ProjectId,
        message_id: MessageId,
        sequence: u64,
        dispatch_id: DispatchId,
    },
    Output(MessageId),
}

impl StableIdentity {
    fn matches(&self, candidate: &Fact) -> bool {
        match (self, candidate.payload()) {
            (
                Self::Input {
                    project_id,
                    message_id,
                    input_fact_id,
                    sequence,
                },
                SemanticPayload::ProjectInputAccepted {
                    project_id: candidate_project,
                    message_id: candidate_message,
                    input_fact_id: candidate_input,
                    sequence: candidate_sequence,
                },
            ) => {
                candidate_project == project_id
                    && (candidate_message == message_id
                        || candidate_input == input_fact_id
                        || candidate_sequence.get() == *sequence)
            }
            (
                Self::Dispatch {
                    project_id,
                    message_id,
                    sequence,
                    dispatch_id,
                },
                SemanticPayload::ProjectInputDispatched {
                    project_id: candidate_project,
                    message_id: candidate_message,
                    sequence: candidate_sequence,
                    dispatch_id: candidate_dispatch,
                    ..
                },
            ) => {
                candidate_project == project_id
                    && (candidate_dispatch == dispatch_id
                        || candidate_message == message_id
                        || candidate_sequence.get() == *sequence)
            }
            (
                Self::Output(output_id),
                SemanticPayload::ProjectOutputRecorded {
                    output_id: candidate,
                    ..
                },
            ) => candidate == output_id,
            _ => false,
        }
    }
}

fn valid_project_home(
    fact: &Fact,
    home: InstallationId,
    context: &ReductionContext<'_, ProjectReason>,
) -> bool {
    fact.causal().authority(AuthorityRole::ProjectHome).and_then(|id| context.facts().get(id)).is_some_and(|root| {
        context.is_projected(root.id()) && root.author() == fact.author()
            && matches!(root.payload(), SemanticPayload::InstallationDeclared { installation_id, .. } if *installation_id == home)
    })
}

fn valid_active_human(fact: &Fact, context: &ReductionContext<'_, ProjectReason>) -> bool {
    let Some(active) = fact.causal().authority(AuthorityRole::ActiveHuman) else {
        return false;
    };
    fact.causal().authority(AuthorityRole::AccountMembership) == Some(active)
        && context.is_projected(active)
}

fn require_active_human(
    fact: &Fact,
    context: &ReductionContext<'_, ProjectReason>,
) -> Result<(), ClassificationError> {
    valid_active_human(fact, context)
        .then_some(())
        .ok_or_else(|| invalid(ProjectReason::ProjectAuthorityMismatch))
}

fn sibling_participants(
    fact: &Fact,
    project_id: ProjectId,
    previous: FactId,
    context: &ReductionContext<'_, ProjectReason>,
) -> BTreeSet<FactId> {
    context
        .facts()
        .facts()
        .filter(|candidate| {
            project_id_of(candidate) == Some(project_id)
                && candidate.causal().authority(AuthorityRole::PreviousState) == Some(previous)
                && candidate.id() != fact.id()
                && !matches!(
                    context
                        .decisions()
                        .get(&candidate.id())
                        .map(crate::FactDecision::status),
                    Some(
                        DecisionStatus::Invalid
                            | DecisionStatus::Unauthorized
                            | DecisionStatus::Unsupported
                            | DecisionStatus::Unresolved
                    )
                )
        })
        .map(Fact::id)
        .chain([fact.id()])
        .collect()
}

fn state_at(
    fact_id: FactId,
    context: &ReductionContext<'_, ProjectReason>,
) -> Option<InternalProjectState> {
    let fact = context.facts().get(fact_id)?;
    if !context.is_projected(fact_id) {
        return None;
    }
    match fact.payload() {
        SemanticPayload::ProjectCreated {
            project_id,
            mailbox_id,
            home,
            name,
            brief,
            predecessor,
            resources,
            primary,
            initial_state,
        } => Some(InternalProjectState {
            project_id: *project_id,
            root: fact.id(),
            head: fact.id(),
            home: *home,
            mailbox_id: *mailbox_id,
            predecessor: *predecessor,
            name: name.clone(),
            brief: brief.clone(),
            resources: resources
                .as_slice()
                .iter()
                .cloned()
                .map(|resource| (resource.resource_id, resource))
                .collect(),
            primary: *primary,
            lifecycle: match initial_state {
                InitialProjectState::Open => ProjectLifecycle::Open,
                InitialProjectState::Closed => ProjectLifecycle::Closed,
            },
            archived: false,
            assignment: None,
            input_sequence: 0,
        }),
        payload if project_id(payload).is_some() && !is_remote(payload) => {
            let previous = fact.causal().authority(AuthorityRole::PreviousState)?;
            let mut state = state_at(previous, context)?;
            apply_payload(&mut state, fact, context).ok()?;
            Some(state)
        }
        _ => None,
    }
}

#[allow(clippy::too_many_lines)]
fn apply_payload(
    state: &mut InternalProjectState,
    fact: &Fact,
    context: &ReductionContext<'_, ProjectReason>,
) -> Result<(), ClassificationError> {
    match fact.payload() {
        SemanticPayload::ProjectOpened { .. } => {
            require_active_human(fact, context)?;
            if state.lifecycle != ProjectLifecycle::Closed || state.archived {
                return Err(invalid(ProjectReason::InvalidTransition));
            }
            state.lifecycle = ProjectLifecycle::Open;
        }
        SemanticPayload::ProjectClosingStarted { .. } => {
            require_active_human(fact, context)?;
            if state.lifecycle != ProjectLifecycle::Open {
                return Err(invalid(ProjectReason::InvalidTransition));
            }
            state.lifecycle = ProjectLifecycle::Closing;
        }
        SemanticPayload::ProjectClosed { .. } => {
            require_active_human(fact, context)?;
            if state.lifecycle != ProjectLifecycle::Closing || state.assignment.is_some() {
                return Err(invalid(ProjectReason::InvalidTransition));
            }
            state.lifecycle = ProjectLifecycle::Closed;
        }
        SemanticPayload::ProjectArchived { .. } => {
            require_active_human(fact, context)?;
            if state.lifecycle != ProjectLifecycle::Closed
                || state.archived
                || state.assignment.is_some()
            {
                return Err(invalid(ProjectReason::InvalidTransition));
            }
            state.archived = true;
        }
        SemanticPayload::ProjectUnarchived { .. } => {
            require_active_human(fact, context)?;
            if state.lifecycle != ProjectLifecycle::Closed || !state.archived {
                return Err(invalid(ProjectReason::InvalidTransition));
            }
            state.archived = false;
        }
        SemanticPayload::ProjectMetadataUpdated { name, brief, .. } => {
            require_active_human(fact, context)?;
            if name.as_str().is_empty() {
                return Err(invalid(ProjectReason::InvalidTransition));
            }
            state.name = name.clone();
            state.brief.clone_from(brief);
        }
        SemanticPayload::ProjectResourceAdded {
            resource,
            make_primary,
            ..
        } => {
            require_active_human(fact, context)?;
            if !valid_path_resource(resource) || state.resources.contains_key(&resource.resource_id)
            {
                return Err(invalid(ProjectReason::ResourceInvariant));
            }
            state
                .resources
                .insert(resource.resource_id, resource.clone());
            if *make_primary {
                state.primary = Some(resource.resource_id);
            }
        }
        SemanticPayload::ProjectResourceRemoved {
            resource_id, force, ..
        } => {
            require_active_human(fact, context)?;
            if !state.resources.contains_key(resource_id) || (state.assignment.is_some() && !force)
            {
                return Err(invalid(ProjectReason::ResourceInvariant));
            }
            state.resources.remove(resource_id);
            if state.primary == Some(*resource_id) {
                state.primary = state.resources.keys().next().copied();
            }
        }
        SemanticPayload::ProjectResourceReplaced {
            old_resource_id,
            new_resource,
            ..
        } => {
            require_active_human(fact, context)?;
            if !valid_path_resource(new_resource)
                || !state.resources.contains_key(old_resource_id)
                || (*old_resource_id != new_resource.resource_id
                    && state.resources.contains_key(&new_resource.resource_id))
            {
                return Err(invalid(ProjectReason::ResourceInvariant));
            }
            state.resources.remove(old_resource_id);
            state
                .resources
                .insert(new_resource.resource_id, new_resource.clone());
            if state.primary == Some(*old_resource_id) {
                state.primary = Some(new_resource.resource_id);
            }
        }
        SemanticPayload::ProjectPrimaryResourceChanged { resource_id, .. } => {
            require_active_human(fact, context)?;
            if !state.resources.contains_key(resource_id) {
                return Err(invalid(ProjectReason::ResourceInvariant));
            }
            state.primary = Some(*resource_id);
        }
        SemanticPayload::ProjectResourceHealthObserved {
            resource_id,
            health,
            ..
        } => {
            let Some(resource) = state.resources.get_mut(resource_id) else {
                return Err(invalid(ProjectReason::ResourceInvariant));
            };
            resource.health = *health;
        }
        SemanticPayload::ProjectAssignmentConfiguring { intent, .. } => {
            require_active_human(fact, context)?;
            if state.lifecycle != ProjectLifecycle::Open || state.assignment.is_some() {
                return Err(invalid(ProjectReason::InvalidTransition));
            }
            if !valid_agent_parent(fact, intent.agent_id, state.home, context) {
                return Err(invalid(ProjectReason::AssignmentBindingMismatch));
            }
            state.assignment = Some(InternalAssignment {
                intent: intent.clone(),
                binding: None,
                phase: ProjectAssignmentPhase::Configuring,
                support: BTreeSet::from([fact.id()]),
            });
        }
        SemanticPayload::ProjectAssignmentRunnable {
            binding,
            thread_id,
            launch_directory,
            ..
        } => {
            let valid_thread = valid_project_thread(
                fact,
                *thread_id,
                state.project_id,
                state.home,
                state.mailbox_id,
                context,
            );
            let Some(assignment) = state.assignment.as_mut() else {
                return Err(invalid(ProjectReason::AssignmentBindingMismatch));
            };
            if assignment.intent.assignment_id != binding.assignment_id
                || assignment.intent.agent_id != binding.agent_id
                || assignment.intent.provider != binding.provider
                || !matches!(
                    assignment.phase,
                    ProjectAssignmentPhase::Configuring | ProjectAssignmentPhase::Blocked(_)
                )
                || !valid_agent_parent(fact, binding.agent_id, state.home, context)
                || !valid_thread
            {
                return Err(invalid(ProjectReason::AssignmentBindingMismatch));
            }
            assignment.binding = Some(binding.clone());
            assignment.phase = ProjectAssignmentPhase::Runnable {
                thread_id: *thread_id,
                launch_directory: launch_directory.clone(),
            };
            assignment.support.insert(fact.id());
        }
        SemanticPayload::ProjectAssignmentBlocked {
            assignment_id,
            cause,
            ..
        } => {
            let Some(assignment) = state.assignment.as_mut() else {
                return Err(invalid(ProjectReason::AssignmentBindingMismatch));
            };
            if assignment.intent.assignment_id != *assignment_id {
                return Err(invalid(ProjectReason::AssignmentBindingMismatch));
            }
            assignment.phase = ProjectAssignmentPhase::Blocked(cause.clone());
            assignment.support.insert(fact.id());
        }
        SemanticPayload::ProjectAssignmentEnded {
            assignment_id,
            forced,
            ..
        } => {
            if *forced {
                require_active_human(fact, context)?;
            }
            if state
                .assignment
                .as_ref()
                .is_none_or(|assignment| assignment.intent.assignment_id != *assignment_id)
            {
                return Err(invalid(ProjectReason::AssignmentBindingMismatch));
            }
            state.assignment = None;
        }
        SemanticPayload::ProjectInputAccepted {
            message_id,
            input_fact_id,
            sequence,
            ..
        } => {
            if sequence.get() != state.input_sequence + 1
                || !valid_project_input(
                    fact,
                    *input_fact_id,
                    *message_id,
                    state.project_id,
                    state.home,
                    state.mailbox_id,
                    context,
                )
            {
                return Err(invalid(ProjectReason::InputSequenceConflict));
            }
            state.input_sequence = sequence.get();
        }
        SemanticPayload::ProjectInputDispatched {
            message_id,
            sequence,
            binding,
            thread_id,
            ..
        } => {
            let Some(assignment) = state.assignment.as_ref() else {
                return Err(invalid(ProjectReason::AssignmentBindingMismatch));
            };
            if state.lifecycle != ProjectLifecycle::Open
                || assignment.binding.as_ref() != Some(binding)
                || !matches!(assignment.phase, ProjectAssignmentPhase::Runnable { thread_id: active, .. } if active == *thread_id)
                || !valid_acceptance_parent(fact, *message_id, sequence.get(), context)
            {
                return Err(invalid(ProjectReason::DispatchConflict));
            }
        }
        SemanticPayload::ProjectOutputRecorded {
            dispatch_id,
            binding,
            thread_id,
            message,
            ..
        } if !valid_dispatch_parent(fact, *dispatch_id, binding, *thread_id, context)
            || message.recipient != Some(MailboxAddress::new(state.home, state.mailbox_id))
            || unique_agent_mailbox(binding.agent_id, state.home, context)
                != Some(message.sender) =>
        {
            return Err(invalid(ProjectReason::OutputConflict));
        }
        _ => {}
    }
    state.head = fact.id();
    Ok(())
}

fn valid_path_resource(resource: &ProjectResource) -> bool {
    resource.display_locator.scheme() == ResourceScheme::WorkingTree
        && resource.canonical_locator.scheme() == ResourceScheme::WorkingTree
        && valid_absolute_path(resource.display_locator.value())
        && valid_absolute_path(resource.canonical_locator.value())
}

fn valid_absolute_path(value: &str) -> bool {
    value.starts_with('/')
        && !value.contains('\0')
        && (value == "/" || (!value.ends_with('/') && !value.contains("//")))
        && !value
            .split('/')
            .any(|component| matches!(component, "." | ".."))
}

fn valid_agent_parent(
    fact: &Fact,
    requested_agent: AgentId,
    home: InstallationId,
    context: &ReductionContext<'_, ProjectReason>,
) -> bool {
    let Some(mailbox) = unique_agent_mailbox(requested_agent, home, context) else {
        return false;
    };
    fact.causal().parents().iter().any(|parent| {
        context.facts().get(*parent).is_some_and(|candidate| {
            context.is_projected(candidate.id())
                && matches!(candidate.payload(), SemanticPayload::AgentNameClaimed { agent_id, mailbox_id, .. }
                    if *agent_id == requested_agent && *mailbox_id == mailbox.mailbox_id())
        })
    }) && !projected_facts(context).any(|candidate| {
        matches!(candidate.payload(), SemanticPayload::AgentRetired { agent_id: retired, .. }
            if *retired == requested_agent)
            && !context
                .graph()
                .structurally_reaches(fact.id(), candidate.id())
    })
}

fn unique_agent_mailbox(
    agent_id: AgentId,
    home: InstallationId,
    context: &ReductionContext<'_, ProjectReason>,
) -> Option<MailboxAddress> {
    let claims = projected_facts(context)
        .filter_map(|candidate| match candidate.payload() {
            SemanticPayload::AgentNameClaimed {
                agent_id: candidate_agent,
                mailbox_id,
                ..
            } if *candidate_agent == agent_id && candidate.author().installation_id() == home => {
                Some(MailboxAddress::new(home, *mailbox_id))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    (claims.len() == 1)
        .then(|| claims.into_iter().next())
        .flatten()
}

fn valid_project_thread(
    fact: &Fact,
    thread_id: ThreadId,
    project_id: ProjectId,
    home: InstallationId,
    mailbox_id: MailboxId,
    context: &ReductionContext<'_, ProjectReason>,
) -> bool {
    fact.causal().parents().iter().any(|parent| {
        context.facts().get(*parent).is_some_and(|candidate| {
            context.is_projected(candidate.id())
                && ThreadId::from_bytes(*candidate.id().as_bytes()) == thread_id
                && message_content(candidate.payload()).is_some_and(|message| {
                    message.project_id == Some(project_id)
                        && message.recipient == Some(MailboxAddress::new(home, mailbox_id))
                })
        })
    })
}

fn valid_project_input(
    fact: &Fact,
    input_fact_id: FactId,
    message_id: MessageId,
    project_id: ProjectId,
    home: InstallationId,
    mailbox_id: MailboxId,
    context: &ReductionContext<'_, ProjectReason>,
) -> bool {
    fact.causal().parents().contains(&input_fact_id)
        && context.facts().get(input_fact_id).is_some_and(|candidate| {
            context.is_projected(candidate.id())
                && message_content(candidate.payload()).is_some_and(|message| {
                    message.message_id == message_id
                        && message.project_id == Some(project_id)
                        && message.recipient == Some(MailboxAddress::new(home, mailbox_id))
                })
        })
}

fn valid_acceptance_parent(
    fact: &Fact,
    message_id: MessageId,
    sequence: u64,
    context: &ReductionContext<'_, ProjectReason>,
) -> bool {
    fact.causal().parents().iter().any(|parent| context.facts().get(*parent).is_some_and(|candidate| {
        context.is_projected(candidate.id()) && matches!(candidate.payload(), SemanticPayload::ProjectInputAccepted { message_id: candidate_message, sequence: candidate_sequence, .. } if *candidate_message == message_id && candidate_sequence.get() == sequence)
    }))
}

fn valid_dispatch_parent(
    fact: &Fact,
    dispatch_id: DispatchId,
    binding: &AssignmentBinding,
    thread_id: ThreadId,
    context: &ReductionContext<'_, ProjectReason>,
) -> bool {
    fact.causal().parents().iter().any(|parent| context.facts().get(*parent).is_some_and(|candidate| {
        context.is_projected(candidate.id()) && matches!(candidate.payload(), SemanticPayload::ProjectInputDispatched { dispatch_id: candidate_dispatch, binding: candidate_binding, thread_id: candidate_thread, .. } if *candidate_dispatch == dispatch_id && candidate_binding == binding && *candidate_thread == thread_id)
    }))
}

fn message_content(payload: &SemanticPayload) -> Option<&MessageContent> {
    match payload {
        SemanticPayload::QuestionAsked(message)
        | SemanticPayload::AsynchronousMessageSent(message)
        | SemanticPayload::AnswerGiven { message, .. }
        | SemanticPayload::ProjectOutputRecorded { message, .. } => Some(message),
        _ => None,
    }
}

fn validate_remote_control(
    fact: &Fact,
    context: &ReductionContext<'_, ProjectReason>,
) -> Result<(), ClassificationError> {
    match fact.payload() {
        SemanticPayload::RemoteProjectCommandRequested {
            command_id,
            digest,
            project_id,
            target_home,
            expected_head,
            ..
        } => {
            if !matches!(fact.scope(), FactScope::RemoteControl { target_home: scope_home, .. } if *scope_home == *target_home)
                || !valid_active_human(fact, context)
                || context
                    .facts()
                    .get(*expected_head)
                    .is_none_or(|head| project_id_of(head) != Some(*project_id))
            {
                return Err(invalid(ProjectReason::RemoteCommandConflict));
            }
            let participants = remote_identity_participants(*command_id, digest, context);
            if !participants.is_empty() {
                return Err((ProjectReason::RemoteCommandConflict, participants));
            }
            Ok(())
        }
        SemanticPayload::RemoteProjectCommandReceipt {
            command_id,
            digest,
            project_id,
            received_head,
            ..
        } => {
            let request = remote_request_parent(fact, *command_id, digest, *project_id, context)
                .ok_or_else(|| invalid(ProjectReason::RemoteCommandConflict))?;
            let target_home = match request.payload() {
                SemanticPayload::RemoteProjectCommandRequested { target_home, .. } => *target_home,
                _ => unreachable!(),
            };
            if fact.author().installation_id() != target_home
                || !valid_project_home(fact, target_home, context)
                || context
                    .facts()
                    .get(*received_head)
                    .is_none_or(|head| project_id_of(head) != Some(*project_id))
            {
                return Err(invalid(ProjectReason::RemoteCommandConflict));
            }
            Ok(())
        }
        SemanticPayload::RemoteProjectCommandOutcome {
            command_id,
            digest,
            project_id,
            result,
            ..
        } => {
            let request = remote_request_parent(fact, *command_id, digest, *project_id, context)
                .ok_or_else(|| invalid(ProjectReason::RemoteCommandConflict))?;
            let target_home = match request.payload() {
                SemanticPayload::RemoteProjectCommandRequested { target_home, .. } => *target_home,
                _ => unreachable!(),
            };
            let has_receipt = fact.causal().parents().iter().any(|parent| context.facts().get(*parent).is_some_and(|candidate| {
                context.is_projected(candidate.id()) && matches!(candidate.payload(), SemanticPayload::RemoteProjectCommandReceipt { command_id: candidate_command, digest: candidate_digest, project_id: candidate_project, .. } if *candidate_command == *command_id && candidate_digest == digest && *candidate_project == *project_id)
            }));
            let committed_valid = match result {
                RemoteCommandResult::Committed(head) => {
                    fact.causal().parents().contains(head)
                        && context.facts().get(*head).is_some_and(|candidate| {
                            context.is_projected(candidate.id())
                                && project_id_of(candidate) == Some(*project_id)
                        })
                }
                RemoteCommandResult::Rejected(_) => true,
            };
            if fact.author().installation_id() != target_home
                || !valid_project_home(fact, target_home, context)
                || !has_receipt
                || !committed_valid
            {
                return Err(invalid(ProjectReason::RemoteCommandConflict));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn remote_request_parent<'a>(
    fact: &Fact,
    command_id: CommandId,
    digest: &CommandDigest,
    project_id: ProjectId,
    context: &'a ReductionContext<'_, ProjectReason>,
) -> Option<&'a Fact> {
    fact.causal().parents().iter().find_map(|parent| context.facts().get(*parent).filter(|candidate| {
        context.is_projected(candidate.id()) && matches!(candidate.payload(), SemanticPayload::RemoteProjectCommandRequested { command_id: candidate_command, digest: candidate_digest, project_id: candidate_project, .. } if *candidate_command == command_id && candidate_digest == digest && *candidate_project == project_id)
    }))
}

fn remote_identity_participants(
    command_id: CommandId,
    digest: &CommandDigest,
    context: &ReductionContext<'_, ProjectReason>,
) -> BTreeSet<FactId> {
    let requests = context.facts().facts().filter(|candidate| matches!(candidate.payload(), SemanticPayload::RemoteProjectCommandRequested { command_id: candidate_command, .. } if *candidate_command == command_id)).collect::<Vec<_>>();
    if requests.iter().any(|request| matches!(request.payload(), SemanticPayload::RemoteProjectCommandRequested { digest: candidate_digest, .. } if candidate_digest != digest)) {
        requests.into_iter().map(Fact::id).collect()
    } else { BTreeSet::new() }
}

fn aggregate_keys(fact: &Fact) -> Vec<ProjectAggregateKey> {
    match fact.payload() {
        SemanticPayload::ProjectCreated {
            project_id,
            home,
            resources,
            initial_state,
            ..
        } => {
            let mut keys = vec![ProjectAggregateKey::Project(*project_id)];
            if *initial_state == InitialProjectState::Open {
                keys.extend(resources.as_slice().iter().map(|resource| {
                    ProjectAggregateKey::Resource {
                        home: *home,
                        locator: resource.canonical_locator.clone(),
                    }
                }));
            }
            keys
        }
        SemanticPayload::ProjectAssignmentConfiguring { project_id, intent } => vec![
            ProjectAggregateKey::Project(*project_id),
            ProjectAggregateKey::AgentAssignment {
                home: fact.author().installation_id(),
                agent: intent.agent_id,
            },
        ],
        SemanticPayload::ProjectAssignmentRunnable {
            project_id,
            binding,
            ..
        } => vec![
            ProjectAggregateKey::Project(*project_id),
            ProjectAggregateKey::AgentAssignment {
                home: fact.author().installation_id(),
                agent: binding.agent_id,
            },
        ],
        SemanticPayload::ProjectInputAccepted {
            project_id,
            message_id,
            ..
        } => vec![
            ProjectAggregateKey::Project(*project_id),
            ProjectAggregateKey::Input(*message_id),
        ],
        SemanticPayload::ProjectInputDispatched {
            project_id,
            dispatch_id,
            ..
        } => vec![
            ProjectAggregateKey::Project(*project_id),
            ProjectAggregateKey::Dispatch(*dispatch_id),
        ],
        SemanticPayload::ProjectOutputRecorded {
            project_id,
            output_id,
            ..
        } => vec![
            ProjectAggregateKey::Project(*project_id),
            ProjectAggregateKey::Output(*output_id),
        ],
        SemanticPayload::RemoteProjectCommandRequested { command_id, .. }
        | SemanticPayload::RemoteProjectCommandReceipt { command_id, .. }
        | SemanticPayload::RemoteProjectCommandOutcome { command_id, .. } => {
            vec![ProjectAggregateKey::Command(*command_id)]
        }
        payload => project_id(payload)
            .map(ProjectAggregateKey::Project)
            .into_iter()
            .collect(),
    }
}

fn derive_projections<P: ResourceConflictPolicy>(
    context: &ReductionContext<'_, ProjectReason>,
    policy: &P,
) -> Vec<ProjectionContribution<ProjectProjectionKey, ProjectProjection>> {
    let states = final_states(context);
    let claim_conflicts = claim_conflict_map(&states, policy);
    let assignment_conflicts = assignment_conflict_map(&states);
    let mut projections = states
        .values()
        .map(|state| {
            let project_claim_conflicts = claim_conflicts
                .iter()
                .filter_map(|((project, resource), conflicts)| {
                    (*project == state.project_id).then_some((*resource, conflicts.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            let claimable = project_claim_conflicts.is_empty();
            let active_claims = if claimable
                && matches!(
                    state.lifecycle,
                    ProjectLifecycle::Open | ProjectLifecycle::Closing
                ) {
                state.resources.keys().copied().collect()
            } else {
                BTreeSet::new()
            };
            let assignment = state.assignment.as_ref().map(|assignment| {
                let cardinality_conflicted = assignment_conflicts
                    .get(&(state.home, assignment.intent.agent_id))
                    .is_some_and(|projects| projects.len() > 1);
                ProjectAssignmentView {
                    intent: assignment.intent.clone(),
                    binding: assignment.binding.clone(),
                    phase: assignment.phase.clone(),
                    cardinality_conflicted,
                    runnable: !cardinality_conflicted
                        && claimable
                        && state.lifecycle == ProjectLifecycle::Open
                        && matches!(assignment.phase, ProjectAssignmentPhase::Runnable { .. }),
                    support: assignment.support.clone(),
                }
            });
            let mut support = project_support(state, context);
            for conflicting_project in project_claim_conflicts.values().flatten() {
                if let Some(conflicting_state) = states.get(conflicting_project) {
                    support.insert(conflicting_state.head);
                }
            }
            if let Some(assignment) = &state.assignment
                && let Some(conflicting_projects) =
                    assignment_conflicts.get(&(state.home, assignment.intent.agent_id))
            {
                support.extend(
                    conflicting_projects
                        .iter()
                        .filter_map(|project| states.get(project).map(|other| other.head)),
                );
            }
            expand_direct_support(&mut support, context);
            ProjectionContribution::new(
                ProjectProjectionKey::Project(state.project_id),
                ProjectProjection::Project(Box::new(ProjectView {
                    root: state.root,
                    head: state.head,
                    fork_participants: fork_participants(state.head, state.project_id, context),
                    home: state.home,
                    mailbox: MailboxAddress::new(state.home, state.mailbox_id),
                    predecessor: state.predecessor,
                    name: state.name.clone(),
                    brief: state.brief.clone(),
                    resources: state.resources.clone(),
                    primary: state.primary,
                    lifecycle: state.lifecycle,
                    archived: state.archived,
                    active_claims,
                    claim_conflicts: project_claim_conflicts,
                    claimable,
                    assignment,
                    input_sequence: state.input_sequence,
                })),
                support,
            )
        })
        .collect::<Vec<_>>();
    projections.extend(input_projections(context));
    projections.extend(dispatch_projections(context));
    projections.extend(output_projections(context, &states));
    projections.extend(command_projections(context));
    projections
}

fn final_states(
    context: &ReductionContext<'_, ProjectReason>,
) -> BTreeMap<ProjectId, InternalProjectState> {
    let mut states = BTreeMap::new();
    for root in context.facts().facts().filter(|fact| {
        context.is_projected(fact.id())
            && matches!(fact.payload(), SemanticPayload::ProjectCreated { .. })
    }) {
        let Some(mut state) = state_at(root.id(), context) else {
            continue;
        };
        loop {
            let children = context
                .facts()
                .facts()
                .filter(|candidate| {
                    context.is_projected(candidate.id())
                        && candidate.causal().authority(AuthorityRole::PreviousState)
                            == Some(state.head)
                        && project_id_of(candidate) == Some(state.project_id)
                })
                .collect::<Vec<_>>();
            if children.len() != 1 {
                break;
            }
            let Some(next) = state_at(children[0].id(), context) else {
                break;
            };
            state = next;
        }
        states.insert(state.project_id, state);
    }
    states
}

fn project_support(
    state: &InternalProjectState,
    context: &ReductionContext<'_, ProjectReason>,
) -> BTreeSet<FactId> {
    let mut support = BTreeSet::new();
    let mut current = Some(state.head);
    while let Some(fact_id) = current {
        if !support.insert(fact_id) || fact_id == state.root {
            break;
        }
        current = context
            .facts()
            .get(fact_id)
            .and_then(|fact| fact.causal().authority(AuthorityRole::PreviousState));
    }
    support
}

fn expand_direct_support(
    support: &mut BTreeSet<FactId>,
    context: &ReductionContext<'_, ProjectReason>,
) {
    let facts = support.iter().copied().collect::<Vec<_>>();
    for fact_id in facts {
        if let Some(fact) = context.facts().get(fact_id) {
            support.extend(
                fact.causal()
                    .parents()
                    .iter()
                    .copied()
                    .filter(|parent| context.is_projected(*parent)),
            );
        }
    }
}

fn fork_participants(
    head: FactId,
    project_id: ProjectId,
    context: &ReductionContext<'_, ProjectReason>,
) -> BTreeSet<FactId> {
    context
        .facts()
        .facts()
        .filter(|candidate| {
            candidate.causal().authority(AuthorityRole::PreviousState) == Some(head)
                && project_id_of(candidate) == Some(project_id)
                && context
                    .decisions()
                    .get(&candidate.id())
                    .is_some_and(|decision| decision.status() == DecisionStatus::Conflicted)
        })
        .map(Fact::id)
        .collect()
}

fn claim_conflict_map<P: ResourceConflictPolicy>(
    states: &BTreeMap<ProjectId, InternalProjectState>,
    policy: &P,
) -> BTreeMap<(ProjectId, ResourceId), BTreeSet<ProjectId>> {
    let mut conflicts = BTreeMap::<(ProjectId, ResourceId), BTreeSet<ProjectId>>::new();
    let active = states
        .values()
        .filter(|state| {
            matches!(
                state.lifecycle,
                ProjectLifecycle::Open | ProjectLifecycle::Closing
            )
        })
        .collect::<Vec<_>>();
    for (index, left) in active.iter().enumerate() {
        for right in active.iter().skip(index + 1) {
            if left.home != right.home || left.project_id == right.project_id {
                continue;
            }
            for left_resource in left.resources.values() {
                for right_resource in right.resources.values() {
                    if policy.conflicts(
                        &left_resource.canonical_locator,
                        &right_resource.canonical_locator,
                    ) {
                        conflicts
                            .entry((left.project_id, left_resource.resource_id))
                            .or_default()
                            .insert(right.project_id);
                        conflicts
                            .entry((right.project_id, right_resource.resource_id))
                            .or_default()
                            .insert(left.project_id);
                    }
                }
            }
        }
    }
    conflicts
}

fn claim_conflict_participants<P: ResourceConflictPolicy>(
    candidate: &InternalProjectState,
    candidate_fact: FactId,
    context: &ReductionContext<'_, ProjectReason>,
    policy: &P,
) -> BTreeSet<FactId> {
    let states = final_states(context);
    let mut participants = BTreeSet::new();
    for other in states.values() {
        if other.project_id == candidate.project_id
            || other.home != candidate.home
            || !matches!(
                other.lifecycle,
                ProjectLifecycle::Open | ProjectLifecycle::Closing
            )
        {
            continue;
        }
        if candidate.resources.values().any(|left| {
            other
                .resources
                .values()
                .any(|right| policy.conflicts(&left.canonical_locator, &right.canonical_locator))
        }) {
            participants.insert(candidate_fact);
            participants.insert(other.head);
        }
    }
    participants
}

fn assignment_conflict_map(
    states: &BTreeMap<ProjectId, InternalProjectState>,
) -> BTreeMap<(InstallationId, AgentId), BTreeSet<ProjectId>> {
    let mut assignments = BTreeMap::<(InstallationId, AgentId), BTreeSet<ProjectId>>::new();
    for state in states.values() {
        if let Some(assignment) = &state.assignment {
            assignments
                .entry((state.home, assignment.intent.agent_id))
                .or_default()
                .insert(state.project_id);
        }
    }
    assignments
}

fn input_projections(
    context: &ReductionContext<'_, ProjectReason>,
) -> Vec<ProjectionContribution<ProjectProjectionKey, ProjectProjection>> {
    projected_facts(context)
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::ProjectInputAccepted {
                project_id,
                message_id,
                input_fact_id,
                sequence,
            } => Some(ProjectionContribution::new(
                ProjectProjectionKey::Input(*message_id),
                ProjectProjection::Input(Box::new(ProjectInputView {
                    project_id: *project_id,
                    message_id: *message_id,
                    input_fact_id: *input_fact_id,
                    sequence: sequence.get(),
                    accepted_fact: fact.id(),
                })),
                [fact.id(), *input_fact_id],
            )),
            _ => None,
        })
        .collect()
}

fn dispatch_projections(
    context: &ReductionContext<'_, ProjectReason>,
) -> Vec<ProjectionContribution<ProjectProjectionKey, ProjectProjection>> {
    let mut groups = BTreeMap::<DispatchId, Vec<&Fact>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::ProjectInputDispatched { dispatch_id, .. } = fact.payload() {
            groups.entry(*dispatch_id).or_default().push(fact);
        }
    }
    groups
        .into_iter()
        .filter_map(|(dispatch_id, facts)| {
            let first = *facts.first()?;
            let SemanticPayload::ProjectInputDispatched {
                message_id,
                sequence,
                binding,
                thread_id,
                ..
            } = first.payload()
            else {
                return None;
            };
            let conflicted = facts
                .iter()
                .any(|candidate| candidate.payload() != first.payload());
            Some(ProjectionContribution::new(
                ProjectProjectionKey::Dispatch(dispatch_id),
                ProjectProjection::Dispatch(Box::new(ProjectDispatchView {
                    dispatch_id,
                    message_id: *message_id,
                    sequence: sequence.get(),
                    binding: binding.clone(),
                    thread_id: *thread_id,
                    fact_id: first.id(),
                    conflicted,
                })),
                facts.iter().map(|fact| fact.id()),
            ))
        })
        .collect()
}

fn output_projections(
    context: &ReductionContext<'_, ProjectReason>,
    _states: &BTreeMap<ProjectId, InternalProjectState>,
) -> Vec<ProjectionContribution<ProjectProjectionKey, ProjectProjection>> {
    let mut groups = BTreeMap::<MessageId, Vec<&Fact>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::ProjectOutputRecorded { output_id, .. } = fact.payload() {
            groups.entry(*output_id).or_default().push(fact);
        }
    }
    groups
        .into_iter()
        .filter_map(|(output_id, facts)| {
            let first = *facts.first()?;
            let SemanticPayload::ProjectOutputRecorded {
                project_id,
                dispatch_id,
                binding,
                thread_id,
                message,
                ..
            } = first.payload()
            else {
                return None;
            };
            let unequal = facts
                .iter()
                .any(|candidate| candidate.payload() != first.payload());
        let inactive_before = projected_facts(context).any(|candidate| {
            matches!(candidate.payload(), SemanticPayload::ProjectAssignmentEnded { project_id: ended_project, assignment_id, .. }
                if *ended_project == *project_id && *assignment_id == binding.assignment_id)
                && context.graph().structurally_reaches(candidate.id(), first.id())
        });
            let status = if unequal {
                ProjectOutputStatus::Conflicted
        } else if inactive_before {
            ProjectOutputStatus::LateFromInactive
        } else {
            ProjectOutputStatus::Current
        };
            Some(ProjectionContribution::new(
                ProjectProjectionKey::Output(output_id),
                ProjectProjection::Output(Box::new(ProjectOutputView {
                    output_id,
                    dispatch_id: *dispatch_id,
                    binding: binding.clone(),
                    thread_id: *thread_id,
                    message: message.clone(),
                    status,
                    facts: facts.iter().map(|fact| fact.id()).collect(),
                })),
                facts.iter().map(|fact| fact.id()),
            ))
        })
        .collect()
}

fn command_projections(
    context: &ReductionContext<'_, ProjectReason>,
) -> Vec<ProjectionContribution<ProjectProjectionKey, ProjectProjection>> {
    let mut groups = BTreeMap::<CommandId, Vec<&Fact>>::new();
    for fact in projected_facts(context) {
        match fact.payload() {
            SemanticPayload::RemoteProjectCommandRequested { command_id, .. }
            | SemanticPayload::RemoteProjectCommandReceipt { command_id, .. }
            | SemanticPayload::RemoteProjectCommandOutcome { command_id, .. } => {
                groups.entry(*command_id).or_default().push(fact);
            }
            _ => {}
        }
    }
    groups
        .into_iter()
        .filter_map(|(command_id, facts)| {
            let request = facts.iter().find_map(|fact| match fact.payload() {
                SemanticPayload::RemoteProjectCommandRequested {
                    digest,
                    project_id,
                    expected_head,
                    ..
                } => Some((*fact, *digest, *project_id, *expected_head)),
                _ => None,
            })?;
            let receipts = facts
                .iter()
                .filter_map(|fact| match fact.payload() {
                    SemanticPayload::RemoteProjectCommandReceipt { received_head, .. } => {
                        Some((*fact, *received_head))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let outcomes = facts
                .iter()
                .filter_map(|fact| match fact.payload() {
                    SemanticPayload::RemoteProjectCommandOutcome {
                        result, runtime, ..
                    } => Some((*fact, result.clone(), runtime.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let stage = if outcomes.len() > 1
                && outcomes
                    .windows(2)
                    .any(|pair| pair[0].1 != pair[1].1 || pair[0].2 != pair[1].2)
                || receipts.len() > 1 && receipts.windows(2).any(|pair| pair[0].1 != pair[1].1)
            {
                RemoteCommandStage::Conflicted
            } else if let Some((_, result, runtime)) = outcomes.first() {
                RemoteCommandStage::Terminal {
                    result: result.clone(),
                    runtime: runtime.clone(),
                }
            } else if let Some((_, received_head)) = receipts.first() {
                RemoteCommandStage::Received {
                    received_head: *received_head,
                }
            } else {
                RemoteCommandStage::Queued
            };
            Some(ProjectionContribution::new(
                ProjectProjectionKey::Command(command_id),
                ProjectProjection::Command(Box::new(RemoteCommandView {
                    digest: request.1,
                    project_id: request.2,
                    expected_head: request.3,
                    stage,
                    support: facts.iter().map(|fact| fact.id()).collect(),
                })),
                facts.iter().map(|fact| fact.id()),
            ))
        })
        .collect()
}

fn derive_conflicts<P: ResourceConflictPolicy>(
    context: &ReductionContext<'_, ProjectReason>,
    policy: &P,
) -> Vec<ConflictObservation<ProjectReason>> {
    let states = final_states(context);
    let mut observations = Vec::new();
    for ((project_id, _), conflicts) in claim_conflict_map(&states, policy) {
        observations.push(ConflictObservation::new(
            ConflictReason::Domain(ProjectReason::ResourceClaimConflict),
            std::iter::once(states[&project_id].head).chain(
                conflicts
                    .iter()
                    .filter_map(|project| states.get(project).map(|state| state.head)),
            ),
        ));
    }
    for projects in assignment_conflict_map(&states)
        .values()
        .filter(|projects| projects.len() > 1)
    {
        observations.push(ConflictObservation::new(
            ConflictReason::Domain(ProjectReason::AssignmentCardinalityConflict),
            projects
                .iter()
                .filter_map(|project| states.get(project).map(|state| state.head)),
        ));
    }
    observations
}

fn projected_facts<'facts, 'context>(
    context: &'context ReductionContext<'facts, ProjectReason>,
) -> impl Iterator<Item = &'facts Fact> + 'context
where
    'facts: 'context,
{
    context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
}

fn project_id_of(fact: &Fact) -> Option<ProjectId> {
    project_id(fact.payload())
}

fn project_id(payload: &SemanticPayload) -> Option<ProjectId> {
    match payload {
        SemanticPayload::ProjectCreated { project_id, .. }
        | SemanticPayload::ProjectOpened { project_id }
        | SemanticPayload::ProjectClosingStarted { project_id }
        | SemanticPayload::ProjectClosed { project_id, .. }
        | SemanticPayload::ProjectArchived { project_id }
        | SemanticPayload::ProjectUnarchived { project_id }
        | SemanticPayload::ProjectMetadataUpdated { project_id, .. }
        | SemanticPayload::ProjectResourceAdded { project_id, .. }
        | SemanticPayload::ProjectResourceRemoved { project_id, .. }
        | SemanticPayload::ProjectResourceReplaced { project_id, .. }
        | SemanticPayload::ProjectPrimaryResourceChanged { project_id, .. }
        | SemanticPayload::ProjectResourceHealthObserved { project_id, .. }
        | SemanticPayload::ProjectAssignmentConfiguring { project_id, .. }
        | SemanticPayload::ProjectAssignmentRunnable { project_id, .. }
        | SemanticPayload::ProjectAssignmentBlocked { project_id, .. }
        | SemanticPayload::ProjectAssignmentEnded { project_id, .. }
        | SemanticPayload::ProjectInputAccepted { project_id, .. }
        | SemanticPayload::ProjectInputDispatched { project_id, .. }
        | SemanticPayload::ProjectOutputRecorded { project_id, .. }
        | SemanticPayload::RemoteProjectCommandRequested { project_id, .. }
        | SemanticPayload::RemoteProjectCommandReceipt { project_id, .. }
        | SemanticPayload::RemoteProjectCommandOutcome { project_id, .. } => Some(*project_id),
        _ => None,
    }
}

fn is_remote(payload: &SemanticPayload) -> bool {
    matches!(
        payload,
        SemanticPayload::RemoteProjectCommandRequested { .. }
            | SemanticPayload::RemoteProjectCommandReceipt { .. }
            | SemanticPayload::RemoteProjectCommandOutcome { .. }
    )
}
