//! Durable project saga intake contract.

#![allow(clippy::expect_used)]

use std::{cell::RefCell, collections::BTreeMap};

use hq_application::{
    ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage,
};
use hq_domain::{
    AccountId, CommandDigest, CommandId, FactId, InstallationId, OperationId, ProjectId, Timestamp,
};
use hq_projects::{
    BeginSagaOutcome, ProjectSagaManager, ProjectSagaRecord, ProjectSagaState, ProjectSagaStore,
    SagaStoreError,
};

#[derive(Default)]
struct MemoryStore {
    records: RefCell<BTreeMap<OperationId, ProjectSagaRecord>>,
}

impl ProjectSagaStore for MemoryStore {
    fn begin(&self, record: ProjectSagaRecord) -> Result<BeginSagaOutcome, SagaStoreError> {
        let mut records = self.records.borrow_mut();
        if let Some(existing) = records.get(&record.operation_id) {
            return Ok(if existing.request_digest == record.request_digest {
                BeginSagaOutcome::Existing(existing.clone())
            } else {
                BeginSagaOutcome::IdentityConflict
            });
        }
        if records.values().any(|existing| {
            existing.project_id == record.project_id && !existing.state.is_terminal()
        }) {
            return Ok(BeginSagaOutcome::ProjectBusy);
        }
        records.insert(record.operation_id, record.clone());
        Ok(BeginSagaOutcome::Inserted(record))
    }

    fn replace(&self, record: ProjectSagaRecord) -> Result<(), SagaStoreError> {
        self.records
            .borrow_mut()
            .insert(record.operation_id, record);
        Ok(())
    }

    fn runnable(&self, limit: usize) -> Result<Vec<ProjectSagaRecord>, SagaStoreError> {
        Ok(self
            .records
            .borrow()
            .values()
            .filter(|record| !record.state.is_terminal())
            .take(limit)
            .cloned()
            .collect())
    }
}

fn request(operation: u8, digest: u8, project: u8) -> ProjectCommandRequest {
    ProjectCommandRequest {
        command_id: CommandId::from_bytes([operation; 32]),
        operation_id: OperationId::from_bytes([operation; 32]),
        request_digest: CommandDigest::from_bytes([digest; 32]),
        account_id: AccountId::from_bytes([1; 32]),
        project_id: ProjectId::from_bytes([project; 32]),
        home: InstallationId::from_bytes([2; 32]),
        expected_head: Some(FactId::from_bytes([3; 32])),
        issued_at: Timestamp::from_unix_millis(4),
        action: ProjectCommandAction::Open,
    }
}

#[test]
fn exact_replay_returns_the_same_checkpoint_and_changed_digest_conflicts() {
    let manager = ProjectSagaManager::new(MemoryStore::default());

    let first = manager.accept(request(10, 11, 12)).expect("first intake");
    let replay = manager.accept(request(10, 11, 12)).expect("exact replay");
    let collision = manager
        .accept(request(10, 99, 12))
        .expect("typed collision");

    assert_eq!(first, replay);
    assert_eq!(
        first,
        ProjectCommandOutcome::Accepted {
            operation_id: OperationId::from_bytes([10; 32]),
            stage: ProjectCommandStage::Accepted,
        }
    );
    assert!(matches!(collision, ProjectCommandOutcome::Rejected { .. }));
}

#[test]
fn only_one_unresolved_command_is_accepted_per_project() {
    let manager = ProjectSagaManager::new(MemoryStore::default());

    manager.accept(request(10, 11, 12)).expect("first intake");
    let competing = manager
        .accept(request(20, 21, 12))
        .expect("typed busy result");
    let independent = manager.accept(request(30, 31, 32)).expect("other project");

    assert!(matches!(competing, ProjectCommandOutcome::Rejected { .. }));
    assert!(matches!(
        independent,
        ProjectCommandOutcome::Accepted { .. }
    ));
}

#[test]
fn recovery_scan_is_bounded_and_excludes_terminal_records() {
    let store = MemoryStore::default();
    let mut terminal = ProjectSagaRecord::from_request(request(30, 31, 32));
    terminal.state = ProjectSagaState::Completed {
        project_head: FactId::from_bytes([33; 32]),
    };
    store.begin(terminal).expect("terminal insert");
    let manager = ProjectSagaManager::new(store);
    manager.accept(request(10, 11, 12)).expect("first intake");
    manager.accept(request(20, 21, 22)).expect("second intake");

    let runnable = manager.runnable(1).expect("bounded recovery scan");
    assert_eq!(runnable.len(), 1);
    assert!(!runnable[0].state.is_terminal());
}
