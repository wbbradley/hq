//! Durable non-home routing and immutable-home execution contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use hq_application::{
    ApplicationError, ControlProjects, ProjectCommandAction, ProjectCommandOutcome,
    ProjectCommandRequest, ProjectCommandStage,
};
use hq_domain::{
    AccountId, CommandDigest, CommandId, ErrorCategory, ErrorCode, FactId, InstallationId,
    OperationId, ProjectId, RemoteCommandResult, RuntimeObservation, Timestamp,
};
use hq_projects::{
    ProjectCommandRouter, RemoteProjectCommandPort, RemoteProjectCommandProgress,
    RemoteProjectCommandRecord, RemoteProjectFactOutcome, project_command_request_digest,
};

#[derive(Clone)]
struct ScriptedLocal {
    calls: Rc<Cell<usize>>,
    outcome: ProjectCommandOutcome,
}

impl ControlProjects for ScriptedLocal {
    fn control_project(
        &self,
        _request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.calls.set(self.calls.get() + 1);
        Ok(self.outcome.clone())
    }
}

#[derive(Clone, Default)]
struct MemoryRemote {
    record: Rc<RefCell<Option<RemoteProjectCommandRecord>>>,
    request_calls: Rc<Cell<usize>>,
    receipt_calls: Rc<Cell<usize>>,
    outcome_calls: Rc<Cell<usize>>,
    lose_request_response_once: Rc<Cell<bool>>,
}

impl RemoteProjectCommandPort for MemoryRemote {
    fn command(
        &self,
        command_id: CommandId,
    ) -> Result<Option<RemoteProjectCommandRecord>, ApplicationError> {
        Ok(self
            .record
            .borrow()
            .as_ref()
            .filter(|record| record.request.command_id == command_id)
            .cloned())
    }

    fn pending(
        &self,
        home: InstallationId,
        limit: usize,
    ) -> Result<Vec<RemoteProjectCommandRecord>, ApplicationError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .record
            .borrow()
            .as_ref()
            .filter(|record| {
                record.request.home == home
                    && matches!(
                        record.progress,
                        RemoteProjectCommandProgress::Queued
                            | RemoteProjectCommandProgress::Received { .. }
                    )
            })
            .cloned()
            .into_iter()
            .collect())
    }

    fn author_request(
        &self,
        request: &ProjectCommandRequest,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError> {
        self.request_calls.set(self.request_calls.get() + 1);
        *self.record.borrow_mut() = Some(RemoteProjectCommandRecord {
            request: request.clone(),
            request_fact: FactId::from_bytes([31; 32]),
            progress: RemoteProjectCommandProgress::Queued,
        });
        if self.lose_request_response_once.replace(false) {
            Ok(RemoteProjectFactOutcome::Uncertain)
        } else {
            Ok(RemoteProjectFactOutcome::Committed)
        }
    }

    fn author_receipt(
        &self,
        command_id: CommandId,
        received_at: Timestamp,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError> {
        self.receipt_calls.set(self.receipt_calls.get() + 1);
        let mut retained = self.record.borrow_mut();
        let record = retained.as_mut().expect("queued record");
        assert_eq!(record.request.command_id, command_id);
        record.progress = RemoteProjectCommandProgress::Received {
            receipt_fact: FactId::from_bytes([32; 32]),
            received_head: record
                .request
                .expected_head
                .expect("remote commands target an existing project"),
            received_at,
        };
        Ok(RemoteProjectFactOutcome::Committed)
    }

    fn author_outcome(
        &self,
        command_id: CommandId,
        result: RemoteCommandResult,
        runtime: Option<RuntimeObservation>,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError> {
        self.outcome_calls.set(self.outcome_calls.get() + 1);
        let mut retained = self.record.borrow_mut();
        let record = retained.as_mut().expect("received record");
        assert_eq!(record.request.command_id, command_id);
        let RemoteProjectCommandProgress::Received {
            receipt_fact,
            received_head,
            received_at,
        } = record.progress.clone()
        else {
            panic!("outcome must follow receipt");
        };
        record.progress = RemoteProjectCommandProgress::Terminal {
            receipt_fact,
            received_head,
            received_at,
            outcome_fact: FactId::from_bytes([33; 32]),
            result,
            runtime,
        };
        Ok(RemoteProjectFactOutcome::Committed)
    }
}

#[test]
fn non_home_intake_authors_only_one_request_and_repairs_a_lost_response() {
    let request = request(InstallationId::from_bytes([8; 32]));
    let remote = MemoryRemote::default();
    remote.lose_request_response_once.set(true);
    let local = ScriptedLocal {
        calls: Rc::new(Cell::new(0)),
        outcome: completed(&request, None),
    };
    let router = ProjectCommandRouter::new(
        InstallationId::from_bytes([9; 32]),
        local.clone(),
        remote.clone(),
    );

    let first = router
        .control_project(request.clone())
        .expect("lost request response");
    assert!(matches!(
        first,
        ProjectCommandOutcome::Reconcilable {
            stage: ProjectCommandStage::AwaitingHome,
            ..
        }
    ));
    let repaired = router.control_project(request).expect("request repair");
    assert!(matches!(
        repaired,
        ProjectCommandOutcome::Accepted {
            stage: ProjectCommandStage::AwaitingHome,
            ..
        }
    ));
    assert_eq!(remote.request_calls.get(), 1);
    assert_eq!(local.calls.get(), 0);
}

#[test]
fn immutable_home_receipts_before_execution_and_authors_one_typed_outcome() {
    let home = InstallationId::from_bytes([8; 32]);
    let request = request(home);
    let remote = MemoryRemote::default();
    remote
        .author_request(&request)
        .expect("seed queued request");
    let local = ScriptedLocal {
        calls: Rc::new(Cell::new(0)),
        outcome: completed(
            &request,
            Some(RuntimeObservation::Uncertain(
                ErrorCode::new("runtime-stop-unknown").expect("code"),
            )),
        ),
    };
    let router = ProjectCommandRouter::new(home, local.clone(), remote.clone());

    let repaired = router
        .repair_remote(Timestamp::from_unix_millis(99), 4)
        .expect("home repair");
    assert_eq!(repaired.len(), 1);
    assert!(matches!(
        repaired[0],
        ProjectCommandOutcome::Completed { .. }
    ));
    assert_eq!(remote.receipt_calls.get(), 1);
    assert_eq!(local.calls.get(), 1);
    assert_eq!(remote.outcome_calls.get(), 1);
    let terminal = remote
        .command(request.command_id)
        .expect("lookup")
        .expect("record");
    assert!(matches!(
        terminal.progress,
        RemoteProjectCommandProgress::Terminal {
            result: RemoteCommandResult::Committed(head),
            runtime: Some(RuntimeObservation::Uncertain(_)),
            ..
        } if Some(head) == request.expected_head
    ));
}

#[test]
fn restarted_home_repairs_a_retained_receipt_without_authoring_another() {
    let home = InstallationId::from_bytes([8; 32]);
    let request = request(home);
    let remote = MemoryRemote::default();
    remote.author_request(&request).expect("seed request");
    remote
        .author_receipt(request.command_id, Timestamp::from_unix_millis(88))
        .expect("seed receipt before restart");
    let local = ScriptedLocal {
        calls: Rc::new(Cell::new(0)),
        outcome: completed(&request, None),
    };

    let restarted = ProjectCommandRouter::new(home, local.clone(), remote.clone());
    let outcomes = restarted
        .repair_remote(Timestamp::from_unix_millis(99), 4)
        .expect("restart repair");

    assert_eq!(outcomes, vec![completed(&request, None)]);
    assert_eq!(
        remote.receipt_calls.get(),
        1,
        "the retained receipt replays"
    );
    assert_eq!(local.calls.get(), 1);
    assert_eq!(remote.outcome_calls.get(), 1);
}

#[test]
fn changed_digest_reuse_and_malformed_request_digest_fail_before_routing() {
    let home = InstallationId::from_bytes([8; 32]);
    let request = request(home);
    let remote = MemoryRemote::default();
    let local = ScriptedLocal {
        calls: Rc::new(Cell::new(0)),
        outcome: completed(&request, None),
    };
    let router = ProjectCommandRouter::new(
        InstallationId::from_bytes([9; 32]),
        local.clone(),
        remote.clone(),
    );
    router
        .control_project(request.clone())
        .expect("first request");

    let mut changed = request;
    changed.request_digest = CommandDigest::from_bytes([0xff; 32]);
    let ProjectCommandOutcome::Rejected { error, .. } = router
        .control_project(changed)
        .expect("changed request rejection")
    else {
        panic!("expected rejection");
    };
    assert_eq!(error.category(), ErrorCategory::InvalidInput);
    assert_eq!(remote.request_calls.get(), 1);
    assert_eq!(local.calls.get(), 0);
}

fn request(home: InstallationId) -> ProjectCommandRequest {
    let mut request = ProjectCommandRequest {
        command_id: CommandId::from_bytes([1; 32]),
        operation_id: OperationId::from_bytes([2; 32]),
        request_digest: CommandDigest::from_bytes([0; 32]),
        account_id: AccountId::from_bytes([3; 32]),
        project_id: ProjectId::from_bytes([4; 32]),
        home,
        expected_head: Some(FactId::from_bytes([5; 32])),
        issued_at: Timestamp::from_unix_millis(6),
        action: ProjectCommandAction::Open,
    };
    request.request_digest = project_command_request_digest(&request).expect("request digest");
    request
}

fn completed(
    request: &ProjectCommandRequest,
    runtime: Option<RuntimeObservation>,
) -> ProjectCommandOutcome {
    ProjectCommandOutcome::Completed {
        operation_id: request.operation_id,
        project_head: request
            .expected_head
            .expect("remote commands target an existing project"),
        runtime,
    }
}
