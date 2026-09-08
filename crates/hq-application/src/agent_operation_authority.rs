//! Canonical authorization and identity resolution for recipient-specific operation controls.

use std::collections::BTreeMap;

use hq_domain::{
    AccountId, ActivityKind, ActivityStatus, AgentId, FactId, InstallationId, MailboxAddress,
    OperationId,
};
use hq_reducer::{
    AuthorityProjection, AuthorityProjectionKey, ConversationProjection, MembershipState,
};

use crate::{
    AgentOperationCanonical, AgentOperationProject, AgentOperationQuery, AgentOperationScope,
    AgentOperationStatus, ApplicationError, ApplicationErrorCode, ClientAgentLifecycle,
    ClientProjectAssignmentPhase, ClientProjectLifecycle, ClientProjection, ConversationKey,
    DomainSnapshot,
};

/// Returns exact active-human authority at a home installation.
pub fn active_human_authority(
    snapshot: &DomainSnapshot,
    account: AccountId,
    home: InstallationId,
) -> Option<FactId> {
    if let Some(AuthorityProjection::Account {
        root_fact, creator, ..
    }) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Account(account))
        && creator.installation_id() == home
    {
        return Some(*root_fact);
    }
    match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Membership {
            account,
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

/// Resolves a conversation to canonical agent/session authority and exact turn evidence.
pub fn agent_operation_evidence(
    snapshot: &DomainSnapshot,
    query: &AgentOperationQuery,
) -> Result<AgentOperationCanonical, ApplicationError> {
    if active_human_authority(snapshot, query.account_id, query.home).is_none() {
        return Err(ApplicationError::new(
            ApplicationErrorCode::AuthorityRejected,
        ));
    }
    let items = snapshot
        .client_projections()
        .map_err(|_| ApplicationError::new(ApplicationErrorCode::StateCorrupt))?;
    let Some(scope) = resolve_scope(&items, query) else {
        return Ok(AgentOperationCanonical::default());
    };
    let mut operations: BTreeMap<OperationId, AgentOperationStatus> = BTreeMap::new();
    for projection in snapshot.conversation().projections().values() {
        let ConversationProjection::Activity(activity) = projection else {
            continue;
        };
        if activity.source != scope.mailbox
            || activity.kind != ActivityKind::AgentTurn
            || activity.correlation.provider() != &scope.provider
            || activity.correlation.session() != &scope.session
        {
            continue;
        }
        let status = AgentOperationStatus {
            operation_id: activity.correlation.operation(),
            sequence: activity.sequence,
            status: activity.status.clone(),
        };
        match operations.get(&status.operation_id) {
            Some(prior) if prior.sequence > status.sequence => {}
            Some(prior) if prior.sequence == status.sequence && prior != &status => {
                return Err(ApplicationError::new(
                    ApplicationErrorCode::StateIdentityConflict,
                ));
            }
            _ => {
                operations.insert(status.operation_id, status);
            }
        }
    }
    let mut running = operations
        .values()
        .filter(|status| status.status == ActivityStatus::Running);
    let first = running.next().cloned();
    let running = if running.next().is_none() {
        first
    } else {
        None
    };
    Ok(AgentOperationCanonical {
        scope: Some(scope),
        running,
        tracked: query
            .tracked_operation
            .and_then(|operation| operations.get(&operation).cloned()),
    })
}

fn resolve_scope(
    items: &[ClientProjection],
    query: &AgentOperationQuery,
) -> Option<AgentOperationScope> {
    match &query.conversation {
        ConversationKey::ProjectThread { project_id, thread } => {
            if !items.iter().any(|item| {
                matches!(item, ClientProjection::Project {
                project_id: actual, home, account_id, lifecycle: ClientProjectLifecycle::Open,
                archived: false, claimable: true, ..
            } if actual == project_id && *home == query.home && *account_id == query.account_id)
            }) {
                return None;
            }
            let assignment = items.iter().find_map(|item| match item {
                ClientProjection::ProjectAssignment { assignment }
                    if assignment.project_id == *project_id && assignment.runnable && !assignment.cardinality_conflicted
                        && matches!(&assignment.phase, ClientProjectAssignmentPhase::Runnable { thread_id, .. } if thread_id == thread) => Some(assignment),
                _ => None,
            })?;
            let mailbox = agent_mailbox(items, query.home, assignment.agent_id)?;
            Some(AgentOperationScope {
                agent_id: assignment.agent_id,
                mailbox,
                provider: assignment.provider.clone(),
                session: assignment.session.clone()?,
                project: Some(AgentOperationProject {
                    project_id: *project_id,
                    assignment_id: assignment.assignment_id,
                    thread_id: *thread,
                }),
            })
        }
        ConversationKey::ProviderSession {
            counterparty,
            provider,
            session,
        } => {
            let agent_id = items.iter().find_map(|item| match item {
                ClientProjection::Agent { agent_id, .. }
                    if agent_mailbox(items, query.home, *agent_id) == Some(*counterparty) =>
                {
                    Some(*agent_id)
                }
                _ => None,
            })?;
            let selected = items.iter().any(|item| matches!(item, ClientProjection::AgentSelection {
                agent_id: actual, selected: Some((selected_provider, selected_session)), conflicted: false, ..
            } if *actual == agent_id && selected_provider == provider && selected_session == session));
            let bound = items.iter().any(|item| matches!(item, ClientProjection::AgentSession {
                provider: actual_provider, session: actual_session, mailbox: Some(mailbox), conflicted: false, ..
            } if actual_provider == provider && actual_session == session && mailbox == counterparty));
            let assigned = items.iter().any(|item| matches!(item,
                ClientProjection::ProjectAssignment { assignment } if assignment.agent_id == agent_id));
            (selected && bound && !assigned).then(|| AgentOperationScope {
                agent_id,
                mailbox: *counterparty,
                provider: provider.clone(),
                session: session.clone(),
                project: None,
            })
        }
        ConversationKey::Thread { .. } => None,
    }
}

fn agent_mailbox(
    items: &[ClientProjection],
    home: InstallationId,
    agent: AgentId,
) -> Option<MailboxAddress> {
    items.iter().find_map(|item| match item {
        ClientProjection::Agent {
            agent_id,
            claims,
            mailboxes,
            lifecycle: ClientAgentLifecycle::Active,
            ..
        } if *agent_id == agent && claims.len() == 1 && mailboxes.len() == 1 => mailboxes
            .iter()
            .next()
            .copied()
            .filter(|mailbox| mailbox.installation_id() == home),
        _ => None,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use crate::{AgentOperationQuery, ClientAgentLifecycle, ClientProjection, ConversationKey};
    use hq_domain::{
        AccountId, AgentId, FactId, InstallationId, MailboxAddress, MailboxId, ProviderId,
        ProviderSessionId, ThreadId,
    };
    use std::collections::BTreeSet;

    fn direct_query() -> AgentOperationQuery {
        AgentOperationQuery {
            account_id: AccountId::from_bytes([1; 32]),
            home: InstallationId::from_bytes([2; 32]),
            conversation: ConversationKey::ProviderSession {
                counterparty: MailboxAddress::new(
                    InstallationId::from_bytes([2; 32]),
                    MailboxId::from_bytes([3; 32]),
                ),
                provider: ProviderId::new("worker").expect("provider"),
                session: ProviderSessionId::new("session").expect("session"),
            },
            tracked_operation: None,
        }
    }

    #[test]
    fn direct_control_requires_an_active_agent_and_exact_selected_binding() {
        let query = direct_query();
        let ConversationKey::ProviderSession {
            counterparty,
            provider,
            session,
        } = &query.conversation
        else {
            unreachable!()
        };
        let agent = AgentId::from_bytes([4; 32]);
        let mut items = vec![
            ClientProjection::Agent {
                agent_id: agent,
                claims: BTreeSet::from([FactId::from_bytes([5; 32])]),
                names: BTreeSet::new(),
                mailboxes: BTreeSet::from([*counterparty]),
                retirements: BTreeSet::new(),
                lifecycle: ClientAgentLifecycle::Active,
                runnable: true,
            },
            ClientProjection::AgentSession {
                provider: provider.clone(),
                session: session.clone(),
                bindings: Vec::new(),
                mailbox: Some(*counterparty),
                conflicted: false,
            },
            ClientProjection::AgentSelection {
                agent_id: agent,
                candidates: Vec::new(),
                selected: Some((provider.clone(), session.clone())),
                frontier: BTreeSet::new(),
                conflicted: false,
            },
        ];
        assert_eq!(
            super::resolve_scope(&items, &query).map(|scope| scope.agent_id),
            Some(agent)
        );
        items.pop();
        assert!(super::resolve_scope(&items, &query).is_none());
        items.clear();
        assert!(
            super::resolve_scope(&items, &query).is_none(),
            "a mailbox alone is not an agent"
        );
    }

    #[test]
    fn project_control_requires_current_assignment_home_and_account() {
        use crate::{
            ClientProjectAssignment, ClientProjectAssignmentPhase, ClientProjectLifecycle,
        };
        use hq_domain::{AssignmentId, ProjectId, ResourceLocator, ShortText};
        let mut query = direct_query();
        let project_id = ProjectId::from_bytes([10; 32]);
        let thread = ThreadId::from_bytes([11; 32]);
        query.conversation = ConversationKey::ProjectThread { project_id, thread };
        let agent_id = AgentId::from_bytes([12; 32]);
        let mailbox = MailboxAddress::new(query.home, MailboxId::from_bytes([13; 32]));
        let assignment_id = AssignmentId::from_bytes([14; 32]);
        let items = vec![
            ClientProjection::Project {
                project_id,
                home: query.home,
                account_id: query.account_id,
                mailbox,
                name: ShortText::new("Work").expect("name"),
                lifecycle: ClientProjectLifecycle::Open,
                archived: false,
                claimable: true,
                head: FactId::from_bytes([15; 32]),
                input_sequence: 0,
            },
            ClientProjection::Agent {
                agent_id,
                claims: BTreeSet::from([FactId::from_bytes([16; 32])]),
                names: BTreeSet::new(),
                mailboxes: BTreeSet::from([mailbox]),
                retirements: BTreeSet::new(),
                lifecycle: ClientAgentLifecycle::Active,
                runnable: false,
            },
            ClientProjection::ProjectAssignment {
                assignment: ClientProjectAssignment {
                    project_id,
                    assignment_id,
                    agent_id,
                    provider: ProviderId::new("worker").expect("provider"),
                    session: Some(ProviderSessionId::new("session").expect("session")),
                    phase: ClientProjectAssignmentPhase::Runnable {
                        thread_id: thread,
                        launch_directory: ResourceLocator::new(
                            hq_domain::ResourceScheme::WorkingTree,
                            hq_domain::BoundedText::new("/tmp/work").expect("directory"),
                        ),
                    },
                    cardinality_conflicted: false,
                    runnable: true,
                    support: BTreeSet::new(),
                },
            },
        ];
        let scope = super::resolve_scope(&items, &query).expect("project scope");
        assert_eq!(
            scope.project.expect("assignment").assignment_id,
            assignment_id
        );
        let mut other_account = query.clone();
        other_account.account_id = AccountId::from_bytes([17; 32]);
        assert!(super::resolve_scope(&items, &other_account).is_none());
        let mut other_home = query.clone();
        other_home.home = InstallationId::from_bytes([18; 32]);
        assert!(super::resolve_scope(&items, &other_home).is_none());
        let mut old_thread = query.clone();
        old_thread.conversation = ConversationKey::ProjectThread {
            project_id,
            thread: ThreadId::from_bytes([19; 32]),
        };
        assert!(super::resolve_scope(&items, &old_thread).is_none());
        let mut conflicted = items.clone();
        if let ClientProjection::ProjectAssignment { assignment } = &mut conflicted[2] {
            assignment.cardinality_conflicted = true;
        }
        assert!(super::resolve_scope(&conflicted, &query).is_none());
    }

    #[test]
    fn no_human_authority_cannot_query_even_an_empty_conversation() {
        assert!(
            super::agent_operation_evidence(&crate::DomainSnapshot::empty(), &direct_query())
                .is_err()
        );
    }

    #[test]
    fn a_human_thread_has_no_agent_control_capability() {
        let mut query = direct_query();
        query.conversation = ConversationKey::Thread {
            counterparty: MailboxAddress::new(query.home, MailboxId::from_bytes([3; 32])),
            thread: ThreadId::from_bytes([6; 32]),
        };
        assert!(super::resolve_scope(&[], &query).is_none());
    }
}
