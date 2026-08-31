//! Home-authored sequencing for ordinary project-addressed conversation messages.

use std::{collections::BTreeSet, num::NonZeroU64};

use hq_application::{
    ApplicationError, ApplicationErrorCode, CommitFacts, DomainSnapshot, FactMutation, FactPlan,
    LocalFactInputs, MutationAttempt, MutationDecision, MutationOutcome, ProjectCommandAction,
    ProjectCommandRequest, QueryDomain,
};
use hq_domain::{
    AccountId, AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, CommandDigest,
    CommandId, FactId, FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS,
    MessageId, OperationId, ProjectId, SemanticPayload,
};
use hq_reducer::{
    AuthorityProjection, AuthorityProjectionKey, ConversationProjection, ProjectAssignmentPhase,
    ProjectInputView, ProjectProjection, ProjectProjectionKey, ProjectView,
};
use sha2::{Digest, Sha256};

/// Exact immutable project input selected for the next home sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectInputAcceptanceRequest {
    /// Home installation that exclusively sequences the project.
    pub home: InstallationId,
    /// Human account carried by the project root and input scope.
    pub account_id: AccountId,
    /// Stable project identity.
    pub project_id: ProjectId,
    /// Stable public message identity.
    pub message_id: MessageId,
    /// Exact projected message-bearing fact.
    pub input_fact_id: FactId,
    /// Deterministic local authoring inputs.
    pub inputs: LocalFactInputs,
}

/// Result of one bounded home input reconciliation pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectInputReconciliation {
    /// Newly sequenced inputs.
    pub accepted: usize,
    /// Whether more candidates may remain after the requested bound.
    pub truncated: bool,
}

/// Bounded deterministic automatic-dispatch requests derived from authoritative pending input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomaticProjectDispatchPlan {
    /// At most one request per runnable project, ordered by stable project identity.
    pub requests: Vec<ProjectCommandRequest>,
    /// Whether additional runnable projects with pending input remain beyond the requested bound.
    pub truncated: bool,
}

/// Read-only capability for deriving durable automatic dispatch commands from canonical state.
pub trait PlanAutomaticProjectDispatches {
    /// Plans at most `limit` runnable projects without submitting provider work directly.
    fn plan_automatic_project_dispatches(
        &self,
        limit: usize,
    ) -> Result<AutomaticProjectDispatchPlan, ApplicationError>;
}

/// Home-side capability for sequencing ordinary project-addressed messages.
pub trait ReconcileProjectInputs {
    /// Reconciles at most `limit` currently usable unaccepted inputs.
    fn reconcile_project_inputs(
        &self,
        limit: usize,
    ) -> Result<ProjectInputReconciliation, ApplicationError>;
}

/// Stateless application adapter over transaction-consistent query and commit ports.
#[derive(Clone, Debug)]
pub struct ApplicationProjectInputReconciler<P> {
    ports: P,
    home: InstallationId,
}

impl<P> ApplicationProjectInputReconciler<P> {
    /// Binds an authoritative home and its application capabilities.
    pub const fn new(ports: P, home: InstallationId) -> Self {
        Self { ports, home }
    }
}

impl<P: QueryDomain + CommitFacts> ReconcileProjectInputs for ApplicationProjectInputReconciler<P> {
    fn reconcile_project_inputs(
        &self,
        limit: usize,
    ) -> Result<ProjectInputReconciliation, ApplicationError> {
        if limit == 0 {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
        }
        let mut accepted = 0;
        while accepted < limit {
            let snapshot = self.ports.authoritative_snapshot()?;
            let Some(request) = next_input(snapshot.domain(), self.home) else {
                return Ok(ProjectInputReconciliation {
                    accepted,
                    truncated: false,
                });
            };
            let (command_id, request_digest) = acceptance_identity(&request);
            let decision_request = request;
            let rejection = input_not_acceptable()?;
            let attempt = self.ports.commit_facts(FactMutation::new(
                command_id,
                request_digest,
                move |current| match plan_project_input_acceptance(current, decision_request) {
                    Ok(plan) => MutationDecision::commit(plan),
                    Err(_) => MutationDecision::reject(rejection),
                },
            ))?;
            match attempt {
                MutationAttempt::Completed(receipt)
                    if matches!(receipt.outcome(), MutationOutcome::Committed) =>
                {
                    accepted += 1;
                }
                MutationAttempt::Completed(_) | MutationAttempt::Uncertain { .. } => {
                    return Err(ApplicationError::new(
                        ApplicationErrorCode::AdapterUnavailable,
                    ));
                }
            }
        }
        let snapshot = self.ports.authoritative_snapshot()?;
        Ok(ProjectInputReconciliation {
            accepted,
            truncated: next_input(snapshot.domain(), self.home).is_some(),
        })
    }
}

impl<P: QueryDomain> PlanAutomaticProjectDispatches for ApplicationProjectInputReconciler<P> {
    fn plan_automatic_project_dispatches(
        &self,
        limit: usize,
    ) -> Result<AutomaticProjectDispatchPlan, ApplicationError> {
        let snapshot = self.ports.authoritative_snapshot()?;
        plan_automatic_project_dispatches(snapshot.domain(), self.home, limit)
    }
}

/// Derives stable dispatch commands for a bounded set of runnable projects with pending input.
pub fn plan_automatic_project_dispatches(
    snapshot: &DomainSnapshot,
    home: InstallationId,
    limit: usize,
) -> Result<AutomaticProjectDispatchPlan, ApplicationError> {
    if limit == 0 {
        return Err(invalid());
    }
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
        .collect::<BTreeSet<_>>();
    let projects = snapshot
        .project()
        .projections()
        .iter()
        .filter_map(|(key, projection)| match (key, projection) {
            (ProjectProjectionKey::Project(project_id), ProjectProjection::Project(project))
                if automatic_dispatch_runnable(project, home)
                    && active_human_authority(snapshot, project.account_id, home).is_some() =>
            {
                Some((*project_id, project.as_ref()))
            }
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut pending = std::collections::BTreeMap::<
        ProjectId,
        (&ProjectInputView, &hq_reducer::MessageView),
    >::new();
    for projection in snapshot.project().projections().values() {
        let ProjectProjection::Input(input) = projection else {
            continue;
        };
        let Some(project) = projects.get(&input.project_id) else {
            continue;
        };
        if dispatched.contains(&input.message_id) {
            continue;
        }
        let Some(ConversationProjection::Message(message)) =
            snapshot
                .conversation()
                .projection(hq_reducer::ConversationProjectionKey::Message(
                    input.message_id,
                ))
        else {
            continue;
        };
        if !eligible_project_message(message, input.project_id, project) {
            continue;
        }
        match pending.entry(input.project_id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((input, message));
            }
            std::collections::btree_map::Entry::Occupied(mut entry)
                if input.sequence < entry.get().0.sequence =>
            {
                entry.insert((input, message));
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    let truncated = pending.len() > limit;
    let requests = pending
        .into_iter()
        .take(limit)
        .map(|(project_id, (input, message))| {
            automatic_dispatch_request(project_id, projects[&project_id], input, message)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AutomaticProjectDispatchPlan {
        requests,
        truncated,
    })
}

fn automatic_dispatch_runnable(project: &ProjectView, home: InstallationId) -> bool {
    project.home == home
        && project.lifecycle == hq_reducer::ProjectLifecycle::Open
        && project.claimable
        && project.assignment.as_ref().is_some_and(|assignment| {
            assignment.runnable
                && !assignment.cardinality_conflicted
                && assignment.binding.is_some()
                && matches!(assignment.phase, ProjectAssignmentPhase::Runnable { .. })
        })
}

fn automatic_dispatch_request(
    project_id: ProjectId,
    project: &ProjectView,
    input: &ProjectInputView,
    message: &hq_reducer::MessageView,
) -> Result<ProjectCommandRequest, ApplicationError> {
    let command_id = CommandId::from_bytes(automatic_dispatch_identity(b"command", project, input));
    let operation_id =
        OperationId::from_bytes(automatic_dispatch_identity(b"operation", project, input));
    let mut request = ProjectCommandRequest {
        command_id,
        operation_id,
        request_digest: CommandDigest::from_bytes([0; 32]),
        account_id: project.account_id,
        project_id,
        home: project.home,
        expected_head: Some(project.head),
        issued_at: message.authored_at,
        action: ProjectCommandAction::DispatchPending,
    };
    request.request_digest =
        crate::command_codec::project_command_request_digest(&request).map_err(|_| invalid())?;
    Ok(request)
}

fn automatic_dispatch_identity(
    kind: &[u8],
    project: &ProjectView,
    input: &ProjectInputView,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"hq.project.automatic-dispatch.v1");
    digest.update(kind);
    digest.update(project.home.as_bytes());
    digest.update(input.project_id.as_bytes());
    digest.update(project.head.as_bytes());
    digest.update(input.message_id.as_bytes());
    digest.update(input.accepted_fact.as_bytes());
    digest.finalize().into()
}

fn eligible_project_message(
    message: &hq_reducer::MessageView,
    project_id: ProjectId,
    project: &ProjectView,
) -> bool {
    message.account_id == Some(project.account_id)
        && message.content.project_id == Some(project_id)
        && message.content.recipient == Some(project.mailbox)
        && message.content.purpose.is_project_input()
}

/// Plans the next contiguous acceptance against one serialized authoritative snapshot.
pub fn plan_project_input_acceptance(
    snapshot: &DomainSnapshot,
    request: ProjectInputAcceptanceRequest,
) -> Result<FactPlan, ApplicationError> {
    let Some(ProjectProjection::Project(project)) = snapshot
        .project()
        .projection(ProjectProjectionKey::Project(request.project_id))
    else {
        return Err(invalid());
    };
    let Some(ConversationProjection::Message(message)) =
        snapshot
            .conversation()
            .projection(hq_reducer::ConversationProjectionKey::Message(
                request.message_id,
            ))
    else {
        return Err(invalid());
    };
    if project.home != request.home
        || project.account_id != request.account_id
        || message.account_id != Some(request.account_id)
        || message.fact_id != request.input_fact_id
        || message.content.project_id != Some(request.project_id)
        || message.content.recipient != Some(project.mailbox)
        || !message.content.purpose.is_project_input()
        || snapshot
            .project()
            .projection(ProjectProjectionKey::Input(request.message_id))
            .is_some()
    {
        return Err(invalid());
    }
    let installation_root = match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(request.home))
    {
        Some(AuthorityProjection::Installation(installation)) => installation.root_fact,
        _ => return Err(invalid()),
    };
    let membership =
        active_human_authority(snapshot, request.account_id, request.home).ok_or_else(invalid)?;
    let parents = BoundedSet::<FactId, MAX_FACT_PARENTS>::new(BTreeSet::from([
        project.head,
        installation_root,
        membership,
        request.input_fact_id,
    ]))
    .map_err(|_| invalid())?;
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        parents,
        [
            AuthorityReference::new(AuthorityRole::PreviousState, project.head),
            AuthorityReference::new(AuthorityRole::ProjectHome, installation_root),
            AuthorityReference::new(AuthorityRole::AccountMembership, membership),
        ],
    )
    .map_err(|_| invalid())?;
    let sequence = project
        .input_sequence
        .checked_add(1)
        .and_then(NonZeroU64::new)
        .ok_or_else(invalid)?;
    Ok(FactPlan::new(
        request.home,
        request.inputs.authored_at,
        FactScope::AccountAddressed(request.account_id),
        causal,
        SemanticPayload::ProjectInputAccepted {
            project_id: request.project_id,
            message_id: request.message_id,
            input_fact_id: request.input_fact_id,
            sequence,
        },
        request.inputs.auxiliary_randomness,
    ))
}

fn next_input(
    snapshot: &DomainSnapshot,
    home: InstallationId,
) -> Option<ProjectInputAcceptanceRequest> {
    let accepted = snapshot
        .project()
        .projections()
        .keys()
        .filter_map(|key| match key {
            ProjectProjectionKey::Input(message_id) => Some(*message_id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let projects = snapshot
        .project()
        .projections()
        .iter()
        .filter_map(|(key, projection)| match (key, projection) {
            (ProjectProjectionKey::Project(project_id), ProjectProjection::Project(project))
                if project.home == home =>
            {
                Some((*project_id, project.as_ref()))
            }
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    snapshot
        .conversation()
        .projections()
        .values()
        .filter_map(|projection| match projection {
            ConversationProjection::Message(message)
                if !accepted.contains(&message.content.message_id)
                    && message.content.purpose.is_project_input() =>
            {
                let project_id = message.content.project_id?;
                let project = projects.get(&project_id)?;
                (message.content.recipient == Some(project.mailbox)).then(|| {
                    ProjectInputAcceptanceRequest {
                        home,
                        account_id: project.account_id,
                        project_id,
                        message_id: message.content.message_id,
                        input_fact_id: message.fact_id,
                        inputs: LocalFactInputs {
                            authored_at: message.authored_at,
                            auxiliary_randomness: acceptance_randomness(
                                project_id,
                                message.content.message_id,
                                message.fact_id,
                            ),
                        },
                    }
                })
            }
            _ => None,
        })
        .min_by_key(|request| request.input_fact_id)
}

fn acceptance_identity(request: &ProjectInputAcceptanceRequest) -> (CommandId, CommandDigest) {
    let identity = acceptance_randomness(
        request.project_id,
        request.message_id,
        request.input_fact_id,
    );
    let mut digest = Sha256::new();
    digest.update(b"hq.project.input.accept.digest.v1");
    digest.update(identity);
    digest.update(request.home.as_bytes());
    digest.update(request.account_id.as_bytes());
    digest.update(request.inputs.authored_at.as_unix_millis().to_be_bytes());
    digest.update(request.inputs.auxiliary_randomness);
    (
        CommandId::from_bytes(identity),
        CommandDigest::from_bytes(digest.finalize().into()),
    )
}

fn acceptance_randomness(
    project_id: ProjectId,
    message_id: MessageId,
    input_fact_id: FactId,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"hq.project.input.accept.v1");
    digest.update(project_id.as_bytes());
    digest.update(message_id.as_bytes());
    digest.update(input_fact_id.as_bytes());
    digest.finalize().into()
}

fn active_human_authority(
    snapshot: &DomainSnapshot,
    account_id: AccountId,
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
            if membership.state() == hq_reducer::MembershipState::Active =>
        {
            membership.active_acceptances.iter().next().copied()
        }
        _ => None,
    }
}

const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}

fn input_not_acceptable() -> Result<hq_domain::DomainError, ApplicationError> {
    hq_domain::ErrorCode::new("project_input_not_acceptable")
        .map(|code| hq_domain::DomainError::new(hq_domain::ErrorCategory::Conflict, code))
        .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvariantViolation))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

    use hq_application::{
        AuthoritativeSnapshot, CommitFacts, DomainSnapshot, MutationReceipt,
        ProjectProjectionSnapshot, ProjectionSnapshot, QueryDomain,
    };
    use hq_domain::{
        AgentId, AssignmentBinding, AssignmentId, AssignmentIntent, BoundedText, ContentText,
        EncryptionPublicKey, InstallationAddress, MailboxAddress, MailboxId, MessageContent,
        MessagePurpose, Page, PageCursor, PresentationKind, ProviderId, ProviderSessionId,
        ResourceLocator, ResourceScheme, Revision, ShortText, SigningPublicKey, ThreadId,
        Timestamp,
    };
    use hq_reducer::{
        AuthorityProjection, AuthorityProjectionKey, ConversationProjection,
        ConversationProjectionKey, InstallationView, MessageView, ProjectAssignmentPhase,
        ProjectAssignmentView, ProjectInputView, ProjectLifecycle, ProjectProjection,
        ProjectProjectionKey, ProjectView,
    };

    use crate::project_command_request_digest;

    use super::*;

    #[derive(Clone, Copy)]
    struct ProjectFixtureState {
        accepted: bool,
        runnable: bool,
    }

    fn snapshot(
        recipient: MailboxAddress,
        account_id: AccountId,
        message_account_id: AccountId,
        purpose: MessagePurpose,
        state: ProjectFixtureState,
    ) -> DomainSnapshot {
        let home = InstallationId::from_bytes([1; 32]);
        let installation_root = FactId::from_bytes([2; 32]);
        let project_id = ProjectId::from_bytes([3; 32]);
        let message_id = MessageId::from_bytes([4; 32]);
        let message_fact = FactId::from_bytes([5; 32]);
        let project_mailbox = MailboxAddress::new(home, MailboxId::from_bytes([6; 32]));
        DomainSnapshot::new(
            ProjectionSnapshot::new(
                BTreeMap::new(),
                BTreeMap::from([
                    (
                        AuthorityProjectionKey::Installation(home),
                        AuthorityProjection::Installation(InstallationView {
                            root_fact: installation_root,
                            signing_key: SigningPublicKey::from_bytes([7; 32]),
                            encryption_key: EncryptionPublicKey::from_bytes([8; 32]),
                            label: None,
                        }),
                    ),
                    (
                        AuthorityProjectionKey::Account(account_id),
                        AuthorityProjection::Account {
                            root_fact: FactId::from_bytes([9; 32]),
                            creator: InstallationAddress::new(
                                home,
                                SigningPublicKey::from_bytes([7; 32]),
                            ),
                            label: None,
                        },
                    ),
                ]),
                BTreeMap::new(),
            ),
            ProjectionSnapshot::new(
                BTreeMap::new(),
                BTreeMap::from([(
                    ConversationProjectionKey::Message(message_id),
                    ConversationProjection::Message(Box::new(MessageView {
                        fact_id: message_fact,
                        authored_at: Timestamp::from_unix_millis(14),
                        account_id: Some(message_account_id),
                        thread_id: ThreadId::from_bytes(*message_fact.as_bytes()),
                        content: MessageContent {
                            message_id,
                            sender: MailboxAddress::new(home, MailboxId::from_bytes([10; 32])),
                            recipient: Some(recipient),
                            body: ContentText::new("pending work").expect("body"),
                            purpose,
                            presentation: PresentationKind::Message,
                            correlation: None,
                            project_id: Some(project_id),
                        },
                        open: true,
                        rejected: false,
                        state_frontier: BTreeSet::new(),
                        peer_received_by: BTreeSet::new(),
                    })),
                )]),
                BTreeMap::new(),
            ),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            project_snapshot(
                home,
                account_id,
                project_mailbox,
                project_id,
                message_id,
                message_fact,
                state,
            ),
        )
    }

    fn project_snapshot(
        home: InstallationId,
        account_id: AccountId,
        mailbox: MailboxAddress,
        project_id: ProjectId,
        message_id: MessageId,
        input_fact_id: FactId,
        state: ProjectFixtureState,
    ) -> ProjectProjectionSnapshot {
        let agent_id = AgentId::from_bytes([17; 32]);
        let assignment_id = AssignmentId::from_bytes([18; 32]);
        let provider = ProviderId::new("codex").expect("provider");
        let assignment = state.runnable.then(|| ProjectAssignmentView {
            intent: AssignmentIntent {
                assignment_id,
                agent_id,
                provider: provider.clone(),
            },
            binding: Some(AssignmentBinding {
                assignment_id,
                agent_id,
                provider,
                session: ProviderSessionId::new("session").expect("session"),
            }),
            phase: ProjectAssignmentPhase::Runnable {
                thread_id: ThreadId::from_bytes(*input_fact_id.as_bytes()),
                launch_directory: ResourceLocator::new(
                    ResourceScheme::WorkingTree,
                    BoundedText::new("/work/project").expect("locator"),
                ),
            },
            cardinality_conflicted: false,
            runnable: true,
            support: BTreeSet::new(),
        });
        let mut projections = BTreeMap::from([(
            ProjectProjectionKey::Project(project_id),
            ProjectProjection::Project(Box::new(ProjectView {
                root: FactId::from_bytes([11; 32]),
                head: FactId::from_bytes([12; 32]),
                fork_participants: BTreeSet::new(),
                home,
                account_id,
                mailbox,
                predecessor: None,
                name: ShortText::new("project").expect("name"),
                brief: None,
                resources: BTreeMap::new(),
                primary: None,
                lifecycle: if state.runnable {
                    ProjectLifecycle::Open
                } else {
                    ProjectLifecycle::Closed
                },
                archived: !state.runnable,
                active_claims: BTreeSet::new(),
                claim_conflicts: BTreeMap::new(),
                claimable: true,
                assignment,
                input_sequence: 7,
            })),
        )]);
        if state.accepted {
            projections.insert(
                ProjectProjectionKey::Input(message_id),
                ProjectProjection::Input(Box::new(ProjectInputView {
                    project_id,
                    message_id,
                    input_fact_id,
                    sequence: 8,
                    accepted_fact: FactId::from_bytes([16; 32]),
                })),
            );
        }
        ProjectionSnapshot::new(BTreeMap::new(), projections, BTreeMap::new())
    }

    #[test]
    fn closed_unassigned_project_input_plans_the_next_home_sequence() {
        let home = InstallationId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([13; 32]);
        let project_mailbox = MailboxAddress::new(home, MailboxId::from_bytes([6; 32]));
        let snapshot = snapshot(
            project_mailbox,
            account_id,
            account_id,
            MessagePurpose::Asynchronous,
            ProjectFixtureState {
                accepted: false,
                runnable: false,
            },
        );
        let request = ProjectInputAcceptanceRequest {
            home,
            account_id,
            project_id: ProjectId::from_bytes([3; 32]),
            message_id: MessageId::from_bytes([4; 32]),
            input_fact_id: FactId::from_bytes([5; 32]),
            inputs: LocalFactInputs {
                authored_at: Timestamp::from_unix_millis(14),
                auxiliary_randomness: [15; 32],
            },
        };
        let plan = plan_project_input_acceptance(&snapshot, request).expect("acceptance plan");
        assert_eq!(plan.author(), home);
        assert_eq!(
            plan.authored_at(),
            Timestamp::from_unix_millis(14),
            "acceptance inherits the signed message time"
        );
        assert_eq!(plan.scope(), &FactScope::AccountAddressed(account_id));
        assert!(plan.causal().parents().contains(&request.input_fact_id));
        assert!(matches!(
            plan.payload(),
            SemanticPayload::ProjectInputAccepted {
                project_id,
                message_id,
                input_fact_id,
                sequence,
            } if *project_id == request.project_id
                && *message_id == request.message_id
                && *input_fact_id == request.input_fact_id
                && sequence.get() == 8
        ));
    }

    #[test]
    fn input_planner_fails_closed_for_a_different_project_mailbox() {
        let home = InstallationId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([13; 32]);
        let snapshot = snapshot(
            MailboxAddress::new(home, MailboxId::from_bytes([99; 32])),
            account_id,
            account_id,
            MessagePurpose::Asynchronous,
            ProjectFixtureState {
                accepted: false,
                runnable: false,
            },
        );
        assert!(
            plan_project_input_acceptance(
                &snapshot,
                ProjectInputAcceptanceRequest {
                    home,
                    account_id,
                    project_id: ProjectId::from_bytes([3; 32]),
                    message_id: MessageId::from_bytes([4; 32]),
                    input_fact_id: FactId::from_bytes([5; 32]),
                    inputs: LocalFactInputs {
                        authored_at: Timestamp::from_unix_millis(14),
                        auxiliary_randomness: [15; 32],
                    },
                },
            )
            .is_err()
        );
    }

    #[test]
    fn input_planner_fails_closed_for_a_different_account_scope() {
        let home = InstallationId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([13; 32]);
        let snapshot = snapshot(
            MailboxAddress::new(home, MailboxId::from_bytes([6; 32])),
            account_id,
            AccountId::from_bytes([14; 32]),
            MessagePurpose::Asynchronous,
            ProjectFixtureState {
                accepted: false,
                runnable: false,
            },
        );
        assert!(
            plan_project_input_acceptance(
                &snapshot,
                ProjectInputAcceptanceRequest {
                    home,
                    account_id,
                    project_id: ProjectId::from_bytes([3; 32]),
                    message_id: MessageId::from_bytes([4; 32]),
                    input_fact_id: FactId::from_bytes([5; 32]),
                    inputs: LocalFactInputs {
                        authored_at: Timestamp::from_unix_millis(14),
                        auxiliary_randomness: [15; 32],
                    },
                },
            )
            .is_err()
        );
    }

    #[test]
    fn project_output_is_neither_selected_nor_accepted_as_project_input() {
        let home = InstallationId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([13; 32]);
        let project_mailbox = MailboxAddress::new(home, MailboxId::from_bytes([6; 32]));
        let snapshot = snapshot(
            project_mailbox,
            account_id,
            account_id,
            MessagePurpose::ProjectOutput,
            ProjectFixtureState {
                accepted: false,
                runnable: false,
            },
        );
        assert_eq!(next_input(&snapshot, home), None);
        assert!(
            plan_project_input_acceptance(
                &snapshot,
                ProjectInputAcceptanceRequest {
                    home,
                    account_id,
                    project_id: ProjectId::from_bytes([3; 32]),
                    message_id: MessageId::from_bytes([4; 32]),
                    input_fact_id: FactId::from_bytes([5; 32]),
                    inputs: LocalFactInputs {
                        authored_at: Timestamp::from_unix_millis(14),
                        auxiliary_randomness: [15; 32],
                    },
                },
            )
            .is_err()
        );
    }

    #[test]
    fn runnable_pending_input_plans_one_stable_automatic_dispatch() {
        let home = InstallationId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([13; 32]);
        let snapshot = snapshot(
            MailboxAddress::new(home, MailboxId::from_bytes([6; 32])),
            account_id,
            account_id,
            MessagePurpose::Asynchronous,
            ProjectFixtureState {
                accepted: true,
                runnable: true,
            },
        );

        let first =
            plan_automatic_project_dispatches(&snapshot, home, 1).expect("automatic dispatch plan");
        let second = plan_automatic_project_dispatches(&snapshot, home, 1)
            .expect("stable automatic dispatch plan");
        assert_eq!(first, second);
        assert!(!first.truncated);
        assert_eq!(first.requests.len(), 1);
        let request = &first.requests[0];
        assert_eq!(request.account_id, account_id);
        assert_eq!(request.project_id, ProjectId::from_bytes([3; 32]));
        assert_eq!(request.home, home);
        assert_eq!(request.expected_head, Some(FactId::from_bytes([12; 32])));
        assert_eq!(request.issued_at, Timestamp::from_unix_millis(14));
        assert_eq!(request.action, ProjectCommandAction::DispatchPending);
        assert_eq!(
            project_command_request_digest(request).expect("request digest"),
            request.request_digest
        );
    }

    #[test]
    fn accepted_input_waits_without_dispatch_when_project_is_not_runnable() {
        let home = InstallationId::from_bytes([1; 32]);
        let account_id = AccountId::from_bytes([13; 32]);
        let snapshot = snapshot(
            MailboxAddress::new(home, MailboxId::from_bytes([6; 32])),
            account_id,
            account_id,
            MessagePurpose::Asynchronous,
            ProjectFixtureState {
                accepted: true,
                runnable: false,
            },
        );

        let plan =
            plan_automatic_project_dispatches(&snapshot, home, 1).expect("automatic dispatch plan");

        assert!(plan.requests.is_empty());
        assert!(!plan.truncated);
        assert!(
            snapshot
                .project()
                .projection(ProjectProjectionKey::Input(MessageId::from_bytes([4; 32])))
                .is_some()
        );
    }

    #[derive(Clone)]
    struct LossyPorts {
        state: Rc<RefCell<LossyState>>,
    }

    struct LossyState {
        accepted: bool,
        lose_first_response: bool,
        commands: Vec<(CommandId, CommandDigest)>,
    }

    impl QueryDomain for LossyPorts {
        fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError> {
            let state = self.state.borrow();
            let home = InstallationId::from_bytes([1; 32]);
            Ok(AuthoritativeSnapshot::new(
                Revision::new(u64::from(state.accepted)),
                snapshot(
                    MailboxAddress::new(home, MailboxId::from_bytes([6; 32])),
                    AccountId::from_bytes([13; 32]),
                    AccountId::from_bytes([13; 32]),
                    MessagePurpose::Asynchronous,
                    ProjectFixtureState {
                        accepted: state.accepted,
                        runnable: false,
                    },
                ),
            ))
        }

        fn conversation_entries(
            &self,
            _key: &hq_reducer::ConversationKey,
            _limit: usize,
            _cursor: Option<&PageCursor>,
        ) -> Result<Page<hq_application::ConversationEntry>, ApplicationError> {
            Ok(Page::new(Vec::new(), None))
        }
    }

    impl CommitFacts for LossyPorts {
        fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
            let (command_id, request_digest, decide) = request.into_parts();
            let current = {
                let state = self.state.borrow();
                snapshot(
                    MailboxAddress::new(
                        InstallationId::from_bytes([1; 32]),
                        MailboxId::from_bytes([6; 32]),
                    ),
                    AccountId::from_bytes([13; 32]),
                    AccountId::from_bytes([13; 32]),
                    MessagePurpose::Asynchronous,
                    ProjectFixtureState {
                        accepted: state.accepted,
                        runnable: false,
                    },
                )
            };
            assert!(matches!(decide(&current), MutationDecision::Commit(_)));
            let mut state = self.state.borrow_mut();
            state.commands.push((command_id, request_digest));
            if state.lose_first_response {
                state.lose_first_response = false;
                return Ok(MutationAttempt::Uncertain {
                    command_id,
                    request_digest,
                });
            }
            state.accepted = true;
            Ok(MutationAttempt::Completed(MutationReceipt::new(
                command_id,
                request_digest,
                Revision::new(1),
                MutationOutcome::Committed,
            )))
        }
    }

    #[test]
    fn response_loss_retries_the_exact_acceptance_identity_without_duplicate_input() {
        let state = Rc::new(RefCell::new(LossyState {
            accepted: false,
            lose_first_response: true,
            commands: Vec::new(),
        }));
        let reconciler = ApplicationProjectInputReconciler::new(
            LossyPorts {
                state: Rc::clone(&state),
            },
            InstallationId::from_bytes([1; 32]),
        );

        assert_eq!(
            reconciler
                .reconcile_project_inputs(1)
                .expect_err("lost response is uncertain")
                .code(),
            ApplicationErrorCode::AdapterUnavailable
        );
        assert_eq!(
            reconciler
                .reconcile_project_inputs(1)
                .expect("exact retry commits"),
            ProjectInputReconciliation {
                accepted: 1,
                truncated: false,
            }
        );
        let state = state.borrow();
        assert_eq!(state.commands.len(), 2);
        assert_eq!(state.commands[0], state.commands[1]);
        assert!(state.accepted);
    }
}
