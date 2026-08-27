//! Complete-batch snapshot and rebuildable-index repair contracts.

#![allow(clippy::expect_used)]

use hq_reducer::{
    AgentReducer, AuthorityReducer, ConversationReducer, DecisionStatus, ProjectReducer,
    reduce_complete,
};
use hq_store::{ReductionDomain, StoreErrorClass};
use rusqlite::Connection;

mod support;

use support::{
    TestDirectory, TestStoreExt, authority_policy, open_store, verified_child, verified_fact,
    verified_fact_with_label,
};

#[test]
fn empty_complete_snapshot_has_four_empty_reports_and_can_be_repaired() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let snapshot = store
        .complete_snapshot(authority_policy())
        .expect("empty snapshot reduces");

    assert_eq!(snapshot.policy(), authority_policy());
    assert_eq!(snapshot.authority().facts().facts().count(), 0);
    assert_eq!(snapshot.conversation().facts().facts().count(), 0);
    assert_eq!(snapshot.agent().facts().facts().count(), 0);
    assert_eq!(snapshot.project().facts().facts().count(), 0);
    let repaired = store
        .repair(authority_policy())
        .expect("empty repair succeeds");
    assert_eq!(repaired.persisted(), &snapshot.normalized_index());
    assert_eq!(
        repaired.conversation(),
        &snapshot.conversation_projection_snapshot()
    );
    assert_eq!(repaired.agent(), &snapshot.agent_projection_snapshot());
    assert_eq!(repaired.project(), &snapshot.project_projection_snapshot());
}

#[test]
fn complete_snapshot_runs_every_reducer_from_one_reverified_corpus() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    store.append_verified(root).expect("root appends");

    let snapshot = store
        .complete_snapshot(authority_policy())
        .expect("complete snapshot reduces");
    let semantic = store
        .load_corpus()
        .expect("same corpus reloads")
        .iter()
        .map(|fact| fact.fact().clone())
        .collect::<Vec<_>>();
    assert_eq!(
        snapshot.authority(),
        &reduce_complete(semantic.clone(), &AuthorityReducer::new(authority_policy()))
            .expect("direct authority reduction succeeds")
    );
    assert_eq!(
        snapshot.conversation(),
        &reduce_complete(
            semantic.clone(),
            &ConversationReducer::new(authority_policy())
        )
        .expect("direct conversation reduction succeeds")
    );
    assert_eq!(
        snapshot.agent(),
        &reduce_complete(semantic.clone(), &AgentReducer::new(authority_policy()))
            .expect("direct agent reduction succeeds")
    );
    assert_eq!(
        snapshot.project(),
        &reduce_complete(semantic, &ProjectReducer::new(authority_policy()))
            .expect("direct project reduction succeeds")
    );
    assert_eq!(snapshot.authority().facts().facts().count(), 1);
    assert_eq!(snapshot.conversation().facts().facts().count(), 1);
    assert_eq!(snapshot.agent().facts().facts().count(), 1);
    assert_eq!(snapshot.project().facts().facts().count(), 1);
    for domain in ReductionDomain::ALL {
        assert_eq!(
            snapshot
                .normalized_index()
                .decision(domain, hq_domain::FactId::from_bytes(root_id))
                .expect("root decision exists")
                .status(),
            DecisionStatus::Projected
        );
    }
}

#[test]
fn unique_root_conflicts_round_trip_with_all_participants() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let alpha = verified_fact_with_label("alpha", [7; 32]);
    let beta = verified_fact_with_label("beta", [8; 32]);
    let alpha_id = hq_domain::FactId::from_bytes(alpha.verified_event().event_id());
    let beta_id = hq_domain::FactId::from_bytes(beta.verified_event().event_id());
    let child = verified_child(alpha.verified_event().event_id());
    let child_id = hq_domain::FactId::from_bytes(child.verified_event().event_id());
    store.append_verified(alpha).expect("alpha root appends");
    store.append_verified(beta).expect("beta root appends");
    store
        .append_verified(child)
        .expect("dependent child appends");

    let repaired = store.repair(authority_policy()).expect("repair succeeds");
    let index = repaired.persisted();
    for fact_id in [alpha_id, beta_id] {
        assert_eq!(
            index
                .decision(ReductionDomain::Authority, fact_id)
                .expect("authority decision exists")
                .status(),
            DecisionStatus::Conflicted
        );
    }
    let conflict = index
        .conflicts(ReductionDomain::Authority)
        .first()
        .expect("unique-root conflict exists");
    assert_eq!(
        conflict.participants(),
        &[alpha_id, beta_id].into_iter().collect()
    );
    let child_decision = index
        .decision(ReductionDomain::Authority, child_id)
        .expect("dependent decision exists");
    assert_eq!(
        child_decision.unusable_dependencies().get(&alpha_id),
        Some(&DecisionStatus::Conflicted)
    );
    assert_eq!(
        store.load_reduction_index().expect("conflict reloads"),
        *index
    );
}

#[test]
fn repair_is_explicit_idempotent_and_survives_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let unrepaired = store
        .load_reduction_index()
        .expect_err("fresh structural index is absent");
    assert_eq!(unrepaired.class(), StoreErrorClass::NotRepaired);
    store
        .append_verified(verified_fact())
        .expect("root appends");

    let first = store
        .repair(authority_policy())
        .expect("first repair succeeds");
    assert_eq!(first.complete().normalized_index(), *first.persisted());
    let second = store
        .repair(authority_policy())
        .expect("repeated repair succeeds");
    assert_eq!(first.persisted(), second.persisted());
    assert_eq!(
        store.load_reduction_index().expect("persisted index loads"),
        *second.persisted()
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_reduction_index()
            .expect("index survives reopen"),
        *second.persisted()
    );
}

#[test]
fn late_parent_reduces_atomically_and_matches_explicit_repair() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let child = verified_child(root_id);
    let child_id = hq_domain::FactId::from_bytes(child.verified_event().event_id());
    store.append_verified(child).expect("child arrives first");

    let before = store
        .repair(authority_policy())
        .expect("initial repair succeeds");
    assert_eq!(
        before
            .persisted()
            .decision(ReductionDomain::Authority, child_id)
            .expect("child decision exists")
            .status(),
        DecisionStatus::Unresolved
    );
    store.append_verified(root).expect("late root appends");
    assert_eq!(
        store
            .load_reduction_index()
            .expect("ingest updates index")
            .decision(ReductionDomain::Authority, child_id)
            .expect("child decision exists")
            .status(),
        DecisionStatus::Projected
    );

    let after = store
        .repair(authority_policy())
        .expect("late repair succeeds");
    assert_eq!(
        after
            .persisted()
            .decision(ReductionDomain::Authority, child_id)
            .expect("child decision updates")
            .status(),
        DecisionStatus::Projected
    );
}

#[test]
fn repair_never_changes_immutable_corpus_rows_or_bytes() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    store
        .append_verified(verified_fact())
        .expect("root appends");
    store.close().expect("store closes for inspection");
    let before = immutable_corpus(&database);

    let reopened = open_store(&database);
    reopened
        .repair(authority_policy())
        .expect("repair succeeds");
    reopened.close().expect("store recloses");
    assert_eq!(immutable_corpus(&database), before);
}

#[test]
fn changed_rebuildable_rows_fail_closed_until_repair() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    store
        .append_verified(verified_fact())
        .expect("root appends");
    store.repair(authority_policy()).expect("repair succeeds");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("test corruption connection opens");
    connection
        .execute(
            "UPDATE reduction_decisions SET status = 2 WHERE domain = 1",
            [],
        )
        .expect("valid-looking status corruption writes");
    drop(connection);

    let reopened = open_store(&database);
    let corrupt = reopened
        .load_reduction_index()
        .expect_err("digest mismatch rejects");
    assert_eq!(corrupt.class(), StoreErrorClass::RebuildableStateCorrupt);
    let repaired = reopened
        .repair(authority_policy())
        .expect("explicit repair recovers");
    assert_eq!(
        reopened
            .load_reduction_index()
            .expect("repaired index loads"),
        *repaired.persisted()
    );
}

#[test]
fn partial_unknown_oversized_and_cross_domain_rows_fail_closed() {
    for mutation in [
        "DELETE FROM reduction_state",
        "UPDATE reduction_state SET decision_count = 64000001",
        "UPDATE reduction_decisions SET reason_code = 9999, reason_parameter = 0 WHERE domain = 1",
        "UPDATE reduction_decisions SET reason_code = 3101, reason_parameter = 0 WHERE domain = 1",
        "UPDATE reduction_dependency_order SET position = 99 WHERE domain = 1",
        "DELETE FROM reduction_affected_dependencies",
        "UPDATE reduction_missing_dependencies SET dependency_id = zeroblob(32) WHERE domain = 1",
    ] {
        assert_rebuildable_corruption(mutation);
    }
}

fn assert_rebuildable_corruption(mutation: &str) {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let missing_root = verified_fact();
    store
        .append_verified(verified_child(missing_root.verified_event().event_id()))
        .expect("unresolved child appends");
    store.repair(authority_policy()).expect("repair succeeds");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("corruption connection opens");
    connection
        .execute(mutation, [])
        .expect("valid-looking structural corruption writes");
    drop(connection);

    let reopened = open_store(&database);
    let error = reopened
        .load_reduction_index()
        .expect_err("structural corruption rejects");
    assert_eq!(error.class(), StoreErrorClass::RebuildableStateCorrupt);
}

fn immutable_corpus(path: &std::path::Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    let connection = Connection::open(path).expect("inspection connection opens");
    let mut statement = connection
        .prepare("SELECT fact_id, event_bytes FROM canonical_facts ORDER BY fact_id")
        .expect("corpus query prepares");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("corpus rows query")
        .map(|row| row.expect("corpus row decodes"))
        .collect()
}
