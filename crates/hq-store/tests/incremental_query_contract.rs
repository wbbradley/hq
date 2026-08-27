//! Incremental materialization and indexed conversation-query contracts.

#![allow(clippy::expect_used)]

use hq_domain::{AuthorityRole, MailboxAddress, PageCursor, ProviderId, ProviderSessionId};
use hq_protocol::VerifiedSemanticFact;
use hq_store::{ConversationEntry, ConversationKey, IngestOutcome, StoreErrorClass};
use rusqlite::{Connection, params};

mod support;

use support::{
    TestDirectory, TestStoreExt, authored_conversation_entry, authored_durable_conversation_entry,
    authority_policy, open_store, verified_account, verified_child, verified_fact,
    verified_question,
};

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
    assert!(matches!(page.items()[0], ConversationEntry::Message(_)));
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
fn equal_time_mixed_pages_concatenate_to_local_reducer_order_after_repair_and_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("authority root ingests");
    store
        .append_verified(verified_child(root_id))
        .expect("activity source mailbox ingests");
    for index in (0..24).rev() {
        store
            .append_verified(authored_conversation_entry(index, index % 2 == 1))
            .expect("mixed entry ingests");
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
    assert_eq!(load_all_ids(&store, &key, 5), expected);

    let first = store
        .load_conversation_entries(&key, 5, None)
        .expect("first page loads");
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
    assert_eq!(load_all_ids(&store, &key, 7), expected);
    store.close().expect("store closes");
    let reopened = open_store(&database);
    assert_eq!(load_all_ids(&reopened, &key, 6), expected);
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
        ids.extend(page.items().iter().map(ConversationEntry::fact_id));
        cursor = page.next_cursor().cloned();
        if cursor.is_none() {
            return ids;
        }
    }
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
             WHERE key_digest = ?1 AND position > ?2 ORDER BY position LIMIT ?3",
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

fn seed_canonical_corpus(path: &std::path::Path, facts: &[VerifiedSemanticFact]) {
    let mut connection = Connection::open(path).expect("seed connection opens");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("foreign keys enable");
    let transaction = connection.transaction().expect("seed transaction begins");
    for verified in facts {
        let fact = verified.fact();
        let family = fact
            .kind()
            .catalog_id()
            .strip_prefix("FCT-")
            .expect("catalog prefix exists")
            .parse::<i64>()
            .expect("catalog suffix is numeric");
        transaction
            .execute(
                "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                 VALUES (?1, ?2, 1, ?3)",
                params![
                    fact.id().as_bytes().as_slice(),
                    verified.verified_event().exact_event_bytes(),
                    family
                ],
            )
            .expect("canonical fact seeds");
        for parent in fact.causal().parents().iter() {
            transaction
                .execute(
                    "INSERT INTO fact_parents(fact_id, parent_id) VALUES (?1, ?2)",
                    params![
                        fact.id().as_bytes().as_slice(),
                        parent.as_bytes().as_slice()
                    ],
                )
                .expect("parent seeds");
        }
        for role in AuthorityRole::ALL {
            if let Some(authority) = fact.causal().authority(role) {
                transaction
                    .execute(
                        "INSERT INTO fact_authorities(fact_id, authority_role, authority_fact_id) \
                         VALUES (?1, ?2, ?3)",
                        params![
                            fact.id().as_bytes().as_slice(),
                            authority_role_code(role),
                            authority.as_bytes().as_slice()
                        ],
                    )
                    .expect("authority seeds");
            }
        }
    }
    transaction.commit().expect("seed transaction commits");
}

fn authority_role_code(role: AuthorityRole) -> i64 {
    AuthorityRole::ALL
        .iter()
        .position(|candidate| *candidate == role)
        .and_then(|index| i64::try_from(index + 1).ok())
        .expect("closed role has a code")
}
