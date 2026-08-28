//! Transaction-consistent canonical project mutation adapter over application ports.

use std::collections::{BTreeSet, HashSet};

use hq_application::{
    ApplicationError, ApplicationErrorCode, CommitFacts, DomainSnapshot, FactMutation, FactPlan,
    MutationAttempt, MutationDecision, MutationOutcome, QueryDomain,
};
use hq_domain::{
    AgentId, AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, DomainError,
    ErrorCategory, ErrorCode, FactId, FactScope, InstallationId, MAX_FACT_AUTHORITIES,
    MAX_FACT_PARENTS, ProjectId, ProjectResource, SemanticPayload,
};
use hq_reducer::{
    AgentLifecycle, AgentProjection, AgentProjectionKey, AuthorityProjection,
    AuthorityProjectionKey, ConversationProjection, ConversationProjectionKey, MembershipState,
    ProjectAssignmentPhase, ProjectLifecycle, ProjectProjection, ProjectProjectionKey,
};
use hq_resources::{PathClaim, claim_conflict, valid_path_resource};

use crate::{
    CanonicalProjectAssignment, CanonicalProjectLifecycle, CanonicalProjectMutation,
    CanonicalProjectMutationAction, CanonicalProjectMutationOutcome, CanonicalProjectPort,
    PendingProjectInput, ProjectWorkflowSnapshot,
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
    let historical_threads = snapshot
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
        .collect();
    let assignment = view.assignment.as_ref().map(|assignment| {
        let (thread_id, phase_runnable) = match assignment.phase {
            ProjectAssignmentPhase::Runnable { thread_id, .. } => (Some(thread_id), true),
            ProjectAssignmentPhase::Configuring | ProjectAssignmentPhase::Blocked(_) => {
                (None, false)
            }
        };
        CanonicalProjectAssignment {
            intent: assignment.intent.clone(),
            binding: assignment.binding.clone(),
            thread_id,
            runnable: assignment.runnable && phase_runnable,
        }
    });
    let requested_agent_available = requested_agent.is_none_or(|agent_id| {
        active_local_agent(snapshot, view.home, agent_id)
            && snapshot.project().projections().values().all(|projection| {
                !matches!(projection, ProjectProjection::Project(project)
                    if project.assignment.as_ref().is_some_and(|assignment|
                        assignment.intent.agent_id == agent_id))
            })
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
    let view = project_view(snapshot, mutation.project_id)
        .map_err(|_| domain_error(ErrorCategory::NotFound, "project_not_found"))?;
    if view.home != mutation.home {
        return Err(domain_error(
            ErrorCategory::Unauthorized,
            "project_wrong_home",
        ));
    }
    if view.head != mutation.expected_head {
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
        CanonicalProjectMutationAction::EndAssignment { assignment_id }
            if view.assignment.as_ref().is_some_and(|assignment| {
                assignment.intent.assignment_id == *assignment_id
            }) =>
        {
            Ok(SemanticPayload::ProjectAssignmentEnded {
                project_id: mutation.project_id,
                assignment_id: *assignment_id,
                forced: false,
                runtime: None,
            })
        }
        CanonicalProjectMutationAction::BeginClosing
            if view.lifecycle == ProjectLifecycle::Open && view.assignment.is_none() =>
        {
            Ok(SemanticPayload::ProjectClosingStarted {
                project_id: mutation.project_id,
            })
        }
        CanonicalProjectMutationAction::FinishClosing
            if view.lifecycle == ProjectLifecycle::Closing && view.assignment.is_none() =>
        {
            Ok(SemanticPayload::ProjectClosed {
                project_id: mutation.project_id,
                forced: false,
                runtime: None,
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
        AccountId, BoundedText, CommandDigest, CommandId, FactId, InstallationId, MailboxAddress,
        MailboxId, ProjectId, ProjectResource, ResourceHealth, ResourceId, ResourceLocator,
        ResourceScheme, ShortText, Timestamp,
    };
    use hq_reducer::{ProjectLifecycle, ProjectProjection, ProjectProjectionKey, ProjectView};

    use super::{
        CanonicalProjectMutation, CanonicalProjectMutationAction, payload, resource_payload,
    };

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
            expected_head: FactId::from_bytes([11; 32]),
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
