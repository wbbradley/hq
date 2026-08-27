//! Persisted project projection contracts.

#![allow(clippy::expect_used)]

use hq_domain::{InstallationId, MailboxId, ProjectId};
use hq_reducer::{ProjectProjection, ProjectProjectionKey};
use hq_store::StoreErrorClass;
use rusqlite::Connection;

mod support;

use support::{
    TestDirectory, TestStoreExt, authority_policy, open_store, verified_account, verified_fact,
    verified_project, verified_question,
};

#[test]
fn repair_persists_every_report_exactly_and_project_rows_reopen() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    append_project_fixture(&store, true);

    let ingested = store
        .load_project_snapshot()
        .expect("ingest materializes project rows");
    assert_eq!(
        ingested,
        store
            .complete_snapshot(authority_policy())
            .expect("complete oracle succeeds")
            .project_projection_snapshot()
    );
    let repaired = store.repair(authority_policy()).expect("repair succeeds");
    assert_eq!(
        repaired.complete().project_projection_snapshot(),
        *repaired.project()
    );
    assert_eq!(
        repaired.complete().authority_projection_snapshot(),
        *repaired.authority()
    );
    assert_eq!(
        repaired.complete().conversation_projection_snapshot(),
        *repaired.conversation()
    );
    assert_eq!(
        repaired.complete().agent_projection_snapshot(),
        *repaired.agent()
    );
    assert!(matches!(
        repaired.project().projection(project_key()),
        Some(ProjectProjection::Project(_))
    ));
    assert!(
        !repaired.conversation().projections().is_empty(),
        "compacted/activity-capable conversation rows coexist with projects"
    );
    let expected = repaired.project().clone();
    assert_eq!(
        store
            .repair(authority_policy())
            .expect("repeated repair succeeds")
            .project(),
        &expected
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_project_snapshot()
            .expect("project snapshot reopens"),
        expected
    );
}

#[test]
fn project_snapshot_changes_atomically_on_ingest_and_repair_is_equal() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let account = verified_account(root_id);
    let account_id = account.verified_event().event_id();
    store.append_verified(root).expect("root appends");
    store.append_verified(account).expect("account appends");
    let before = store
        .load_project_snapshot()
        .expect("initial project snapshot loads");
    assert!(before.projections().is_empty());
    store
        .append_verified(verified_project(root_id, account_id))
        .expect("project appends");

    let after = store
        .load_project_snapshot()
        .expect("ingest updates project snapshot");
    assert_ne!(after, before);
    assert!(after.projection(project_key()).is_some());
    assert_eq!(
        store
            .repair(authority_policy())
            .expect("repair succeeds")
            .project(),
        &after
    );
}

#[test]
fn repair_replaces_project_rows_under_the_new_explicit_policy() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    append_project_fixture(&store, false);
    assert!(
        !store
            .repair(authority_policy())
            .expect("first repair succeeds")
            .project()
            .projections()
            .is_empty()
    );
    let replacement_policy = hq_reducer::AuthorityPolicy::new(
        InstallationId::from_bytes([0x22; 32]),
        MailboxId::from_bytes([0x44; 32]),
    );

    let replacement = store
        .repair(replacement_policy)
        .expect("replacement repair succeeds");
    assert_eq!(replacement.complete().policy(), replacement_policy);
    assert_eq!(
        replacement.complete().project_projection_snapshot(),
        *replacement.project()
    );
    assert_eq!(
        store
            .load_project_snapshot()
            .expect("replacement project snapshot loads"),
        *replacement.project()
    );
}

#[test]
fn project_corruption_fails_closed_until_explicit_repair() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    append_project_fixture(&store, false);
    store.repair(authority_policy()).expect("repair succeeds");
    store.close().expect("store closes");

    let connection = Connection::open(&database).expect("corruption connection opens");
    connection
        .execute("UPDATE project_projects SET name = 'changed-project'", [])
        .expect("valid-looking corruption writes");
    drop(connection);

    let reopened = open_store(&database);
    assert_eq!(
        reopened
            .load_project_snapshot()
            .expect_err("changed projection rejects")
            .class(),
        StoreErrorClass::RebuildableStateCorrupt
    );
    let repaired = reopened
        .repair(authority_policy())
        .expect("repair recovers project rows");
    assert_eq!(
        reopened
            .load_project_snapshot()
            .expect("project snapshot reloads"),
        *repaired.project()
    );
}

fn append_project_fixture(store: &hq_store::Store, conversation: bool) {
    let root = verified_fact();
    let root_id = root.verified_event().event_id();
    let account = verified_account(root_id);
    let account_id = account.verified_event().event_id();
    store.append_verified(root).expect("root appends");
    store.append_verified(account).expect("account appends");
    if conversation {
        store
            .append_verified(verified_question(root_id))
            .expect("conversation appends");
    }
    store
        .append_verified(verified_project(root_id, account_id))
        .expect("project appends");
}

fn project_key() -> ProjectProjectionKey {
    ProjectProjectionKey::Project(ProjectId::from_bytes([0x66; 32]))
}
