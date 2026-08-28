//! Message authoring planner contracts.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;

use hq_application::{
    LocalFactInputs, MessageAuthoringAuthority, MessageStateRequest, NewMessageRequest,
    ReplyRequest, ThreadCancellationRequest, plan_message_archive, plan_question, plan_reply,
    plan_thread_cancellation,
};
use hq_domain::{
    AuthorityReference, AuthorityRole, ContentText, FactId, FactScope, InstallationId,
    MailboxAddress, MailboxId, MessageContent, MessageId, MessagePurpose, PresentationKind,
    ProjectId, SemanticPayload, ThreadId, Timestamp,
};

fn fact(byte: u8) -> FactId {
    FactId::from_bytes([byte; 32])
}

fn message(byte: u8) -> MessageId {
    MessageId::from_bytes([byte; 32])
}

fn authority(sender_mailbox: u8) -> MessageAuthoringAuthority {
    let installation = InstallationId::from_bytes([1; 32]);
    MessageAuthoringAuthority {
        author: installation,
        sender: MailboxAddress::new(installation, MailboxId::from_bytes([sender_mailbox; 32])),
        scope: FactScope::InstallationPrivate(installation),
        authority: AuthorityReference::new(AuthorityRole::LocalInstallation, fact(2)),
        support: [fact(2), fact(sender_mailbox)].into_iter().collect(),
    }
}

fn inputs() -> LocalFactInputs {
    LocalFactInputs {
        authored_at: Timestamp::from_unix_millis(100),
        auxiliary_randomness: [9; 32],
    }
}

#[test]
fn question_plan_keeps_typed_content_and_complete_authority_support() {
    let sender = authority(3);
    let recipient = MailboxAddress::new(sender.author, MailboxId::from_bytes([4; 32]));
    let plan = plan_question(
        sender.clone(),
        inputs(),
        NewMessageRequest {
            message_id: message(5),
            recipient: Some(recipient),
            body: ContentText::new("Can you review this?").expect("bounded body"),
            presentation: PresentationKind::Message,
            project_id: None,
        },
    )
    .expect("valid local question");

    assert_eq!(plan.author(), sender.author);
    assert_eq!(plan.scope(), &sender.scope);
    assert_eq!(
        plan.causal()
            .parents()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
        sender.support
    );
    assert_eq!(
        plan.payload(),
        &SemanticPayload::QuestionAsked(MessageContent {
            message_id: message(5),
            sender: sender.sender,
            recipient: Some(recipient),
            body: ContentText::new("Can you review this?").expect("bounded body"),
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: None,
        })
    );
}

#[test]
fn reply_requires_the_recipient_mailbox_and_exact_root_derived_thread() {
    let question_sender = authority(3).sender;
    let answering = authority(4);
    let root_fact = fact(8);
    let root = MessageContent {
        message_id: message(5),
        sender: question_sender,
        recipient: Some(answering.sender),
        body: ContentText::new("Question").expect("bounded body"),
        purpose: MessagePurpose::Question,
        presentation: PresentationKind::Message,
        correlation: None,
        project_id: None,
    };
    let plan = plan_reply(
        answering.clone(),
        inputs(),
        ReplyRequest {
            thread_id: ThreadId::from_bytes(*root_fact.as_bytes()),
            root_fact,
            root: root.clone(),
            root_scope: answering.scope.clone(),
            message_id: message(9),
            body: ContentText::new("Answer").expect("bounded body"),
            presentation: PresentationKind::FinalAnswer,
        },
    )
    .expect("reversed local reply");
    assert!(plan.causal().parents().contains(&root_fact));
    assert!(
        matches!(plan.payload(), SemanticPayload::AnswerGiven { message, .. }
        if message.sender == answering.sender && message.recipient == Some(question_sender))
    );

    let mut wrong_thread = ThreadId::from_bytes([7; 32]);
    if wrong_thread == ThreadId::from_bytes(*root_fact.as_bytes()) {
        wrong_thread = ThreadId::from_bytes([6; 32]);
    }
    assert!(
        plan_reply(
            answering.clone(),
            inputs(),
            ReplyRequest {
                thread_id: wrong_thread,
                root_fact,
                root,
                root_scope: answering.scope.clone(),
                message_id: message(10),
                body: ContentText::new("No").expect("bounded body"),
                presentation: PresentationKind::Message,
            },
        )
        .is_err()
    );
}

#[test]
fn cancellation_and_archive_include_the_target_and_complete_state_frontier() {
    let sender = authority(3);
    let root_fact = fact(8);
    let root = MessageContent {
        message_id: message(5),
        sender: sender.sender,
        recipient: Some(authority(4).sender),
        body: ContentText::new("Question").expect("bounded body"),
        purpose: MessagePurpose::Question,
        presentation: PresentationKind::Message,
        correlation: None,
        project_id: None,
    };
    let cancellation = plan_thread_cancellation(
        sender.clone(),
        inputs(),
        ThreadCancellationRequest {
            thread_id: ThreadId::from_bytes(*root_fact.as_bytes()),
            root_fact,
            root,
            root_scope: sender.scope.clone(),
            reason: Some(ContentText::new("No longer needed").expect("bounded reason")),
        },
    )
    .expect("sender can cancel");
    assert!(cancellation.causal().parents().contains(&root_fact));

    let frontier = BTreeSet::from([fact(10), fact(11)]);
    let archive = plan_message_archive(
        sender,
        inputs(),
        MessageStateRequest {
            message_id: message(5),
            target_fact: root_fact,
            state_frontier: frontier.clone(),
        },
    )
    .expect("archive plan");
    assert!(archive.causal().parents().contains(&root_fact));
    assert!(
        frontier
            .iter()
            .all(|fact_id| archive.causal().parents().contains(fact_id))
    );
}

#[test]
fn local_planner_rejects_cross_installation_recipient() {
    let sender = authority(3);
    let remote = MailboxAddress::new(
        InstallationId::from_bytes([7; 32]),
        MailboxId::from_bytes([8; 32]),
    );
    assert!(
        plan_question(
            sender,
            inputs(),
            NewMessageRequest {
                message_id: message(5),
                recipient: Some(remote),
                body: ContentText::new("invalid route").expect("bounded body"),
                presentation: PresentationKind::Message,
                project_id: None,
            },
        )
        .is_err()
    );
}

#[test]
fn account_planner_allows_a_direct_recipient_only_for_typed_project_input() {
    let installation = InstallationId::from_bytes([1; 32]);
    let sender = MailboxAddress::new(installation, MailboxId::from_bytes([3; 32]));
    let recipient = MailboxAddress::new(
        InstallationId::from_bytes([7; 32]),
        MailboxId::from_bytes([8; 32]),
    );
    let authority = MessageAuthoringAuthority {
        author: installation,
        sender,
        scope: FactScope::AccountAddressed(hq_domain::AccountId::from_bytes([4; 32])),
        authority: AuthorityReference::new(AuthorityRole::AccountMembership, fact(2)),
        support: [fact(2), fact(3)].into_iter().collect(),
    };
    let project_id = ProjectId::from_bytes([6; 32]);
    let request = NewMessageRequest {
        message_id: message(5),
        recipient: Some(recipient),
        body: ContentText::new("queued project work").expect("bounded body"),
        presentation: PresentationKind::Message,
        project_id: Some(project_id),
    };
    let plan =
        hq_application::plan_asynchronous_message(authority.clone(), inputs(), request.clone())
            .expect("typed project input");
    assert!(matches!(
        plan.payload(),
        SemanticPayload::AsynchronousMessageSent(content)
            if content.recipient == Some(recipient) && content.project_id == Some(project_id)
    ));

    let mut projectless = request.clone();
    projectless.project_id = None;
    assert!(
        hq_application::plan_asynchronous_message(authority.clone(), inputs(), projectless,)
            .is_err()
    );

    let mut unaddressed_project = request;
    unaddressed_project.recipient = None;
    assert!(
        hq_application::plan_asynchronous_message(authority, inputs(), unaddressed_project)
            .is_err()
    );
}
