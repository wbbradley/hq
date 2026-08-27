//! Application-owned query and mutation contracts over the durable store adapter.

#![allow(clippy::expect_used)]

use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use hq_application::{
    ApplicationErrorClass, CommitFacts, FactMutation, FactPlan, MutationAttempt, MutationDecision,
    MutationOutcome, QueryDomain,
};
use hq_domain::{CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode, Revision};
use hq_store::StoreGateway;
use rusqlite::Connection;

mod support;

use support::{
    TestDirectory, TestStoreExt, authority_policy, open_store, signer, verified_child,
    verified_fact, verified_question,
};

#[test]
fn authoritative_snapshot_is_one_revisioned_application_view() -> Result<(), Box<dyn Error>> {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let question = verified_question(root.verified_event().event_id());
    let question_id = question.fact().id();
    store.append_verified(root)?;
    store.append_verified(question)?;
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));

    let snapshot = gateway.authoritative_snapshot()?;

    assert_eq!(snapshot.revision(), Revision::new(2));
    assert_eq!(
        snapshot.domain().authority(),
        &store.load_authority_snapshot()?
    );
    assert_eq!(
        snapshot.domain().conversation(),
        &store.load_conversation_snapshot()?
    );
    assert_eq!(snapshot.domain().agent(), &store.load_agent_snapshot()?);
    assert_eq!(snapshot.domain().project(), &store.load_project_snapshot()?);
    assert_eq!(snapshot.conversations().len(), 1);
    assert_eq!(snapshot.conversations()[0].latest_fact, Some(question_id));
    assert_eq!(snapshot.conversations()[0].open_messages, 1);
    Ok(())
}

#[test]
fn gateway_executes_pure_fact_plan_and_replays_without_redeciding() -> Result<(), Box<dyn Error>> {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let child = verified_child(root.verified_event().event_id());
    store.append_verified(root)?;
    let fact = child.fact().clone();
    let decisions = Arc::new(AtomicUsize::new(0));
    let first_counter = Arc::clone(&decisions);
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));
    let command_id = CommandId::from_bytes([0xa1; 32]);
    let digest = CommandDigest::from_bytes([0xa2; 32]);

    let first = gateway.commit_facts(FactMutation::new(command_id, digest, move |_| {
        first_counter.fetch_add(1, Ordering::Relaxed);
        MutationDecision::commit(FactPlan::new(
            fact.author().installation_id(),
            fact.authored_at(),
            fact.scope().clone(),
            fact.causal().clone(),
            fact.payload().clone(),
            [0xa3; 32],
        ))
    }))?;
    let second_counter = Arc::clone(&decisions);
    let second = gateway.commit_facts(FactMutation::new(command_id, digest, move |_| {
        second_counter.fetch_add(1, Ordering::Relaxed);
        MutationDecision::reject(DomainError::new(
            ErrorCategory::InvariantViolation,
            ErrorCode::new("must_not_decide").expect("fixture code validates"),
        ))
    }))?;

    assert_eq!(first, second);
    assert_eq!(decisions.load(Ordering::Relaxed), 1);
    let MutationAttempt::Completed(receipt) = first else {
        return Err("synchronous store result must be complete".into());
    };
    assert_eq!(receipt.outcome(), &MutationOutcome::Committed);
    assert_eq!(receipt.revision(), Revision::new(2));
    assert_eq!(store.load_corpus()?.len(), 2);

    let conflict_counter = Arc::clone(&decisions);
    let conflict = gateway
        .commit_facts(FactMutation::new(
            command_id,
            CommandDigest::from_bytes([0xa4; 32]),
            move |_| {
                conflict_counter.fetch_add(1, Ordering::Relaxed);
                MutationDecision::reject(DomainError::new(
                    ErrorCategory::InvariantViolation,
                    ErrorCode::new("must_not_decide").expect("fixture code validates"),
                ))
            },
        ))
        .expect_err("changed digest conflicts before decision");
    assert_eq!(conflict.class(), ApplicationErrorClass::Conflict);
    assert_eq!(decisions.load(Ordering::Relaxed), 1);
    Ok(())
}

#[test]
fn gateway_rejects_noncanonical_retained_application_result() -> Result<(), Box<dyn Error>> {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let store = open_store(&database);
    let command_id = CommandId::from_bytes([0xb1; 32]);
    let digest = CommandDigest::from_bytes([0xb2; 32]);
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));
    gateway.commit_facts(FactMutation::new(command_id, digest, |_| {
        MutationDecision::reject(DomainError::new(
            ErrorCategory::Unauthorized,
            ErrorCode::new("authority_missing").expect("fixture code validates"),
        ))
    }))?;
    drop(gateway);
    store.close()?;

    let connection = Connection::open(&database)?;
    connection.execute(
        "UPDATE mutation_receipts SET result_bytes = X'0100FF' WHERE command_id = ?1",
        [command_id.as_bytes().as_slice()],
    )?;
    drop(connection);

    let reopened = open_store(&database);
    let gateway = StoreGateway::new(&reopened, authority_policy(), Arc::new(signer(1)));
    let error = gateway
        .commit_facts(FactMutation::new(command_id, digest, |_| {
            MutationDecision::reject(DomainError::new(
                ErrorCategory::InvariantViolation,
                ErrorCode::new("must_not_decide").expect("fixture code validates"),
            ))
        }))
        .expect_err("invalid application result rejects");
    assert_eq!(error.class(), ApplicationErrorClass::CorruptState);
    Ok(())
}
