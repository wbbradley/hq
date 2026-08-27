//! Named-agent, provider-session, context, selection, rename, and retirement contracts.

use std::error::Error;

use hq_domain::{
    AgentId, AuthorityReference, AuthorityRole, Fact, FactScope, InstallationAddress,
    InstallationId, MailboxAddress, MailboxId, MailboxKind, ProviderId, ProviderSessionId,
    RepositoryContext, ResourceLocator, ResourceScheme, SemanticPayload, ShortText,
    SigningPublicKey, Timestamp,
};
use hq_reducer::{
    AgentLifecycle, AgentProjection, AgentProjectionKey, AgentReason, AgentReducer,
    AuthorityPolicy, ConflictReason, DecisionStatus, SessionIdentity, reduce_complete,
};
use hq_testkit::{DeterministicValues, FactBuilder, arrival_permutations};

const AGENT_SCENARIO_COVERAGE: [(&str, &str); 10] = [
    ("AGT-001", "unique agent name"),
    ("AGT-002", "concurrent name claims"),
    ("AGT-003", "session binding reassignment"),
    ("AGT-004", "concurrent selection"),
    ("AGT-005", "selection resolution"),
    ("AGT-006", "concurrent rename"),
    ("AGT-007", "retirement remove wins"),
    ("AGT-008", "retired name reservation"),
    ("AGT-009", "context history frontier"),
    ("AGT-010", "provider namespace isolation"),
];

#[test]
fn every_named_agent_scenario_is_mapped() {
    assert_eq!(
        AGENT_SCENARIO_COVERAGE.map(|(scenario, _)| scenario),
        [
            "AGT-001", "AGT-002", "AGT-003", "AGT-004", "AGT-005", "AGT-006", "AGT-007", "AGT-008",
            "AGT-009", "AGT-010",
        ]
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
    address: MailboxAddress,
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
            mailbox_id: address.mailbox_id(),
            kind,
            label: Some(ShortText::new("mailbox")?),
        },
    )?)
}

#[test]
fn unique_name_claim_creates_an_active_reserved_agent() -> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(1);
    let installation = address(1);
    let installation_root = root(&mut values, installation)?;
    let human = mailbox(installation, 1);
    let human_root = mailbox_created(&mut values, &installation_root, human, MailboxKind::Human)?;
    let agent_mailbox = mailbox(installation, 2);
    let mailbox_root = mailbox_created(
        &mut values,
        &installation_root,
        agent_mailbox,
        MailboxKind::Agent,
    )?;
    let agent_id = AgentId::from_bytes([7; 32]);
    let claim = FactBuilder::with_causal(
        &mut values,
        installation,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(installation.installation_id()),
        [installation_root.id(), mailbox_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation_root.id(),
        )],
        SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id: agent_mailbox.mailbox_id(),
            name: ShortText::new("helper")?,
        },
    )?;
    let report = reduce_complete(
        [installation_root, human_root, mailbox_root, claim.clone()],
        &AgentReducer::new(AuthorityPolicy::new(
            installation.installation_id(),
            human.mailbox_id(),
        )),
    )?;
    assert_eq!(
        report.decisions()[&claim.id()].status(),
        DecisionStatus::Projected
    );
    let Some(AgentProjection::Agent(agent)) = report
        .projections()
        .get(&AgentProjectionKey::Agent(agent_id))
    else {
        return Err("missing agent projection".into());
    };
    assert_eq!(
        agent.names,
        [ShortText::new("helper")?].into_iter().collect()
    );
    assert_eq!(agent.mailboxes, [agent_mailbox].into_iter().collect());
    assert!(agent.name_reserved);
    assert_eq!(agent.lifecycle, AgentLifecycle::Active);
    assert!(!agent.runnable);
    Ok(())
}

#[derive(Clone)]
struct AgentWorld {
    installation: InstallationAddress,
    root: Fact,
    human: MailboxAddress,
    human_root: Fact,
    first: MailboxAddress,
    first_root: Fact,
    second: MailboxAddress,
    second_root: Fact,
}

impl AgentWorld {
    fn reducer(&self) -> AgentReducer {
        AgentReducer::new(AuthorityPolicy::new(
            self.installation.installation_id(),
            self.human.mailbox_id(),
        ))
    }

    fn base_facts(&self) -> Vec<Fact> {
        vec![
            self.root.clone(),
            self.human_root.clone(),
            self.first_root.clone(),
            self.second_root.clone(),
        ]
    }
}

fn agent_world(values: &mut DeterministicValues) -> Result<AgentWorld, Box<dyn Error>> {
    let installation = address(10);
    let root = root(values, installation)?;
    let human = mailbox(installation, 10);
    let first = mailbox(installation, 11);
    let second = mailbox(installation, 12);
    Ok(AgentWorld {
        installation,
        human_root: mailbox_created(values, &root, human, MailboxKind::Human)?,
        first_root: mailbox_created(values, &root, first, MailboxKind::Agent)?,
        second_root: mailbox_created(values, &root, second, MailboxKind::Agent)?,
        root,
        human,
        first,
        second,
    })
}

fn local_fact(
    values: &mut DeterministicValues,
    world: &AgentWorld,
    authored_at: i64,
    parents: impl IntoIterator<Item = hq_domain::FactId>,
    payload: SemanticPayload,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.installation,
        Timestamp::from_unix_millis(authored_at),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        std::iter::once(world.root.id()).chain(parents),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.root.id(),
        )],
        payload,
    )?)
}

fn claim(
    values: &mut DeterministicValues,
    world: &AgentWorld,
    agent_id: AgentId,
    mailbox: MailboxAddress,
    mailbox_root: &Fact,
    name: &str,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    local_fact(
        values,
        world,
        2,
        std::iter::once(mailbox_root.id()).chain(extra_parents),
        SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id: mailbox.mailbox_id(),
            name: ShortText::new(name)?,
        },
    )
}

fn session(provider: &str, value: &str) -> Result<SessionIdentity, Box<dyn Error>> {
    Ok(SessionIdentity {
        provider: ProviderId::new(provider)?,
        session: ProviderSessionId::new(value)?,
    })
}

fn binding(
    values: &mut DeterministicValues,
    world: &AgentWorld,
    mailbox: MailboxAddress,
    mailbox_root: &Fact,
    session: &SessionIdentity,
) -> Result<Fact, Box<dyn Error>> {
    local_fact(
        values,
        world,
        3,
        [mailbox_root.id()],
        SemanticPayload::MailboxSessionBound {
            mailbox_id: mailbox.mailbox_id(),
            provider: session.provider.clone(),
            session: session.session.clone(),
        },
    )
}

fn repository_context(path: &str) -> Result<RepositoryContext, Box<dyn Error>> {
    Ok(RepositoryContext {
        directory: ResourceLocator::new(
            ResourceScheme::WorkingTree,
            hq_domain::BoundedText::new(path)?,
        ),
        repository: None,
        worktree: None,
        branch: Some(ShortText::new("main")?),
    })
}

fn context_fact(
    values: &mut DeterministicValues,
    world: &AgentWorld,
    mailbox: MailboxAddress,
    mailbox_root: &Fact,
    context: RepositoryContext,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    local_fact(
        values,
        world,
        4,
        std::iter::once(mailbox_root.id()).chain(extra_parents),
        SemanticPayload::MailboxContextRecorded {
            mailbox_id: mailbox.mailbox_id(),
            context,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn selection(
    values: &mut DeterministicValues,
    world: &AgentWorld,
    agent_id: AgentId,
    mailbox: MailboxAddress,
    claim: &Fact,
    binding: &Fact,
    context_fact: &Fact,
    session: &SessionIdentity,
    context: RepositoryContext,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    local_fact(
        values,
        world,
        5,
        [claim.id(), binding.id(), context_fact.id()]
            .into_iter()
            .chain(extra_parents),
        SemanticPayload::ProviderSessionSelected {
            agent_id,
            mailbox_id: mailbox.mailbox_id(),
            provider: session.provider.clone(),
            session: session.session.clone(),
            context,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn rename(
    values: &mut DeterministicValues,
    world: &AgentWorld,
    agent_id: AgentId,
    claim: &Fact,
    binding: &Fact,
    session: &SessionIdentity,
    name: Option<&str>,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    local_fact(
        values,
        world,
        6,
        [claim.id(), binding.id()].into_iter().chain(extra_parents),
        SemanticPayload::ProviderSessionRenamed {
            agent_id,
            provider: session.provider.clone(),
            session: session.session.clone(),
            display_name: name.map(ShortText::new).transpose()?,
        },
    )
}

fn retirement(
    values: &mut DeterministicValues,
    world: &AgentWorld,
    agent_id: AgentId,
    mailbox: MailboxAddress,
    claim: &Fact,
    extra_parents: impl IntoIterator<Item = hq_domain::FactId>,
) -> Result<Fact, Box<dyn Error>> {
    local_fact(
        values,
        world,
        7,
        std::iter::once(claim.id()).chain(extra_parents),
        SemanticPayload::AgentRetired {
            agent_id,
            mailbox_id: mailbox.mailbox_id(),
        },
    )
}

#[test]
fn incompatible_name_agent_and_mailbox_claims_conflict_and_retired_names_stay_reserved()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(30);
    let world = agent_world(&mut values)?;
    let first_agent = values.agent_id();
    let second_agent = values.agent_id();
    let first = claim(
        &mut values,
        &world,
        first_agent,
        world.first,
        &world.first_root,
        "helper",
        [],
    )?;
    let second = claim(
        &mut values,
        &world,
        second_agent,
        world.second,
        &world.second_root,
        "helper",
        [],
    )?;
    let invalid_agent = values.agent_id();
    let invalid = claim(
        &mut values,
        &world,
        invalid_agent,
        world.second,
        &world.second_root,
        "Not-Lowercase",
        [],
    )?;
    let report = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([first.clone(), second.clone(), invalid.clone()]),
        &world.reducer(),
    )?;
    assert_eq!(
        report.decisions()[&invalid.id()].status(),
        DecisionStatus::Invalid
    );
    let name = ShortText::new("helper")?;
    let Some(AgentProjection::Name(reservation)) = report
        .projections()
        .get(&AgentProjectionKey::Name(name.clone()))
    else {
        return Err("missing name reservation".into());
    };
    assert!(reservation.conflicted);
    assert_eq!(reservation.claims.len(), 2);
    assert!(report.conflicts().iter().any(|conflict| {
        conflict.reason() == &ConflictReason::Domain(AgentReason::NameConflict)
    }));
    for agent_id in [first_agent, second_agent] {
        let Some(AgentProjection::Agent(agent)) = report
            .projections()
            .get(&AgentProjectionKey::Agent(agent_id))
        else {
            return Err("missing conflicted agent".into());
        };
        assert_eq!(agent.lifecycle, AgentLifecycle::Conflicted);
        assert!(!agent.runnable);
    }

    let retired = retirement(&mut values, &world, first_agent, world.first, &first, [])?;
    let reused = claim(
        &mut values,
        &world,
        second_agent,
        world.second,
        &world.second_root,
        "helper",
        [retired.id()],
    )?;
    let retired_report = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([first, retired, second, reused]),
        &world.reducer(),
    )?;
    let Some(AgentProjection::Name(reservation)) = retired_report
        .projections()
        .get(&AgentProjectionKey::Name(name))
    else {
        return Err("missing retired reservation".into());
    };
    assert!(reservation.retired);
    assert!(reservation.conflicted);
    Ok(())
}

#[test]
fn session_bindings_are_immutable_provider_scoped_and_preserve_direct_history()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(60);
    let world = agent_world(&mut values)?;
    let first_session = session("provider-a", "same-text")?;
    let other_provider = session("provider-b", "same-text")?;
    let first = binding(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        &first_session,
    )?;
    let conflicting = binding(
        &mut values,
        &world,
        world.second,
        &world.second_root,
        &first_session,
    )?;
    let isolated = binding(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        &other_provider,
    )?;
    let report = reduce_complete(
        world.base_facts().into_iter().chain([
            conflicting.clone(),
            isolated.clone(),
            first.clone(),
        ]),
        &world.reducer(),
    )?;
    let Some(AgentProjection::Session(binding_view)) = report
        .projections()
        .get(&AgentProjectionKey::Session(first_session.clone()))
    else {
        return Err("missing binding conflict".into());
    };
    assert!(binding_view.conflicted);
    assert!(binding_view.mailbox.is_none());
    let Some(AgentProjection::Session(isolated_view)) = report
        .projections()
        .get(&AgentProjectionKey::Session(other_provider.clone()))
    else {
        return Err("missing isolated provider binding".into());
    };
    assert!(!isolated_view.conflicted);
    assert_eq!(isolated_view.mailbox, Some(world.first));
    let direct_key = AgentProjectionKey::DirectSession {
        mailbox: world.first,
        session: other_provider,
    };
    let Some(AgentProjection::DirectSession(direct)) = report.projections().get(&direct_key) else {
        return Err("missing projectless direct session".into());
    };
    assert!(direct.named_agent.is_none());
    assert!(!direct.conflicted);
    assert!(report.conflicts().iter().any(|conflict| {
        conflict.reason() == &ConflictReason::Domain(AgentReason::SessionBindingConflict)
    }));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn selection_is_multivalue_then_resolves_only_through_the_complete_frontier()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(90);
    let world = agent_world(&mut values)?;
    let agent_id = values.agent_id();
    let claim = claim(
        &mut values,
        &world,
        agent_id,
        world.first,
        &world.first_root,
        "helper",
        [],
    )?;
    let first_session = session("provider", "one")?;
    let second_session = session("provider", "two")?;
    let first_binding = binding(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        &first_session,
    )?;
    let second_binding = binding(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        &second_session,
    )?;
    let context_value = repository_context("/work/project")?;
    let context = context_fact(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        context_value.clone(),
        [],
    )?;
    let first_selection = selection(
        &mut values,
        &world,
        agent_id,
        world.first,
        &claim,
        &first_binding,
        &context,
        &first_session,
        context_value.clone(),
        [],
    )?;
    let second_selection = selection(
        &mut values,
        &world,
        agent_id,
        world.first,
        &claim,
        &second_binding,
        &context,
        &second_session,
        context_value.clone(),
        [],
    )?;
    let conflicted = reduce_complete(
        world.base_facts().into_iter().chain([
            claim.clone(),
            first_binding.clone(),
            second_binding.clone(),
            context.clone(),
            first_selection.clone(),
            second_selection.clone(),
        ]),
        &world.reducer(),
    )?;
    let Some(AgentProjection::Selection(view)) = conflicted
        .projections()
        .get(&AgentProjectionKey::Selection(agent_id))
    else {
        return Err("missing conflicted selection".into());
    };
    assert!(view.conflicted);
    assert!(view.active.is_none());
    assert_eq!(view.frontier.len(), 2);
    let Some(AgentProjection::Agent(agent)) = conflicted
        .projections()
        .get(&AgentProjectionKey::Agent(agent_id))
    else {
        return Err("missing agent".into());
    };
    assert!(!agent.runnable);

    let resolution = selection(
        &mut values,
        &world,
        agent_id,
        world.first,
        &claim,
        &first_binding,
        &context,
        &first_session,
        context_value,
        [first_selection.id(), second_selection.id()],
    )?;
    let variable = vec![
        first_selection.clone(),
        second_selection.clone(),
        resolution.clone(),
    ];
    let reports = arrival_permutations(&variable)
        .into_iter()
        .map(|arrival| {
            reduce_complete(
                world
                    .base_facts()
                    .into_iter()
                    .chain([
                        claim.clone(),
                        first_binding.clone(),
                        second_binding.clone(),
                        context.clone(),
                    ])
                    .chain(arrival),
                &world.reducer(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert!(reports.windows(2).all(|pair| pair[0] == pair[1]));
    let resolved = reports.first().ok_or("missing selection schedules")?;
    let duplicate_report = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([
                claim.clone(),
                first_binding.clone(),
                second_binding.clone(),
                context.clone(),
            ])
            .chain(variable.iter().cloned())
            .chain(variable.iter().cloned()),
        &world.reducer(),
    )?;
    assert_eq!(resolved, &duplicate_report);
    let Some(AgentProjection::Selection(view)) = resolved
        .projections()
        .get(&AgentProjectionKey::Selection(agent_id))
    else {
        return Err("missing resolved selection".into());
    };
    assert!(!view.conflicted);
    assert_eq!(
        view.active.as_ref().map(|value| &value.session),
        Some(&first_session)
    );
    let Some(AgentProjection::Agent(agent)) = resolved
        .projections()
        .get(&AgentProjectionKey::Agent(agent_id))
    else {
        return Err("missing runnable agent".into());
    };
    assert!(agent.runnable);
    assert_eq!(agent.selected_session.as_ref(), Some(&first_session));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn rename_and_context_frontiers_retain_multivalue_history_without_changing_selection()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(120);
    let world = agent_world(&mut values)?;
    let agent_id = values.agent_id();
    let claim = claim(
        &mut values,
        &world,
        agent_id,
        world.first,
        &world.first_root,
        "helper",
        [],
    )?;
    let session = session("provider", "session")?;
    let binding = binding(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        &session,
    )?;
    let first_context_value = repository_context("/work/one")?;
    let second_context_value = repository_context("/work/two")?;
    let first_context = context_fact(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        first_context_value.clone(),
        [],
    )?;
    let second_context = context_fact(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        second_context_value.clone(),
        [],
    )?;
    let selected = selection(
        &mut values,
        &world,
        agent_id,
        world.first,
        &claim,
        &binding,
        &first_context,
        &session,
        first_context_value,
        [],
    )?;
    let first_rename = rename(
        &mut values,
        &world,
        agent_id,
        &claim,
        &binding,
        &session,
        Some("Alpha"),
        [],
    )?;
    let second_rename = rename(
        &mut values,
        &world,
        agent_id,
        &claim,
        &binding,
        &session,
        Some("Beta"),
        [],
    )?;
    let mismatched_selection = selection(
        &mut values,
        &world,
        agent_id,
        world.first,
        &claim,
        &binding,
        &first_context,
        &session,
        second_context_value,
        [],
    )?;
    let report = reduce_complete(
        world.base_facts().into_iter().chain([
            claim.clone(),
            binding.clone(),
            first_context.clone(),
            second_context.clone(),
            selected.clone(),
            first_rename.clone(),
            second_rename.clone(),
            mismatched_selection.clone(),
        ]),
        &world.reducer(),
    )?;
    assert_eq!(
        report.decisions()[&mismatched_selection.id()].status(),
        DecisionStatus::Invalid
    );
    let Some(AgentProjection::Context(context)) = report
        .projections()
        .get(&AgentProjectionKey::Context(world.first))
    else {
        return Err("missing context history".into());
    };
    assert_eq!(context.history.len(), 2);
    assert_eq!(context.frontier.len(), 2);
    let rename_key = AgentProjectionKey::Rename {
        agent: agent_id,
        session: session.clone(),
    };
    let Some(AgentProjection::Rename(rename_view)) = report.projections().get(&rename_key) else {
        return Err("missing rename conflict".into());
    };
    assert!(!rename_view.resolved);
    assert_eq!(rename_view.candidates.len(), 2);
    let selection_before = report
        .projections()
        .get(&AgentProjectionKey::Selection(agent_id))
        .cloned();

    let resolution = rename(
        &mut values,
        &world,
        agent_id,
        &claim,
        &binding,
        &session,
        None,
        [first_rename.id(), second_rename.id()],
    )?;
    let resolved = reduce_complete(
        world.base_facts().into_iter().chain([
            claim,
            binding,
            first_context,
            second_context,
            selected,
            first_rename,
            second_rename,
            resolution,
        ]),
        &world.reducer(),
    )?;
    let Some(AgentProjection::Rename(rename)) = resolved.projections().get(&rename_key) else {
        return Err("missing resolved rename".into());
    };
    assert!(rename.resolved);
    assert!(rename.display_name.is_none());
    assert_eq!(
        selection_before,
        resolved
            .projections()
            .get(&AgentProjectionKey::Selection(agent_id))
            .cloned()
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn retirement_is_absorbing_remove_wins_and_preserves_historical_sessions()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(150);
    let world = agent_world(&mut values)?;
    let agent_id = values.agent_id();
    let claim = claim(
        &mut values,
        &world,
        agent_id,
        world.first,
        &world.first_root,
        "helper",
        [],
    )?;
    let session = session("provider", "session")?;
    let binding = binding(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        &session,
    )?;
    let context_value = repository_context("/work")?;
    let context = context_fact(
        &mut values,
        &world,
        world.first,
        &world.first_root,
        context_value.clone(),
        [],
    )?;
    let selected = selection(
        &mut values,
        &world,
        agent_id,
        world.first,
        &claim,
        &binding,
        &context,
        &session,
        context_value.clone(),
        [],
    )?;
    let retired = retirement(&mut values, &world, agent_id, world.first, &claim, [])?;
    let report = reduce_complete(
        world.base_facts().into_iter().chain([
            claim.clone(),
            binding.clone(),
            context.clone(),
            selected.clone(),
            retired.clone(),
        ]),
        &world.reducer(),
    )?;
    let Some(AgentProjection::Agent(agent)) = report
        .projections()
        .get(&AgentProjectionKey::Agent(agent_id))
    else {
        return Err("missing retired agent".into());
    };
    assert_eq!(agent.lifecycle, AgentLifecycle::Retired);
    assert!(!agent.runnable);
    assert!(
        report
            .projections()
            .contains_key(&AgentProjectionKey::Session(session.clone()))
    );
    let Some(AgentProjection::Selection(selection_view)) = report
        .projections()
        .get(&AgentProjectionKey::Selection(agent_id))
    else {
        return Err("missing historical selection".into());
    };
    assert!(selection_view.active.is_none());

    let after_retirement = selection(
        &mut values,
        &world,
        agent_id,
        world.first,
        &claim,
        &binding,
        &context,
        &session,
        context_value,
        [retired.id()],
    )?;
    let after = reduce_complete(
        world.base_facts().into_iter().chain([
            claim,
            binding,
            context,
            selected,
            retired,
            after_retirement.clone(),
        ]),
        &world.reducer(),
    )?;
    assert_eq!(
        after.decisions()[&after_retirement.id()].status(),
        DecisionStatus::Invalid
    );
    Ok(())
}
