//! Structural and opt-in timing evidence for canonical-ingest scaling.

#![allow(clippy::expect_used, clippy::print_stdout)]

use std::{path::Path, time::Instant};

use hq_domain::{ActivityKind, ActivityStatus, OperationId};
use hq_protocol::VerifiedSemanticFact;
use hq_store::ConversationKey;
use rusqlite::Connection;

mod support;

use support::{
    TestDirectory, TestStoreExt, authored_agent_activity, authored_durable_conversation_entry,
    authority_policy, open_store, seed_canonical_corpus, verified_child, verified_fact,
};

#[derive(Clone, Copy, Debug)]
struct ScalingObservation {
    activity_replacements: usize,
    canonical_facts: usize,
    affected_edges: usize,
    database_bytes: usize,
    complete_snapshot_micros: u128,
    repair_micros: u128,
    append_micros: u128,
    conversation_read_micros: u128,
}

#[test]
fn same_key_activity_history_materializes_quadratic_affected_edges() {
    let smaller = observe_workload(8);
    let larger = observe_workload(16);

    for observation in [smaller, larger] {
        let history = observation.activity_replacements + 1;
        assert_eq!(observation.canonical_facts, history + 7);
        assert!(
            observation.affected_edges >= history * history.saturating_sub(1),
            "same-key support alone requires a directed clique: {observation:?}"
        );
    }
    assert!(
        larger.affected_edges >= smaller.affected_edges * 3,
        "doubling same-key activity should expose the current near-quadratic row growth: \
         smaller={smaller:?}, larger={larger:?}"
    );
}

#[test]
fn report_mixed_activity_scaling_when_explicitly_requested() {
    if std::env::var_os("HQ_STORE_SCALING_REPORT").is_none() {
        return;
    }
    let sizes = std::env::var("HQ_STORE_SCALING_SIZES")
        .ok()
        .map_or_else(|| vec![32, 64, 128], |value| parse_sizes(&value));
    println!(
        "activity_replacements,canonical_facts,affected_edges,database_bytes,\
         complete_snapshot_micros,repair_micros,append_micros,conversation_read_micros"
    );
    for size in sizes {
        let observation = observe_workload(size);
        println!(
            "{},{},{},{},{},{},{},{}",
            observation.activity_replacements,
            observation.canonical_facts,
            observation.affected_edges,
            observation.database_bytes,
            observation.complete_snapshot_micros,
            observation.repair_micros,
            observation.append_micros,
            observation.conversation_read_micros,
        );
    }
}

fn observe_workload(activity_replacements: usize) -> ScalingObservation {
    let replacements = u16::try_from(activity_replacements).expect("fixture size fits u16");
    let directory = TestDirectory::new();
    let database = directory.database_path();
    open_store(&database)
        .close()
        .expect("schema initialization closes");
    let operation = OperationId::from_bytes([0xa1; 32]);
    let facts = mixed_activity_corpus(replacements, operation);
    seed_canonical_corpus(&database, &facts);

    let store = open_store(&database);
    let started = Instant::now();
    store
        .complete_snapshot(authority_policy())
        .expect("complete snapshot succeeds");
    let complete_snapshot_micros = started.elapsed().as_micros();

    let started = Instant::now();
    store.repair(authority_policy()).expect("repair succeeds");
    let repair_micros = started.elapsed().as_micros();

    let started = Instant::now();
    store
        .append_verified(authored_agent_activity(
            2_000,
            operation,
            Some("analysis"),
            ActivityKind::Progress,
            "analysis-progress",
            u64::from(replacements) + 5,
            ActivityStatus::Running,
            "finishing",
        ))
        .expect("one later activity ingests");
    let append_micros = started.elapsed().as_micros();

    let conversation_read_micros = measure_conversation_read(&store);
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("diagnostic database opens");
    ScalingObservation {
        activity_replacements,
        canonical_facts: count(&connection, "canonical_facts"),
        affected_edges: count(&connection, "reduction_affected_dependencies"),
        database_bytes: database_bytes(&connection, &database),
        complete_snapshot_micros,
        repair_micros,
        append_micros,
        conversation_read_micros,
    }
}

fn mixed_activity_corpus(replacements: u16, operation: OperationId) -> Vec<VerifiedSemanticFact> {
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let mut facts = vec![root, verified_child(root_id)];
    facts.push(authored_agent_activity(
        1,
        operation,
        None,
        ActivityKind::AgentTurn,
        "operation",
        1,
        ActivityStatus::Running,
        "working",
    ));
    for offset in 0..replacements {
        facts.push(authored_agent_activity(
            10 + offset,
            operation,
            Some("analysis"),
            ActivityKind::Progress,
            "analysis-progress",
            u64::from(offset) + 2,
            ActivityStatus::Running,
            if offset % 2 == 0 {
                "working"
            } else {
                "checking"
            },
        ));
    }
    facts.extend([
        authored_agent_activity(
            1_000,
            operation,
            Some("compile"),
            ActivityKind::Progress,
            "compile-progress",
            u64::from(replacements) + 2,
            ActivityStatus::Running,
            "compiling",
        ),
        authored_agent_activity(
            1_001,
            operation,
            Some("tests"),
            ActivityKind::Progress,
            "test-progress",
            u64::from(replacements) + 3,
            ActivityStatus::Running,
            "testing",
        ),
        authored_agent_activity(
            1_002,
            operation,
            Some("compile"),
            ActivityKind::CompletedItem,
            "compile-result",
            u64::from(replacements) + 4,
            ActivityStatus::Succeeded,
            "compile completed",
        ),
        authored_durable_conversation_entry(1_500, false),
    ]);
    facts
}

fn measure_conversation_read(store: &hq_store::Store) -> u128 {
    let conversation = ConversationKey::ProviderSession {
        counterparty: hq_domain::MailboxAddress::new(
            authority_policy().local_installation(),
            authority_policy().local_human_mailbox(),
        ),
        provider: hq_domain::ProviderId::new("paged-provider").expect("provider validates"),
        session: hq_domain::ProviderSessionId::new("paged-session").expect("session validates"),
    };
    let started = Instant::now();
    store
        .load_conversation_entries(&conversation, 20, None)
        .expect("unrelated actor read succeeds");
    started.elapsed().as_micros()
}

fn count(connection: &Connection, table: &str) -> usize {
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("diagnostic count succeeds")
        .try_into()
        .expect("diagnostic count fits usize")
}

fn database_bytes(connection: &Connection, database: &Path) -> usize {
    let page_count = connection
        .query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))
        .expect("page count loads");
    let page_size = connection
        .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
        .expect("page size loads");
    let allocated = page_count
        .checked_mul(page_size)
        .expect("allocated bytes fit i64");
    assert_eq!(
        allocated,
        i64::try_from(std::fs::metadata(database).expect("database exists").len())
            .expect("database size fits i64")
    );
    allocated.try_into().expect("allocated bytes fit usize")
}

fn parse_sizes(value: &str) -> Vec<usize> {
    let sizes = value
        .split(',')
        .map(str::trim)
        .map(|part| part.parse::<usize>().expect("scaling size is numeric"))
        .filter(|size| *size > 0)
        .collect::<Vec<_>>();
    assert!(!sizes.is_empty(), "at least one scaling size is required");
    sizes
}
