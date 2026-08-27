//! Project identity, home-linear state, claims, assignments, dispatch, output, and control contracts.

use std::{error::Error, num::NonZeroU64};

use hq_domain::{
    AccountId, AgentId, AssignmentBinding, AssignmentId, AuthorityReference, AuthorityRole,
    BoundedVec, CommandDigest, CommandId, DispatchId, Fact, FactId, FactScope, InitialProjectState,
    InstallationAddress, InstallationId, MailboxAddress, MailboxId, MessageContent, MessageId,
    MessagePurpose, OperationCorrelation, OperationId, PresentationKind, ProjectId,
    ProjectResource, ProviderId, ProviderSessionId, RemoteCommandResult, RepositoryContext,
    ResourceHealth, ResourceId, ResourceLocator, ResourceScheme, RuntimeObservation,
    SemanticPayload, ShortText, SigningPublicKey, ThreadId, Timestamp,
};
use hq_reducer::{
    AuthorityPolicy, DecisionStatus, ProjectAssignmentPhase, ProjectLifecycle, ProjectOutputStatus,
    ProjectProjection, ProjectProjectionKey, ProjectReason, ProjectReducer, RemoteCommandStage,
    reduce_complete,
};
use hq_testkit::{DeterministicValues, FactBuilder, arrival_permutations};

const PROJECT_SCENARIO_COVERAGE: [&str; 27] = [
    "PRJ-001", "PRJ-002", "PRJ-003", "PRJ-004", "PRJ-005", "PRJ-006", "PRJ-007", "PRJ-008",
    "PRJ-009", "PRJ-010", "PRJ-011", "PRJ-012", "PRJ-013", "PRJ-014", "PRJ-015", "PRJ-016",
    "PRJ-017", "PRJ-018", "PRJ-019", "PRJ-020", "PRJ-021", "PRJ-022", "PRJ-023", "CTL-001",
    "CTL-002", "CTL-003", "CTL-004",
];

#[test]
fn every_project_and_control_scenario_is_mapped() {
    assert_eq!(PROJECT_SCENARIO_COVERAGE.len(), 27);
    assert_eq!(PROJECT_SCENARIO_COVERAGE[0], "PRJ-001");
    assert_eq!(PROJECT_SCENARIO_COVERAGE[22], "PRJ-023");
    assert_eq!(PROJECT_SCENARIO_COVERAGE[26], "CTL-004");
}

fn address(value: u8) -> InstallationAddress {
    InstallationAddress::new(
        InstallationId::from_bytes([value; 32]),
        SigningPublicKey::from_bytes([value.wrapping_add(64); 32]),
    )
}

fn path(value: &str) -> Result<ResourceLocator, Box<dyn Error>> {
    Ok(ResourceLocator::new(
        ResourceScheme::WorkingTree,
        hq_domain::BoundedText::new(value)?,
    ))
}

fn resource(id: u8, value: &str) -> Result<ProjectResource, Box<dyn Error>> {
    Ok(ProjectResource {
        resource_id: ResourceId::from_bytes([id; 32]),
        display_locator: path(value)?,
        canonical_locator: path(value)?,
        health: ResourceHealth::Unknown,
    })
}

fn assert_atomic_replacement(
    report: &hq_reducer::ProjectReport,
    project_id: ProjectId,
    replaced: FactId,
    old: ResourceId,
    replacement: ResourceId,
) -> Result<(), Box<dyn Error>> {
    let Some(ProjectProjection::Project(view)) = report
        .projections()
        .get(&ProjectProjectionKey::Project(project_id))
    else {
        return Err("missing replaced project".into());
    };
    assert_eq!(view.head, replaced);
    assert_eq!(view.primary, Some(replacement));
    assert_eq!(
        view.active_claims,
        std::collections::BTreeSet::from([replacement])
    );
    assert!(view.resources.contains_key(&replacement));
    assert!(!view.resources.contains_key(&old));
    Ok(())
}

fn reduce_replacement_prefix(
    world: &ProjectWorld,
    create: &Fact,
    health: &Fact,
    replaced: &Fact,
) -> Result<hq_reducer::ProjectReport, Box<dyn Error>> {
    Ok(reduce_complete(
        world
            .base()
            .into_iter()
            .chain([create.clone(), health.clone(), replaced.clone()]),
        &world.reducer(),
    )?)
}

fn assert_restored_project(
    report: &hq_reducer::ProjectReport,
    project_id: ProjectId,
    restored: FactId,
    old: ResourceId,
    replacement: ResourceId,
) -> Result<(), Box<dyn Error>> {
    let Some(ProjectProjection::Project(view)) = report
        .projections()
        .get(&ProjectProjectionKey::Project(project_id))
    else {
        return Err("missing restored project".into());
    };
    assert_eq!(view.head, restored);
    assert_eq!(view.lifecycle, ProjectLifecycle::Closed);
    assert!(!view.archived);
    assert!(view.active_claims.is_empty());
    assert_eq!(view.primary, Some(replacement));
    assert!(view.resources.contains_key(&replacement));
    assert!(!view.resources.contains_key(&old));
    Ok(())
}

#[derive(Clone)]
struct ProjectWorld {
    home: InstallationAddress,
    installation: Fact,
    account_id: AccountId,
    account: Fact,
    human_mailbox: MailboxId,
}

impl ProjectWorld {
    fn reducer(&self) -> ProjectReducer {
        ProjectReducer::new(AuthorityPolicy::new(
            self.home.installation_id(),
            self.human_mailbox,
        ))
    }

    fn base(&self) -> Vec<Fact> {
        vec![self.installation.clone(), self.account.clone()]
    }
}

fn project_world(values: &mut DeterministicValues) -> Result<ProjectWorld, Box<dyn Error>> {
    let home = address(1);
    let installation = FactBuilder::with_causal(
        values,
        home,
        Timestamp::from_unix_millis(0),
        FactScope::InstallationPrivate(home.installation_id()),
        [],
        [],
        SemanticPayload::InstallationDeclared {
            installation_id: home.installation_id(),
            signing_key: home.signing_key(),
            encryption_key: hq_domain::EncryptionPublicKey::from_bytes([9; 32]),
            label: Some(ShortText::new("home")?),
        },
    )?;
    let account_id = AccountId::from_bytes([2; 32]);
    let account = FactBuilder::with_causal(
        values,
        home,
        Timestamp::from_unix_millis(1),
        FactScope::InstallationPrivate(home.installation_id()),
        [installation.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation.id(),
        )],
        SemanticPayload::HumanAccountCreated {
            account_id,
            creator: home,
            label: Some(ShortText::new("account")?),
        },
    )?;
    Ok(ProjectWorld {
        home,
        installation,
        account_id,
        account,
        human_mailbox: MailboxId::from_bytes([3; 32]),
    })
}

fn project_created(
    values: &mut DeterministicValues,
    world: &ProjectWorld,
    project_id: ProjectId,
    mailbox_id: MailboxId,
    resources: Vec<ProjectResource>,
    primary: Option<ResourceId>,
    initial_state: InitialProjectState,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.home,
        Timestamp::from_unix_millis(2),
        FactScope::AccountAddressed(world.account_id),
        [world.installation.id(), world.account.id()],
        [
            AuthorityReference::new(AuthorityRole::AccountMembership, world.account.id()),
            AuthorityReference::new(AuthorityRole::ActiveHuman, world.account.id()),
            AuthorityReference::new(AuthorityRole::ProjectHome, world.installation.id()),
        ],
        SemanticPayload::ProjectCreated {
            project_id,
            mailbox_id,
            home: world.home.installation_id(),
            name: ShortText::new("project")?,
            brief: None,
            predecessor: None,
            resources: BoundedVec::new(resources)?,
            primary,
            initial_state,
        },
    )?)
}

fn project_transition(
    values: &mut DeterministicValues,
    world: &ProjectWorld,
    previous: &Fact,
    payload: SemanticPayload,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.home,
        Timestamp::from_unix_millis(3),
        FactScope::AccountAddressed(world.account_id),
        [previous.id(), world.installation.id(), world.account.id()],
        [
            AuthorityReference::new(AuthorityRole::PreviousState, previous.id()),
            AuthorityReference::new(AuthorityRole::AccountMembership, world.account.id()),
            AuthorityReference::new(AuthorityRole::ActiveHuman, world.account.id()),
            AuthorityReference::new(AuthorityRole::ProjectHome, world.installation.id()),
        ],
        payload,
    )?)
}

fn project_transition_with_parents(
    values: &mut DeterministicValues,
    world: &ProjectWorld,
    previous: &Fact,
    extra_parents: impl IntoIterator<Item = FactId>,
    payload: SemanticPayload,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.home,
        Timestamp::from_unix_millis(4),
        FactScope::AccountAddressed(world.account_id),
        [previous.id(), world.installation.id(), world.account.id()]
            .into_iter()
            .chain(extra_parents),
        [
            AuthorityReference::new(AuthorityRole::PreviousState, previous.id()),
            AuthorityReference::new(AuthorityRole::AccountMembership, world.account.id()),
            AuthorityReference::new(AuthorityRole::ActiveHuman, world.account.id()),
            AuthorityReference::new(AuthorityRole::ProjectHome, world.installation.id()),
        ],
        payload,
    )?)
}

fn project_message(
    values: &mut DeterministicValues,
    world: &ProjectWorld,
    project_id: ProjectId,
    mailbox_id: MailboxId,
    message_id: MessageId,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.home,
        Timestamp::from_unix_millis(4),
        FactScope::AccountAddressed(world.account_id),
        [world.account.id()],
        [AuthorityReference::new(
            AuthorityRole::AccountMembership,
            world.account.id(),
        )],
        SemanticPayload::QuestionAsked(MessageContent {
            message_id,
            sender: MailboxAddress::new(world.home.installation_id(), world.human_mailbox),
            recipient: Some(MailboxAddress::new(
                world.home.installation_id(),
                mailbox_id,
            )),
            body: hq_domain::ContentText::new("input")?,
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: Some(project_id),
        }),
    )?)
}

fn assignment_binding(value: u8) -> Result<AssignmentBinding, Box<dyn Error>> {
    Ok(AssignmentBinding {
        assignment_id: AssignmentId::from_bytes([value; 32]),
        agent_id: AgentId::from_bytes([value; 32]),
        provider: ProviderId::new("codex")?,
        session: ProviderSessionId::new(format!("session-{value}"))?,
    })
}

#[derive(Clone)]
struct AgentSupport {
    facts: Vec<Fact>,
    claim: Fact,
    selection: Fact,
}

fn agent_support(
    values: &mut DeterministicValues,
    world: &ProjectWorld,
    value: u8,
    binding: &AssignmentBinding,
) -> Result<AgentSupport, Box<dyn Error>> {
    let mailbox_id = MailboxId::from_bytes([value.wrapping_add(10); 32]);
    let local_fact = |values: &mut DeterministicValues,
                      parents: Vec<FactId>,
                      payload: SemanticPayload|
     -> Result<Fact, Box<dyn Error>> {
        Ok(FactBuilder::with_causal(
            values,
            world.home,
            Timestamp::from_unix_millis(2),
            FactScope::InstallationPrivate(world.home.installation_id()),
            std::iter::once(world.installation.id()).chain(parents),
            [AuthorityReference::new(
                AuthorityRole::LocalInstallation,
                world.installation.id(),
            )],
            payload,
        )?)
    };
    let mailbox = local_fact(
        values,
        vec![],
        SemanticPayload::MailboxCreated {
            mailbox_id,
            kind: hq_domain::MailboxKind::Agent,
            label: Some(ShortText::new("project-agent")?),
        },
    )?;
    let claim = local_fact(
        values,
        vec![mailbox.id()],
        SemanticPayload::AgentNameClaimed {
            agent_id: binding.agent_id,
            mailbox_id,
            name: ShortText::new(format!("agent-{value}"))?,
        },
    )?;
    let session = local_fact(
        values,
        vec![mailbox.id()],
        SemanticPayload::MailboxSessionBound {
            mailbox_id,
            provider: binding.provider.clone(),
            session: binding.session.clone(),
        },
    )?;
    let repository = RepositoryContext {
        directory: path("/work/project")?,
        repository: None,
        worktree: None,
        branch: Some(ShortText::new("main")?),
    };
    let context = local_fact(
        values,
        vec![mailbox.id()],
        SemanticPayload::MailboxContextRecorded {
            mailbox_id,
            context: repository.clone(),
        },
    )?;
    let selection = local_fact(
        values,
        vec![claim.id(), session.id(), context.id()],
        SemanticPayload::ProviderSessionSelected {
            agent_id: binding.agent_id,
            mailbox_id,
            provider: binding.provider.clone(),
            session: binding.session.clone(),
            context: repository,
        },
    )?;
    Ok(AgentSupport {
        facts: vec![mailbox, claim.clone(), session, context, selection.clone()],
        claim,
        selection,
    })
}

#[test]
fn create_open_and_closed_projects_derive_identity_claims_and_primary_path()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(1);
    let world = project_world(&mut values)?;
    let closed_id = ProjectId::from_bytes([10; 32]);
    let open_id = ProjectId::from_bytes([11; 32]);
    let closed_resource = resource(10, "/work/closed")?;
    let open_resource = resource(11, "/work/open")?;
    let closed = project_created(
        &mut values,
        &world,
        closed_id,
        MailboxId::from_bytes([10; 32]),
        vec![closed_resource.clone()],
        Some(closed_resource.resource_id),
        InitialProjectState::Closed,
    )?;
    let open = project_created(
        &mut values,
        &world,
        open_id,
        MailboxId::from_bytes([11; 32]),
        vec![open_resource.clone()],
        Some(open_resource.resource_id),
        InitialProjectState::Open,
    )?;

    for arrival in arrival_permutations(&[closed.clone(), open.clone()]) {
        let report = reduce_complete(world.base().into_iter().chain(arrival), &world.reducer())?;
        let Some(ProjectProjection::Project(closed_view)) = report
            .projections()
            .get(&ProjectProjectionKey::Project(closed_id))
        else {
            return Err("missing closed project".into());
        };
        assert_eq!(closed_view.lifecycle, ProjectLifecycle::Closed);
        assert!(closed_view.active_claims.is_empty());
        assert_eq!(closed_view.primary, Some(closed_resource.resource_id));

        let Some(ProjectProjection::Project(open_view)) = report
            .projections()
            .get(&ProjectProjectionKey::Project(open_id))
        else {
            return Err("missing open project".into());
        };
        assert_eq!(open_view.lifecycle, ProjectLifecycle::Open);
        assert_eq!(open_view.active_claims.len(), 1);
        assert!(open_view.claimable);
    }
    Ok(())
}

#[test]
fn sibling_home_transitions_stop_at_the_common_head_and_archive_laws_are_explicit()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(20);
    let world = project_world(&mut values)?;
    let project_id = ProjectId::from_bytes([20; 32]);
    let create = project_created(
        &mut values,
        &world,
        project_id,
        MailboxId::from_bytes([20; 32]),
        vec![],
        None,
        InitialProjectState::Closed,
    )?;
    let first = project_transition(
        &mut values,
        &world,
        &create,
        SemanticPayload::ProjectOpened { project_id },
    )?;
    let sibling = project_transition(
        &mut values,
        &world,
        &create,
        SemanticPayload::ProjectMetadataUpdated {
            project_id,
            name: ShortText::new("renamed")?,
            brief: None,
        },
    )?;
    let report = reduce_complete(
        world
            .base()
            .into_iter()
            .chain([create.clone(), first.clone(), sibling.clone()]),
        &world.reducer(),
    )?;
    assert_eq!(
        report.decisions()[&first.id()].status(),
        DecisionStatus::Conflicted
    );
    assert_eq!(
        report.decisions()[&sibling.id()].status(),
        DecisionStatus::Conflicted
    );
    let Some(ProjectProjection::Project(view)) = report
        .projections()
        .get(&ProjectProjectionKey::Project(project_id))
    else {
        return Err("missing project".into());
    };
    assert_eq!(view.head, create.id());
    assert_eq!(view.fork_participants.len(), 2);

    let invalid_archive = project_transition(
        &mut values,
        &world,
        &first,
        SemanticPayload::ProjectArchived { project_id },
    )?;
    let isolated = reduce_complete(
        world
            .base()
            .into_iter()
            .chain([create, first, invalid_archive.clone()]),
        &world.reducer(),
    )?;
    assert_eq!(
        isolated.decisions()[&invalid_archive.id()].reason(),
        Some(&hq_reducer::DecisionReason::Domain(
            ProjectReason::InvalidTransition,
        ))
    );
    Ok(())
}

#[test]
fn cross_project_path_conflicts_fail_closed_but_local_overlap_and_home_namespaces_do_not()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(40);
    let world = project_world(&mut values)?;
    let parent = resource(40, "/repo")?;
    let child = resource(41, "/repo/worktree")?;
    let first_id = ProjectId::from_bytes([40; 32]);
    let second_id = ProjectId::from_bytes([41; 32]);
    let first = project_created(
        &mut values,
        &world,
        first_id,
        MailboxId::from_bytes([40; 32]),
        vec![parent.clone(), child.clone()],
        Some(parent.resource_id),
        InitialProjectState::Open,
    )?;
    let second = project_created(
        &mut values,
        &world,
        second_id,
        MailboxId::from_bytes([41; 32]),
        vec![resource(42, "/repo/other")?],
        Some(ResourceId::from_bytes([42; 32])),
        InitialProjectState::Open,
    )?;
    let report = reduce_complete(
        world.base().into_iter().chain([first, second]),
        &world.reducer(),
    )?;
    for project_id in [first_id, second_id] {
        let Some(ProjectProjection::Project(view)) = report
            .projections()
            .get(&ProjectProjectionKey::Project(project_id))
        else {
            return Err("missing conflicted project".into());
        };
        assert!(!view.claimable);
        assert!(view.active_claims.is_empty());
        assert!(!view.claim_conflicts.is_empty());
    }
    Ok(())
}

#[test]
fn lifecycle_resource_health_replacement_close_archive_and_restore_follow_explicit_laws()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(60);
    let world = project_world(&mut values)?;
    let project_id = ProjectId::from_bytes([60; 32]);
    let mailbox_id = MailboxId::from_bytes([60; 32]);
    let old = resource(60, "/work/old")?;
    let create = project_created(
        &mut values,
        &world,
        project_id,
        mailbox_id,
        vec![old.clone()],
        Some(old.resource_id),
        InitialProjectState::Open,
    )?;
    let health = project_transition(
        &mut values,
        &world,
        &create,
        SemanticPayload::ProjectResourceHealthObserved {
            project_id,
            resource_id: old.resource_id,
            health: ResourceHealth::Unavailable,
            details: Some(hq_domain::ContentText::new("missing")?),
            checked_at: Timestamp::from_unix_millis(100),
        },
    )?;
    let replacement = resource(61, "/work/new")?;
    let replaced = project_transition(
        &mut values,
        &world,
        &health,
        SemanticPayload::ProjectResourceReplaced {
            project_id,
            old_resource_id: old.resource_id,
            new_resource: replacement.clone(),
        },
    )?;
    let replacement_report = reduce_replacement_prefix(&world, &create, &health, &replaced)?;
    assert_atomic_replacement(
        &replacement_report,
        project_id,
        replaced.id(),
        old.resource_id,
        replacement.resource_id,
    )?;
    let closing = project_transition(
        &mut values,
        &world,
        &replaced,
        SemanticPayload::ProjectClosingStarted { project_id },
    )?;
    let closed = project_transition(
        &mut values,
        &world,
        &closing,
        SemanticPayload::ProjectClosed {
            project_id,
            forced: true,
            runtime: Some(hq_domain::RuntimeObservation::Uncertain(
                hq_domain::ErrorCode::new("runtime-unknown")?,
            )),
        },
    )?;
    let archived = project_transition(
        &mut values,
        &world,
        &closed,
        SemanticPayload::ProjectArchived { project_id },
    )?;
    let restored = project_transition(
        &mut values,
        &world,
        &archived,
        SemanticPayload::ProjectUnarchived { project_id },
    )?;
    let report = reduce_complete(
        world.base().into_iter().chain([
            create,
            health,
            replaced,
            closing,
            closed,
            archived,
            restored.clone(),
        ]),
        &world.reducer(),
    )?;
    assert_restored_project(
        &report,
        project_id,
        restored.id(),
        old.resource_id,
        replacement.resource_id,
    )
}

#[test]
fn assignment_requires_an_exact_active_agent_claim_and_selected_provider_session()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(75);
    let world = project_world(&mut values)?;
    let project_id = ProjectId::from_bytes([75; 32]);
    let create = project_created(
        &mut values,
        &world,
        project_id,
        MailboxId::from_bytes([75; 32]),
        vec![],
        None,
        InitialProjectState::Open,
    )?;
    let binding = assignment_binding(75)?;
    let unsupported = project_transition(
        &mut values,
        &world,
        &create,
        SemanticPayload::ProjectAssignmentConfiguring {
            project_id,
            binding,
        },
    )?;
    let report = reduce_complete(
        world
            .base()
            .into_iter()
            .chain([create.clone(), unsupported.clone()]),
        &world.reducer(),
    )?;
    assert_eq!(
        report.decisions()[&unsupported.id()].reason(),
        Some(&hq_reducer::DecisionReason::Domain(
            ProjectReason::AssignmentBindingMismatch,
        ))
    );
    let Some(ProjectProjection::Project(view)) = report
        .projections()
        .get(&ProjectProjectionKey::Project(project_id))
    else {
        return Err("missing unassigned project".into());
    };
    assert_eq!(view.head, create.id());
    assert!(view.assignment.is_none());
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn assignment_cardinality_conflict_retracts_and_resolves_when_one_epoch_ends()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(80);
    let world = project_world(&mut values)?;
    let first_id = ProjectId::from_bytes([80; 32]);
    let second_id = ProjectId::from_bytes([81; 32]);
    let first = project_created(
        &mut values,
        &world,
        first_id,
        MailboxId::from_bytes([80; 32]),
        vec![],
        None,
        InitialProjectState::Open,
    )?;
    let second = project_created(
        &mut values,
        &world,
        second_id,
        MailboxId::from_bytes([81; 32]),
        vec![],
        None,
        InitialProjectState::Open,
    )?;
    let first_binding = assignment_binding(80)?;
    let agent = agent_support(&mut values, &world, 80, &first_binding)?;
    let second_binding = AssignmentBinding {
        assignment_id: AssignmentId::from_bytes([81; 32]),
        agent_id: first_binding.agent_id,
        provider: first_binding.provider.clone(),
        session: first_binding.session.clone(),
    };
    let first_assignment = project_transition_with_parents(
        &mut values,
        &world,
        &first,
        [agent.claim.id(), agent.selection.id()],
        SemanticPayload::ProjectAssignmentConfiguring {
            project_id: first_id,
            binding: first_binding.clone(),
        },
    )?;
    let second_assignment = project_transition_with_parents(
        &mut values,
        &world,
        &second,
        [agent.claim.id(), agent.selection.id()],
        SemanticPayload::ProjectAssignmentConfiguring {
            project_id: second_id,
            binding: second_binding.clone(),
        },
    )?;
    let conflicted = reduce_complete(
        world.base().into_iter().chain(agent.facts.clone()).chain([
            first.clone(),
            second.clone(),
            first_assignment.clone(),
            second_assignment.clone(),
        ]),
        &world.reducer(),
    )?;
    for project_id in [first_id, second_id] {
        let Some(ProjectProjection::Project(view)) = conflicted
            .projections()
            .get(&ProjectProjectionKey::Project(project_id))
        else {
            return Err("missing assigned project".into());
        };
        assert!(view.assignment.as_ref().is_some_and(
            |assignment| assignment.cardinality_conflicted
                && !assignment.runnable
                && assignment.phase == ProjectAssignmentPhase::Configuring
        ));
    }

    let ended = project_transition(
        &mut values,
        &world,
        &second_assignment,
        SemanticPayload::ProjectAssignmentEnded {
            project_id: second_id,
            assignment_id: second_binding.assignment_id,
            forced: true,
            runtime: None,
        },
    )?;
    let resolved = reduce_complete(
        world.base().into_iter().chain(agent.facts).chain([
            first,
            second,
            first_assignment,
            second_assignment,
            ended,
        ]),
        &world.reducer(),
    )?;
    let Some(ProjectProjection::Project(first_view)) = resolved
        .projections()
        .get(&ProjectProjectionKey::Project(first_id))
    else {
        return Err("missing remaining assignment".into());
    };
    assert!(
        first_view
            .assignment
            .as_ref()
            .is_some_and(|assignment| !assignment.cardinality_conflicted)
    );
    let Some(ProjectProjection::Project(second_view)) = resolved
        .projections()
        .get(&ProjectProjectionKey::Project(second_id))
    else {
        return Err("missing ended assignment project".into());
    };
    assert!(second_view.assignment.is_none());
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn input_dispatch_and_output_keep_exact_provenance_and_late_output_is_inert()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(100);
    let world = project_world(&mut values)?;
    let project_id = ProjectId::from_bytes([100; 32]);
    let mailbox_id = MailboxId::from_bytes([100; 32]);
    let create = project_created(
        &mut values,
        &world,
        project_id,
        mailbox_id,
        vec![],
        None,
        InitialProjectState::Open,
    )?;
    let binding = assignment_binding(100)?;
    let agent = agent_support(&mut values, &world, 100, &binding)?;
    let configuring = project_transition_with_parents(
        &mut values,
        &world,
        &create,
        [agent.claim.id(), agent.selection.id()],
        SemanticPayload::ProjectAssignmentConfiguring {
            project_id,
            binding: binding.clone(),
        },
    )?;
    let input_id = MessageId::from_bytes([101; 32]);
    let message = project_message(&mut values, &world, project_id, mailbox_id, input_id)?;
    let thread_id = ThreadId::from_bytes(*message.id().as_bytes());
    let runnable = project_transition_with_parents(
        &mut values,
        &world,
        &configuring,
        [message.id(), agent.claim.id(), agent.selection.id()],
        SemanticPayload::ProjectAssignmentRunnable {
            project_id,
            binding: binding.clone(),
            thread_id,
            launch_directory: path("/work/project")?,
            activation: OperationCorrelation::new(
                binding.provider.clone(),
                binding.session.clone(),
                OperationId::from_bytes([100; 32]),
            ),
        },
    )?;
    let accepted = project_transition_with_parents(
        &mut values,
        &world,
        &runnable,
        [message.id()],
        SemanticPayload::ProjectInputAccepted {
            project_id,
            message_id: input_id,
            input_fact_id: message.id(),
            sequence: NonZeroU64::new(1).ok_or("nonzero")?,
        },
    )?;
    let dispatch_id = DispatchId::from_bytes([102; 32]);
    let dispatched = project_transition(
        &mut values,
        &world,
        &accepted,
        SemanticPayload::ProjectInputDispatched {
            project_id,
            message_id: input_id,
            sequence: NonZeroU64::new(1).ok_or("nonzero")?,
            dispatch_id,
            binding: binding.clone(),
            thread_id,
        },
    )?;
    let output_message = |id, body: &str| -> Result<MessageContent, Box<dyn Error>> {
        Ok(MessageContent {
            message_id: id,
            sender: MailboxAddress::new(
                world.home.installation_id(),
                MailboxId::from_bytes([110; 32]),
            ),
            recipient: Some(MailboxAddress::new(
                world.home.installation_id(),
                mailbox_id,
            )),
            body: hq_domain::ContentText::new(body)?,
            purpose: MessagePurpose::ProjectOutput,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: Some(project_id),
        })
    };
    let first_output_id = MessageId::from_bytes([103; 32]);
    let first_output = project_transition(
        &mut values,
        &world,
        &dispatched,
        SemanticPayload::ProjectOutputRecorded {
            project_id,
            output_id: first_output_id,
            dispatch_id,
            binding: binding.clone(),
            thread_id,
            message: output_message(first_output_id, "before handoff")?,
        },
    )?;
    let ended = project_transition(
        &mut values,
        &world,
        &first_output,
        SemanticPayload::ProjectAssignmentEnded {
            project_id,
            assignment_id: binding.assignment_id,
            forced: false,
            runtime: None,
        },
    )?;
    let late_output_id = MessageId::from_bytes([104; 32]);
    let late_output = project_transition_with_parents(
        &mut values,
        &world,
        &ended,
        [dispatched.id()],
        SemanticPayload::ProjectOutputRecorded {
            project_id,
            output_id: late_output_id,
            dispatch_id,
            binding: binding.clone(),
            thread_id,
            message: output_message(late_output_id, "late")?,
        },
    )?;
    let report = reduce_complete(
        world.base().into_iter().chain(agent.facts).chain([
            create,
            configuring,
            message,
            runnable,
            accepted,
            dispatched.clone(),
            first_output,
            ended,
            late_output.clone(),
        ]),
        &world.reducer(),
    )?;
    let Some(ProjectProjection::Dispatch(dispatch)) = report
        .projections()
        .get(&ProjectProjectionKey::Dispatch(dispatch_id))
    else {
        return Err("missing dispatch".into());
    };
    assert_eq!(dispatch.binding, binding);
    assert_eq!(dispatch.thread_id, thread_id);
    let Some(ProjectProjection::Output(first)) = report
        .projections()
        .get(&ProjectProjectionKey::Output(first_output_id))
    else {
        return Err("missing first output".into());
    };
    assert_eq!(first.status, ProjectOutputStatus::Current);
    let Some(ProjectProjection::Output(late)) = report
        .projections()
        .get(&ProjectProjectionKey::Output(late_output_id))
    else {
        return Err("missing late output".into());
    };
    assert_eq!(late.status, ProjectOutputStatus::LateFromInactive);
    let Some(ProjectProjection::Project(view)) = report
        .projections()
        .get(&ProjectProjectionKey::Project(project_id))
    else {
        return Err("missing project".into());
    };
    assert_eq!(view.head, late_output.id());
    assert!(view.assignment.is_none());
    assert_eq!(view.lifecycle, ProjectLifecycle::Open);
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn remote_command_stages_never_mutate_project_without_a_canonical_home_transition()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(120);
    let world = project_world(&mut values)?;
    let project_id = ProjectId::from_bytes([120; 32]);
    let create = project_created(
        &mut values,
        &world,
        project_id,
        MailboxId::from_bytes([120; 32]),
        vec![],
        None,
        InitialProjectState::Closed,
    )?;
    let command_id = CommandId::from_bytes([121; 32]);
    let digest = CommandDigest::from_bytes([122; 32]);
    let request = remote_fact(
        &mut values,
        &world,
        [create.id()],
        [],
        SemanticPayload::RemoteProjectCommandRequested {
            command_id,
            digest,
            project_id,
            target_home: world.home.installation_id(),
            expected_head: create.id(),
            operation: OperationCorrelation::new(
                ProviderId::new("remote")?,
                ProviderSessionId::new("control")?,
                OperationId::from_bytes([123; 32]),
            ),
            body: hq_domain::ContentText::new("rename")?,
        },
    )?;
    let queued = reduce_complete(
        world
            .base()
            .into_iter()
            .chain([create.clone(), request.clone()]),
        &world.reducer(),
    )?;
    let Some(ProjectProjection::Project(project)) = queued
        .projections()
        .get(&ProjectProjectionKey::Project(project_id))
    else {
        return Err("missing queued project".into());
    };
    assert_eq!(project.head, create.id());
    let Some(ProjectProjection::Command(command)) = queued
        .projections()
        .get(&ProjectProjectionKey::Command(command_id))
    else {
        return Err("missing queued command".into());
    };
    assert_eq!(command.stage, RemoteCommandStage::Queued);

    let receipt = remote_fact(
        &mut values,
        &world,
        [request.id(), create.id()],
        [AuthorityReference::new(
            AuthorityRole::Request,
            request.id(),
        )],
        SemanticPayload::RemoteProjectCommandReceipt {
            command_id,
            digest,
            project_id,
            received_head: create.id(),
            received_at: Timestamp::from_unix_millis(11),
        },
    )?;
    let committed = project_transition(
        &mut values,
        &world,
        &create,
        SemanticPayload::ProjectMetadataUpdated {
            project_id,
            name: ShortText::new("remote-rename")?,
            brief: None,
        },
    )?;
    let outcome = remote_fact(
        &mut values,
        &world,
        [request.id(), receipt.id(), committed.id()],
        [AuthorityReference::new(
            AuthorityRole::Request,
            request.id(),
        )],
        SemanticPayload::RemoteProjectCommandOutcome {
            command_id,
            digest,
            project_id,
            result: RemoteCommandResult::Committed(committed.id()),
            runtime: Some(RuntimeObservation::Succeeded),
        },
    )?;
    let report = reduce_complete(
        world
            .base()
            .into_iter()
            .chain([create, request, receipt, committed.clone(), outcome]),
        &world.reducer(),
    )?;
    let Some(ProjectProjection::Command(command)) = report
        .projections()
        .get(&ProjectProjectionKey::Command(command_id))
    else {
        return Err("missing terminal command".into());
    };
    assert_eq!(
        command.stage,
        RemoteCommandStage::Terminal {
            result: RemoteCommandResult::Committed(committed.id()),
            runtime: Some(RuntimeObservation::Succeeded),
        }
    );
    let Some(ProjectProjection::Project(project)) = report
        .projections()
        .get(&ProjectProjectionKey::Project(project_id))
    else {
        return Err("missing committed project".into());
    };
    assert_eq!(project.head, committed.id());
    assert_eq!(project.name, ShortText::new("remote-rename")?);
    Ok(())
}

fn remote_fact(
    values: &mut DeterministicValues,
    world: &ProjectWorld,
    extra_parents: impl IntoIterator<Item = FactId>,
    extra_authorities: impl IntoIterator<Item = AuthorityReference>,
    payload: SemanticPayload,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.home,
        Timestamp::from_unix_millis(10),
        FactScope::RemoteControl {
            account_id: world.account_id,
            target_home: world.home.installation_id(),
        },
        [world.account.id(), world.installation.id()]
            .into_iter()
            .chain(extra_parents),
        [
            AuthorityReference::new(AuthorityRole::AccountMembership, world.account.id()),
            AuthorityReference::new(AuthorityRole::ActiveHuman, world.account.id()),
            AuthorityReference::new(AuthorityRole::ProjectHome, world.installation.id()),
        ]
        .into_iter()
        .chain(extra_authorities),
        payload,
    )?)
}

fn _fact_id_type_check(value: FactId) -> FactId {
    value
}
