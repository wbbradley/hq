//! Persisted conversation and activity projection contracts.

#![allow(clippy::expect_used)]

use hq_domain::{
    AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, ConversationId,
    InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MessageId, OperationCorrelation,
    OperationId, ProviderId, ProviderSessionId, SemanticPayload, ThreadId, Timestamp,
};
use hq_protocol::CanonicalEventPlan;
use hq_reducer::{ConversationProjection, ConversationProjectionKey};
use hq_store::StoreErrorClass;
use rusqlite::Connection;

mod support;

use support::{
    TestDirectory, TestStoreExt, authority_policy, open_store, verified_fact, verified_question,
};

#[test]
fn repair_persists_the_exact_typed_conversation_report_and_reopens() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let question = verified_question(root_id);
    let question_id = question.verified_event().event_id();
    let thread = ThreadId::from_bytes(question_id);
    store.append_verified(root).expect("root appends");
    store.append_verified(question).expect("question appends");

    let ingested = store
        .load_conversation_snapshot()
        .expect("ingest materializes conversation rows");
    assert_eq!(
        ingested,
        store
            .complete_snapshot(authority_policy())
            .expect("complete oracle succeeds")
            .conversation_projection_snapshot()
    );
    let repaired = store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(
        repaired.complete().conversation_projection_snapshot(),
        *repaired.conversation()
    );
    assert!(matches!(
        repaired
            .conversation()
            .projection(ConversationProjectionKey::Thread(thread)),
        Some(ConversationProjection::Thread(_))
    ));
    assert!(matches!(
        repaired
            .conversation()
            .projection(ConversationProjectionKey::Message(MessageId::from_bytes(
                [0x55; 32]
            ))),
        Some(ConversationProjection::Message(_))
    ));
    assert!(matches!(
        repaired
            .conversation()
            .projection(ConversationProjectionKey::ActionGroup(operation())),
        Some(ConversationProjection::ActionGroup(_))
    ));
    let conversation = ConversationId::ProviderSession {
        counterparty: hq_domain::MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("test-provider").expect("provider validates"),
        session: ProviderSessionId::new("session-1").expect("session validates"),
    };
    let archive = CanonicalEventPlan::new(
        authority_policy().local_installation(),
        Timestamp::from_unix_millis(3_000),
        hq_domain::FactScope::InstallationPrivate(authority_policy().local_installation()),
        CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
            BoundedSet::new([
                hq_domain::FactId::from_bytes(root_id),
                hq_domain::FactId::from_bytes(question_id),
            ])
            .expect("archive parents validate"),
            [AuthorityReference::new(
                AuthorityRole::LocalInstallation,
                hq_domain::FactId::from_bytes(root_id),
            )],
        )
        .expect("archive authority validates"),
        SemanticPayload::ConversationArchived {
            conversation: conversation.clone(),
        },
    )
    .sign(&support::signer(1), [0x91; 32])
    .expect("archive signs");
    store.append_verified(archive).expect("archive appends");
    let archived = store
        .load_conversation_snapshot()
        .expect("archived conversation loads");
    assert!(matches!(
        archived.projection(ConversationProjectionKey::Archive(conversation.clone())),
        Some(ConversationProjection::Archive(view)) if view.archive_facts.len() == 1
    ));
    let expected = archived;
    assert_eq!(
        store
            .repair(authority_policy())
            .expect("repeated repair succeeds")
            .conversation(),
        &expected
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_conversation_snapshot()
            .expect("conversation snapshot reopens"),
        expected
    );
}

#[test]
fn conversation_snapshot_changes_atomically_on_ingest_and_repair_is_equal() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("root appends");
    let before = store
        .load_conversation_snapshot()
        .expect("initial conversation loads");
    assert!(before.frontiers().is_empty());
    assert!(before.projections().is_empty());
    assert!(before.support().is_empty());
    store
        .append_verified(verified_question(root_id))
        .expect("question appends");

    let after = store
        .load_conversation_snapshot()
        .expect("ingest updates conversation snapshot");
    assert_ne!(after, before);
    assert_eq!(after.projections().len(), 3);
    assert_eq!(
        store
            .repair(authority_policy())
            .expect("repair succeeds")
            .conversation(),
        &after
    );
}

#[test]
fn repair_replaces_conversation_rows_under_the_new_explicit_policy() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("root appends");
    store
        .append_verified(verified_question(root_id))
        .expect("question appends");
    assert!(
        !store
            .repair(authority_policy())
            .expect("first repair succeeds")
            .conversation()
            .projections()
            .is_empty()
    );
    let replacement_policy = hq_reducer::AuthorityPolicy::new(
        InstallationId::from_bytes([0x22; 32]),
        hq_domain::MailboxId::from_bytes([0x44; 32]),
    );

    let replacement = store
        .repair(replacement_policy)
        .expect("replacement repair succeeds");
    assert_eq!(replacement.complete().policy(), replacement_policy);
    assert_eq!(
        replacement.complete().conversation_projection_snapshot(),
        *replacement.conversation()
    );
    assert_eq!(
        store
            .load_conversation_snapshot()
            .expect("replacement conversation loads"),
        *replacement.conversation()
    );
}

#[test]
fn conversation_corruption_fails_closed_until_explicit_repair() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("root appends");
    store
        .append_verified(verified_question(root_id))
        .expect("question appends");
    store.repair(authority_policy()).expect("repair succeeds");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("corruption connection opens");
    connection
        .execute("UPDATE conversation_messages SET body = 'changed'", [])
        .expect("valid-looking corruption writes");
    drop(connection);

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_conversation_snapshot()
            .expect_err("changed projection rejects")
            .class(),
        StoreErrorClass::RebuildableStateCorrupt
    );
    let repaired = reopened
        .repair(authority_policy())
        .expect("repair recovers conversation rows");
    assert_eq!(
        reopened
            .load_conversation_snapshot()
            .expect("conversation snapshot reloads"),
        *repaired.conversation()
    );
}

fn operation() -> OperationCorrelation {
    OperationCorrelation::new(
        ProviderId::new("test-provider").expect("provider validates"),
        ProviderSessionId::new("session-1").expect("session validates"),
        OperationId::from_bytes([0x77; 32]),
    )
}
