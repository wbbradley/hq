//! Incremental materialization and indexed conversation-query contracts.

#![allow(clippy::expect_used)]

use hq_application::{ClientProjection, ConversationContext, QueryDomain};
use hq_domain::{
    FactId, MailboxAddress, PageCursor, ProjectId, ProviderId, ProviderSessionId, ThreadId,
};
use hq_store::{ConversationEntry, ConversationKey, IngestOutcome, StoreErrorClass};
use rusqlite::{Connection, params};

mod support;

use support::{
    TestDirectory, TestStoreExt, authored_agent_activity, authored_conversation_entry,
    authored_durable_conversation_entry, authored_local_message, authored_project_input,
    authority_policy, open_store, seed_canonical_corpus, verified_account, verified_child,
    verified_fact, verified_incomplete_peer_question, verified_question,
};

#[test]
fn sender_summary_covers_history_outside_the_selected_page() {
    use hq_domain::{
        AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, ContentText, FactScope,
        MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MessageContent, MessageId, MessagePurpose,
        PresentationKind, SemanticPayload, Timestamp,
    };
    for root_mailbox in [0x33, 0x44] {
        let directory = TestDirectory::new();
        let store = open_store(&directory.database_path());
        let root = verified_fact();
        let root_id = root.fact().id();
        store.append_verified(root).expect("root");
        let project_id = ProjectId::from_bytes([0x91; 32]);
        let local = authority_policy().local_installation();
        let address =
            |mailbox| MailboxAddress::new(local, hq_domain::MailboxId::from_bytes([mailbox; 32]));
        let content = |index, sender, recipient| MessageContent {
            message_id: MessageId::from_bytes([index; 32]),
            sender: address(sender),
            recipient: Some(address(recipient)),
            body: ContentText::new("Message").expect("body"),
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: Some(project_id),
        };
        let sign = |index, parents, payload| {
            hq_protocol::CanonicalEventPlan::new(
                local,
                Timestamp::from_unix_millis(3000 + i64::from(index)),
                FactScope::InstallationPrivate(local),
                CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
                    BoundedSet::new(parents).expect("parents"),
                    [AuthorityReference::new(
                        AuthorityRole::LocalInstallation,
                        root_id,
                    )],
                )
                .expect("causal references"),
                payload,
            )
            .sign(&support::signer(1), [index; 32])
            .expect("signed message")
        };
        let question = sign(
            1,
            vec![root_id],
            SemanticPayload::QuestionAsked(content(1, root_mailbox, 0x55)),
        );
        let question_id = question.fact().id();
        let thread = ThreadId::from_bytes(*question_id.as_bytes());
        store.append_verified(question).expect("question");
        let key = ConversationKey::ProjectThread { project_id, thread };
        let selection = hq_application::ConversationPageSelection::new(key, 1).expect("selection");
        let gateway = hq_store::StoreGateway::new(
            &store,
            authority_policy(),
            std::sync::Arc::new(support::signer(1)),
        );
        assert!(
            !gateway
                .authoritative_snapshot()
                .expect("snapshot")
                .conversations()[0]
                .multiple_non_user_senders
        );
        for index in [2, 3] {
            let answer = sign(
                index,
                vec![root_id, question_id],
                SemanticPayload::AnswerGiven {
                    thread_id: thread,
                    message: content(index, 0x55, root_mailbox),
                },
            );
            store.append_verified(answer).expect("answer");
            let view = gateway
                .authoritative_conversation_view(Some(&selection))
                .expect("view");
            let summary = view
                .snapshot()
                .conversations()
                .iter()
                .find(|summary| summary.key == *selection.key())
                .expect("summary");
            assert_eq!(summary.multiple_non_user_senders, root_mailbox != 0x33);
            assert_eq!(
                view.conversation().expect("selected").page().items().len(),
                1
            );
        }
    }
}

#[test]
fn active_operation_has_one_latest_progress_tail_and_terminal_replaces_it() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("authority root ingests");
    store
        .append_verified(verified_child(root_id))
        .expect("activity source mailbox ingests");
    let operation = hq_domain::OperationId::from_bytes([0x91; 32]);
    for fact in [
        authored_agent_activity(
            1,
            operation,
            None,
            hq_domain::ActivityKind::AgentTurn,
            "operation",
            1,
            hq_domain::ActivityStatus::Running,
            "started",
        ),
        authored_agent_activity(
            2,
            operation,
            Some("compile"),
            hq_domain::ActivityKind::Progress,
            "progress",
            2,
            hq_domain::ActivityStatus::Running,
            "compiling",
        ),
        authored_agent_activity(
            3,
            operation,
            Some("tests"),
            hq_domain::ActivityKind::Progress,
            "progress",
            3,
            hq_domain::ActivityStatus::Running,
            "running tests",
        ),
    ] {
        store.append_verified(fact).expect("activity ingests");
    }
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };
    let active = store
        .load_conversation_entries(&key, 10, None)
        .expect("active page loads");
    assert!(matches!(
        active.items(),
        [ConversationEntry::Activity(activity)]
            if activity.kind == hq_domain::ActivityKind::Progress
                && activity.content.as_str() == "running tests"
    ));

    store
        .append_verified(authored_agent_activity(
            4,
            operation,
            None,
            hq_domain::ActivityKind::AgentTurn,
            "operation",
            4,
            hq_domain::ActivityStatus::Succeeded,
            "completed",
        ))
        .expect("terminal activity ingests");
    let terminal = store
        .load_conversation_entries(&key, 10, None)
        .expect("terminal page loads");
    assert!(matches!(
        terminal.items(),
        [ConversationEntry::Activity(activity)]
            if activity.kind == hq_domain::ActivityKind::AgentTurn
                && activity.status == hq_domain::ActivityStatus::Succeeded
    ));
}

#[test]
fn completed_item_output_is_history_and_running_turn_becomes_the_live_tail() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("authority root ingests");
    store
        .append_verified(verified_child(root_id))
        .expect("activity source mailbox ingests");
    let operation = hq_domain::OperationId::from_bytes([0x93; 32]);
    for fact in [
        authored_agent_activity(
            1,
            operation,
            None,
            hq_domain::ActivityKind::AgentTurn,
            "operation",
            1,
            hq_domain::ActivityStatus::Running,
            "working",
        ),
        authored_agent_activity(
            2,
            operation,
            Some("tests"),
            hq_domain::ActivityKind::Progress,
            "tests-progress",
            2,
            hq_domain::ActivityStatus::Running,
            "error: test failed, to rerun pass -p hq-node",
        ),
        authored_agent_activity(
            3,
            operation,
            Some("tests"),
            hq_domain::ActivityKind::CompletedItem,
            "tests-completed",
            3,
            hq_domain::ActivityStatus::Failed(
                hq_domain::ErrorCode::new("test_failed").expect("failure code"),
            ),
            "tests failed",
        ),
    ] {
        store.append_verified(fact).expect("activity ingests");
    }
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };

    let page = store
        .load_conversation_entries(&key, 10, None)
        .expect("conversation loads");
    assert!(matches!(
        page.items(),
        [ConversationEntry::Activity(completed), ConversationEntry::Activity(live)]
            if completed.kind == hq_domain::ActivityKind::CompletedItem
                && completed.item.as_ref().map(hq_domain::ShortText::as_str) == Some("tests")
                && live.kind == hq_domain::ActivityKind::AgentTurn
                && live.status == hq_domain::ActivityStatus::Running
    ));
}

#[test]
fn minimum_page_limit_keeps_durable_history_bounded_and_cursor_reachable() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let mailbox = verified_child(root_id);
    let first = authored_durable_conversation_entry(1, false);
    let second = authored_durable_conversation_entry(2, false);
    let expected = [first.fact().id(), second.fact().id()];
    for fact in [
        root,
        mailbox,
        first,
        second,
        authored_agent_activity(
            3,
            hq_domain::OperationId::from_bytes([0x95; 32]),
            None,
            hq_domain::ActivityKind::AgentTurn,
            "operation",
            1,
            hq_domain::ActivityStatus::Running,
            "working",
        ),
    ] {
        store
            .append_verified(fact)
            .expect("conversation fact ingests");
    }
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };

    let first_page = store
        .load_conversation_entries(&key, 1, None)
        .expect("minimum first page loads");
    assert_eq!(first_page.items().len(), 1);
    assert_eq!(first_page.items()[0].fact_id(), expected[1]);
    let cursor = first_page
        .next_cursor()
        .expect("remaining durable history has a cursor");
    let second_page = store
        .load_conversation_entries(&key, 1, Some(cursor))
        .expect("minimum continuation page loads");
    assert_eq!(second_page.items().len(), 1);
    assert_eq!(second_page.items()[0].fact_id(), expected[0]);
    assert!(second_page.next_cursor().is_none());
    for anchor in expected {
        for limit in 1..=3 {
            let selection = hq_application::ConversationPageSelection::new(key.clone(), limit)
                .expect("bounded selection")
                .with_anchor(Some(anchor));
            let view = store
                .application_state_handle()
                .authoritative_conversation_view(Some(&selection))
                .expect("small anchored window with a live tail");
            let selected = view.conversation().expect("selected window");
            assert_eq!(selected.anchor(), Some(anchor));
            assert!(selected.page().items().len() <= limit);
            assert!(
                selected
                    .page()
                    .items()
                    .iter()
                    .any(|entry| entry.fact_id() == anchor),
                "live activity must not evict the reading anchor"
            );
        }
    }
}

#[test]
fn minimum_page_limit_returns_live_tail_when_no_durable_history_exists() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    for fact in [
        root,
        verified_child(root_id),
        authored_agent_activity(
            1,
            hq_domain::OperationId::from_bytes([0x96; 32]),
            None,
            hq_domain::ActivityKind::AgentTurn,
            "operation",
            1,
            hq_domain::ActivityStatus::Running,
            "working",
        ),
    ] {
        store
            .append_verified(fact)
            .expect("conversation fact ingests");
    }
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };

    let page = store
        .load_conversation_entries(&key, 1, None)
        .expect("live-only minimum page loads");
    assert!(matches!(
        page.items(),
        [ConversationEntry::Activity(activity)]
            if activity.kind == hq_domain::ActivityKind::AgentTurn
                && activity.content.as_str() == "working"
    ));
    assert!(page.next_cursor().is_none());
}

#[test]
fn newer_human_input_hides_old_progress_until_provider_activity_advances() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("authority root ingests");
    store
        .append_verified(verified_child(root_id))
        .expect("activity source mailbox ingests");
    let operation = hq_domain::OperationId::from_bytes([0x94; 32]);
    for fact in [
        authored_agent_activity(
            1,
            operation,
            None,
            hq_domain::ActivityKind::AgentTurn,
            "operation",
            1,
            hq_domain::ActivityStatus::Running,
            "working",
        ),
        authored_agent_activity(
            2,
            operation,
            Some("tests"),
            hq_domain::ActivityKind::Progress,
            "tests-progress",
            2,
            hq_domain::ActivityStatus::Running,
            "error: test failed",
        ),
        authored_local_message(3, operation, "Are you still working on this?"),
    ] {
        store
            .append_verified(fact)
            .expect("conversation fact ingests");
    }
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };

    let waiting = store
        .load_conversation_entries(&key, 10, None)
        .expect("waiting conversation loads");
    assert!(matches!(
        waiting.items(),
        [ConversationEntry::Message(message), ConversationEntry::Activity(live)]
            if message.message.content.body.as_str() == "Are you still working on this?"
                && live.kind == hq_domain::ActivityKind::AgentTurn
                && live.content.as_str() == "working"
    ));

    store
        .append_verified(authored_agent_activity(
            4,
            operation,
            Some("analysis"),
            hq_domain::ActivityKind::Progress,
            "fresh-progress",
            3,
            hq_domain::ActivityStatus::Running,
            "checking the failure",
        ))
        .expect("fresh progress ingests");
    let advanced = store
        .load_conversation_entries(&key, 10, None)
        .expect("advanced conversation loads");
    assert!(matches!(
        advanced.items().last(),
        Some(ConversationEntry::Activity(live))
            if live.kind == hq_domain::ActivityKind::Progress
                && live.content.as_str() == "checking the failure"
    ));
}

#[test]
fn long_history_pages_expose_one_live_tail_without_repeating_retained_progress() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("schema initializes");
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let operation = hq_domain::OperationId::from_bytes([0x92; 32]);
    let mut facts = vec![root, verified_child(root_id)];
    facts.extend((0..250).map(|index| authored_durable_conversation_entry(index, false)));
    facts.push(authored_agent_activity(
        300,
        operation,
        None,
        hq_domain::ActivityKind::AgentTurn,
        "operation",
        1,
        hq_domain::ActivityStatus::Running,
        "started",
    ));
    for offset in 0..205_u16 {
        let key = format!("progress-{offset}");
        facts.push(authored_agent_activity(
            301 + offset,
            operation,
            Some(&key),
            hq_domain::ActivityKind::Progress,
            &key,
            u64::from(offset) + 2,
            hq_domain::ActivityStatus::Running,
            &key,
        ));
    }
    seed_canonical_corpus(&database, &facts);

    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };
    let store = open_store(&database);
    store
        .repair(authority_policy())
        .expect("large conversation repairs");
    let first = store
        .load_conversation_entries(&key, 200, None)
        .expect("initial page loads");
    assert_eq!(first.items().len(), 200);
    assert!(matches!(
        first.items().last(),
        Some(ConversationEntry::Activity(activity))
            if activity.kind == hq_domain::ActivityKind::Progress
                && activity.content.as_str() == "progress-204"
    ));

    let mut message_count = first
        .items()
        .iter()
        .filter(|entry| matches!(entry, ConversationEntry::Message(_)))
        .count();
    let mut cursor = first.next_cursor().cloned();
    while let Some(current) = cursor {
        let page = store
            .load_conversation_entries(&key, 200, Some(&current))
            .expect("continuation page loads");
        assert!(
            page.items()
                .iter()
                .all(|entry| matches!(entry, ConversationEntry::Message(_)))
        );
        message_count += page.items().len();
        cursor = page.next_cursor().cloned();
    }
    assert_eq!(message_count, 250);

    store.close().expect("store closes");
    let reopened = open_store(&database);
    let reopened_first = reopened
        .load_conversation_entries(&key, 200, None)
        .expect("reopened initial page loads");
    assert_eq!(reopened_first.items(), first.items());
}

#[test]
fn project_thread_keys_persist_rebuild_reopen_and_page_independently() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let project_id = ProjectId::from_bytes([0x91; 32]);
    let first = authored_project_input(1, project_id, "Let's have a conversation.");
    let second = authored_project_input(2, project_id, "Let's have another conversation.");
    let first_id = first.fact().id();
    let second_id = second.fact().id();
    let first_key = ConversationKey::ProjectThread {
        project_id,
        thread: ThreadId::from_bytes(*first_id.as_bytes()),
    };
    let second_key = ConversationKey::ProjectThread {
        project_id,
        thread: ThreadId::from_bytes(*second_id.as_bytes()),
    };
    store.append_verified(root).expect("root ingests");
    store.append_verified(second).expect("second input ingests");
    store.append_verified(first).expect("first input ingests");

    assert_eq!(load_all_ids(&store, &first_key, 1), [first_id]);
    assert_eq!(load_all_ids(&store, &second_key, 1), [second_id]);
    let snapshot = store.authoritative_snapshot().expect("snapshot loads");
    assert_eq!(
        snapshot
            .conversations()
            .iter()
            .map(|summary| summary.key.clone())
            .collect::<Vec<_>>(),
        vec![first_key.clone(), second_key.clone()]
    );
    assert_eq!(
        snapshot
            .conversations()
            .iter()
            .map(|summary| (
                &summary.context,
                summary.preview.as_ref().map(hq_domain::ShortText::as_str)
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                &ConversationContext::Project {
                    project_id,
                    name: None,
                    participant: None,
                },
                Some("Let's have a conversation."),
            ),
            (
                &ConversationContext::Project {
                    project_id,
                    name: None,
                    participant: None,
                },
                Some("Let's have another conversation."),
            ),
        ]
    );
    let index = store.load_reduction_index().expect("index loads");
    assert_eq!(index.conversation_orders().len(), 2);
    assert!(!index.conversation_orders().keys().any(|key| matches!(
        key,
        ConversationKey::Thread { .. } | ConversationKey::ProviderSession { .. }
    )));

    store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(load_all_ids(&store, &first_key, 1), [first_id]);
    assert_eq!(load_all_ids(&store, &second_key, 1), [second_id]);
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(load_all_ids(&reopened, &first_key, 1), [first_id]);
    assert_eq!(load_all_ids(&reopened, &second_key, 1), [second_id]);
}

#[test]
fn incomplete_peer_message_is_bounded_inert_diagnostic_state_across_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let question = verified_incomplete_peer_question();
    let question_fact = question.fact().id();
    store
        .append_verified(question)
        .expect("incomplete addressed evidence ingests");

    for snapshot in [store.authoritative_snapshot().expect("snapshot loads"), {
        drop(store);
        open_store(&database)
            .authoritative_snapshot()
            .expect("reopened snapshot loads")
    }] {
        let matches = snapshot
            .client_projections()
            .expect("client projections")
            .into_iter()
            .filter_map(|projection| match projection {
                ClientProjection::IncompleteMessage { message } => Some(message),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].fact_id, question_fact);
        assert_eq!(matches[0].content.body.as_str(), "incomplete peer question");
        assert_eq!(matches[0].missing_dependencies.len(), 1);
    }
}

#[test]
fn late_parent_affected_closure_and_incremental_state_equal_batch_and_repair() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let root_id = root.fact().id();
    let question = verified_question(root.verified_event().event_id());
    let question_id = question.fact().id();

    store
        .append_verified(question)
        .expect("late-parent child ingests unresolved");
    let before = store
        .load_reduction_index()
        .expect("incremental index loads");
    assert_eq!(
        before.affected_closure([root_id]),
        [root_id, question_id].into_iter().collect()
    );

    assert!(matches!(
        store.append_verified(root),
        Ok(IngestOutcome::Inserted(_))
    ));
    let complete = store
        .complete_snapshot(authority_policy())
        .expect("batch oracle succeeds");
    assert_eq!(
        store
            .load_reduction_index()
            .expect("incremental index loads"),
        complete.normalized_index()
    );
    assert_eq!(
        store
            .load_conversation_snapshot()
            .expect("incremental conversation loads"),
        complete.conversation_projection_snapshot()
    );
    let repaired = store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(repaired.complete(), &complete);
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_reduction_index()
            .expect("reopened index loads"),
        complete.normalized_index()
    );
}

#[test]
fn unrelated_projection_rows_are_not_deleted_or_rewritten() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("root ingests");
    store
        .append_verified(verified_question(root_id))
        .expect("question ingests");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("trigger connection opens");
    connection
        .execute_batch(
            "CREATE TRIGGER protect_conversation_message_update
                 BEFORE UPDATE ON conversation_messages BEGIN SELECT RAISE(ABORT, 'unrelated update'); END;
             CREATE TRIGGER protect_conversation_message_delete
                 BEFORE DELETE ON conversation_messages BEGIN SELECT RAISE(ABORT, 'unrelated delete'); END;",
        )
        .expect("protective triggers install");
    drop(connection);

    let reopened = open_store(&database);
    assert!(matches!(
        reopened.append_verified(verified_account(root_id)),
        Ok(IngestOutcome::Inserted(_))
    ));
    let complete = reopened
        .complete_snapshot(authority_policy())
        .expect("batch oracle succeeds");
    assert_eq!(
        reopened
            .load_conversation_snapshot()
            .expect("conversation remains valid"),
        complete.conversation_projection_snapshot()
    );
}

#[test]
fn cursor_pages_are_bound_to_the_conversation_and_concatenate_to_reducer_order() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let question = verified_question(root_id);
    let question_id = question.fact().id();
    store.append_verified(root).expect("root ingests");
    store.append_verified(question).expect("question ingests");

    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("test-provider").expect("provider validates"),
        session: ProviderSessionId::new("session-1").expect("session validates"),
    };
    let page = store
        .load_conversation_entries(&key, 1, None)
        .expect("first page loads");
    assert_eq!(page.items().len(), 1);
    assert_eq!(page.items()[0].fact_id(), question_id);
    let entry = page
        .items()
        .iter()
        .find_map(|entry| match entry {
            ConversationEntry::Message(message) => Some(message),
            ConversationEntry::Activity(_) => None,
        })
        .expect("question page entry is a message");
    let thread = entry
        .thread
        .as_ref()
        .expect("question carries thread state");
    assert_eq!(thread.root_fact, question_id);
    assert_eq!(entry.message.fact_id, question_id);
    assert!(page.next_cursor().is_none());

    let malformed = PageCursor::new("v1:not-a-cursor").expect("opaque cursor validates");
    assert_eq!(
        store
            .load_conversation_entries(&key, 1, Some(&malformed))
            .expect_err("malformed cursor rejects")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );
    assert_eq!(
        store
            .load_conversation_entries(&key, 0, None)
            .expect_err("zero limit rejects")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );
    assert_eq!(
        store
            .load_conversation_entries(&key, 201, None)
            .expect_err("oversized limit rejects")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );
}

#[test]
fn equal_time_mixed_pages_prepend_to_local_reducer_order_after_repair_and_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("authority root ingests");
    store
        .append_verified(verified_child(root_id))
        .expect("activity source mailbox ingests");
    let mut transient = std::collections::BTreeSet::new();
    for index in (0..24).rev() {
        let fact = authored_conversation_entry(index, index % 2 == 1);
        if index % 2 == 1 {
            transient.insert(FactId::from_bytes(fact.verified_event().event_id()));
        }
        store.append_verified(fact).expect("mixed entry ingests");
    }
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };
    let index = store.load_reduction_index().expect("index loads");
    let expected = index
        .conversation_orders()
        .get(&key)
        .expect("conversation order exists")
        .clone();
    assert_eq!(expected.len(), 24);
    let durable = expected
        .iter()
        .copied()
        .filter(|fact_id| !transient.contains(fact_id))
        .collect::<Vec<_>>();
    assert_eq!(load_all_ids(&store, &key, 5), durable);

    let first = store
        .load_conversation_entries(&key, 5, None)
        .expect("first page loads");
    assert_eq!(
        first
            .items()
            .iter()
            .map(ConversationEntry::fact_id)
            .collect::<Vec<_>>(),
        durable[durable.len() - 5..],
        "opening history returns the latest canonical entries"
    );
    let other_key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("other-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };
    assert_eq!(
        store
            .load_conversation_entries(&other_key, 5, first.next_cursor())
            .expect_err("cross-conversation cursor rejects")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );

    store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(load_all_ids(&store, &key, 7), durable);
    store.close().expect("store closes");
    let reopened = open_store(&database);
    assert_eq!(load_all_ids(&reopened, &key, 6), durable);
}

fn conversation_page_ids(page: &hq_domain::Page<ConversationEntry>) -> Vec<FactId> {
    page.items()
        .iter()
        .map(ConversationEntry::fact_id)
        .collect()
}

fn load_all_ids(
    store: &hq_store::Store,
    key: &ConversationKey,
    limit: usize,
) -> Vec<hq_domain::FactId> {
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = store
            .load_conversation_entries(key, limit, cursor.as_ref())
            .expect("conversation page loads");
        ids.splice(0..0, page.items().iter().map(ConversationEntry::fact_id));
        cursor = page.next_cursor().cloned();
        if cursor.is_none() {
            return ids;
        }
    }
}

#[test]
fn anchored_history_windows_reread_canonical_content_and_page_in_both_directions() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let mailbox = verified_child(root.verified_event().event_id());
    for fact in [root, mailbox]
        .into_iter()
        .chain((0..12).map(|index| authored_durable_conversation_entry(index, false)))
    {
        store.append_verified(fact).expect("history ingests");
    }
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider"),
        session: ProviderSessionId::new("paged-session").expect("session"),
    };
    let expected = load_all_ids(&store, &key, 12);
    let selection = hq_application::ConversationPageSelection::new(key.clone(), 4)
        .expect("selection")
        .with_anchor(Some(expected[4]));
    let view = store
        .application_state_handle()
        .authoritative_conversation_view(Some(&selection))
        .expect("anchored view");
    let window = view.conversation().expect("selected window").page();
    assert_eq!(conversation_page_ids(window), expected[3..7]);
    let older = store
        .load_conversation_entries(&key, 4, window.next_cursor())
        .expect("older page");
    assert_eq!(conversation_page_ids(&older), expected[..3]);
    let newer = store
        .load_conversation_entries(&key, 4, window.previous_cursor())
        .expect("newer page");
    assert_eq!(conversation_page_ids(&newer), expected[7..11]);
    let latest = store
        .load_conversation_entries(&key, 4, newer.previous_cursor())
        .expect("last page");
    assert_eq!(conversation_page_ids(&latest), expected[11..]);
    assert!(latest.previous_cursor().is_none());
    assert!(older.next_cursor().is_none());
    let continuation = hq_application::ConversationPageSelection::new(key.clone(), 4)
        .expect("continuation selection")
        .with_cursor(window.previous_cursor().cloned());
    let coherent = store
        .application_state_handle()
        .authoritative_conversation_view(Some(&continuation))
        .expect("revision-coherent newer page");
    assert_eq!(
        conversation_page_ids(coherent.conversation().expect("selected page").page()),
        expected[7..11]
    );

    store
        .append_verified(authored_durable_conversation_entry(99, false))
        .expect("incoming history");
    let refreshed = store
        .application_state_handle()
        .authoritative_conversation_view(Some(&selection))
        .expect("reread anchored view");
    assert!(refreshed.snapshot().revision() > view.snapshot().revision());
    let page = refreshed.conversation().expect("refreshed window").page();
    assert_eq!(page.items().len(), 4);
    assert!(
        page.items()
            .iter()
            .any(|entry| entry.fact_id() == expected[4])
    );
    let all = load_all_ids(&store, &key, 20);
    let indices = page
        .items()
        .iter()
        .map(|entry| {
            all.iter()
                .position(|id| *id == entry.fact_id())
                .expect("canonical identity")
        })
        .collect::<Vec<_>>();
    assert!(indices.windows(2).all(|pair| pair[1] == pair[0] + 1));
    let wrong = hq_application::ConversationPageSelection::new(key, 4)
        .expect("selection")
        .with_anchor(Some(FactId::from_bytes([0xff; 32])));
    assert_eq!(
        store
            .application_state_handle()
            .authoritative_conversation_view(Some(&wrong))
            .expect_err("unrelated anchor rejects")
            .class(),
        StoreErrorClass::InvalidOperationalRequest
    );
}

#[test]
fn thousand_entry_later_pages_use_the_covering_conversation_index() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("schema initializes");
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let mut facts = Vec::with_capacity(1_002);
    facts.push(root);
    facts.push(verified_child(root_id));
    facts
        .extend((0..1_000).map(|index| authored_durable_conversation_entry(index, index % 2 == 1)));
    seed_canonical_corpus(&database, &facts);

    let store = open_store(&database);
    store
        .repair(authority_policy())
        .expect("batch repair succeeds");
    let key = ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    };
    let index = store.load_reduction_index().expect("index loads");
    assert_eq!(index.affected_closure([facts[0].fact().id()]).len(), 1_002);
    let expected = index
        .conversation_orders()
        .get(&key)
        .expect("large order exists")
        .clone();
    assert_eq!(expected.len(), 1_000);
    assert_eq!(load_all_ids(&store, &key, 73), expected);
    let first = store
        .load_conversation_entries(&key, 17, None)
        .expect("first page loads");
    let later = store
        .load_conversation_entries(&key, 17, first.next_cursor())
        .expect("later page loads");
    assert_eq!(later.items().len(), 17);
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("query-plan connection opens");
    let detail = connection
        .prepare(
            "EXPLAIN QUERY PLAN SELECT position, fact_id, entry_kind \
             FROM reduction_conversation_order \
             WHERE key_digest = ?1 AND position < ?2 ORDER BY position DESC LIMIT ?3",
        )
        .expect("query plan prepares")
        .query_map(params![[0_u8; 32].as_slice(), 900_i64, 18_i64], |row| {
            row.get::<_, String>(3)
        })
        .expect("query plan runs")
        .collect::<Result<Vec<_>, _>>()
        .expect("query plan reads")
        .join(" ");
    assert!(detail.contains("SEARCH reduction_conversation_order USING PRIMARY KEY"));
    assert!(!detail.contains("SCAN reduction_conversation_order"));
    for (table, index_name) in [
        ("conversation_messages", "conversation_messages_by_fact_id"),
        (
            "conversation_activities",
            "conversation_activities_by_fact_id",
        ),
    ] {
        let hydration = connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT key_digest FROM {table} WHERE fact_id = ?1"
            ))
            .expect("hydration query plan prepares")
            .query_map([[0_u8; 32].as_slice()], |row| row.get::<_, String>(3))
            .expect("hydration query plan runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("hydration query plan reads")
            .join(" ");
        assert!(hydration.contains(&format!("USING COVERING INDEX {index_name}")));
        assert!(!hydration.contains(&format!("SCAN {table}")));
    }
}
