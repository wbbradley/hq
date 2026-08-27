//! Deterministic generator contract tests.

use hq_domain::{
    AccountId, FactKind, FactScope, InstallationAddress, InstallationId, SemanticFactError,
    SemanticPayload, SigningPublicKey,
};
use hq_testkit::{
    DeterministicClock, DeterministicValues, FactBuilder, StateMachineSequence,
    arrival_permutations,
};

#[test]
fn equal_seeds_replay_identical_values_and_forks_are_independent() {
    let mut left = DeterministicValues::new(17);
    let mut right = DeterministicValues::new(17);
    assert_eq!(left.fact_id(), right.fact_id());
    assert_eq!(left.signing_key(), right.signing_key());

    let mut fork = left.fork();
    assert_eq!(left.project_id(), fork.project_id());
    let _ = left.fact_id();
    assert_ne!(left.fact_id(), fork.fact_id());
}

#[test]
fn explicit_clock_never_reads_ambient_time() {
    let mut clock = DeterministicClock::new(100, 7);
    assert_eq!(clock.tick().as_unix_millis(), 100);
    assert_eq!(clock.tick().as_unix_millis(), 107);
}

#[test]
fn fact_builder_constructs_catalog_payloads_with_explicit_parents()
-> Result<(), Box<dyn std::error::Error>> {
    let mut values = DeterministicValues::new(9);
    let parent = FactBuilder::installation_declared(&mut values, "home")?;
    let child = FactBuilder::mailbox_created(&mut values, &parent, "agent")?;

    assert_eq!(parent.kind(), FactKind::InstallationDeclared);
    assert_eq!(child.kind(), FactKind::MailboxCreated);
    assert!(child.causal().parents().contains(&parent.id()));
    assert!(matches!(
        child.payload(),
        SemanticPayload::MailboxCreated { .. }
    ));
    Ok(())
}

#[test]
fn permutations_and_state_sequences_are_stable_and_shrink_friendly() {
    assert_eq!(arrival_permutations(&[1, 2, 3]).len(), 6);
    let sequence = StateMachineSequence::new(["create", "open", "close"]);
    assert_eq!(sequence.prefix(2), &["create", "open"]);
    assert_eq!(sequence.without(1).items(), &["create", "close"]);
}

#[test]
fn catalog_fixtures_instantiate_every_payload_variant() -> Result<(), Box<dyn std::error::Error>> {
    let mut values = DeterministicValues::new(48);
    let payloads = FactBuilder::all_catalog_payloads(&mut values)?;
    let kinds = payloads
        .iter()
        .map(SemanticPayload::kind)
        .collect::<Vec<_>>();

    assert_eq!(kinds, FactKind::ALL);
    Ok(())
}

#[test]
fn remote_control_payloads_cannot_enter_canonical_scope() -> Result<(), Box<dyn std::error::Error>>
{
    let mut values = DeterministicValues::new(91);
    let payload = FactBuilder::all_catalog_payloads(&mut values)?
        .into_iter()
        .last()
        .ok_or("catalog fixture is empty")?;
    let installation_id = InstallationId::from_bytes([1; 32]);
    let author = InstallationAddress::new(installation_id, SigningPublicKey::from_bytes([2; 32]));

    assert_eq!(
        FactBuilder::root(
            &mut values,
            author,
            FactScope::InstallationPrivate(installation_id),
            payload.clone(),
        ),
        Err(hq_testkit::FixtureError::Fact(
            SemanticFactError::ProtocolScopeMismatch
        ))
    );

    let target_home = match &payload {
        SemanticPayload::RemoteProjectCommandOutcome { .. } => installation_id,
        _ => return Err("last catalog fixture is not remote control".into()),
    };
    let remote = FactBuilder::root(
        &mut values,
        author,
        FactScope::RemoteControl {
            account_id: AccountId::from_bytes([3; 32]),
            target_home,
        },
        payload,
    )?;
    assert_eq!(remote.kind(), FactKind::RemoteProjectCommandOutcome);
    Ok(())
}
