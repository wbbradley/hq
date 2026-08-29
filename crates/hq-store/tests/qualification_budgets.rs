//! Explicit integrated-store performance regression budgets.

#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use hq_domain::{MailboxAddress, ProviderId, ProviderSessionId};
use hq_store::ConversationKey;

mod support;

use support::{
    TestStoreExt, authored_durable_conversation_entry, authority_policy, open_store,
    seed_canonical_corpus, verified_child, verified_fact,
};

const FULL_REBUILD_FACTS: u16 = 1_000;
const LATE_PARENT_DEPENDANTS: u16 = 500;
const PAGE_SIZE: usize = 10;
const LATER_PAGE_COUNT: usize = 10;

fn budget(name: &str, fallback_milliseconds: u64) -> Duration {
    let milliseconds = std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback_milliseconds);
    Duration::from_millis(milliseconds)
}

fn paged_conversation() -> ConversationKey {
    ConversationKey::ProviderSession {
        counterparty: MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: ProviderId::new("paged-provider").expect("provider validates"),
        session: ProviderSessionId::new("paged-session").expect("session validates"),
    }
}

#[test]
fn complete_rebuild_stays_within_the_declared_budget() {
    let directory = support::TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("schema initializes");
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let mut facts = Vec::with_capacity(usize::from(FULL_REBUILD_FACTS) + 2);
    facts.push(root);
    facts.push(verified_child(root_id));
    facts.extend(
        (0..FULL_REBUILD_FACTS)
            .map(|index| authored_durable_conversation_entry(index, index % 2 == 1)),
    );
    seed_canonical_corpus(&database, &facts);

    let store = open_store(&database);
    let started = Instant::now();
    let repaired = store.repair(authority_policy()).expect("repair succeeds");
    let elapsed = started.elapsed();

    assert_eq!(
        repaired.complete().conversation().facts().facts().count(),
        facts.len()
    );
    let maximum = budget("HQ_QUALIFICATION_FULL_REBUILD_MAX_MILLISECONDS", 10_000);
    assert!(
        elapsed <= maximum,
        "complete rebuild took {elapsed:?}, exceeding {maximum:?}"
    );
}

#[test]
fn one_late_parent_wakes_high_fanout_within_the_declared_budget() {
    let directory = support::TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("schema initializes");
    let dependants = (0..LATE_PARENT_DEPENDANTS)
        .map(|index| authored_durable_conversation_entry(index, index % 2 == 1))
        .collect::<Vec<_>>();
    seed_canonical_corpus(&database, &dependants);
    let store = open_store(&database);
    store
        .repair(authority_policy())
        .expect("unresolved fanout repairs");

    let root = verified_fact();
    let root_fact_id = root.fact().id();
    let started = Instant::now();
    store.append_verified(root).expect("late parent ingests");
    let elapsed = started.elapsed();

    let index = store.load_reduction_index().expect("index loads");
    assert_eq!(
        index.affected_closure([root_fact_id]).len(),
        usize::from(LATE_PARENT_DEPENDANTS) + 1
    );
    let maximum = budget(
        "HQ_QUALIFICATION_LATE_PARENT_FANOUT_MAX_MILLISECONDS",
        5_000,
    );
    assert!(
        elapsed <= maximum,
        "late-parent fanout ingest took {elapsed:?}, exceeding {maximum:?}"
    );
}

#[test]
fn indexed_later_pages_stay_within_the_declared_budget() {
    let directory = support::TestDirectory::new();
    let database = directory.database_path();
    open_store(&database).close().expect("schema initializes");
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let mut facts = Vec::with_capacity(usize::from(FULL_REBUILD_FACTS) + 2);
    facts.push(root);
    facts.push(verified_child(root_id));
    facts.extend(
        (0..FULL_REBUILD_FACTS)
            .map(|index| authored_durable_conversation_entry(index, index % 2 == 1)),
    );
    seed_canonical_corpus(&database, &facts);
    let store = open_store(&database);
    store.repair(authority_policy()).expect("repair succeeds");
    let key = paged_conversation();

    let mut cursor = None;
    for _ in 0..90 {
        let page = store
            .load_conversation_entries(&key, PAGE_SIZE, cursor.as_ref())
            .expect("leading page loads");
        cursor = page.next_cursor().cloned();
    }
    let started = Instant::now();
    let mut loaded = 0;
    for _ in 0..LATER_PAGE_COUNT {
        let page = store
            .load_conversation_entries(&key, PAGE_SIZE, cursor.as_ref())
            .expect("later page loads");
        loaded += page.items().len();
        cursor = page.next_cursor().cloned();
    }
    let elapsed = started.elapsed();

    assert_eq!(loaded, PAGE_SIZE * LATER_PAGE_COUNT);
    assert!(cursor.is_none());
    let maximum = budget("HQ_QUALIFICATION_LATER_PAGE_BATCH_MAX_MILLISECONDS", 1_000);
    assert!(
        elapsed <= maximum,
        "later-page batch took {elapsed:?}, exceeding {maximum:?}"
    );
}
