//! Transaction-consistent canonical project mutation adapter over application ports.

use std::collections::{BTreeSet, HashSet};

use hq_application::{
    AgentRetirementPlanRequest, AgentRetirementRequest, ApplicationError, ApplicationErrorCode,
    CommitFacts, DomainSnapshot, FactMutation, FactPlan, LocalFactInputs,
    LocalInstallationAuthority, MutationAttempt, MutationDecision, MutationOutcome, QueryDomain,
    plan_agent_retirement,
};
use hq_domain::{
    AgentId, AuthorityReference, AuthorityRole, BoundedSet, BoundedVec, CausalReferences,
    ContentText, DomainError, ErrorCategory, ErrorCode, FactId, FactScope, InitialProjectState,
    InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxId, ProjectId, ProjectResource,
    ResourceHealth, SemanticPayload, ShortText,
};
use hq_reducer::{
    AgentLifecycle, AgentProjection, AgentProjectionKey, AuthorityProjection,
    AuthorityProjectionKey, ConversationProjection, ConversationProjectionKey, MembershipState,
    ProjectAssignmentPhase, ProjectLifecycle, ProjectOutputStatus, ProjectProjection,
    ProjectProjectionKey,
};
use hq_resources::{PathClaim, claim_conflict, valid_path_resource};

use crate::{
    AgentRetirementAssignment, AgentRetirementSnapshot, CanonicalProjectAssignment,
    CanonicalProjectLifecycle, CanonicalProjectMutation, CanonicalProjectMutationAction,
    CanonicalProjectMutationOutcome, CanonicalProjectPort, PendingProjectInput,
    ProjectWorkflowSnapshot,
};

/// Concrete transaction-consistent project adapter over query and canonical-commit capabilities.
pub struct ApplicationCanonicalProjectPort<P> {
    ports: P,
}

impl<P> ApplicationCanonicalProjectPort<P> {
    /// Owns the application capabilities used for serialized reads and commits.
    pub const fn new(ports: P) -> Self {
        Self { ports }
    }

    /// Consumes the adapter and returns its capability bundle.
    pub fn into_ports(self) -> P {
        self.ports
    }
}

impl<P> CanonicalProjectPort for ApplicationCanonicalProjectPort<P>
where
    P: QueryDomain + CommitFacts,
{
    fn agent_retirement_snapshot(
        &self,
        request: &AgentRetirementRequest,
    ) -> Result<AgentRetirementSnapshot, ApplicationError> {
        Ok(retirement_snapshot(
            self.ports.authoritative_snapshot()?.domain(),
            request,
        ))
    }

    fn retire_idle_agent(
        &self,
        request: &AgentRetirementRequest,
    ) -> Result<MutationAttempt, ApplicationError> {
        let request = *request;
        let command_id = request.command_id;
        let request_digest = request.request_digest;
        self.ports.commit_facts(FactMutation::new(
            command_id,
            request_digest,
            move |snapshot| match build_idle_retirement_plan(snapshot, &request) {
                Ok(plan) => MutationDecision::commit(plan),
                Err(error) => MutationDecision::reject(error),
            },
        ))
    }

    fn snapshot(
        &self,
        project_id: ProjectId,
        account_id: hq_domain::AccountId,
        requested_agent: Option<AgentId>,
    ) -> Result<ProjectWorkflowSnapshot, ApplicationError> {
        workflow_snapshot(
            self.ports.authoritative_snapshot()?.domain(),
            project_id,
            account_id,
            requested_agent,
        )
    }

    fn mutate(
        &self,
        mutation: CanonicalProjectMutation,
    ) -> Result<CanonicalProjectMutationOutcome, ApplicationError> {
        let command_id = mutation.command_id;
        let request_digest = mutation.request_digest;
        let project_id = mutation.project_id;
        let decision_input = mutation.clone();
        let attempt = self.ports.commit_facts(FactMutation::new(
            command_id,
            request_digest,
            move |snapshot| decide(snapshot, &decision_input),
        ))?;
        match attempt {
            MutationAttempt::Uncertain { .. } => Ok(CanonicalProjectMutationOutcome::Uncertain),
            MutationAttempt::Completed(receipt) => match receipt.outcome() {
                MutationOutcome::Rejected(error) => {
                    Ok(CanonicalProjectMutationOutcome::Rejected(error.clone()))
                }
                MutationOutcome::Committed => {
                    let snapshot = self.ports.authoritative_snapshot()?;
                    let head = project_view(snapshot.domain(), project_id)?.head;
                    Ok(CanonicalProjectMutationOutcome::Committed { project_head: head })
                }
            },
        }
    }
}

fn workflow_snapshot(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
    account_id: hq_domain::AccountId,
    requested_agent: Option<AgentId>,
) -> Result<ProjectWorkflowSnapshot, ApplicationError> {
    let view = project_view(snapshot, project_id)?;
    let dispatched = snapshot
        .project()
        .projections()
        .values()
        .filter_map(|projection| match projection {
            ProjectProjection::Dispatch(dispatch) if !dispatch.conflicted => {
                Some(dispatch.message_id)
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut pending_inputs = snapshot
        .project()
        .projections()
        .values()
        .filter_map(|projection| match projection {
            ProjectProjection::Input(input)
                if input.project_id == project_id && !dispatched.contains(&input.message_id) =>
            {
                let ConversationProjection::Message(message) = snapshot
                    .conversation()
                    .projection(ConversationProjectionKey::Message(input.message_id))?
                else {
                    return None;
                };
                Some(PendingProjectInput {
                    message_id: input.message_id,
                    input_fact_id: input.input_fact_id,
                    accepted_fact: input.accepted_fact,
                    sequence: std::num::NonZeroU64::new(input.sequence)?,
                    thread_id: message.thread_id,
                    body: message.content.body.clone(),
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    pending_inputs.sort_by_key(|input| input.sequence);
    let historical_threads = requested_agent.map_or_else(
        || {
            snapshot
                .conversation()
                .projections()
                .values()
                .filter_map(|projection| match projection {
                    ConversationProjection::Message(message)
                        if message.content.project_id == Some(project_id) =>
                    {
                        Some(message.thread_id)
                    }
                    _ => None,
                })
                .collect()
        },
        |agent_id| agent_project_threads(snapshot, project_id, agent_id),
    );
    let assignment = view.assignment.as_ref().map(|assignment| {
        let (thread_id, phase_runnable, blocked) = match assignment.phase {
            ProjectAssignmentPhase::Runnable { thread_id, .. } => (Some(thread_id), true, false),
            ProjectAssignmentPhase::Configuring | ProjectAssignmentPhase::Blocked(_) => (
                None,
                false,
                matches!(assignment.phase, ProjectAssignmentPhase::Blocked(_)),
            ),
        };
        CanonicalProjectAssignment {
            intent: assignment.intent.clone(),
            binding: assignment.binding.clone(),
            thread_id,
            runnable: assignment.runnable && phase_runnable,
            blocked,
        }
    });
    let requested_agent_available = requested_agent.is_none_or(|agent_id| {
        agent_available_to_project(snapshot, project_id, view.home, agent_id)
    });
    Ok(ProjectWorkflowSnapshot {
        project_id,
        home: view.home,
        head: view.head,
        lifecycle: match view.lifecycle {
            ProjectLifecycle::Open => CanonicalProjectLifecycle::Open,
            ProjectLifecycle::Closing => CanonicalProjectLifecycle::Closing,
            ProjectLifecycle::Closed => CanonicalProjectLifecycle::Closed,
        },
        archived: view.archived,
        resources: view.resources.values().cloned().collect(),
        claimable: view.claimable,
        assignment,
        active_human: active_human_authority(snapshot, account_id, view.home).is_some(),
        requested_agent_available,
        pending_inputs,
        historical_threads,
    })
}

fn agent_available_to_project(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
    home: InstallationId,
    agent_id: AgentId,
) -> bool {
    active_local_agent(snapshot, home, agent_id)
        && snapshot
            .project()
            .projections()
            .iter()
            .all(|(key, projection)| {
                !matches!(projection, ProjectProjection::Project(project)
                    if project.assignment.as_ref().is_some_and(|assignment|
                        assignment.intent.agent_id == agent_id)
                        && *key != ProjectProjectionKey::Project(project_id))
            })
}

fn retirement_snapshot(
    snapshot: &DomainSnapshot,
    request: &AgentRetirementRequest,
) -> AgentRetirementSnapshot {
    let agent = match snapshot
        .agent()
        .projection(AgentProjectionKey::Agent(request.agent_id))
    {
        Some(AgentProjection::Agent(agent)) => Some(agent.as_ref()),
        _ => None,
    };
    let agent_active = agent.is_some_and(|agent| {
        agent.lifecycle == AgentLifecycle::Active
            && agent.claims.len() == 1
            && agent.mailboxes.len() == 1
            && agent
                .mailboxes
                .iter()
                .all(|mailbox| mailbox.installation_id() == request.home)
    });
    let claim_fact = agent
        .filter(|_| agent_active)
        .and_then(|agent| agent.claims.iter().next().copied());
    let mut conflicted = false;
    let assignments = snapshot
        .project()
        .projections()
        .iter()
        .filter_map(|(key, projection)| match (key, projection) {
            (ProjectProjectionKey::Project(project_id), ProjectProjection::Project(project))
                if project
                    .assignment
                    .as_ref()
                    .is_some_and(|assignment| assignment.intent.agent_id == request.agent_id) =>
            {
                conflicted |= !project.fork_participants.is_empty() || project.home != request.home;
                Some(AgentRetirementAssignment {
                    project_id: *project_id,
                    project_head: project.head,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    conflicted |= assignments.len() > 1;
    AgentRetirementSnapshot {
        active_human: active_human_authority(snapshot, request.account_id, request.home).is_some(),
        agent_active,
        claim_fact,
        conflicted,
        assignments,
    }
}

fn build_idle_retirement_plan(
    snapshot: &DomainSnapshot,
    request: &AgentRetirementRequest,
) -> Result<FactPlan, DomainError> {
    let observed = retirement_snapshot(snapshot, request);
    if !observed.active_human {
        return Err(domain_error(
            ErrorCategory::Unauthorized,
            "agent_retirement_inactive_human",
        ));
    }
    if observed.conflicted
        || !observed.assignments.is_empty()
        || !observed.agent_active
        || observed.claim_fact != Some(request.expected_claim)
    {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "agent_retirement_state_changed",
        ));
    }
    let authority = match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(request.home))
    {
        Some(AuthorityProjection::Installation(installation)) => LocalInstallationAuthority {
            installation_id: request.home,
            signing_key: installation.signing_key,
            root_fact: installation.root_fact,
        },
        _ => {
            return Err(domain_error(
                ErrorCategory::Unauthorized,
                "agent_retirement_home_missing",
            ));
        }
    };
    let Some(AgentProjection::Agent(agent)) = snapshot
        .agent()
        .projection(AgentProjectionKey::Agent(request.agent_id))
    else {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "agent_retirement_agent_unavailable",
        ));
    };
    let mailbox = agent.mailboxes.iter().next().copied().ok_or_else(|| {
        domain_error(
            ErrorCategory::Conflict,
            "agent_retirement_agent_unavailable",
        )
    })?;
    let agent_frontier = match snapshot
        .agent()
        .projection(AgentProjectionKey::Selection(request.agent_id))
    {
        Some(AgentProjection::Selection(selection)) => selection.frontier.clone(),
        _ => BTreeSet::new(),
    };
    plan_agent_retirement(
        authority,
        LocalFactInputs {
            authored_at: request.issued_at,
            auxiliary_randomness: *request.request_digest.as_bytes(),
        },
        AgentRetirementPlanRequest {
            agent_id: request.agent_id,
            mailbox,
            claim_fact: request.expected_claim,
            agent_frontier,
        },
    )
    .map_err(|_| {
        domain_error(
            ErrorCategory::InvariantViolation,
            "agent_retirement_plan_invalid",
        )
    })
}

fn decide(snapshot: &DomainSnapshot, mutation: &CanonicalProjectMutation) -> MutationDecision {
    match build_plan(snapshot, mutation) {
        Ok(plan) => MutationDecision::commit(plan),
        Err(error) => MutationDecision::reject(error),
    }
}

fn build_plan(
    snapshot: &DomainSnapshot,
    mutation: &CanonicalProjectMutation,
) -> Result<FactPlan, DomainError> {
    if let CanonicalProjectMutationAction::Create {
        mailbox_id,
        name,
        brief,
        resource,
    } = &mutation.action
    {
        return build_creation_plan(
            snapshot,
            mutation,
            *mailbox_id,
            name,
            brief.as_ref(),
            resource,
        );
    }
    let Some(expected_head) = mutation.expected_head else {
        return Err(domain_error(
            ErrorCategory::InvalidInput,
            "project_existing_head_required",
        ));
    };
    let view = project_view(snapshot, mutation.project_id)
        .map_err(|_| domain_error(ErrorCategory::NotFound, "project_not_found"))?;
    if view.home != mutation.home {
        return Err(domain_error(
            ErrorCategory::Unauthorized,
            "project_wrong_home",
        ));
    }
    if view.head != expected_head {
        return Err(domain_error(ErrorCategory::Conflict, "project_stale_head"));
    }
    let active_human = active_human_authority(snapshot, mutation.account_id, mutation.home)
        .ok_or_else(|| domain_error(ErrorCategory::Unauthorized, "project_inactive_human"))?;
    let installation_root = match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(mutation.home))
    {
        Some(AuthorityProjection::Installation(installation)) => installation.root_fact,
        _ => {
            return Err(domain_error(
                ErrorCategory::Unauthorized,
                "project_home_missing",
            ));
        }
    };
    if let CanonicalProjectMutationAction::RetireAgent { agent_id } = mutation.action {
        return build_retirement_plan(snapshot, mutation, installation_root, agent_id);
    }
    let mut parents = BTreeSet::from([view.head, installation_root, active_human]);
    let payload = payload(snapshot, mutation, view, &mut parents)?;
    let parents = BoundedSet::<FactId, MAX_FACT_PARENTS>::new(parents)
        .map_err(|_| domain_error(ErrorCategory::InvariantViolation, "project_parent_overflow"))?;
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        parents,
        [
            AuthorityReference::new(AuthorityRole::PreviousState, view.head),
            AuthorityReference::new(AuthorityRole::ProjectHome, installation_root),
            AuthorityReference::new(AuthorityRole::AccountMembership, active_human),
            AuthorityReference::new(AuthorityRole::ActiveHuman, active_human),
        ],
    )
    .map_err(|_| domain_error(ErrorCategory::InvariantViolation, "project_causal_invalid"))?;
    Ok(FactPlan::new(
        mutation.home,
        mutation.issued_at,
        FactScope::AccountAddressed(mutation.account_id),
        causal,
        payload,
        *mutation.request_digest.as_bytes(),
    ))
}

fn build_creation_plan(
    snapshot: &DomainSnapshot,
    mutation: &CanonicalProjectMutation,
    mailbox_id: MailboxId,
    name: &ShortText,
    brief: Option<&ContentText>,
    resource: &ProjectResource,
) -> Result<FactPlan, DomainError> {
    if mutation.expected_head.is_some()
        || snapshot
            .project()
            .projection(ProjectProjectionKey::Project(mutation.project_id))
            .is_some()
        || snapshot.project().projections().values().any(|projection| {
            matches!(projection, ProjectProjection::Project(project)
                if project.home == mutation.home && project.mailbox.mailbox_id() == mailbox_id)
        })
    {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "project_creation_identity_conflict",
        ));
    }
    if resource.health != ResourceHealth::Healthy
        || !valid_path_resource(resource)
        || !project_resources_are_claimable(snapshot, mutation, &[resource])
    {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "project_creation_resource_conflict",
        ));
    }
    let active_human = active_human_authority(snapshot, mutation.account_id, mutation.home)
        .ok_or_else(|| domain_error(ErrorCategory::Unauthorized, "project_inactive_human"))?;
    let installation_root = match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(mutation.home))
    {
        Some(AuthorityProjection::Installation(installation)) => installation.root_fact,
        _ => {
            return Err(domain_error(
                ErrorCategory::Unauthorized,
                "project_home_missing",
            ));
        }
    };
    let parents = BoundedSet::<FactId, MAX_FACT_PARENTS>::new(BTreeSet::from([
        installation_root,
        active_human,
    ]))
    .map_err(|_| domain_error(ErrorCategory::InvariantViolation, "project_parent_overflow"))?;
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        parents,
        [
            AuthorityReference::new(AuthorityRole::ProjectHome, installation_root),
            AuthorityReference::new(AuthorityRole::AccountMembership, active_human),
            AuthorityReference::new(AuthorityRole::ActiveHuman, active_human),
        ],
    )
    .map_err(|_| domain_error(ErrorCategory::InvariantViolation, "project_causal_invalid"))?;
    let resources = BoundedVec::new([resource.clone()]).map_err(|_| {
        domain_error(
            ErrorCategory::InvariantViolation,
            "project_resource_overflow",
        )
    })?;
    Ok(FactPlan::new(
        mutation.home,
        mutation.issued_at,
        FactScope::AccountAddressed(mutation.account_id),
        causal,
        SemanticPayload::ProjectCreated {
            project_id: mutation.project_id,
            mailbox_id,
            home: mutation.home,
            name: name.clone(),
            brief: brief.cloned(),
            predecessor: None,
            resources,
            primary: Some(resource.resource_id),
            initial_state: InitialProjectState::Open,
        },
        *mutation.request_digest.as_bytes(),
    ))
}

#[allow(clippy::too_many_lines, reason = "closed canonical transition table")]
fn payload(
    snapshot: &DomainSnapshot,
    mutation: &CanonicalProjectMutation,
    view: &hq_reducer::ProjectView,
    parents: &mut BTreeSet<FactId>,
) -> Result<SemanticPayload, DomainError> {
    let invalid = || domain_error(ErrorCategory::Conflict, "project_invalid_transition");
    if matches!(
        mutation.action,
        CanonicalProjectMutationAction::AddResource { .. }
            | CanonicalProjectMutationAction::RemoveResource { .. }
            | CanonicalProjectMutationAction::ReplaceResource { .. }
            | CanonicalProjectMutationAction::SetPrimaryResource { .. }
    ) {
        return resource_payload(snapshot, mutation, view).ok_or_else(invalid);
    }
    match &mutation.action {
        CanonicalProjectMutationAction::Open
            if view.lifecycle == ProjectLifecycle::Closed
                && !view.archived
                && project_resources_are_claimable(
                    snapshot,
                    mutation,
                    &view.resources.values().collect::<Vec<_>>(),
                ) =>
        {
            Ok(SemanticPayload::ProjectOpened {
                project_id: mutation.project_id,
            })
        }
        CanonicalProjectMutationAction::Configure(intent)
            if view.lifecycle == ProjectLifecycle::Open
                && view.claimable
                && view.assignment.is_none()
                && active_local_agent(snapshot, mutation.home, intent.agent_id)
                && agent_is_unassigned_elsewhere(
                    snapshot,
                    mutation.project_id,
                    intent.agent_id,
                ) =>
        {
            parents.insert(agent_claim(snapshot, mutation.home, intent.agent_id)?);
            Ok(SemanticPayload::ProjectAssignmentConfiguring {
                project_id: mutation.project_id,
                intent: intent.clone(),
            })
        }
        CanonicalProjectMutationAction::MakeRunnable {
            binding,
            thread_id,
            launch_directory,
            activation,
        } if can_make_runnable(snapshot, mutation.project_id, view, binding) =>
        {
            parents.insert(agent_claim(snapshot, mutation.home, binding.agent_id)?);
            parents.insert(thread_root(snapshot, *thread_id, mutation.project_id)?);
            Ok(SemanticPayload::ProjectAssignmentRunnable {
                project_id: mutation.project_id,
                binding: binding.clone(),
                thread_id: *thread_id,
                launch_directory: launch_directory.clone(),
                activation: activation.clone(),
            })
        }
        CanonicalProjectMutationAction::EndAssignment {
            assignment_id,
            forced,
            runtime,
        }
            if view.assignment.as_ref().is_some_and(|assignment| {
                assignment.intent.assignment_id == *assignment_id
            }) =>
        {
            Ok(SemanticPayload::ProjectAssignmentEnded {
                project_id: mutation.project_id,
                assignment_id: *assignment_id,
                forced: *forced,
                runtime: runtime.clone(),
            })
        }
        CanonicalProjectMutationAction::BlockAssignment {
            assignment_id,
            cause,
        } if view.assignment.as_ref().is_some_and(|assignment| {
            assignment.intent.assignment_id == *assignment_id
        }) => Ok(SemanticPayload::ProjectAssignmentBlocked {
            project_id: mutation.project_id,
            assignment_id: *assignment_id,
            cause: cause.clone(),
        }),
        CanonicalProjectMutationAction::BeginClosing
            if view.lifecycle == ProjectLifecycle::Open =>
        {
            Ok(SemanticPayload::ProjectClosingStarted {
                project_id: mutation.project_id,
            })
        }
        CanonicalProjectMutationAction::FinishClosing { forced, runtime }
            if view.lifecycle == ProjectLifecycle::Closing && view.assignment.is_none() =>
        {
            Ok(SemanticPayload::ProjectClosed {
                project_id: mutation.project_id,
                forced: *forced,
                runtime: runtime.clone(),
            })
        }
        CanonicalProjectMutationAction::Archive
            if view.lifecycle == ProjectLifecycle::Closed
                && !view.archived
                && view.assignment.is_none() =>
        {
            Ok(SemanticPayload::ProjectArchived {
                project_id: mutation.project_id,
            })
        }
        CanonicalProjectMutationAction::Unarchive
            if view.lifecycle == ProjectLifecycle::Closed && view.archived =>
        {
            Ok(SemanticPayload::ProjectUnarchived {
                project_id: mutation.project_id,
            })
        }
        CanonicalProjectMutationAction::RecordDispatch {
            input,
            dispatch_id,
            binding,
            thread_id,
        } if view.lifecycle == ProjectLifecycle::Open
            && view.claimable
            && view.assignment.as_ref().is_some_and(|assignment| {
                assignment.binding.as_ref() == Some(binding)
                    && matches!(assignment.phase, ProjectAssignmentPhase::Runnable { thread_id: active, .. } if active == *thread_id)
                    && assignment.runnable
            }) =>
        {
            let accepted = accepted_input_parent(snapshot, mutation.project_id, input)?;
            parents.insert(accepted);
            Ok(SemanticPayload::ProjectInputDispatched {
                project_id: mutation.project_id,
                message_id: input.message_id,
                sequence: input.sequence,
                dispatch_id: *dispatch_id,
                binding: binding.clone(),
                thread_id: *thread_id,
            })
        }
        _ => Err(invalid()),
    }
}

fn build_retirement_plan(
    snapshot: &DomainSnapshot,
    mutation: &CanonicalProjectMutation,
    installation_root: FactId,
    agent_id: AgentId,
) -> Result<FactPlan, DomainError> {
    if !agent_is_unassigned(snapshot, agent_id) {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "project_agent_assigned",
        ));
    }
    let Some(AgentProjection::Agent(agent)) = snapshot
        .agent()
        .projection(AgentProjectionKey::Agent(agent_id))
    else {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "project_agent_unavailable",
        ));
    };
    if agent.lifecycle != AgentLifecycle::Active
        || agent.mailboxes.len() != 1
        || !agent
            .mailboxes
            .iter()
            .all(|mailbox| mailbox.installation_id() == mutation.home)
    {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "project_agent_unavailable",
        ));
    }
    let mailbox_id = agent
        .mailboxes
        .iter()
        .next()
        .map(|mailbox| mailbox.mailbox_id())
        .ok_or_else(|| domain_error(ErrorCategory::Conflict, "project_agent_unavailable"))?;
    let mut parents = BTreeSet::from([installation_root]);
    parents.extend(agent.claims.iter().copied());
    if let Some(AgentProjection::Selection(selection)) = snapshot
        .agent()
        .projection(AgentProjectionKey::Selection(agent_id))
    {
        parents.extend(selection.frontier.iter().copied());
    }
    let parents = BoundedSet::<FactId, MAX_FACT_PARENTS>::new(parents)
        .map_err(|_| domain_error(ErrorCategory::InvariantViolation, "agent_parent_overflow"))?;
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        parents,
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation_root,
        )],
    )
    .map_err(|_| domain_error(ErrorCategory::InvariantViolation, "agent_causal_invalid"))?;
    Ok(FactPlan::new(
        mutation.home,
        mutation.issued_at,
        FactScope::InstallationPrivate(mutation.home),
        causal,
        SemanticPayload::AgentRetired {
            agent_id,
            mailbox_id,
        },
        *mutation.request_digest.as_bytes(),
    ))
}

fn agent_project_threads(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
    agent_id: AgentId,
) -> BTreeSet<hq_domain::ThreadId> {
    snapshot
        .project()
        .projections()
        .values()
        .filter_map(|projection| match projection {
            ProjectProjection::Dispatch(dispatch)
                if !dispatch.conflicted && dispatch.binding.agent_id == agent_id =>
            {
                matches!(
                    snapshot
                        .project()
                        .projection(ProjectProjectionKey::Input(dispatch.message_id)),
                    Some(ProjectProjection::Input(input)) if input.project_id == project_id
                )
                .then_some(dispatch.thread_id)
            }
            ProjectProjection::Output(output)
                if output.binding.agent_id == agent_id
                    && output.message.project_id == Some(project_id)
                    && output.status != ProjectOutputStatus::Conflicted =>
            {
                Some(output.thread_id)
            }
            _ => None,
        })
        .collect()
}

fn resource_payload(
    snapshot: &DomainSnapshot,
    mutation: &CanonicalProjectMutation,
    view: &hq_reducer::ProjectView,
) -> Option<SemanticPayload> {
    if !can_mutate_resources(view) {
        return None;
    }
    match &mutation.action {
        CanonicalProjectMutationAction::AddResource {
            resource,
            make_primary,
        } if valid_path_resource(resource)
            && !view.resources.contains_key(&resource.resource_id) =>
        {
            let resulting = view
                .resources
                .values()
                .chain(std::iter::once(resource))
                .collect::<Vec<_>>();
            (view.lifecycle == ProjectLifecycle::Closed
                || project_resources_are_claimable(snapshot, mutation, &resulting))
            .then(|| SemanticPayload::ProjectResourceAdded {
                project_id: mutation.project_id,
                resource: resource.clone(),
                make_primary: *make_primary,
            })
        }
        CanonicalProjectMutationAction::RemoveResource { resource_id, force }
            if view.resources.contains_key(resource_id)
                && (view.assignment.is_none() || *force) =>
        {
            Some(SemanticPayload::ProjectResourceRemoved {
                project_id: mutation.project_id,
                resource_id: *resource_id,
                force: *force,
            })
        }
        CanonicalProjectMutationAction::ReplaceResource {
            old_resource_id,
            new_resource,
        } if valid_path_resource(new_resource)
            && view.resources.contains_key(old_resource_id)
            && (*old_resource_id == new_resource.resource_id
                || !view.resources.contains_key(&new_resource.resource_id)) =>
        {
            let resulting = view
                .resources
                .iter()
                .filter_map(|(resource_id, resource)| {
                    (*resource_id != *old_resource_id).then_some(resource)
                })
                .chain(std::iter::once(new_resource))
                .collect::<Vec<_>>();
            (view.lifecycle == ProjectLifecycle::Closed
                || project_resources_are_claimable(snapshot, mutation, &resulting))
            .then(|| SemanticPayload::ProjectResourceReplaced {
                project_id: mutation.project_id,
                old_resource_id: *old_resource_id,
                new_resource: new_resource.clone(),
            })
        }
        CanonicalProjectMutationAction::SetPrimaryResource { resource_id }
            if view.resources.contains_key(resource_id) && view.primary != Some(*resource_id) =>
        {
            Some(SemanticPayload::ProjectPrimaryResourceChanged {
                project_id: mutation.project_id,
                resource_id: *resource_id,
            })
        }
        _ => None,
    }
}

fn can_mutate_resources(view: &hq_reducer::ProjectView) -> bool {
    !view.archived && view.lifecycle != ProjectLifecycle::Closing
}

fn project_resources_are_claimable(
    snapshot: &DomainSnapshot,
    mutation: &CanonicalProjectMutation,
    resources: &[&ProjectResource],
) -> bool {
    snapshot
        .project()
        .projections()
        .iter()
        .all(|(key, projection)| {
            let ProjectProjection::Project(other) = projection else {
                return true;
            };
            let ProjectProjectionKey::Project(other_project_id) = key else {
                return false;
            };
            if !matches!(
                other.lifecycle,
                ProjectLifecycle::Open | ProjectLifecycle::Closing
            ) {
                return true;
            }
            resources.iter().all(|resource| {
                let requested = PathClaim {
                    project_id: mutation.project_id,
                    home: mutation.home,
                    resource: (*resource).clone(),
                };
                other.resources.values().all(|existing| {
                    claim_conflict(
                        &requested,
                        &PathClaim {
                            project_id: *other_project_id,
                            home: other.home,
                            resource: existing.clone(),
                        },
                    )
                    .is_none()
                })
            })
        })
}

fn can_make_runnable(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
    view: &hq_reducer::ProjectView,
    binding: &hq_domain::AssignmentBinding,
) -> bool {
    view.lifecycle == ProjectLifecycle::Open
        && view.claimable
        && agent_is_unassigned_elsewhere(snapshot, project_id, binding.agent_id)
        && view.assignment.as_ref().is_some_and(|assignment| {
            assignment.intent.assignment_id == binding.assignment_id
                && assignment.intent.agent_id == binding.agent_id
                && assignment.intent.provider == binding.provider
                && assignment.binding.is_none()
        })
}

fn accepted_input_parent(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
    input: &PendingProjectInput,
) -> Result<FactId, DomainError> {
    match snapshot
        .project()
        .projection(ProjectProjectionKey::Input(input.message_id))
    {
        Some(ProjectProjection::Input(candidate))
            if candidate.project_id == project_id
                && candidate.input_fact_id == input.input_fact_id
                && candidate.accepted_fact == input.accepted_fact
                && candidate.sequence == input.sequence.get()
                && matches!(
                    snapshot
                        .conversation()
                        .projection(ConversationProjectionKey::Message(input.message_id)),
                    Some(ConversationProjection::Message(message))
                        if message.fact_id == input.input_fact_id
                            && message.thread_id == input.thread_id
                            && message.content.project_id == Some(project_id)
                            && message.content.body == input.body
                ) =>
        {
            Ok(candidate.accepted_fact)
        }
        _ => Err(domain_error(
            ErrorCategory::Conflict,
            "project_input_changed",
        )),
    }
}

fn project_view(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
) -> Result<&hq_reducer::ProjectView, ApplicationError> {
    match snapshot
        .project()
        .projection(ProjectProjectionKey::Project(project_id))
    {
        Some(ProjectProjection::Project(view)) => Ok(view),
        _ => Err(ApplicationError::new(ApplicationErrorCode::ItemNotFound)),
    }
}

fn active_human_authority(
    snapshot: &DomainSnapshot,
    account_id: hq_domain::AccountId,
    home: InstallationId,
) -> Option<FactId> {
    if let Some(AuthorityProjection::Account {
        root_fact, creator, ..
    }) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Account(account_id))
        && creator.installation_id() == home
    {
        return Some(*root_fact);
    }
    match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Membership {
            account: account_id,
            device: home,
        }) {
        Some(AuthorityProjection::Membership(membership))
            if membership.state() == MembershipState::Active =>
        {
            membership.active_acceptances.iter().next().copied()
        }
        _ => None,
    }
}

fn active_local_agent(snapshot: &DomainSnapshot, home: InstallationId, agent_id: AgentId) -> bool {
    match snapshot
        .agent()
        .projection(AgentProjectionKey::Agent(agent_id))
    {
        Some(AgentProjection::Agent(agent)) => {
            agent.lifecycle == AgentLifecycle::Active
                && agent.mailboxes.len() == 1
                && agent
                    .mailboxes
                    .iter()
                    .all(|mailbox| mailbox.installation_id() == home)
        }
        _ => false,
    }
}

fn agent_is_unassigned_elsewhere(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
    agent_id: AgentId,
) -> bool {
    snapshot
        .project()
        .projections()
        .iter()
        .all(|(key, projection)| {
            !matches!(
                (key, projection),
                (
                    ProjectProjectionKey::Project(candidate_id),
                    ProjectProjection::Project(candidate)
                ) if *candidate_id != project_id
                    && candidate.assignment.as_ref().is_some_and(|assignment| {
                        assignment.intent.agent_id == agent_id
                    })
            )
        })
}

fn agent_is_unassigned(snapshot: &DomainSnapshot, agent_id: AgentId) -> bool {
    snapshot.project().projections().values().all(|projection| {
        !matches!(projection, ProjectProjection::Project(project)
        if project.assignment.as_ref().is_some_and(|assignment| {
            assignment.intent.agent_id == agent_id
        }))
    })
}

fn agent_claim(
    snapshot: &DomainSnapshot,
    home: InstallationId,
    agent_id: AgentId,
) -> Result<FactId, DomainError> {
    match snapshot
        .agent()
        .projection(AgentProjectionKey::Agent(agent_id))
    {
        Some(AgentProjection::Agent(agent))
            if agent.lifecycle == AgentLifecycle::Active
                && agent.mailboxes.len() == 1
                && agent
                    .mailboxes
                    .iter()
                    .all(|mailbox| mailbox.installation_id() == home) =>
        {
            agent
                .claims
                .iter()
                .next()
                .copied()
                .ok_or_else(|| domain_error(ErrorCategory::Conflict, "project_agent_claim_missing"))
        }
        _ => Err(domain_error(
            ErrorCategory::Conflict,
            "project_agent_unavailable",
        )),
    }
}

fn thread_root(
    snapshot: &DomainSnapshot,
    thread_id: hq_domain::ThreadId,
    project_id: ProjectId,
) -> Result<FactId, DomainError> {
    match snapshot
        .conversation()
        .projection(ConversationProjectionKey::Thread(thread_id))
    {
        Some(ConversationProjection::Thread(thread)) => {
            let valid = snapshot
                .conversation()
                .projections()
                .values()
                .any(|projection| {
                    matches!(projection, ConversationProjection::Message(message)
                    if message.thread_id == thread_id
                        && message.content.project_id == Some(project_id))
                });
            valid
                .then_some(thread.root_fact)
                .ok_or_else(|| domain_error(ErrorCategory::Conflict, "project_thread_mismatch"))
        }
        _ => Err(domain_error(
            ErrorCategory::NotFound,
            "project_thread_missing",
        )),
    }
}

#[allow(
    clippy::expect_used,
    reason = "all callers pass reviewed static error codes"
)]
fn domain_error(category: ErrorCategory, code: &'static str) -> DomainError {
    DomainError::new(
        category,
        ErrorCode::new(code).expect("static canonical project error code"),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::collections::{BTreeMap, BTreeSet};

    use hq_application::{DomainSnapshot, ProjectionSnapshot};
    use hq_domain::{
        AccountId, AgentId, AssignmentId, AssignmentIntent, AuthorityRole, BoundedText,
        CommandDigest, CommandId, EncryptionPublicKey, FactId, FactScope, InstallationAddress,
        InstallationId, MailboxAddress, MailboxId, ProjectId, ProjectResource, ProviderId,
        ResourceHealth, ResourceId, ResourceLocator, ResourceScheme, SemanticPayload, ShortText,
        SigningPublicKey, Timestamp,
    };
    use hq_reducer::{
        AgentLifecycle, AgentProjection, AgentProjectionKey, AgentView, AuthorityProjection,
        AuthorityProjectionKey, InstallationView, ProjectAssignmentPhase, ProjectAssignmentView,
        ProjectLifecycle, ProjectProjection, ProjectProjectionKey, ProjectView, SelectionView,
    };

    use super::{
        CanonicalProjectMutation, CanonicalProjectMutationAction, build_creation_plan,
        build_retirement_plan, payload, resource_payload, workflow_snapshot,
    };

    #[test]
    fn creation_has_no_previous_state_and_binds_home_and_active_human_authority() {
        let home = InstallationId::from_bytes([3; 32]);
        let account = AccountId::from_bytes([4; 32]);
        let installation_root = FactId::from_bytes([5; 32]);
        let human_root = FactId::from_bytes([6; 32]);
        let signing_key = SigningPublicKey::from_bytes([7; 32]);
        let snapshot = DomainSnapshot::new(
            ProjectionSnapshot::new(
                BTreeMap::new(),
                BTreeMap::from([
                    (
                        AuthorityProjectionKey::Installation(home),
                        AuthorityProjection::Installation(InstallationView {
                            root_fact: installation_root,
                            signing_key,
                            encryption_key: EncryptionPublicKey::from_bytes([8; 32]),
                            label: None,
                        }),
                    ),
                    (
                        AuthorityProjectionKey::Account(account),
                        AuthorityProjection::Account {
                            root_fact: human_root,
                            creator: InstallationAddress::new(home, signing_key),
                            label: None,
                        },
                    ),
                ]),
                BTreeMap::new(),
            ),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        );
        let project_id = ProjectId::from_bytes([9; 32]);
        let mailbox_id = MailboxId::from_bytes([10; 32]);
        let resource = resource(11, "/repo/worktree");
        let mut mutation = mutation(
            project_id,
            CanonicalProjectMutationAction::Create {
                mailbox_id,
                name: ShortText::new("created").expect("name"),
                brief: None,
                resource: resource.clone(),
            },
        );
        mutation.account_id = account;
        mutation.expected_head = None;

        let plan = build_creation_plan(
            &snapshot,
            &mutation,
            mailbox_id,
            &ShortText::new("created").expect("name"),
            None,
            &resource,
        )
        .expect("creation is authorized");
        assert_eq!(plan.causal().parents().iter().len(), 2);
        assert_eq!(plan.causal().authority(AuthorityRole::PreviousState), None);
        assert_eq!(
            plan.causal().authority(AuthorityRole::ProjectHome),
            Some(installation_root)
        );
        assert_eq!(
            plan.causal().authority(AuthorityRole::ActiveHuman),
            Some(human_root)
        );
        assert!(matches!(
            plan.payload(),
            SemanticPayload::ProjectCreated {
                project_id: created,
                primary: Some(primary),
                initial_state: hq_domain::InitialProjectState::Open,
                predecessor: None,
                ..
            } if *created == project_id && *primary == resource.resource_id
        ));
    }

    #[test]
    fn canonical_resource_policy_uses_the_complete_resulting_active_claim_set() {
        let current_id = ProjectId::from_bytes([1; 32]);
        let other_id = ProjectId::from_bytes([2; 32]);
        let old = resource(1, "/shared");
        let candidate = resource(2, "/shared/child");
        let current = view(current_id, ProjectLifecycle::Open, [old.clone()]);
        let other = view(other_id, ProjectLifecycle::Open, [candidate.clone()]);

        let open = mutation(current_id, CanonicalProjectMutationAction::Open);
        let closed = view(current_id, ProjectLifecycle::Closed, [old.clone()]);
        let closed_snapshot = project_snapshot(current_id, closed.clone(), other_id, other.clone());
        assert!(payload(&closed_snapshot, &open, &closed, &mut BTreeSet::new(),).is_err());

        let add = mutation(
            current_id,
            CanonicalProjectMutationAction::AddResource {
                resource: candidate.clone(),
                make_primary: false,
            },
        );
        let snapshot = project_snapshot(current_id, current.clone(), other_id, other.clone());
        assert!(resource_payload(&snapshot, &add, &current).is_none());

        let snapshot = project_snapshot(current_id, closed.clone(), other_id, other.clone());
        assert!(resource_payload(&snapshot, &add, &closed).is_some());

        let with_two = view(
            current_id,
            ProjectLifecycle::Open,
            [old.clone(), candidate.clone()],
        );
        let select_primary = mutation(
            current_id,
            CanonicalProjectMutationAction::SetPrimaryResource {
                resource_id: candidate.resource_id,
            },
        );
        let snapshot = project_snapshot(current_id, with_two.clone(), other_id, other.clone());
        assert!(matches!(
            resource_payload(&snapshot, &select_primary, &with_two),
            Some(SemanticPayload::ProjectPrimaryResourceChanged { resource_id, .. })
                if resource_id == candidate.resource_id
        ));

        let replacement = resource(3, "/independent");
        let replace = mutation(
            current_id,
            CanonicalProjectMutationAction::ReplaceResource {
                old_resource_id: old.resource_id,
                new_resource: replacement,
            },
        );
        let snapshot = project_snapshot(current_id, current.clone(), other_id, other);
        assert!(resource_payload(&snapshot, &replace, &current).is_some());

        let closing = view(other_id, ProjectLifecycle::Closing, [candidate]);
        let snapshot = project_snapshot(current_id, current.clone(), other_id, closing);
        assert!(resource_payload(&snapshot, &add, &current).is_none());
    }

    #[test]
    fn retirement_is_installation_private_frontier_complete_and_requires_no_assignment() {
        let project_id = ProjectId::from_bytes([1; 32]);
        let agent_id = AgentId::from_bytes([2; 32]);
        let home = InstallationId::from_bytes([3; 32]);
        let mailbox = MailboxAddress::new(home, MailboxId::from_bytes([4; 32]));
        let claim = FactId::from_bytes([5; 32]);
        let selection = FactId::from_bytes([6; 32]);
        let root = FactId::from_bytes([7; 32]);
        let mut project = view(project_id, ProjectLifecycle::Open, []);
        let snapshot = retirement_snapshot(
            project_id,
            project.clone(),
            agent_id,
            mailbox,
            claim,
            selection,
        );
        let mutation = mutation(
            project_id,
            CanonicalProjectMutationAction::RetireAgent { agent_id },
        );

        let plan =
            build_retirement_plan(&snapshot, &mutation, root, agent_id).expect("retirement plan");
        assert_eq!(plan.scope(), &FactScope::InstallationPrivate(home));
        assert_eq!(
            plan.payload(),
            &SemanticPayload::AgentRetired {
                agent_id,
                mailbox_id: mailbox.mailbox_id(),
            }
        );
        assert_eq!(
            plan.causal().authority(AuthorityRole::LocalInstallation),
            Some(root)
        );
        assert!(plan.causal().parents().contains(&claim));
        assert!(plan.causal().parents().contains(&selection));

        project.assignment = Some(ProjectAssignmentView {
            intent: AssignmentIntent {
                assignment_id: AssignmentId::from_bytes([8; 32]),
                agent_id,
                provider: ProviderId::new("provider").expect("provider"),
            },
            binding: None,
            phase: ProjectAssignmentPhase::Configuring,
            cardinality_conflicted: false,
            runnable: false,
            support: BTreeSet::new(),
        });
        let assigned =
            retirement_snapshot(project_id, project, agent_id, mailbox, claim, selection);
        let error = build_retirement_plan(&assigned, &mutation, root, agent_id)
            .expect_err("assigned retirement must fail");
        assert_eq!(error.code().as_str(), "project_agent_assigned");
    }

    #[test]
    fn assigned_agent_remains_available_to_its_own_project_retirement_workflow() {
        let project_id = ProjectId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([2; 32]);
        let agent_id = AgentId::from_bytes([3; 32]);
        let home = InstallationId::from_bytes([3; 32]);
        let mailbox = MailboxAddress::new(home, MailboxId::from_bytes([4; 32]));
        let mut project = view(project_id, ProjectLifecycle::Open, []);
        project.assignment = Some(ProjectAssignmentView {
            intent: AssignmentIntent {
                assignment_id: AssignmentId::from_bytes([8; 32]),
                agent_id,
                provider: ProviderId::new("provider").expect("provider"),
            },
            binding: None,
            phase: ProjectAssignmentPhase::Configuring,
            cardinality_conflicted: false,
            runnable: false,
            support: BTreeSet::new(),
        });
        let snapshot = retirement_snapshot(
            project_id,
            project,
            agent_id,
            mailbox,
            FactId::from_bytes([5; 32]),
            FactId::from_bytes([6; 32]),
        );

        let observed = workflow_snapshot(&snapshot, project_id, account_id, Some(agent_id))
            .expect("target project snapshot");
        assert!(observed.requested_agent_available);
        assert_eq!(
            observed
                .assignment
                .map(|assignment| assignment.intent.agent_id),
            Some(agent_id)
        );
    }

    fn retirement_snapshot(
        project_id: ProjectId,
        project: ProjectView,
        agent_id: AgentId,
        mailbox: MailboxAddress,
        claim: FactId,
        selection: FactId,
    ) -> DomainSnapshot {
        DomainSnapshot::new(
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(
                BTreeMap::new(),
                BTreeMap::from([
                    (
                        AgentProjectionKey::Agent(agent_id),
                        AgentProjection::Agent(Box::new(AgentView {
                            claims: BTreeSet::from([claim]),
                            names: BTreeSet::from([ShortText::new("agent").expect("name")]),
                            mailboxes: BTreeSet::from([mailbox]),
                            retirements: BTreeSet::new(),
                            lifecycle: AgentLifecycle::Active,
                            runnable: true,
                            selected_session: None,
                            name_reserved: true,
                        })),
                    ),
                    (
                        AgentProjectionKey::Selection(agent_id),
                        AgentProjection::Selection(Box::new(SelectionView {
                            candidates: BTreeMap::new(),
                            frontier: BTreeSet::from([selection]),
                            active: None,
                            conflicted: false,
                        })),
                    ),
                ]),
                BTreeMap::new(),
            ),
            ProjectionSnapshot::new(
                BTreeMap::new(),
                BTreeMap::from([(
                    ProjectProjectionKey::Project(project_id),
                    ProjectProjection::Project(Box::new(project)),
                )]),
                BTreeMap::new(),
            ),
        )
    }

    fn project_snapshot(
        current_id: ProjectId,
        current: ProjectView,
        other_id: ProjectId,
        other: ProjectView,
    ) -> DomainSnapshot {
        DomainSnapshot::new(
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(
                BTreeMap::new(),
                BTreeMap::from([
                    (
                        ProjectProjectionKey::Project(current_id),
                        ProjectProjection::Project(Box::new(current)),
                    ),
                    (
                        ProjectProjectionKey::Project(other_id),
                        ProjectProjection::Project(Box::new(other)),
                    ),
                ]),
                BTreeMap::new(),
            ),
        )
    }

    fn view<const N: usize>(
        project_id: ProjectId,
        lifecycle: ProjectLifecycle,
        resources: [ProjectResource; N],
    ) -> ProjectView {
        let resources = resources
            .into_iter()
            .map(|resource| (resource.resource_id, resource))
            .collect::<BTreeMap<_, _>>();
        ProjectView {
            root: FactId::from_bytes([10; 32]),
            head: FactId::from_bytes([11; 32]),
            fork_participants: BTreeSet::new(),
            home: InstallationId::from_bytes([3; 32]),
            account_id: AccountId::from_bytes([22; 32]),
            mailbox: MailboxAddress::new(
                InstallationId::from_bytes([3; 32]),
                MailboxId::from_bytes(*project_id.as_bytes()),
            ),
            predecessor: None,
            name: ShortText::new("project").expect("name"),
            brief: None,
            primary: resources.keys().next().copied(),
            resources,
            lifecycle,
            archived: false,
            active_claims: BTreeSet::new(),
            claim_conflicts: BTreeMap::new(),
            claimable: true,
            assignment: None,
            input_sequence: 0,
        }
    }

    fn mutation(
        project_id: ProjectId,
        action: CanonicalProjectMutationAction,
    ) -> CanonicalProjectMutation {
        CanonicalProjectMutation {
            command_id: CommandId::from_bytes([20; 32]),
            request_digest: CommandDigest::from_bytes([21; 32]),
            account_id: AccountId::from_bytes([22; 32]),
            project_id,
            home: InstallationId::from_bytes([3; 32]),
            expected_head: Some(FactId::from_bytes([11; 32])),
            issued_at: Timestamp::from_unix_millis(23),
            action,
        }
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
            health: ResourceHealth::Healthy,
        }
    }
}
