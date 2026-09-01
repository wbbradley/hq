//! Application-owned query and mutation contracts over the durable store adapter.

#![allow(clippy::expect_used)]

use std::{
    collections::BTreeSet,
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use hq_application::{
    ApplicationErrorClass, CanonicalEvidence, CommitFacts, ConversationPageSelection, FactMutation,
    FactPlan, HealthDomain, MutationAttempt, MutationDecision, MutationOutcome, QueryDomain,
};
use hq_domain::{
    CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode, FactId, OperationId, Revision,
};
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
    assert_eq!(snapshot.conversations()[0].archived_messages, 0);
    assert_eq!(snapshot.conversations()[0].sent_messages, 1);
    Ok(())
}

#[test]
fn authoritative_conversation_view_pairs_one_snapshot_and_page_from_the_store_actor()
-> Result<(), Box<dyn Error>> {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let question = verified_question(root.verified_event().event_id());
    let question_id = question.fact().id();
    store.append_verified(question)?;
    store.append_verified(root)?;
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));
    let snapshot = gateway.authoritative_snapshot()?;
    let key = snapshot.conversations()[0].key.clone();
    let selection = ConversationPageSelection::new(key.clone(), 10)?;

    let selected = gateway.authoritative_conversation_view(Some(&selection))?;

    assert_eq!(selected.snapshot().revision(), Revision::new(2));
    let conversation = selected.conversation().expect("selected page");
    assert_eq!(conversation.key(), &key);
    assert!(conversation.page().items().iter().any(|entry| {
        matches!(entry, hq_application::ConversationEntry::Message(message)
            if message.message.fact_id == question_id)
    }));

    let unselected = gateway.authoritative_conversation_view(None)?;
    assert_eq!(unselected.snapshot(), selected.snapshot());
    assert!(unselected.conversation().is_none());
    assert!(ConversationPageSelection::new(key.clone(), 0).is_err());
    assert!(ConversationPageSelection::new(key, 201).is_err());
    Ok(())
}

#[test]
fn health_and_repair_report_every_domain_at_the_observed_revision() -> Result<(), Box<dyn Error>> {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    store.append_verified(verified_fact())?;
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));

    let health = gateway.state_health()?;
    assert_eq!(health.revision, Revision::new(1));
    assert_eq!(
        health
            .domains
            .iter()
            .map(|domain| domain.domain)
            .collect::<Vec<_>>(),
        vec![
            HealthDomain::Authority,
            HealthDomain::Conversation,
            HealthDomain::Agent,
            HealthDomain::Project,
        ]
    );

    let operation_id = OperationId::from_bytes([0xc1; 32]);
    let repaired = gateway.repair_state(operation_id)?;
    assert_eq!(repaired.operation_id, operation_id);
    assert_eq!(repaired.revision, Revision::new(1));
    assert_eq!(repaired.domains, health.domains);
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

#[test]
fn canonical_evidence_query_returns_the_exact_bounded_transitive_closure()
-> Result<(), Box<dyn Error>> {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let root = verified_fact();
    let root_id = root.fact().id();
    let root_exact = root.verified_event().exact_event_bytes().to_vec();
    let child = verified_child(root.verified_event().event_id());
    let child_id = child.fact().id();
    let child_exact = child.verified_event().exact_event_bytes().to_vec();
    store.append_verified(root)?;
    store.append_verified(child)?;
    let gateway = StoreGateway::new(&store, authority_policy(), Arc::new(signer(1)));

    let evidence = gateway.canonical_evidence(
        &BTreeSet::from([child_id]),
        2,
        root_exact.len() + child_exact.len(),
    )?;

    let mut expected = vec![(root_id, root_exact), (child_id, child_exact)];
    expected.sort_by_key(|(fact_id, _)| *fact_id);
    assert_eq!(
        evidence
            .iter()
            .map(|item| (item.fact_id, item.exact_event.clone()))
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        gateway
            .canonical_evidence(&BTreeSet::from([child_id]), 1, usize::MAX)
            .expect_err("fact limit is strict")
            .class(),
        ApplicationErrorClass::InvalidInput
    );
    assert_eq!(
        gateway
            .canonical_evidence(&BTreeSet::from([child_id]), 2, 1)
            .expect_err("byte limit is strict")
            .class(),
        ApplicationErrorClass::InvalidInput
    );
    assert_eq!(
        gateway
            .canonical_evidence(
                &BTreeSet::from([FactId::from_bytes([0xff; 32])]),
                2,
                usize::MAX,
            )
            .expect_err("unknown roots are rejected")
            .class(),
        ApplicationErrorClass::InvalidInput
    );
    Ok(())
}

#[test]
fn evidence_import_reverifies_the_whole_batch_and_is_idempotent() -> Result<(), Box<dyn Error>> {
    let source_directory = TestDirectory::new();
    let source = open_store(&source_directory.database_path());
    let root = verified_fact();
    let root_id = root.fact().id();
    let root_exact = root.verified_event().exact_event_bytes().to_vec();
    let child = verified_child(root.verified_event().event_id());
    let child_id = child.fact().id();
    let child_exact = child.verified_event().exact_event_bytes().to_vec();
    source.append_verified(root)?;
    source.append_verified(child)?;
    let source_gateway = StoreGateway::new(&source, authority_policy(), Arc::new(signer(1)));
    let evidence = source_gateway.canonical_evidence(
        &BTreeSet::from([child_id]),
        2,
        root_exact.len() + child_exact.len(),
    )?;

    let destination_directory = TestDirectory::new();
    let destination = open_store(&destination_directory.database_path());
    let destination_gateway =
        StoreGateway::new(&destination, authority_policy(), Arc::new(signer(1)));
    let first = destination_gateway.ingest_canonical_evidence(&evidence)?;
    assert_eq!(first.len(), 2);
    assert!(first.iter().all(|outcome| outcome.inserted));
    assert_eq!(destination.load_corpus()?.len(), 2);

    let second = destination_gateway.ingest_canonical_evidence(&evidence)?;
    assert_eq!(second.len(), 2);
    assert!(second.iter().all(|outcome| !outcome.inserted));
    assert_eq!(destination.load_corpus()?.len(), 2);

    let invalid_directory = TestDirectory::new();
    let invalid_destination = open_store(&invalid_directory.database_path());
    let invalid_gateway = StoreGateway::new(
        &invalid_destination,
        authority_policy(),
        Arc::new(signer(1)),
    );
    let mut invalid = evidence.clone();
    invalid[1] = CanonicalEvidence {
        fact_id: invalid[1].fact_id,
        exact_event: b"{}".to_vec(),
    };
    assert_eq!(
        invalid_gateway
            .ingest_canonical_evidence(&invalid)
            .expect_err("one invalid event rejects the whole batch before insertion")
            .class(),
        ApplicationErrorClass::InvalidInput
    );
    assert!(invalid_destination.load_corpus()?.is_empty());

    assert!(evidence.iter().any(|item| item.fact_id == root_id));
    Ok(())
}
