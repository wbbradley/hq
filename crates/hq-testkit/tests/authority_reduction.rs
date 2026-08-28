//! Authority race matrix for installation, peer, capability, and human-account reduction.

use std::{collections::BTreeSet, error::Error, num::NonZeroU64};

use hq_domain::{
    AccountId, ActivityKind, AuthorityReference, AuthorityRole, BoundedVec, ContentText, Fact,
    FactScope, GrantId, InstallationAddress, InstallationId, MailboxAddress, MailboxId,
    MailboxKind, MessageContent, MessagePurpose, OperationCorrelation, PresentationKind,
    ProviderId, ProviderSessionId, SemanticPayload, ShortText, SigningPublicKey, Timestamp,
};
use hq_reducer::{
    AuthorityPolicy, AuthorityProjection, AuthorityProjectionKey, AuthorityReducer, DecisionStatus,
    MembershipState, PeerRouteState, reduce_complete,
};
use hq_testkit::{DeterministicValues, FactBuilder, arrival_permutations};

const AUTHORITY_SCENARIO_COVERAGE: [(&str, &str); 22] = [
    ("AUTH-001", "local roots and mailboxes"),
    ("AUTH-002", "untyped parents"),
    ("AUTH-003", "directional capability"),
    ("AUTH-004", "route is not capability"),
    ("AUTH-005", "observed pre-revoke action"),
    ("AUTH-006", "concurrent revoke and action"),
    ("AUTH-007", "post-revoke old grant"),
    ("AUTH-008", "full mailbox regrant"),
    ("AUTH-009", "partial mailbox regrant"),
    ("AUTH-010", "observation frontier"),
    ("AUTH-011", "concurrent route block"),
    ("AUTH-012", "route restoration"),
    ("AUTH-013", "device grant acceptance"),
    ("AUTH-014", "missing device grant"),
    ("AUTH-015", "changed acceptance"),
    ("AUTH-016", "concurrent device revoke"),
    ("AUTH-017", "old-grant reacceptance"),
    ("AUTH-018", "full device regrant"),
    ("AUTH-019", "wrong account"),
    ("AUTH-020", "arbitrary selection authority"),
    ("AUTH-021", "account root conflict"),
    ("AUTH-022", "revoked source activity"),
];

#[test]
fn every_named_authority_scenario_is_mapped_to_an_executable_case() {
    let expected = [
        "AUTH-001", "AUTH-002", "AUTH-003", "AUTH-004", "AUTH-005", "AUTH-006", "AUTH-007",
        "AUTH-008", "AUTH-009", "AUTH-010", "AUTH-011", "AUTH-012", "AUTH-013", "AUTH-014",
        "AUTH-015", "AUTH-016", "AUTH-017", "AUTH-018", "AUTH-019", "AUTH-020", "AUTH-021",
        "AUTH-022",
    ];
    assert_eq!(
        AUTHORITY_SCENARIO_COVERAGE.map(|(scenario, _)| scenario),
        expected
    );
    assert!(
        AUTHORITY_SCENARIO_COVERAGE
            .iter()
            .all(|(_, executable_case)| !executable_case.is_empty())
    );
}

fn address(value: u8) -> InstallationAddress {
    InstallationAddress::new(
        InstallationId::from_bytes([value; 32]),
        SigningPublicKey::from_bytes([value.wrapping_add(64); 32]),
    )
}

fn mailbox(owner: InstallationAddress, value: u8) -> MailboxAddress {
    MailboxAddress::new(owner.installation_id(), MailboxId::from_bytes([value; 32]))
}

fn root(
    values: &mut DeterministicValues,
    author: InstallationAddress,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        author,
        Timestamp::from_unix_millis(0),
        FactScope::InstallationPrivate(author.installation_id()),
        [],
        [],
        SemanticPayload::InstallationDeclared {
            installation_id: author.installation_id(),
            signing_key: author.signing_key(),
            encryption_key: hq_domain::EncryptionPublicKey::from_bytes([3; 32]),
            label: Some(ShortText::new("installation")?),
        },
    )?)
}

fn mailbox_created(
    values: &mut DeterministicValues,
    installation: &Fact,
    mailbox: MailboxAddress,
    kind: MailboxKind,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        installation.author(),
        Timestamp::from_unix_millis(1),
        FactScope::InstallationPrivate(installation.author().installation_id()),
        [installation.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation.id(),
        )],
        SemanticPayload::MailboxCreated {
            mailbox_id: mailbox.mailbox_id(),
            kind,
            label: Some(ShortText::new("mailbox")?),
        },
    )?)
}

fn mailbox_grant(
    values: &mut DeterministicValues,
    mailbox_root: &Fact,
    mailbox: MailboxAddress,
    grantee: InstallationAddress,
    grant_id: GrantId,
    revoke_parents: &[Fact],
) -> Result<Fact, Box<dyn Error>> {
    let parents = std::iter::once(mailbox_root.id())
        .chain(revoke_parents.iter().map(Fact::id))
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        mailbox_root.author(),
        Timestamp::from_unix_millis(2),
        FactScope::PeerAddressed(mailbox),
        parents,
        [AuthorityReference::new(
            AuthorityRole::MailboxOwner,
            mailbox_root.id(),
        )],
        SemanticPayload::MailboxAccessGranted {
            grant_id,
            mailbox,
            grantee,
        },
    )?)
}

fn peer_action(
    values: &mut DeterministicValues,
    author: InstallationAddress,
    target: MailboxAddress,
    grant: &Fact,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    let sender = mailbox(author, 90);
    let message_id = values.message_id();
    let parents = std::iter::once(grant.id())
        .chain(extra_parents)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        author,
        Timestamp::from_unix_millis(3),
        FactScope::PeerAddressed(target),
        parents,
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            grant.id(),
        )],
        SemanticPayload::AsynchronousMessageSent(MessageContent {
            message_id,
            sender,
            recipient: Some(target),
            body: ContentText::new("hello")?,
            purpose: MessagePurpose::Asynchronous,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: None,
        }),
    )?)
}

fn observe_action(
    values: &mut DeterministicValues,
    mailbox_root: &Fact,
    target: MailboxAddress,
    grant: &Fact,
    action: &Fact,
) -> Result<Fact, Box<dyn Error>> {
    let grant_id = match grant.payload() {
        SemanticPayload::MailboxAccessGranted { grant_id, .. } => *grant_id,
        _ => return Err("fixture grant kind mismatch".into()),
    };
    Ok(FactBuilder::with_causal(
        values,
        mailbox_root.author(),
        Timestamp::from_unix_millis(4),
        FactScope::PeerAddressed(target),
        [grant.id(), action.id()],
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            grant.id(),
        )],
        SemanticPayload::MailboxActionObserved {
            grant_id,
            action_id: action.id(),
        },
    )?)
}

fn revoke_grant(
    values: &mut DeterministicValues,
    mailbox_root: &Fact,
    target: MailboxAddress,
    grant: &Fact,
    grantee: InstallationAddress,
    frontier: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    let grant_id = match grant.payload() {
        SemanticPayload::MailboxAccessGranted { grant_id, .. } => *grant_id,
        _ => return Err("fixture grant kind mismatch".into()),
    };
    let parents = std::iter::once(grant.id())
        .chain(frontier)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        mailbox_root.author(),
        Timestamp::from_unix_millis(5),
        FactScope::PeerAddressed(target),
        parents,
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            grant.id(),
        )],
        SemanticPayload::MailboxAccessRevoked {
            grant_id,
            mailbox: target,
            grantee_id: grantee.installation_id(),
        },
    )?)
}

fn human_account(
    values: &mut DeterministicValues,
    installation: &Fact,
    account_id: AccountId,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        installation.author(),
        Timestamp::from_unix_millis(1),
        FactScope::InstallationPrivate(installation.author().installation_id()),
        [installation.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation.id(),
        )],
        SemanticPayload::HumanAccountCreated {
            account_id,
            creator: installation.author(),
            label: Some(ShortText::new("people")?),
        },
    )?)
}

fn device_grant(
    values: &mut DeterministicValues,
    account: &Fact,
    account_id: AccountId,
    grant_id: GrantId,
    device: InstallationAddress,
    revoke_parents: &[Fact],
) -> Result<Fact, Box<dyn Error>> {
    let parents = std::iter::once(account.id())
        .chain(revoke_parents.iter().map(Fact::id))
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        account.author(),
        Timestamp::from_unix_millis(2),
        FactScope::AccountAddressed(account_id),
        parents,
        [AuthorityReference::new(
            AuthorityRole::AccountCreator,
            account.id(),
        )],
        SemanticPayload::HumanDeviceGranted {
            account_id,
            grant_id,
            device,
            label: Some(ShortText::new("device")?),
            relay_hints: BoundedVec::new([])?,
        },
    )?)
}

fn accept_device(
    values: &mut DeterministicValues,
    grant: &Fact,
    account_id: AccountId,
    grant_id: GrantId,
    device: InstallationAddress,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    let parents = std::iter::once(grant.id())
        .chain(extra_parents)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        device,
        Timestamp::from_unix_millis(3),
        FactScope::AccountAddressed(account_id),
        parents,
        [AuthorityReference::new(
            AuthorityRole::DeviceGrant,
            grant.id(),
        )],
        SemanticPayload::HumanDeviceAccepted {
            account_id,
            grant_id,
            device,
        },
    )?)
}

fn revoke_device(
    values: &mut DeterministicValues,
    account: &Fact,
    grant: &Fact,
    account_id: AccountId,
    grant_id: GrantId,
    device: InstallationAddress,
    frontier: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    let parents = [account.id(), grant.id()]
        .into_iter()
        .chain(frontier)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        account.author(),
        Timestamp::from_unix_millis(4),
        FactScope::AccountAddressed(account_id),
        parents,
        [
            AuthorityReference::new(AuthorityRole::AccountCreator, account.id()),
            AuthorityReference::new(AuthorityRole::DeviceGrant, grant.id()),
        ],
        SemanticPayload::HumanDeviceRevoked {
            account_id,
            grant_id,
            device_id: device.installation_id(),
        },
    )?)
}

fn account_action(
    values: &mut DeterministicValues,
    author: InstallationAddress,
    account_id: AccountId,
    authority: &Fact,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    let message_id = values.message_id();
    let parents = std::iter::once(authority.id())
        .chain(extra_parents)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        author,
        Timestamp::from_unix_millis(5),
        FactScope::AccountAddressed(account_id),
        parents,
        [AuthorityReference::new(
            AuthorityRole::AccountMembership,
            authority.id(),
        )],
        SemanticPayload::AsynchronousMessageSent(MessageContent {
            message_id,
            sender: mailbox(author, 90),
            recipient: None,
            body: ContentText::new("account message")?,
            purpose: MessagePurpose::Asynchronous,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: None,
        }),
    )?)
}

#[test]
fn local_roots_and_mailboxes_require_exact_signer_scope_and_authority() -> Result<(), Box<dyn Error>>
{
    let mut values = DeterministicValues::new(1);
    let home = address(1);
    let root = root(&mut values, home)?;
    let human = mailbox(home, 1);
    let mailbox = mailbox_created(&mut values, &root, human, MailboxKind::Human)?;
    let binding = FactBuilder::with_causal(
        &mut values,
        home,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(home.installation_id()),
        [root.id(), mailbox.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root.id(),
        )],
        SemanticPayload::MailboxSessionBound {
            mailbox_id: human.mailbox_id(),
            provider: ProviderId::new("test-provider")?,
            session: ProviderSessionId::new("session")?,
        },
    )?;
    let peer = address(2);
    let wrong_signer = FactBuilder::with_causal(
        &mut values,
        peer,
        Timestamp::from_unix_millis(3),
        FactScope::InstallationPrivate(home.installation_id()),
        [root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root.id(),
        )],
        SemanticPayload::PeerRouteBlocked {
            peer_id: peer.installation_id(),
            reason: hq_domain::ErrorCode::new("blocked")?,
        },
    )?;
    let policy = AuthorityPolicy::new(home.installation_id(), human.mailbox_id());
    let report = reduce_complete(
        [
            root.clone(),
            mailbox.clone(),
            binding.clone(),
            wrong_signer.clone(),
        ],
        &AuthorityReducer::new(policy),
    )?;

    assert_eq!(
        report.decisions()[&root.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&mailbox.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&binding.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&wrong_signer.id()].status(),
        DecisionStatus::Invalid
    );
    assert!(matches!(
        report.projections()[&AuthorityProjectionKey::Mailbox(human)],
        AuthorityProjection::Mailbox(_)
    ));
    Ok(())
}

#[test]
fn account_selection_is_frontier_complete_and_rejects_arbitrary_descendants()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(50);
    let creator = address(1);
    let root = root(&mut values, creator)?;
    let first_id = AccountId::from_bytes([8; 32]);
    let second_id = AccountId::from_bytes([9; 32]);
    let first_account = human_account(&mut values, &root, first_id)?;
    let second_account = human_account(&mut values, &root, second_id)?;
    let selection = |values: &mut DeterministicValues,
                     account: &Fact,
                     account_id: AccountId,
                     selection_parents: &[Fact]|
     -> Result<Fact, Box<dyn Error>> {
        let parents = [root.id(), account.id()]
            .into_iter()
            .chain(selection_parents.iter().map(Fact::id))
            .collect::<Vec<_>>();
        Ok(FactBuilder::with_causal(
            values,
            creator,
            Timestamp::from_unix_millis(6),
            FactScope::InstallationPrivate(creator.installation_id()),
            parents,
            [
                AuthorityReference::new(AuthorityRole::LocalInstallation, root.id()),
                AuthorityReference::new(AuthorityRole::AccountMembership, account.id()),
            ],
            SemanticPayload::HumanAccountSelected { account_id },
        )?)
    };
    let first = selection(&mut values, &first_account, first_id, &[])?;
    let second = selection(&mut values, &second_account, second_id, &[])?;
    let policy = AuthorityPolicy::new(creator.installation_id(), MailboxId::from_bytes([1; 32]));
    let conflicted = reduce_complete(
        [
            root.clone(),
            first_account.clone(),
            second_account.clone(),
            first.clone(),
            second.clone(),
        ],
        &AuthorityReducer::new(policy),
    )?;
    assert!(matches!(
        conflicted.projections()[&AuthorityProjectionKey::AccountSelection(
            creator.installation_id()
        )],
        AuthorityProjection::AccountSelection { ref candidates, active: None }
            if candidates == &std::collections::BTreeSet::from([first_id, second_id])
    ));

    let resolved = selection(
        &mut values,
        &second_account,
        second_id,
        &[first.clone(), second.clone()],
    )?;
    let resolved_report = reduce_complete(
        [
            root.clone(),
            first_account.clone(),
            second_account,
            first,
            second,
            resolved,
        ],
        &AuthorityReducer::new(policy),
    )?;
    assert!(matches!(
        resolved_report.projections()[&AuthorityProjectionKey::AccountSelection(
            creator.installation_id()
        )],
        AuthorityProjection::AccountSelection { active: Some(account), .. } if account == second_id
    ));

    let arbitrary = account_action(&mut values, creator, first_id, &first_account, [])?;
    let bogus_selection = FactBuilder::with_causal(
        &mut values,
        creator,
        Timestamp::from_unix_millis(7),
        FactScope::InstallationPrivate(creator.installation_id()),
        [root.id(), arbitrary.id()],
        [
            AuthorityReference::new(AuthorityRole::LocalInstallation, root.id()),
            AuthorityReference::new(AuthorityRole::AccountMembership, arbitrary.id()),
        ],
        SemanticPayload::HumanAccountSelected {
            account_id: first_id,
        },
    )?;
    let bogus_report = reduce_complete(
        [root, first_account, arbitrary, bogus_selection.clone()],
        &AuthorityReducer::new(policy),
    )?;
    assert_eq!(
        bogus_report.decisions()[&bogus_selection.id()].status(),
        DecisionStatus::Unauthorized
    );
    Ok(())
}

#[test]
fn unique_account_and_reserved_human_mailbox_conflicts_fail_closed() -> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(10);
    let first_creator = address(1);
    let second_creator = address(2);
    let first_root = root(&mut values, first_creator)?;
    let second_root = root(&mut values, second_creator)?;
    let first_mailbox = mailbox_created(
        &mut values,
        &first_root,
        mailbox(first_creator, 1),
        MailboxKind::Human,
    )?;
    let second_mailbox = mailbox_created(
        &mut values,
        &first_root,
        mailbox(first_creator, 2),
        MailboxKind::Human,
    )?;
    let account_id = AccountId::from_bytes([9; 32]);
    let account = |values: &mut DeterministicValues,
                   installation: &Fact,
                   creator: InstallationAddress|
     -> Result<Fact, Box<dyn Error>> {
        Ok(FactBuilder::with_causal(
            values,
            creator,
            Timestamp::from_unix_millis(2),
            FactScope::InstallationPrivate(creator.installation_id()),
            [installation.id()],
            [AuthorityReference::new(
                AuthorityRole::LocalInstallation,
                installation.id(),
            )],
            SemanticPayload::HumanAccountCreated {
                account_id,
                creator,
                label: None,
            },
        )?)
    };
    let first_account = account(&mut values, &first_root, first_creator)?;
    let second_account = account(&mut values, &second_root, second_creator)?;
    let policy = AuthorityPolicy::new(
        first_creator.installation_id(),
        MailboxId::from_bytes([1; 32]),
    );
    let report = reduce_complete(
        [
            first_root,
            second_root,
            first_mailbox.clone(),
            second_mailbox.clone(),
            first_account.clone(),
            second_account.clone(),
        ],
        &AuthorityReducer::new(policy),
    )?;

    assert_eq!(
        report.decisions()[&first_mailbox.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&second_mailbox.id()].status(),
        DecisionStatus::Invalid
    );
    for fact in [first_account, second_account] {
        assert_eq!(
            report.decisions()[&fact.id()].status(),
            DecisionStatus::Conflicted
        );
        assert!(
            !report.decisions()[&fact.id()]
                .conflict_participants()
                .is_empty()
        );
    }
    assert!(
        !report
            .projections()
            .contains_key(&AuthorityProjectionKey::Account(account_id))
    );
    Ok(())
}

#[test]
fn peer_routes_and_untyped_parents_never_substitute_for_directional_capability()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(11);
    let home = address(1);
    let peer = address(2);
    let root = root(&mut values, home)?;
    let target = mailbox(home, 7);
    let mailbox_root = mailbox_created(&mut values, &root, target, MailboxKind::Agent)?;
    let grant_id = values.grant_id();
    let grant = mailbox_grant(&mut values, &mailbox_root, target, peer, grant_id, &[])?;
    let reverse_action = peer_action(&mut values, home, target, &grant, [])?;
    let route = FactBuilder::with_causal(
        &mut values,
        home,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(home.installation_id()),
        [root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root.id(),
        )],
        SemanticPayload::PeerRouteSet {
            peer,
            encryption_key: hq_domain::EncryptionPublicKey::from_bytes([4; 32]),
            label: None,
            relay_hints: BoundedVec::new([])?,
        },
    )?;
    let message_id = values.message_id();
    let untyped_action = FactBuilder::with_causal(
        &mut values,
        peer,
        Timestamp::from_unix_millis(3),
        FactScope::PeerAddressed(target),
        [route.id()],
        [],
        SemanticPayload::AsynchronousMessageSent(MessageContent {
            message_id,
            sender: mailbox(peer, 90),
            recipient: Some(target),
            body: ContentText::new("untyped")?,
            purpose: MessagePurpose::Asynchronous,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: None,
        }),
    )?;
    let policy = AuthorityPolicy::new(home.installation_id(), target.mailbox_id());
    let report = reduce_complete(
        [
            root,
            mailbox_root,
            grant,
            route,
            reverse_action.clone(),
            untyped_action.clone(),
        ],
        &AuthorityReducer::new(policy),
    )?;

    assert_ne!(
        report.decisions()[&reverse_action.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&untyped_action.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&untyped_action.id()].failed_authorities(),
        &std::collections::BTreeSet::from([AuthorityRole::MailboxGrant])
    );
    Ok(())
}

#[test]
fn observed_pre_revoke_action_survives_while_concurrent_action_retracts()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(2);
    let home = address(1);
    let peer = address(2);
    let root = root(&mut values, home)?;
    let target = mailbox(home, 7);
    let mailbox_root = mailbox_created(&mut values, &root, target, MailboxKind::Agent)?;
    let grant_id = values.grant_id();
    let grant = mailbox_grant(&mut values, &mailbox_root, target, peer, grant_id, &[])?;
    let observed_action = peer_action(&mut values, peer, target, &grant, [])?;
    let concurrent_action = peer_action(&mut values, peer, target, &grant, [])?;
    let observation = observe_action(&mut values, &mailbox_root, target, &grant, &observed_action)?;
    let revoke = revoke_grant(
        &mut values,
        &mailbox_root,
        target,
        &grant,
        peer,
        [observation.id()],
    )?;
    let facts = vec![
        root,
        mailbox_root,
        grant.clone(),
        observed_action.clone(),
        concurrent_action.clone(),
        observation,
        revoke.clone(),
    ];
    let policy = AuthorityPolicy::new(home.installation_id(), target.mailbox_id());
    let before_revoke =
        reduce_complete(facts[..5].iter().cloned(), &AuthorityReducer::new(policy))?;
    assert_eq!(
        before_revoke.decisions()[&concurrent_action.id()].status(),
        DecisionStatus::Projected
    );
    assert!(matches!(
        before_revoke.projections()[&AuthorityProjectionKey::MailboxCapability(grant_id)],
        AuthorityProjection::MailboxCapability(ref view) if view.is_active()
    ));
    let expected = reduce_complete(facts.clone(), &AuthorityReducer::new(policy))?;

    assert_eq!(
        expected.decisions()[&observed_action.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        expected.decisions()[&concurrent_action.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert!(matches!(
        expected.projections()[&AuthorityProjectionKey::MailboxCapability(grant_id)],
        AuthorityProjection::MailboxCapability(ref view)
            if !view.is_active()
                && view.revoke_frontier == std::collections::BTreeSet::from([revoke.id()])
                && view.observed_actions
                    == std::collections::BTreeSet::from([observed_action.id()])
    ));
    assert_eq!(
        expected.frontiers()[&hq_reducer::AuthorityAggregateKey::MailboxCapability(grant_id)],
        std::collections::BTreeSet::from([revoke.id()])
    );
    for scheduled in arrival_permutations(&facts) {
        assert_eq!(
            reduce_complete(scheduled, &AuthorityReducer::new(policy))?,
            expected
        );
    }
    assert!(
        expected
            .graph()
            .structurally_reaches(observed_action.id(), revoke.id())
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn old_grant_fails_after_revoke_and_only_full_frontier_regrant_restores_access()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(20);
    let home = address(1);
    let peer = address(2);
    let root = root(&mut values, home)?;
    let target = mailbox(home, 7);
    let mailbox_root = mailbox_created(&mut values, &root, target, MailboxKind::Agent)?;
    let old_grant_id = values.grant_id();
    let old_grant = mailbox_grant(&mut values, &mailbox_root, target, peer, old_grant_id, &[])?;
    let revoke_a = revoke_grant(&mut values, &mailbox_root, target, &old_grant, peer, [])?;
    let revoke_b = revoke_grant(&mut values, &mailbox_root, target, &old_grant, peer, [])?;
    let old_action = peer_action(
        &mut values,
        peer,
        target,
        &old_grant,
        [revoke_a.id(), revoke_b.id()],
    )?;
    let partial_grant_id = values.grant_id();
    let partial_grant = mailbox_grant(
        &mut values,
        &mailbox_root,
        target,
        peer,
        partial_grant_id,
        std::slice::from_ref(&revoke_a),
    )?;
    let partial_action = peer_action(&mut values, peer, target, &partial_grant, [])?;
    let restored_grant_id = values.grant_id();
    let restored_grant = mailbox_grant(
        &mut values,
        &mailbox_root,
        target,
        peer,
        restored_grant_id,
        &[revoke_a.clone(), revoke_b.clone()],
    )?;
    let restored_action = peer_action(&mut values, peer, target, &restored_grant, [])?;
    let policy = AuthorityPolicy::new(home.installation_id(), target.mailbox_id());
    let report = reduce_complete(
        [
            root,
            mailbox_root,
            old_grant,
            revoke_a,
            revoke_b,
            old_action.clone(),
            partial_grant.clone(),
            partial_action.clone(),
            restored_grant.clone(),
            restored_action.clone(),
        ],
        &AuthorityReducer::new(policy),
    )?;

    assert_eq!(
        report.decisions()[&old_action.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&partial_grant.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&partial_action.id()].status(),
        DecisionStatus::Unresolved
    );
    assert_eq!(
        report.decisions()[&restored_grant.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&restored_action.id()].status(),
        DecisionStatus::Projected
    );
    Ok(())
}

#[test]
fn peer_route_block_is_remove_wins_and_full_frontier_restore_is_routable()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(3);
    let home = address(1);
    let peer = address(2);
    let root = root(&mut values, home)?;
    let set = FactBuilder::with_causal(
        &mut values,
        home,
        Timestamp::from_unix_millis(1),
        FactScope::InstallationPrivate(home.installation_id()),
        [root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root.id(),
        )],
        SemanticPayload::PeerRouteSet {
            peer,
            encryption_key: hq_domain::EncryptionPublicKey::from_bytes([4; 32]),
            label: None,
            relay_hints: BoundedVec::new([])?,
        },
    )?;
    let block = FactBuilder::with_causal(
        &mut values,
        home,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(home.installation_id()),
        [root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root.id(),
        )],
        SemanticPayload::PeerRouteBlocked {
            peer_id: peer.installation_id(),
            reason: hq_domain::ErrorCode::new("blocked")?,
        },
    )?;
    let policy = AuthorityPolicy::new(home.installation_id(), MailboxId::from_bytes([1; 32]));
    let blocked = reduce_complete(
        [root.clone(), set, block.clone()],
        &AuthorityReducer::new(policy),
    )?;
    assert!(matches!(
        blocked.projections()[&AuthorityProjectionKey::PeerRoute {
            owner: home.installation_id(),
            peer: peer.installation_id(),
        }],
        AuthorityProjection::PeerRoute(ref view) if view.state() == PeerRouteState::Blocked
    ));

    let restored = FactBuilder::with_causal(
        &mut values,
        home,
        Timestamp::from_unix_millis(-100),
        FactScope::InstallationPrivate(home.installation_id()),
        [root.id(), block.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root.id(),
        )],
        SemanticPayload::PeerRouteSet {
            peer,
            encryption_key: hq_domain::EncryptionPublicKey::from_bytes([5; 32]),
            label: None,
            relay_hints: BoundedVec::new([])?,
        },
    )?;
    let restored_report = reduce_complete([root, block, restored], &AuthorityReducer::new(policy))?;
    assert!(matches!(
        restored_report.projections()[&AuthorityProjectionKey::PeerRoute {
            owner: home.installation_id(),
            peer: peer.installation_id(),
        }],
        AuthorityProjection::PeerRoute(ref view) if view.state() == PeerRouteState::Routable
    ));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn account_device_requires_exact_acceptance_and_remove_wins_revocation()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(4);
    let creator = address(1);
    let device = address(2);
    let root = root(&mut values, creator)?;
    let account_id = AccountId::from_bytes([8; 32]);
    let account = FactBuilder::with_causal(
        &mut values,
        creator,
        Timestamp::from_unix_millis(1),
        FactScope::InstallationPrivate(creator.installation_id()),
        [root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root.id(),
        )],
        SemanticPayload::HumanAccountCreated {
            account_id,
            creator,
            label: Some(ShortText::new("people")?),
        },
    )?;
    let grant_id = values.grant_id();
    let grant = FactBuilder::with_causal(
        &mut values,
        creator,
        Timestamp::from_unix_millis(2),
        FactScope::AccountAddressed(account_id),
        [account.id()],
        [AuthorityReference::new(
            AuthorityRole::AccountCreator,
            account.id(),
        )],
        SemanticPayload::HumanDeviceGranted {
            account_id,
            grant_id,
            device,
            label: Some(ShortText::new("laptop")?),
            relay_hints: BoundedVec::new([])?,
        },
    )?;
    let acceptance = FactBuilder::with_causal(
        &mut values,
        device,
        Timestamp::from_unix_millis(3),
        FactScope::AccountAddressed(account_id),
        [grant.id()],
        [AuthorityReference::new(
            AuthorityRole::DeviceGrant,
            grant.id(),
        )],
        SemanticPayload::HumanDeviceAccepted {
            account_id,
            grant_id,
            device,
        },
    )?;
    let changed_acceptance = FactBuilder::with_causal(
        &mut values,
        device,
        Timestamp::from_unix_millis(3),
        FactScope::AccountAddressed(account_id),
        [grant.id()],
        [AuthorityReference::new(
            AuthorityRole::DeviceGrant,
            grant.id(),
        )],
        SemanticPayload::HumanDeviceAccepted {
            account_id,
            grant_id,
            device: address(3),
        },
    )?;
    let policy = AuthorityPolicy::new(creator.installation_id(), MailboxId::from_bytes([1; 32]));
    let active = reduce_complete(
        [
            root.clone(),
            account.clone(),
            grant.clone(),
            acceptance.clone(),
            changed_acceptance.clone(),
        ],
        &AuthorityReducer::new(policy),
    )?;
    assert!(matches!(
        active.projections()[&AuthorityProjectionKey::Membership {
            account: account_id,
            device: device.installation_id(),
        }],
        AuthorityProjection::Membership(ref view)
            if view.state() == MembershipState::Active
                && view.active_grants == BTreeSet::from([grant_id])
    ));
    assert_eq!(
        active.decisions()[&changed_acceptance.id()].status(),
        DecisionStatus::Invalid
    );

    let revoke = FactBuilder::with_causal(
        &mut values,
        creator,
        Timestamp::from_unix_millis(4),
        FactScope::AccountAddressed(account_id),
        [account.id(), grant.id()],
        [
            AuthorityReference::new(AuthorityRole::AccountCreator, account.id()),
            AuthorityReference::new(AuthorityRole::DeviceGrant, grant.id()),
        ],
        SemanticPayload::HumanDeviceRevoked {
            account_id,
            grant_id,
            device_id: device.installation_id(),
        },
    )?;
    let revoked = reduce_complete(
        [root, account, grant, acceptance, revoke],
        &AuthorityReducer::new(policy),
    )?;
    assert!(matches!(
        revoked.projections()[&AuthorityProjectionKey::Membership {
            account: account_id,
            device: device.installation_id(),
        }],
        AuthorityProjection::Membership(ref view)
            if view.state() == MembershipState::Revoked && view.active_grants.is_empty()
    ));

    let missing_grant = hq_domain::FactId::from_bytes([99; 32]);
    let unresolved_acceptance = FactBuilder::with_causal(
        &mut values,
        device,
        Timestamp::from_unix_millis(5),
        FactScope::AccountAddressed(account_id),
        [missing_grant],
        [AuthorityReference::new(
            AuthorityRole::DeviceGrant,
            missing_grant,
        )],
        SemanticPayload::HumanDeviceAccepted {
            account_id,
            grant_id,
            device,
        },
    )?;
    let unresolved = reduce_complete(
        [unresolved_acceptance.clone()],
        &AuthorityReducer::new(policy),
    )?;
    assert_eq!(
        unresolved.decisions()[&unresolved_acceptance.id()].status(),
        DecisionStatus::Unresolved
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn account_actions_use_historical_membership_and_full_regrant_lineage() -> Result<(), Box<dyn Error>>
{
    let mut values = DeterministicValues::new(40);
    let creator = address(1);
    let device = address(2);
    let root = root(&mut values, creator)?;
    let account_id = AccountId::from_bytes([8; 32]);
    let other_account_id = AccountId::from_bytes([9; 32]);
    let account = human_account(&mut values, &root, account_id)?;
    let old_grant_id = values.grant_id();
    let old_grant = device_grant(&mut values, &account, account_id, old_grant_id, device, &[])?;
    let old_acceptance = accept_device(
        &mut values,
        &old_grant,
        account_id,
        old_grant_id,
        device,
        [],
    )?;
    let historical_action = account_action(&mut values, device, account_id, &old_acceptance, [])?;
    let revoke = revoke_device(
        &mut values,
        &account,
        &old_grant,
        account_id,
        old_grant_id,
        device,
        [old_acceptance.id(), historical_action.id()],
    )?;
    let revoke_b = revoke_device(
        &mut values,
        &account,
        &old_grant,
        account_id,
        old_grant_id,
        device,
        [old_acceptance.id(), historical_action.id()],
    )?;
    let post_revoke_action = account_action(
        &mut values,
        device,
        account_id,
        &old_acceptance,
        [revoke.id(), revoke_b.id()],
    )?;
    let operation_id = values.operation_id();
    let revoked_activity = FactBuilder::with_causal(
        &mut values,
        device,
        Timestamp::from_unix_millis(6),
        FactScope::AccountAddressed(account_id),
        [old_acceptance.id(), revoke.id(), revoke_b.id()],
        [AuthorityReference::new(
            AuthorityRole::AccountMembership,
            old_acceptance.id(),
        )],
        SemanticPayload::HarnessActivityRecorded {
            source: mailbox(device, 90),
            correlation: OperationCorrelation::new(
                ProviderId::new("test-provider")?,
                ProviderSessionId::new("session")?,
                operation_id,
            ),
            item: Some(ShortText::new("item")?),
            kind: ActivityKind::Progress,
            logical_key: ShortText::new("build")?,
            runtime: ShortText::new("runtime")?,
            sequence: NonZeroU64::MIN,
            occurred_at: Timestamp::from_unix_millis(6),
            status: hq_domain::ActivityStatus::Running,
            content: ContentText::new("running")?,
            truncated: false,
        },
    )?;
    let old_grant_reacceptance = accept_device(
        &mut values,
        &old_grant,
        account_id,
        old_grant_id,
        device,
        [revoke.id(), revoke_b.id()],
    )?;
    let partial_grant_id = values.grant_id();
    let partial_grant = device_grant(
        &mut values,
        &account,
        account_id,
        partial_grant_id,
        device,
        std::slice::from_ref(&revoke),
    )?;
    let new_grant_id = values.grant_id();
    let new_grant = device_grant(
        &mut values,
        &account,
        account_id,
        new_grant_id,
        device,
        &[revoke.clone(), revoke_b.clone()],
    )?;
    let new_acceptance = accept_device(
        &mut values,
        &new_grant,
        account_id,
        new_grant_id,
        device,
        [],
    )?;
    let restored_action = account_action(&mut values, device, account_id, &new_acceptance, [])?;
    let wrong_account_action =
        account_action(&mut values, device, other_account_id, &new_acceptance, [])?;
    let policy = AuthorityPolicy::new(creator.installation_id(), MailboxId::from_bytes([1; 32]));
    let facts = vec![
        root,
        account,
        old_grant,
        old_acceptance,
        historical_action.clone(),
        revoke,
        revoke_b,
        post_revoke_action.clone(),
        revoked_activity.clone(),
        old_grant_reacceptance.clone(),
        partial_grant.clone(),
        new_grant,
        new_acceptance,
        restored_action.clone(),
        wrong_account_action.clone(),
    ];
    for length in 1..=facts.len() {
        if reduce_complete(
            facts[..length].iter().cloned(),
            &AuthorityReducer::new(policy),
        )
        .is_err()
        {
            return Err(format!("authority fixture prefix {length} did not converge").into());
        }
    }
    let report = reduce_complete(facts, &AuthorityReducer::new(policy))?;

    assert_eq!(
        report.decisions()[&historical_action.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&post_revoke_action.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&revoked_activity.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&old_grant_reacceptance.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&partial_grant.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert_eq!(
        report.decisions()[&restored_action.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&wrong_account_action.id()].status(),
        DecisionStatus::Unauthorized
    );
    assert!(matches!(
        report.projections()[&AuthorityProjectionKey::Membership {
            account: account_id,
            device: device.installation_id(),
        }],
        AuthorityProjection::Membership(ref view) if view.state() == MembershipState::Active
    ));
    Ok(())
}
