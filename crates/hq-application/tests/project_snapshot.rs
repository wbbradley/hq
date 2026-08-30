//! Client-facing project assignment and historical-thread projection tests.

#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};

use hq_application::{
    ClientProjectAssignmentPhase, ClientProjection, DomainSnapshot, ProjectProjectionSnapshot,
    ProjectionSnapshot,
};
use hq_domain::{
    AccountId, AgentId, AssignmentBinding, AssignmentId, AssignmentIntent, ContentText, DispatchId,
    FactId, InstallationId, MailboxAddress, MailboxId, MessageContent, MessageId, MessagePurpose,
    PresentationKind, ProjectId, ProviderId, ProviderSessionId, ResourceLocator, ResourceScheme,
    ShortText, ThreadId, Timestamp,
};
use hq_reducer::{
    ConversationProjection, ConversationProjectionKey, MessageView, ProjectAssignmentPhase,
    ProjectAssignmentView, ProjectDispatchView, ProjectInputView, ProjectLifecycle,
    ProjectProjection, ProjectProjectionKey, ProjectView,
};

#[test]
fn client_project_snapshot_exposes_assignment_and_deduplicated_exact_threads() {
    let project_id = ProjectId::from_bytes([1; 32]);
    let agent_id = AgentId::from_bytes([2; 32]);
    let assignment_id = AssignmentId::from_bytes([3; 32]);
    let provider = ProviderId::new("fake").expect("provider");
    let session = ProviderSessionId::new("session-1").expect("session");
    let thread_id = ThreadId::from_bytes([4; 32]);
    let home = InstallationId::from_bytes([5; 32]);
    let directory = ResourceLocator::new(
        ResourceScheme::WorkingTree,
        hq_domain::BoundedText::new("/work/project").expect("directory"),
    );
    let binding = AssignmentBinding {
        assignment_id,
        agent_id,
        provider: provider.clone(),
        session: session.clone(),
    };
    let mut projections = BTreeMap::new();
    projections.insert(
        ProjectProjectionKey::Project(project_id),
        ProjectProjection::Project(Box::new(ProjectView {
            root: FactId::from_bytes([6; 32]),
            head: FactId::from_bytes([7; 32]),
            fork_participants: BTreeSet::new(),
            home,
            account_id: AccountId::from_bytes([8; 32]),
            mailbox: MailboxAddress::new(home, MailboxId::from_bytes([9; 32])),
            predecessor: None,
            name: ShortText::new("project").expect("name"),
            brief: None,
            resources: BTreeMap::new(),
            primary: None,
            lifecycle: ProjectLifecycle::Open,
            archived: false,
            active_claims: BTreeSet::new(),
            claim_conflicts: BTreeMap::new(),
            claimable: true,
            assignment: Some(ProjectAssignmentView {
                intent: AssignmentIntent {
                    assignment_id,
                    agent_id,
                    provider: provider.clone(),
                },
                binding: Some(binding.clone()),
                phase: ProjectAssignmentPhase::Runnable {
                    thread_id,
                    launch_directory: directory.clone(),
                },
                cardinality_conflicted: false,
                runnable: true,
                support: BTreeSet::from([FactId::from_bytes([10; 32])]),
            }),
            input_sequence: 2,
        })),
    );
    let mut conversation_projections = BTreeMap::new();
    for (message_byte, dispatch_byte, sequence) in [(11, 12, 1), (13, 14, 2)] {
        let message_id = MessageId::from_bytes([message_byte; 32]);
        let dispatch_id = DispatchId::from_bytes([dispatch_byte; 32]);
        conversation_projections.insert(
            ConversationProjectionKey::Message(message_id),
            ConversationProjection::Message(Box::new(MessageView {
                fact_id: FactId::from_bytes([message_byte + 20; 32]),
                authored_at: Timestamp::from_unix_millis(i64::from(message_byte)),
                account_id: Some(AccountId::from_bytes([8; 32])),
                thread_id: ThreadId::from_bytes([message_byte + 50; 32]),
                content: MessageContent {
                    message_id,
                    sender: MailboxAddress::new(home, MailboxId::from_bytes([9; 32])),
                    recipient: None,
                    body: ContentText::new("project input").expect("content"),
                    purpose: MessagePurpose::Asynchronous,
                    presentation: PresentationKind::Message,
                    correlation: None,
                    project_id: Some(project_id),
                },
                open: true,
                rejected: false,
                state_frontier: BTreeSet::new(),
                peer_received_by: BTreeSet::new(),
            })),
        );
        projections.insert(
            ProjectProjectionKey::Input(message_id),
            ProjectProjection::Input(Box::new(ProjectInputView {
                project_id,
                message_id,
                input_fact_id: FactId::from_bytes([message_byte + 20; 32]),
                sequence,
                accepted_fact: FactId::from_bytes([message_byte + 30; 32]),
            })),
        );
        projections.insert(
            ProjectProjectionKey::Dispatch(dispatch_id),
            ProjectProjection::Dispatch(Box::new(ProjectDispatchView {
                dispatch_id,
                message_id,
                sequence,
                binding: binding.clone(),
                thread_id,
                fact_id: FactId::from_bytes([dispatch_byte + 40; 32]),
                conflicted: false,
            })),
        );
    }
    let snapshot = DomainSnapshot::new(
        ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        ProjectionSnapshot::new(BTreeMap::new(), conversation_projections, BTreeMap::new()),
        ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        ProjectProjectionSnapshot::new(BTreeMap::new(), projections, BTreeMap::new()),
    );

    let client = snapshot.client_projections().expect("client projections");
    let assignments = client
        .iter()
        .filter_map(|projection| match projection {
            ClientProjection::ProjectAssignment { assignment } => Some(assignment),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].assignment_id, assignment_id);
    assert_eq!(assignments[0].session.as_ref(), Some(&session));
    assert!(matches!(
        &assignments[0].phase,
        ClientProjectAssignmentPhase::Runnable {
            thread_id: selected_thread,
            launch_directory,
        } if *selected_thread == thread_id && launch_directory == &directory
    ));
    assert!(assignments[0].runnable);

    let threads = client
        .iter()
        .filter_map(|projection| match projection {
            ClientProjection::ProjectThread { thread } => Some(thread),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].project_id, project_id);
    assert_eq!(threads[0].agent_id, agent_id);
    assert_eq!(threads[0].provider, provider);
    assert_eq!(threads[0].session, session);
    assert_eq!(threads[0].thread_id, thread_id);

    let inputs = client
        .iter()
        .filter_map(|projection| match projection {
            ClientProjection::ProjectInput {
                message_id,
                thread_id,
                sequence,
                ..
            } => Some((*message_id, *thread_id, *sequence)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(inputs.len(), 2);
    assert!(inputs.contains(&(
        MessageId::from_bytes([11; 32]),
        ThreadId::from_bytes([61; 32]),
        1,
    )));
    assert!(inputs.contains(&(
        MessageId::from_bytes([13; 32]),
        ThreadId::from_bytes([63; 32]),
        2,
    )));
}
