//! Authoritative mailbox-command reconciliation and draft-consumption contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{collections::BTreeSet, sync::Arc};

use hq_application::{
    ControlMailbox, LocalFactInputs, MailboxCommandAction, MailboxCommandRequest,
    MailboxDraftSaveOutcome, MailboxDraftSaveRequest, MailboxDraftTarget,
    MessageAuthoringAuthority, MutationAttempt, MutationOutcome, NewMessageRequest, QueryDomain,
    plan_question,
};
use hq_domain::{
    AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, CommandDigest, CommandId,
    FactId, FactScope, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxAddress, MailboxId,
    MailboxKind, MessageId, OperationId, PresentationKind, ProjectId, SemanticPayload, Timestamp,
};
use hq_protocol::CanonicalEventPlan;
use hq_store::StoreGateway;

mod support;

use support::{TestDirectory, TestStoreExt, authority_policy, open_store, signer, verified_fact};

#[test]
fn committed_submission_consumes_draft_and_replays_exact_receipt() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    seed_human_authority(&store);
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));
    let draft_id = OperationId::from_bytes([0x71; 32]);
    let saved = gateway
        .save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id,
            target: MailboxDraftTarget::SelfNote,
            content: "durable note".to_owned(),
            expected_version: None,
        })
        .expect("draft saves");
    assert!(matches!(saved, MailboxDraftSaveOutcome::Saved(_)));
    let request = self_note_request(draft_id, [0x81; 32], [0x82; 32]);

    let first = gateway
        .control_mailbox(request.clone())
        .expect("mailbox command commits");
    let MutationAttempt::Completed(first_receipt) = first else {
        panic!("command is definite");
    };
    assert_eq!(first_receipt.outcome(), &MutationOutcome::Committed);
    assert!(gateway.mailbox_drafts().expect("drafts load").is_empty());

    let replay = gateway
        .control_mailbox(request)
        .expect("exact replay reconciles");
    assert_eq!(replay, MutationAttempt::Completed(first_receipt));
    assert!(gateway.mailbox_drafts().expect("drafts load").is_empty());

    let conflict = gateway
        .control_mailbox(self_note_request(draft_id, [0x81; 32], [0x83; 32]))
        .expect_err("changed request under one identity conflicts");
    assert_eq!(
        conflict.code(),
        hq_application::ApplicationErrorCode::CommandIdentityConflict
    );
}

#[test]
fn stale_target_rejection_preserves_recoverable_text() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    seed_human_authority(&store);
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));
    let draft_id = OperationId::from_bytes([0x91; 32]);
    let stale = MailboxAddress::new(
        authority_policy().local_installation(),
        MailboxId::from_bytes([0x92; 32]),
    );
    gateway
        .save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id,
            target: MailboxDraftTarget::Direct { recipient: stale },
            content: "do not lose this".to_owned(),
            expected_version: None,
        })
        .expect("draft saves");
    let attempt = gateway
        .control_mailbox(MailboxCommandRequest {
            command_id: CommandId::from_bytes([0x93; 32]),
            request_digest: CommandDigest::from_bytes([0x94; 32]),
            draft_id: Some(draft_id),
            action: MailboxCommandAction::Direct {
                recipient: stale,
                message_id: MessageId::from_bytes([0x95; 32]),
            },
            content: None,
            authored_at: Timestamp::from_unix_millis(5),
            auxiliary_randomness: [0x96; 32],
        })
        .expect("domain rejection is a retained result");
    let MutationAttempt::Completed(receipt) = attempt else {
        panic!("rejection is definite");
    };
    assert!(
        matches!(receipt.outcome(), MutationOutcome::Rejected(error) if error.category() == hq_domain::ErrorCategory::NotFound)
    );
    let drafts = gateway.mailbox_drafts().expect("draft remains loadable");
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].content, "do not lose this");

    let project_draft = OperationId::from_bytes([0x97; 32]);
    let missing_project = ProjectId::from_bytes([0x98; 32]);
    gateway
        .save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id: project_draft,
            target: MailboxDraftTarget::Project {
                project_id: missing_project,
                thread_id: None,
            },
            content: "keep this project message too".to_owned(),
            expected_version: None,
        })
        .expect("project draft saves");
    let attempt = gateway
        .control_mailbox(MailboxCommandRequest {
            command_id: CommandId::from_bytes([0x99; 32]),
            request_digest: CommandDigest::from_bytes([0x9a; 32]),
            draft_id: Some(project_draft),
            action: MailboxCommandAction::Project {
                project_id: missing_project,
                thread_id: None,
                message_id: MessageId::from_bytes([0x9b; 32]),
            },
            content: None,
            authored_at: Timestamp::from_unix_millis(6),
            auxiliary_randomness: [0x9c; 32],
        })
        .expect("stale project rejection is retained");
    assert!(matches!(
        attempt,
        MutationAttempt::Completed(receipt)
            if matches!(receipt.outcome(), MutationOutcome::Rejected(error)
                if error.category() == hq_domain::ErrorCategory::NotFound)
    ));
    assert!(
        gateway
            .mailbox_drafts()
            .expect("project draft remains loadable")
            .iter()
            .any(|draft| draft.draft_id == project_draft
                && draft.content == "keep this project message too")
    );
}

#[test]
fn direct_target_and_conversation_archive_are_resolved_from_transaction_state() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    seed_human_authority(&store);
    let (agent, _) = seed_agent_mailbox(&store);
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));

    let direct_draft = OperationId::from_bytes([0xc1; 32]);
    gateway
        .save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id: direct_draft,
            target: MailboxDraftTarget::Direct { recipient: agent },
            content: "hello agent".to_owned(),
            expected_version: None,
        })
        .expect("direct draft saves");
    let direct = gateway
        .control_mailbox(MailboxCommandRequest {
            command_id: CommandId::from_bytes([0xc2; 32]),
            request_digest: CommandDigest::from_bytes([0xc3; 32]),
            draft_id: Some(direct_draft),
            action: MailboxCommandAction::Direct {
                recipient: agent,
                message_id: MessageId::from_bytes([0xc4; 32]),
            },
            content: None,
            authored_at: Timestamp::from_unix_millis(3),
            auxiliary_randomness: [0xc5; 32],
        })
        .expect("direct command resolves");
    assert!(
        matches!(direct, MutationAttempt::Completed(receipt) if receipt.outcome() == &MutationOutcome::Committed)
    );
    let conversation = gateway
        .authoritative_snapshot()
        .expect("conversation snapshot loads")
        .conversations()
        .iter()
        .find(|conversation| {
            matches!(
                conversation.context,
                hq_application::ConversationContext::Direct { .. }
            )
        })
        .expect("direct conversation is indexed")
        .clone();
    let archive = gateway
        .control_mailbox(MailboxCommandRequest {
            command_id: CommandId::from_bytes([0xc6; 32]),
            request_digest: CommandDigest::from_bytes([0xc7; 32]),
            draft_id: None,
            action: MailboxCommandAction::ArchiveConversation {
                conversation: conversation.key,
            },
            content: None,
            authored_at: Timestamp::from_unix_millis(4),
            auxiliary_randomness: [0xc8; 32],
        })
        .expect("whole-conversation archive commits");
    assert!(matches!(
        archive,
        MutationAttempt::Completed(receipt)
            if receipt.outcome() == &MutationOutcome::Committed
    ));
    assert!(
        gateway
            .authoritative_snapshot()
            .expect("archived snapshot loads")
            .conversations()
            .iter()
            .any(|candidate| candidate.archived)
    );
}

#[test]
fn reply_target_is_resolved_from_transaction_state() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    seed_human_authority(&store);
    let (agent, agent_fact) = seed_agent_mailbox(&store);
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));

    let root_id = FactId::from_bytes(verified_fact().verified_event().event_id());
    let human = MailboxAddress::new(
        authority_policy().local_installation(),
        authority_policy().local_human_mailbox(),
    );
    let question_id = MessageId::from_bytes([0xd1; 32]);
    let question = plan_question(
        MessageAuthoringAuthority {
            author: authority_policy().local_installation(),
            sender: agent,
            scope: FactScope::InstallationPrivate(authority_policy().local_installation()),
            authority: AuthorityReference::new(AuthorityRole::LocalInstallation, root_id),
            support: BTreeSet::from([root_id, agent_fact]),
        },
        LocalFactInputs {
            authored_at: Timestamp::from_unix_millis(4),
            auxiliary_randomness: [0xd2; 32],
        },
        NewMessageRequest {
            message_id: question_id,
            recipient: Some(human),
            body: hq_domain::ContentText::new("question").expect("body"),
            presentation: PresentationKind::Message,
            project_id: None,
        },
    )
    .expect("question plans");
    let (author, time, scope, causal, payload, randomness) = question.into_parts();
    let question = CanonicalEventPlan::new(author, time, scope, causal, payload)
        .sign(&signer(1), randomness)
        .expect("question signs");
    store.append_verified(question).expect("question commits");

    let reply_draft = OperationId::from_bytes([0xd3; 32]);
    gateway
        .save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id: reply_draft,
            target: MailboxDraftTarget::Reply {
                message_id: question_id,
            },
            content: "answer".to_owned(),
            expected_version: None,
        })
        .expect("reply draft saves");
    let reply = gateway
        .control_mailbox(MailboxCommandRequest {
            command_id: CommandId::from_bytes([0xd4; 32]),
            request_digest: CommandDigest::from_bytes([0xd5; 32]),
            draft_id: Some(reply_draft),
            action: MailboxCommandAction::Reply {
                target_message: question_id,
                message_id: MessageId::from_bytes([0xd6; 32]),
            },
            content: None,
            authored_at: Timestamp::from_unix_millis(5),
            auxiliary_randomness: [0xd7; 32],
        })
        .expect("reply resolves exact root and frontier");
    assert!(
        matches!(reply, MutationAttempt::Completed(receipt) if receipt.outcome() == &MutationOutcome::Committed)
    );
    assert!(gateway.mailbox_drafts().expect("drafts load").is_empty());
}

fn self_note_request(
    draft_id: OperationId,
    command: [u8; 32],
    digest: [u8; 32],
) -> MailboxCommandRequest {
    MailboxCommandRequest {
        command_id: CommandId::from_bytes(command),
        request_digest: CommandDigest::from_bytes(digest),
        draft_id: Some(draft_id),
        action: MailboxCommandAction::SelfNote {
            message_id: MessageId::from_bytes([0xa1; 32]),
        },
        content: None,
        authored_at: Timestamp::from_unix_millis(4),
        auxiliary_randomness: [0xa2; 32],
    }
}

fn seed_human_authority(store: &hq_store::Store) {
    let root = verified_fact();
    let root_id = FactId::from_bytes(root.verified_event().event_id());
    store.append_verified(root).expect("installation commits");
    let policy = authority_policy();
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new(BTreeSet::from([root_id])).expect("one parent"),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root_id,
        )],
    )
    .expect("authority references validate");
    let human = CanonicalEventPlan::new(
        policy.local_installation(),
        Timestamp::from_unix_millis(1),
        FactScope::InstallationPrivate(policy.local_installation()),
        causal,
        SemanticPayload::MailboxCreated {
            mailbox_id: policy.local_human_mailbox(),
            kind: MailboxKind::Human,
            label: None,
        },
    )
    .sign(&signer(1), [0xb1; 32])
    .expect("human mailbox signs");
    store.append_verified(human).expect("human mailbox commits");
}

fn seed_agent_mailbox(store: &hq_store::Store) -> (MailboxAddress, FactId) {
    let policy = authority_policy();
    let root_id = FactId::from_bytes(verified_fact().verified_event().event_id());
    let mailbox_id = MailboxId::from_bytes([0x44; 32]);
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new(BTreeSet::from([root_id])).expect("one parent"),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root_id,
        )],
    )
    .expect("authority references validate");
    let agent = CanonicalEventPlan::new(
        policy.local_installation(),
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(policy.local_installation()),
        causal,
        SemanticPayload::MailboxCreated {
            mailbox_id,
            kind: MailboxKind::Agent,
            label: None,
        },
    )
    .sign(&signer(1), [0xb2; 32])
    .expect("agent mailbox signs");
    let fact_id = agent.fact().id();
    store.append_verified(agent).expect("agent mailbox commits");
    (
        MailboxAddress::new(policy.local_installation(), mailbox_id),
        fact_id,
    )
}

#[test]
fn conversation_composition_keeps_successive_direct_messages_and_notes_in_the_same_thread() {
    for personal in [true, false] {
        let directory = TestDirectory::new();
        let store = open_store(&directory.database_path());
        seed_human_authority(&store);
        let (agent, _) = seed_agent_mailbox(&store);
        let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));
        let draft_id = OperationId::from_bytes([0xe1; 32]);
        gateway
            .save_mailbox_draft(MailboxDraftSaveRequest {
                draft_id,
                target: if personal {
                    MailboxDraftTarget::SelfNote
                } else {
                    MailboxDraftTarget::Direct { recipient: agent }
                },
                content: "first note".to_owned(),
                expected_version: None,
            })
            .expect("draft saves");
        let mut first_request = self_note_request(draft_id, [0xe2; 32], [0xe3; 32]);
        if !personal {
            first_request.action = MailboxCommandAction::Direct {
                recipient: agent,
                message_id: MessageId::from_bytes([0xa1; 32]),
            };
        }
        let first = gateway.control_mailbox(first_request).expect("first send");
        assert!(
            matches!(first, MutationAttempt::Completed(ref receipt) if receipt.outcome() == &MutationOutcome::Committed)
        );
        let snapshot = gateway.authoritative_snapshot().expect("snapshot");
        assert_eq!(snapshot.conversations().len(), 1);
        let conversation = snapshot.conversations()[0].key.clone();
        for serial in [0xe4, 0xe5] {
            let draft_id = OperationId::from_bytes([serial; 32]);
            gateway
                .save_mailbox_draft(MailboxDraftSaveRequest {
                    draft_id,
                    target: MailboxDraftTarget::Conversation {
                        conversation: conversation.clone(),
                    },
                    content: format!("continuation {serial}"),
                    expected_version: None,
                })
                .expect("conversation draft saves");
            let request = MailboxCommandRequest {
                command_id: CommandId::from_bytes([serial; 32]),
                request_digest: CommandDigest::from_bytes([serial; 32]),
                draft_id: Some(draft_id),
                action: MailboxCommandAction::Conversation {
                    conversation: conversation.clone(),
                    message_id: MessageId::from_bytes([serial; 32]),
                },
                content: None,
                authored_at: Timestamp::from_unix_millis(i64::from(serial)),
                auxiliary_randomness: [serial; 32],
            };
            let result = gateway
                .control_mailbox(request.clone())
                .expect("conversation send");
            assert!(
                matches!(result, MutationAttempt::Completed(ref receipt) if receipt.outcome() == &MutationOutcome::Committed)
            );
            assert_eq!(gateway.control_mailbox(request).expect("replay"), result);
            assert!(gateway.mailbox_drafts().expect("drafts").is_empty());
        }
        let snapshot = gateway.authoritative_snapshot().expect("snapshot");
        assert_eq!(snapshot.conversations().len(), 1);
        assert_eq!(snapshot.conversations()[0].key, conversation);
        assert_eq!(snapshot.conversations()[0].sent_messages, 3);
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn provider_conversation_uses_unambiguous_question_context_and_preserves_ambiguous_drafts() {
    use hq_domain::{
        ContentText, ConversationId, OperationCorrelation, ProviderId, ProviderSessionId, ThreadId,
    };
    use hq_reducer::{ConversationProjection, ConversationProjectionKey};
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    seed_human_authority(&store);
    let (agent, agent_fact) = seed_agent_mailbox(&store);
    let policy = authority_policy();
    let human = MailboxAddress::new(policy.local_installation(), policy.local_human_mailbox());
    let root_id = FactId::from_bytes(verified_fact().verified_event().event_id());
    let provider = ProviderId::new("provider").expect("provider");
    let session = ProviderSessionId::new("session").expect("session");
    let conversation = ConversationId::ProviderSession {
        counterparty: agent,
        provider: provider.clone(),
        session: session.clone(),
    };
    let mut questions = Vec::new();
    for serial in [0xf1, 0xf2] {
        let plan = plan_question(
            MessageAuthoringAuthority {
                author: policy.local_installation(),
                sender: agent,
                scope: FactScope::InstallationPrivate(policy.local_installation()),
                authority: AuthorityReference::new(AuthorityRole::LocalInstallation, root_id),
                support: BTreeSet::from([root_id, agent_fact]),
            },
            LocalFactInputs {
                authored_at: Timestamp::from_unix_millis(i64::from(255 - serial)),
                auxiliary_randomness: [serial; 32],
            },
            NewMessageRequest {
                message_id: MessageId::from_bytes([serial; 32]),
                recipient: Some(human),
                body: ContentText::new("same display question").expect("body"),
                presentation: PresentationKind::Message,
                project_id: None,
            },
        )
        .expect("question plans");
        let (author, time, scope, causal, mut payload, randomness) = plan.into_parts();
        if let SemanticPayload::QuestionAsked(message) = &mut payload {
            message.correlation = Some(OperationCorrelation::new(
                provider.clone(),
                session.clone(),
                OperationId::from_bytes([serial; 32]),
            ));
        }
        let fact = CanonicalEventPlan::new(author, time, scope, causal, payload)
            .sign(&signer(1), randomness)
            .expect("sign");
        questions.push(ThreadId::from_bytes(fact.verified_event().event_id()));
        store.append_verified(fact).expect("append");
    }
    let gateway = StoreGateway::new(&store, policy, Arc::new(signer(1)));
    let draft_id = OperationId::from_bytes([0xf3; 32]);
    gateway
        .save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id,
            target: MailboxDraftTarget::Conversation {
                conversation: conversation.clone(),
            },
            content: "keep this response".to_owned(),
            expected_version: None,
        })
        .expect("draft");
    let request = MailboxCommandRequest {
        command_id: CommandId::from_bytes([0xf4; 32]),
        request_digest: CommandDigest::from_bytes([0xf4; 32]),
        draft_id: Some(draft_id),
        action: MailboxCommandAction::Conversation {
            conversation,
            message_id: MessageId::from_bytes([0xf5; 32]),
        },
        content: None,
        authored_at: Timestamp::from_unix_millis(30),
        auxiliary_randomness: [0xf5; 32],
    };
    let rejected = gateway
        .control_mailbox(request.clone())
        .expect("retained rejection");
    assert!(
        matches!(rejected, MutationAttempt::Completed(ref receipt) if matches!(receipt.outcome(), MutationOutcome::Rejected(error) if error.code().as_str() == "conversation_input_ambiguous"))
    );
    assert_eq!(
        gateway.mailbox_drafts().expect("drafts")[0].content,
        "keep this response"
    );
    let explicit_answer = MailboxCommandRequest {
        command_id: CommandId::from_bytes([0xf6; 32]),
        request_digest: CommandDigest::from_bytes([0xf6; 32]),
        draft_id: None,
        action: MailboxCommandAction::Reply {
            target_message: MessageId::from_bytes([0xf1; 32]),
            message_id: MessageId::from_bytes([0xf6; 32]),
        },
        content: Some("handled independently".to_owned()),
        authored_at: Timestamp::from_unix_millis(31),
        auxiliary_randomness: [0xf6; 32],
    };
    assert!(
        matches!(gateway.control_mailbox(explicit_answer).expect("resolve one question"), MutationAttempt::Completed(ref receipt) if receipt.outcome() == &MutationOutcome::Committed)
    );
    let mut retry = request;
    retry.command_id = CommandId::from_bytes([0xf7; 32]);
    retry.request_digest = CommandDigest::from_bytes([0xf7; 32]);
    assert!(
        matches!(gateway.control_mailbox(retry).expect("unambiguous conversation send"), MutationAttempt::Completed(ref receipt) if receipt.outcome() == &MutationOutcome::Committed)
    );
    let snapshot = store
        .load_conversation_snapshot()
        .expect("canonical snapshot");
    let Some(ConversationProjection::Message(message)) = snapshot.projection(
        ConversationProjectionKey::Message(MessageId::from_bytes([0xf5; 32])),
    ) else {
        panic!("committed message");
    };
    assert_eq!(message.thread_id, questions[1]);
    assert!(gateway.mailbox_drafts().expect("drafts").is_empty());
}
