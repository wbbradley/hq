//! Conversation, message-state, presentation, and activity reduction contracts.

use std::{collections::BTreeSet, error::Error, num::NonZeroU64};

use hq_domain::{
    AccountId, ActivityKind, ActivityStatus, AgentId, AssignmentBinding, AssignmentId,
    AuthorityReference, AuthorityRole, ContentText, ConversationId, DispatchId, Fact, FactId,
    FactScope, GrantId, InstallationAddress, InstallationId, MailboxAddress, MailboxId,
    MailboxKind, MessageContent, MessageId, MessagePurpose, OperationCorrelation, PresentationKind,
    ProjectActivityAttribution, ProjectId, ProviderId, ProviderSessionId, SemanticPayload,
    ShortText, SigningPublicKey, ThreadId, Timestamp,
};
use hq_reducer::{
    ActivityKey, ActivitySessionKey, AuthorityPolicy, CausalRelation, ConversationKey,
    ConversationProjection, ConversationProjectionKey, ConversationReason, ConversationReducer,
    DecisionReason, DecisionStatus, conversation_orders, incomplete_addressed_observations,
    reduce_complete,
};
use hq_testkit::{DeterministicValues, FactBuilder, arrival_permutations};

const CONVERSATION_SCENARIO_COVERAGE: [(&str, &str); 27] = [
    ("CONV-001", "local question and answer"),
    ("CONV-002", "multiple answers"),
    ("CONV-003", "answer and cancellation relations"),
    ("CONV-004", "missing question observation"),
    ("CONV-005", "stable message identity collision"),
    ("CONV-006", "state target ancestry"),
    ("CONV-007", "archive and restore"),
    ("CONV-008", "concurrent archive and restore"),
    ("CONV-009", "rearchive after restore"),
    ("CONV-010", "absorbing rejection"),
    ("CONV-011", "typed semantics ignore prose"),
    ("CONV-012", "peer received causal proof"),
    ("CONV-013", "account fanout convergence"),
    ("CONV-014", "final answer selection"),
    ("CONV-015", "equal time mixed order"),
    ("CONV-016", "child clock before parent"),
    ("CONV-017", "project exchanges retain initiating thread"),
    ("ACT-001", "activity inertness"),
    ("ACT-002", "sequenced snapshots"),
    ("ACT-003", "concurrent sequence winner"),
    ("ACT-004", "equal sequence collision"),
    ("ACT-005", "provider namespace"),
    ("ACT-006", "source mailbox namespace"),
    ("ACT-007", "delayed occurrence"),
    ("ACT-008", "completed item retention"),
    ("ACT-009", "compacted rebuild"),
    ("REG-002", "canonical mixed conversation comparator"),
];

#[test]
fn every_named_conversation_and_activity_scenario_is_mapped() {
    assert_eq!(CONVERSATION_SCENARIO_COVERAGE.len(), 27);
    assert!(
        CONVERSATION_SCENARIO_COVERAGE
            .iter()
            .all(|(scenario, executable_case)| {
                (scenario.starts_with("CONV-")
                    || scenario.starts_with("ACT-")
                    || *scenario == "REG-002")
                    && !executable_case.is_empty()
            })
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn project_output_and_activity_join_only_their_initiating_exchange_for_every_arrival_order()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(250);
    let world = local_world(&mut values)?;
    let project_id = ProjectId::from_bytes([0x41; 32]);
    let project_mailbox = MailboxAddress::new(
        world.installation.installation_id(),
        MailboxId::from_bytes([0x42; 32]),
    );
    let first_message_id = values.message_id();
    let first = project_input_fact(
        &mut values,
        &world,
        project_id,
        project_mailbox,
        first_message_id,
        10,
        "Let's have a conversation.",
    )?;
    let first_thread = ThreadId::from_bytes(*first.id().as_bytes());
    let second_message_id = values.message_id();
    let second = project_input_fact(
        &mut values,
        &world,
        project_id,
        project_mailbox,
        second_message_id,
        20,
        "Let's have another conversation.",
    )?;
    let second_thread = ThreadId::from_bytes(*second.id().as_bytes());
    let continuation_id = values.message_id();
    let continuation = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(22),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [
            world.installation_root.id(),
            world.human_root.id(),
            first.id(),
        ],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::AsynchronousMessageSent {
            thread_id: Some(first_thread),
            message: MessageContent {
                message_id: continuation_id,
                sender: world.human,
                recipient: Some(project_mailbox),
                body: ContentText::new("One more point on the same topic.")?,
                purpose: MessagePurpose::Asynchronous,
                presentation: PresentationKind::Message,
                correlation: None,
                project_id: Some(project_id),
            },
        },
    )?;
    let agent_id = AgentId::from_bytes([0x43; 32]);
    let claim = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), world.agent_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id: world.agent.mailbox_id(),
            name: ShortText::new("alice")?,
        },
    )?;
    let dispatch_id = DispatchId::from_bytes([0x44; 32]);
    let binding = AssignmentBinding {
        assignment_id: AssignmentId::from_bytes([0x45; 32]),
        agent_id,
        provider: ProviderId::new("codex")?,
        session: ProviderSessionId::new("01a0544a-af23-7a52-8df7-71f6dfbb4efc")?,
    };
    let dispatch = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(25),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), claim.id(), first.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::ProjectInputDispatched {
            project_id,
            message_id: first_message_id,
            sequence: NonZeroU64::MIN,
            dispatch_id,
            binding: binding.clone(),
            thread_id: first_thread,
        },
    )?;
    let correlation = OperationCorrelation::new(
        binding.provider.clone(),
        binding.session.clone(),
        hq_domain::OperationId::from_bytes([0x46; 32]),
    );
    let output_id = values.message_id();
    let output = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(30),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), claim.id(), dispatch.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::ProjectOutputRecorded {
            project_id,
            output_id,
            dispatch_id,
            binding: binding.clone(),
            thread_id: first_thread,
            message: MessageContent {
                message_id: output_id,
                sender: world.agent,
                recipient: Some(project_mailbox),
                body: ContentText::new("Absolutely. What’s on your mind?")?,
                purpose: MessagePurpose::ProjectOutput,
                presentation: PresentationKind::FinalAnswer,
                correlation: Some(correlation.clone()),
                project_id: Some(project_id),
            },
        },
    )?;
    let activity = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(40),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [
            world.installation_root.id(),
            world.agent_root.id(),
            claim.id(),
            dispatch.id(),
        ],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::HarnessActivityRecorded {
            project: Some(ProjectActivityAttribution {
                project_id,
                dispatch_id,
                binding,
                thread_id: first_thread,
            }),
            source: world.agent,
            correlation,
            item: None,
            kind: ActivityKind::Status,
            logical_key: ShortText::new("turn")?,
            runtime: ShortText::new("runtime")?,
            sequence: NonZeroU64::MIN,
            occurred_at: Timestamp::from_unix_millis(40),
            status: ActivityStatus::Succeeded,
            content: ContentText::new("Codex turn completed")?,
            truncated: false,
            completed: None,
        },
    )?;
    let variable = vec![
        first.clone(),
        second.clone(),
        continuation.clone(),
        claim.clone(),
        dispatch.clone(),
        output.clone(),
        activity.clone(),
    ];
    let policy = AuthorityPolicy::new(
        world.installation.installation_id(),
        world.human.mailbox_id(),
    );
    let expected = [
        (
            ConversationKey::ProjectThread {
                project_id,
                thread: first_thread,
            },
            vec![first.id(), continuation.id(), output.id(), activity.id()],
        ),
        (
            ConversationKey::ProjectThread {
                project_id,
                thread: second_thread,
            },
            vec![second.id()],
        ),
    ]
    .into_iter()
    .collect();
    for arrival in arrival_permutations(&variable) {
        let report = reduce_complete(
            world.base_facts().into_iter().chain(arrival),
            &world.reducer(),
        )?;
        assert_eq!(conversation_orders(&report, policy)?, expected);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn project_input_fact(
    values: &mut DeterministicValues,
    world: &LocalWorld,
    project_id: ProjectId,
    project_mailbox: MailboxAddress,
    message_id: MessageId,
    authored_at: i64,
    body: &str,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.installation,
        Timestamp::from_unix_millis(authored_at),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), world.human_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::AsynchronousMessageSent {
            thread_id: None,
            message: MessageContent {
                message_id,
                sender: world.human,
                recipient: Some(project_mailbox),
                body: ContentText::new(body)?,
                purpose: MessagePurpose::Asynchronous,
                presentation: PresentationKind::Message,
                correlation: None,
                project_id: Some(project_id),
            },
        },
    )?)
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

fn message(
    message_id: hq_domain::MessageId,
    sender: MailboxAddress,
    recipient: MailboxAddress,
    purpose: MessagePurpose,
) -> Result<MessageContent, Box<dyn Error>> {
    Ok(MessageContent {
        message_id,
        sender,
        recipient: Some(recipient),
        body: ContentText::new("typed conversation body")?,
        purpose,
        presentation: PresentationKind::Message,
        correlation: None,
        project_id: None,
    })
}

#[test]
fn local_question_and_answer_form_one_ready_thread() -> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(1);
    let installation = address(1);
    let installation_root = root(&mut values, installation)?;
    let human = mailbox(installation, 1);
    let agent = mailbox(installation, 2);
    let human_root = mailbox_created(&mut values, &installation_root, human, MailboxKind::Human)?;
    let agent_root = mailbox_created(&mut values, &installation_root, agent, MailboxKind::Agent)?;
    let question_message_id = values.message_id();
    let question = FactBuilder::with_causal(
        &mut values,
        installation,
        Timestamp::from_unix_millis(10),
        FactScope::InstallationPrivate(installation.installation_id()),
        [installation_root.id(), agent_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation_root.id(),
        )],
        SemanticPayload::QuestionAsked(message(
            question_message_id,
            agent,
            human,
            MessagePurpose::Question,
        )?),
    )?;
    let thread_id = ThreadId::from_bytes(*question.id().as_bytes());
    let answer_message_id = values.message_id();
    let answer = FactBuilder::with_causal(
        &mut values,
        installation,
        Timestamp::from_unix_millis(11),
        FactScope::InstallationPrivate(installation.installation_id()),
        [installation_root.id(), human_root.id(), question.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation_root.id(),
        )],
        SemanticPayload::AnswerGiven {
            thread_id,
            message: message(answer_message_id, human, agent, MessagePurpose::Question)?,
        },
    )?;

    let report = reduce_complete(
        [
            answer.clone(),
            agent_root,
            question.clone(),
            installation_root,
            human_root,
        ],
        &ConversationReducer::new(AuthorityPolicy::new(
            installation.installation_id(),
            human.mailbox_id(),
        )),
    )?;

    assert_eq!(
        report
            .decisions()
            .get(&question.id())
            .map(hq_reducer::FactDecision::status),
        Some(DecisionStatus::Projected)
    );
    assert_eq!(
        report
            .decisions()
            .get(&answer.id())
            .map(hq_reducer::FactDecision::status),
        Some(DecisionStatus::Projected)
    );
    let Some(ConversationProjection::Thread(thread)) = report
        .projections()
        .get(&ConversationProjectionKey::Thread(thread_id))
    else {
        return Err("missing normalized question thread".into());
    };
    assert_eq!(thread.root_fact, question.id());
    assert_eq!(thread.answers, [answer.id()].into_iter().collect());
    assert_eq!(thread.ready_answers, vec![answer.id()]);
    assert!(!thread.cancelled);
    Ok(())
}

#[test]
fn equal_time_mixed_entries_and_delayed_occurrence_use_the_single_parent_first_order()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(240);
    let world = local_world(&mut values)?;
    let question_id = values.message_id();
    let question = question_fact(&mut values, &world, 100, question_id)?;
    let answer_id = values.message_id();
    let answer = answer_fact(
        &mut values,
        &world,
        &question,
        -100,
        answer_id,
        [],
        PresentationKind::Message,
        None,
    )?;
    let async_id = values.message_id();
    let asynchronous = local_message_fact(
        &mut values,
        &world,
        100,
        [world.agent_root.id()],
        async_id,
        world.agent,
        world.human,
        MessagePurpose::Asynchronous,
        PresentationKind::Message,
        None,
        |message| SemanticPayload::AsynchronousMessageSent {
            thread_id: None,
            message,
        },
    )?;
    let correlation = operation(4, "provider", "session")?;
    let early_occurrence = activity_fact(
        &mut values,
        &world,
        100,
        -500,
        world.agent,
        correlation.clone(),
        Some("early"),
        ActivityKind::Status,
        "early",
        "runtime",
        1,
        "delayed occurrence",
    )?;
    let exact_tie = activity_fact(
        &mut values,
        &world,
        100,
        100,
        world.agent,
        correlation,
        Some("tie"),
        ActivityKind::Status,
        "tie",
        "runtime",
        2,
        "exact tie",
    )?;
    let variable = vec![
        question.clone(),
        answer.clone(),
        asynchronous.clone(),
        early_occurrence.clone(),
        exact_tie.clone(),
    ];
    let reports = arrival_permutations(&variable)
        .into_iter()
        .map(|arrival| {
            reduce_complete(
                world.base_facts().into_iter().chain(arrival),
                &world.reducer(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert!(reports.windows(2).all(|pair| pair[0] == pair[1]));
    let order = reports[0].presentation_order();
    let position = |fact_id| order.iter().position(|candidate| *candidate == fact_id);
    let question_position = position(question.id()).ok_or("question not presented")?;
    let answer_position = position(answer.id()).ok_or("answer not presented")?;
    let early_position = position(early_occurrence.id()).ok_or("early activity not presented")?;
    let async_position = position(asynchronous.id()).ok_or("async message not presented")?;
    let tie_position = position(exact_tie.id()).ok_or("tie activity not presented")?;
    assert!(question_position < answer_position);
    assert!(early_position < async_position);
    assert!(async_position < tie_position);
    Ok(())
}

#[derive(Clone)]
struct LocalWorld {
    installation: InstallationAddress,
    installation_root: Fact,
    human: MailboxAddress,
    human_root: Fact,
    agent: MailboxAddress,
    agent_root: Fact,
}

impl LocalWorld {
    fn reducer(&self) -> ConversationReducer {
        ConversationReducer::new(AuthorityPolicy::new(
            self.installation.installation_id(),
            self.human.mailbox_id(),
        ))
    }

    fn base_facts(&self) -> Vec<Fact> {
        vec![
            self.installation_root.clone(),
            self.human_root.clone(),
            self.agent_root.clone(),
        ]
    }
}

fn local_world(values: &mut DeterministicValues) -> Result<LocalWorld, Box<dyn Error>> {
    let installation = address(10);
    let installation_root = root(values, installation)?;
    let human = mailbox(installation, 10);
    let agent = mailbox(installation, 11);
    let human_root = mailbox_created(values, &installation_root, human, MailboxKind::Human)?;
    let agent_root = mailbox_created(values, &installation_root, agent, MailboxKind::Agent)?;
    Ok(LocalWorld {
        installation,
        installation_root,
        human,
        human_root,
        agent,
        agent_root,
    })
}

#[allow(clippy::too_many_arguments)]
fn local_message_fact<F>(
    values: &mut DeterministicValues,
    world: &LocalWorld,
    authored_at: i64,
    parents: impl IntoIterator<Item = FactId>,
    message_id: MessageId,
    sender: MailboxAddress,
    recipient: MailboxAddress,
    purpose: MessagePurpose,
    presentation: PresentationKind,
    correlation: Option<OperationCorrelation>,
    payload: F,
) -> Result<Fact, Box<dyn Error>>
where
    F: FnOnce(MessageContent) -> SemanticPayload,
{
    let parents = std::iter::once(world.installation_root.id())
        .chain(parents)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        world.installation,
        Timestamp::from_unix_millis(authored_at),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        parents,
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        payload(MessageContent {
            message_id,
            sender,
            recipient: Some(recipient),
            body: ContentText::new("typed body")?,
            purpose,
            presentation,
            correlation,
            project_id: None,
        }),
    )?)
}

fn question_fact(
    values: &mut DeterministicValues,
    world: &LocalWorld,
    authored_at: i64,
    message_id: MessageId,
) -> Result<Fact, Box<dyn Error>> {
    local_message_fact(
        values,
        world,
        authored_at,
        [world.agent_root.id()],
        message_id,
        world.agent,
        world.human,
        MessagePurpose::Question,
        PresentationKind::Message,
        None,
        SemanticPayload::QuestionAsked,
    )
}

#[allow(clippy::too_many_arguments)]
fn answer_fact(
    values: &mut DeterministicValues,
    world: &LocalWorld,
    question: &Fact,
    authored_at: i64,
    message_id: MessageId,
    extra_parents: impl IntoIterator<Item = FactId>,
    presentation: PresentationKind,
    correlation: Option<OperationCorrelation>,
) -> Result<Fact, Box<dyn Error>> {
    let thread = ThreadId::from_bytes(*question.id().as_bytes());
    let parents = [world.human_root.id(), question.id()]
        .into_iter()
        .chain(extra_parents)
        .collect::<Vec<_>>();
    local_message_fact(
        values,
        world,
        authored_at,
        parents,
        message_id,
        world.human,
        world.agent,
        MessagePurpose::Question,
        presentation,
        correlation,
        |message| SemanticPayload::AnswerGiven {
            thread_id: thread,
            message,
        },
    )
}

fn cancellation_fact(
    values: &mut DeterministicValues,
    world: &LocalWorld,
    question: &Fact,
    authored_at: i64,
    extra_parents: impl IntoIterator<Item = FactId>,
) -> Result<Fact, Box<dyn Error>> {
    let parents = std::iter::once(question.id())
        .chain(extra_parents)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        world.installation,
        Timestamp::from_unix_millis(authored_at),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        std::iter::once(world.installation_root.id()).chain(parents),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::ThreadCancelled {
            thread_id: ThreadId::from_bytes(*question.id().as_bytes()),
            reason: Some(ContentText::new("cancelled")?),
        },
    )?)
}

fn state_fact<F>(
    values: &mut DeterministicValues,
    world: &LocalWorld,
    target: &Fact,
    authored_at: i64,
    extra_parents: impl IntoIterator<Item = FactId>,
    payload: F,
) -> Result<Fact, Box<dyn Error>>
where
    F: FnOnce(MessageId) -> SemanticPayload,
{
    let message_id = message_content_of(target)?.message_id;
    let parents = [world.installation_root.id(), target.id()]
        .into_iter()
        .chain(extra_parents)
        .collect::<Vec<_>>();
    Ok(FactBuilder::with_causal(
        values,
        world.installation,
        Timestamp::from_unix_millis(authored_at),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        parents,
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        payload(message_id),
    )?)
}

fn message_content_of(fact: &Fact) -> Result<&MessageContent, Box<dyn Error>> {
    match fact.payload() {
        SemanticPayload::QuestionAsked(message)
        | SemanticPayload::AsynchronousMessageSent { message, .. }
        | SemanticPayload::AnswerGiven { message, .. } => Ok(message),
        _ => Err("fixture is not message content".into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn activity_fact(
    values: &mut DeterministicValues,
    world: &LocalWorld,
    authored_at: i64,
    occurred_at: i64,
    source: MailboxAddress,
    correlation: OperationCorrelation,
    item: Option<&str>,
    kind: ActivityKind,
    logical_key: &str,
    runtime: &str,
    sequence: u64,
    content: &str,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        world.installation,
        Timestamp::from_unix_millis(authored_at),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), world.agent_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::HarnessActivityRecorded {
            project: None,
            source,
            correlation,
            item: item.map(ShortText::new).transpose()?,
            kind,
            logical_key: ShortText::new(logical_key)?,
            runtime: ShortText::new(runtime)?,
            sequence: NonZeroU64::new(sequence).ok_or("sequence must be positive")?,
            occurred_at: Timestamp::from_unix_millis(occurred_at),
            status: if kind == ActivityKind::CompletedItem {
                ActivityStatus::Succeeded
            } else {
                ActivityStatus::Running
            },
            content: ContentText::new(content)?,
            truncated: false,
            completed: (kind == ActivityKind::CompletedItem)
                .then_some(hq_domain::CompletedItemPresentation::Unknown),
        },
    )?)
}

fn operation(
    value: u8,
    provider: &str,
    session: &str,
) -> Result<OperationCorrelation, Box<dyn Error>> {
    Ok(OperationCorrelation::new(
        ProviderId::new(provider)?,
        ProviderSessionId::new(session)?,
        hq_domain::OperationId::from_bytes([value; 32]),
    ))
}

#[test]
#[allow(clippy::too_many_lines)]
fn attributed_activity_requires_its_exact_dispatch_and_agent_source() -> Result<(), Box<dyn Error>>
{
    let mut values = DeterministicValues::new(29);
    let world = local_world(&mut values)?;
    let agent_id = AgentId::from_bytes([0x31; 32]);
    let claim = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), world.agent_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id: world.agent.mailbox_id(),
            name: ShortText::new("alice")?,
        },
    )?;
    let project_id = ProjectId::from_bytes([0x32; 32]);
    let dispatch_id = DispatchId::from_bytes([0x33; 32]);
    let thread_id = ThreadId::from_bytes([0x34; 32]);
    let binding = AssignmentBinding {
        assignment_id: AssignmentId::from_bytes([0x35; 32]),
        agent_id,
        provider: ProviderId::new("provider")?,
        session: ProviderSessionId::new("session")?,
    };
    let dispatch = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(3),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), claim.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::ProjectInputDispatched {
            project_id,
            message_id: MessageId::from_bytes([0x37; 32]),
            sequence: NonZeroU64::MIN,
            dispatch_id,
            binding: binding.clone(),
            thread_id,
        },
    )?;
    let activity = |values: &mut DeterministicValues,
                    attribution: ProjectActivityAttribution,
                    logical_key: &str|
     -> Result<Fact, Box<dyn Error>> {
        Ok(FactBuilder::with_causal(
            values,
            world.installation,
            Timestamp::from_unix_millis(4),
            FactScope::InstallationPrivate(world.installation.installation_id()),
            [
                world.installation_root.id(),
                world.agent_root.id(),
                claim.id(),
                dispatch.id(),
            ],
            [AuthorityReference::new(
                AuthorityRole::LocalInstallation,
                world.installation_root.id(),
            )],
            SemanticPayload::HarnessActivityRecorded {
                project: Some(attribution),
                source: world.agent,
                correlation: OperationCorrelation::new(
                    binding.provider.clone(),
                    binding.session.clone(),
                    hq_domain::OperationId::from_bytes([0x38; 32]),
                ),
                item: None,
                kind: ActivityKind::Status,
                logical_key: ShortText::new(logical_key)?,
                runtime: ShortText::new("runtime")?,
                sequence: NonZeroU64::MIN,
                occurred_at: Timestamp::from_unix_millis(4),
                status: ActivityStatus::Succeeded,
                content: ContentText::new("complete")?,
                truncated: false,
                completed: None,
            },
        )?)
    };
    let exact = ProjectActivityAttribution {
        project_id,
        dispatch_id,
        binding: binding.clone(),
        thread_id,
    };
    let valid = activity(&mut values, exact.clone(), "operation")?;
    let mut wrong_thread = exact;
    wrong_thread.thread_id = ThreadId::from_bytes([0x39; 32]);
    let invalid = activity(&mut values, wrong_thread, "other-operation")?;
    let output_id = values.message_id();
    let output = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(5),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), dispatch.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::ProjectOutputRecorded {
            project_id,
            output_id,
            dispatch_id,
            binding: binding.clone(),
            thread_id,
            message: MessageContent {
                message_id: output_id,
                sender: world.agent,
                recipient: Some(world.human),
                body: ContentText::new("project output")?,
                purpose: MessagePurpose::ProjectOutput,
                presentation: PresentationKind::FinalAnswer,
                correlation: Some(OperationCorrelation::new(
                    binding.provider.clone(),
                    binding.session.clone(),
                    hq_domain::OperationId::from_bytes([0x38; 32]),
                )),
                project_id: Some(project_id),
            },
        },
    )?;
    let report = reduce_complete(
        world.base_facts().into_iter().chain([
            claim,
            dispatch,
            valid.clone(),
            invalid.clone(),
            output.clone(),
        ]),
        &world.reducer(),
    )?;
    assert_eq!(
        report.decisions()[&valid.id()].status(),
        DecisionStatus::Projected
    );
    assert_eq!(
        report.decisions()[&invalid.id()].status(),
        DecisionStatus::Invalid
    );
    assert_eq!(
        report.decisions()[&invalid.id()].reason(),
        Some(&DecisionReason::Domain(
            ConversationReason::ActivitySourceMismatch
        ))
    );
    let Some(ConversationProjection::Message(message)) = report
        .projections()
        .get(&ConversationProjectionKey::Message(output_id))
    else {
        return Err("missing project output message projection".into());
    };
    assert_eq!(message.fact_id, output.id());
    assert_eq!(message.thread_id, thread_id);
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn answers_and_cancellations_accumulate_with_exact_relations_for_every_arrival_order()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(30);
    let world = local_world(&mut values)?;
    let question_id = values.message_id();
    let question = question_fact(&mut values, &world, 100, question_id)?;
    let before_id = values.message_id();
    let answer_before = answer_fact(
        &mut values,
        &world,
        &question,
        200,
        before_id,
        [],
        PresentationKind::Message,
        None,
    )?;
    let cancellation_after =
        cancellation_fact(&mut values, &world, &question, 300, [answer_before.id()])?;
    let cancellation_before = cancellation_fact(&mut values, &world, &question, 50, [])?;
    let after_id = values.message_id();
    let answer_after = answer_fact(
        &mut values,
        &world,
        &question,
        40,
        after_id,
        [cancellation_before.id()],
        PresentationKind::Message,
        None,
    )?;
    let concurrent_cancel = cancellation_fact(&mut values, &world, &question, 210, [])?;
    let concurrent_id = values.message_id();
    let concurrent_answer = answer_fact(
        &mut values,
        &world,
        &question,
        210,
        concurrent_id,
        [],
        PresentationKind::Message,
        None,
    )?;
    let variable = vec![
        answer_before.clone(),
        cancellation_after.clone(),
        cancellation_before.clone(),
        answer_after.clone(),
        concurrent_cancel.clone(),
        concurrent_answer.clone(),
    ];
    let mut expected = None;
    for arrival in arrival_permutations(&variable) {
        let report = reduce_complete(
            world
                .base_facts()
                .into_iter()
                .chain(std::iter::once(question.clone()))
                .chain(arrival),
            &world.reducer(),
        )?;
        if let Some(previous) = &expected {
            assert_eq!(previous, &report);
        } else {
            expected = Some(report);
        }
    }
    let report = expected.ok_or("missing arrival schedule")?;
    let thread_id = ThreadId::from_bytes(*question.id().as_bytes());
    let Some(ConversationProjection::Thread(thread)) = report
        .projections()
        .get(&ConversationProjectionKey::Thread(thread_id))
    else {
        return Err("missing thread".into());
    };
    assert_eq!(thread.answers.len(), 3);
    assert_eq!(thread.cancellations.len(), 3);
    assert!(thread.cancelled);
    assert_eq!(
        thread
            .relations
            .get(&(answer_before.id(), cancellation_after.id())),
        Some(&CausalRelation::Before)
    );
    assert_eq!(
        thread
            .relations
            .get(&(answer_after.id(), cancellation_before.id())),
        Some(&CausalRelation::After)
    );
    assert_eq!(
        thread
            .relations
            .get(&(concurrent_answer.id(), concurrent_cancel.id())),
        Some(&CausalRelation::Concurrent)
    );
    assert!(
        thread
            .ready_answers
            .iter()
            .position(|id| *id == cancellation_before.id())
            .is_none()
    );
    assert!(
        thread
            .ready_answers
            .iter()
            .position(|id| *id == answer_after.id())
            < thread
                .ready_answers
                .iter()
                .position(|id| *id == answer_before.id())
    );
    Ok(())
}

#[test]
fn missing_addressed_content_is_inert_and_stable_message_collisions_fail_closed()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(60);
    let world = local_world(&mut values)?;
    let remote = address(90);
    let target = mailbox(world.installation, 90);
    let missing_question = values.fact_id();
    let missing_grant = values.fact_id();
    let unresolved_message_id = values.message_id();
    let unresolved = FactBuilder::with_causal(
        &mut values,
        remote,
        Timestamp::from_unix_millis(1),
        FactScope::PeerAddressed(target),
        [missing_question, missing_grant],
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            missing_grant,
        )],
        SemanticPayload::AnswerGiven {
            thread_id: ThreadId::from_bytes(*missing_question.as_bytes()),
            message: MessageContent {
                message_id: unresolved_message_id,
                sender: mailbox(remote, 1),
                recipient: Some(target),
                body: ContentText::new("visible but incomplete")?,
                purpose: MessagePurpose::Question,
                presentation: PresentationKind::FinalAnswer,
                correlation: Some(operation(1, "provider", "session")?),
                project_id: None,
            },
        },
    )?;
    let report = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain(std::iter::once(unresolved.clone())),
        &world.reducer(),
    )?;
    assert_eq!(
        report.decisions()[&unresolved.id()].status(),
        DecisionStatus::Unresolved
    );
    assert!(report.projections().is_empty());
    let observations = incomplete_addressed_observations(&report);
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].fact_id, unresolved.id());
    assert_eq!(
        observations[0].missing_dependencies,
        [missing_question, missing_grant].into_iter().collect()
    );

    let shared_id = values.message_id();
    let first = question_fact(&mut values, &world, 10, shared_id)?;
    let unequal_body = ContentText::new("unequal body")?;
    let second = local_message_fact(
        &mut values,
        &world,
        11,
        [world.agent_root.id()],
        shared_id,
        world.agent,
        world.human,
        MessagePurpose::Question,
        PresentationKind::Message,
        None,
        move |mut content| {
            content.body = unequal_body;
            SemanticPayload::QuestionAsked(content)
        },
    )?;
    let report = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([first.clone(), second.clone()]),
        &world.reducer(),
    )?;
    for fact in [&first, &second] {
        assert_eq!(
            report.decisions()[&fact.id()].status(),
            DecisionStatus::Conflicted
        );
        assert_eq!(
            report.decisions()[&fact.id()].conflict_participants(),
            &[first.id(), second.id()].into_iter().collect()
        );
    }
    assert!(
        !report
            .projections()
            .contains_key(&ConversationProjectionKey::Message(shared_id))
    );
    Ok(())
}

fn mailbox_grant(
    values: &mut DeterministicValues,
    owner_mailbox_root: &Fact,
    target: MailboxAddress,
    grantee: InstallationAddress,
    grant_id: GrantId,
) -> Result<Fact, Box<dyn Error>> {
    Ok(FactBuilder::with_causal(
        values,
        owner_mailbox_root.author(),
        Timestamp::from_unix_millis(5),
        FactScope::PeerAddressed(target),
        [owner_mailbox_root.id()],
        [AuthorityReference::new(
            AuthorityRole::MailboxOwner,
            owner_mailbox_root.id(),
        )],
        SemanticPayload::MailboxAccessGranted {
            grant_id,
            mailbox: target,
            grantee,
        },
    )?)
}

#[test]
#[allow(clippy::too_many_lines)]
fn peer_received_delivery_requires_a_usable_peer_child_citing_the_outbound_fact()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(75);
    let local = local_world(&mut values)?;
    let remote_installation = address(40);
    let remote_root = root(&mut values, remote_installation)?;
    let remote_agent = mailbox(remote_installation, 40);
    let remote_agent_root =
        mailbox_created(&mut values, &remote_root, remote_agent, MailboxKind::Agent)?;
    let outbound_grant_id = values.grant_id();
    let outbound_grant = mailbox_grant(
        &mut values,
        &remote_agent_root,
        remote_agent,
        local.installation,
        outbound_grant_id,
    )?;
    let inbound_grant_id = values.grant_id();
    let inbound_grant = mailbox_grant(
        &mut values,
        &local.agent_root,
        local.agent,
        remote_installation,
        inbound_grant_id,
    )?;
    let question_id = values.message_id();
    let question = FactBuilder::with_causal(
        &mut values,
        local.installation,
        Timestamp::from_unix_millis(10),
        FactScope::PeerAddressed(remote_agent),
        [outbound_grant.id()],
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            outbound_grant.id(),
        )],
        SemanticPayload::QuestionAsked(MessageContent {
            message_id: question_id,
            sender: local.agent,
            recipient: Some(remote_agent),
            body: ContentText::new("outbound")?,
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: None,
        }),
    )?;
    let answer_id = values.message_id();
    let answer = FactBuilder::with_causal(
        &mut values,
        remote_installation,
        Timestamp::from_unix_millis(11),
        FactScope::PeerAddressed(local.agent),
        [inbound_grant.id(), question.id()],
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            inbound_grant.id(),
        )],
        SemanticPayload::AnswerGiven {
            thread_id: ThreadId::from_bytes(*question.id().as_bytes()),
            message: MessageContent {
                message_id: answer_id,
                sender: remote_agent,
                recipient: Some(local.agent),
                body: ContentText::new("peer answer")?,
                purpose: MessagePurpose::Question,
                presentation: PresentationKind::Message,
                correlation: None,
                project_id: None,
            },
        },
    )?;
    let report_without_child = reduce_complete(
        local.base_facts().into_iter().chain([
            remote_root.clone(),
            remote_agent_root.clone(),
            outbound_grant.clone(),
            inbound_grant.clone(),
            question.clone(),
        ]),
        &local.reducer(),
    )?;
    assert!(
        message_view(&report_without_child, question_id)?
            .peer_received_by
            .is_empty()
    );
    let report = reduce_complete(
        local.base_facts().into_iter().chain([
            remote_root,
            remote_agent_root,
            outbound_grant,
            inbound_grant,
            question.clone(),
            answer.clone(),
        ]),
        &local.reducer(),
    )?;
    assert_eq!(
        message_view(&report, question_id)?.peer_received_by,
        [answer.id()].into_iter().collect()
    );
    Ok(())
}

#[test]
fn one_account_fact_has_identical_conversation_meaning_under_device_local_policies()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(85);
    let creator = local_world(&mut values)?;
    let device = address(41);
    let device_root = root(&mut values, device)?;
    let device_human = mailbox(device, 41);
    let device_human_root =
        mailbox_created(&mut values, &device_root, device_human, MailboxKind::Human)?;
    let account_id = AccountId::from_bytes([9; 32]);
    let account = FactBuilder::with_causal(
        &mut values,
        creator.installation,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(creator.installation.installation_id()),
        [creator.installation_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            creator.installation_root.id(),
        )],
        SemanticPayload::HumanAccountCreated {
            account_id,
            creator: creator.installation,
            label: Some(ShortText::new("account")?),
        },
    )?;
    let message_id = values.message_id();
    let account_message = FactBuilder::with_causal(
        &mut values,
        creator.installation,
        Timestamp::from_unix_millis(10),
        FactScope::AccountAddressed(account_id),
        [account.id()],
        [AuthorityReference::new(
            AuthorityRole::AccountMembership,
            account.id(),
        )],
        SemanticPayload::AsynchronousMessageSent {
            thread_id: None,
            message: MessageContent {
                message_id,
                sender: creator.agent,
                recipient: None,
                body: ContentText::new("one canonical fanout fact")?,
                purpose: MessagePurpose::Asynchronous,
                presentation: PresentationKind::Message,
                correlation: None,
                project_id: None,
            },
        },
    )?;
    let facts = creator
        .base_facts()
        .into_iter()
        .chain([
            device_root,
            device_human_root,
            account,
            account_message.clone(),
        ])
        .collect::<Vec<_>>();
    let creator_report = reduce_complete(facts.clone(), &creator.reducer())?;
    let device_report = reduce_complete(
        facts,
        &ConversationReducer::new(AuthorityPolicy::new(
            device.installation_id(),
            device_human.mailbox_id(),
        )),
    )?;
    assert_eq!(
        creator_report.decisions()[&account_message.id()].status(),
        DecisionStatus::Projected,
        "creator decision: {:?}",
        creator_report.decisions()[&account_message.id()]
    );
    assert_eq!(
        device_report.decisions()[&account_message.id()].status(),
        DecisionStatus::Projected,
        "device decision: {:?}",
        device_report.decisions()[&account_message.id()]
    );
    assert_eq!(
        creator_report.decisions()[&account_message.id()],
        device_report.decisions()[&account_message.id()]
    );
    assert_eq!(
        message_view(&creator_report, message_id)?,
        message_view(&device_report, message_id)?
    );
    assert_eq!(
        creator_report.presentation_order(),
        device_report.presentation_order()
    );
    Ok(())
}

#[test]
fn account_addressed_project_input_requires_both_project_and_direct_recipient()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(86);
    let creator = local_world(&mut values)?;
    let account_id = AccountId::from_bytes([9; 32]);
    let account = FactBuilder::with_causal(
        &mut values,
        creator.installation,
        Timestamp::from_unix_millis(2),
        FactScope::InstallationPrivate(creator.installation.installation_id()),
        [creator.installation_root.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            creator.installation_root.id(),
        )],
        SemanticPayload::HumanAccountCreated {
            account_id,
            creator: creator.installation,
            label: Some(ShortText::new("account")?),
        },
    )?;
    let project_id = ProjectId::from_bytes([10; 32]);
    let project_mailbox = MailboxAddress::new(
        creator.installation.installation_id(),
        MailboxId::from_bytes([11; 32]),
    );
    let project_message = |values: &mut DeterministicValues,
                           message_id,
                           recipient,
                           project_id|
     -> Result<Fact, Box<dyn Error>> {
        Ok(FactBuilder::with_causal(
            values,
            creator.installation,
            Timestamp::from_unix_millis(10),
            FactScope::AccountAddressed(account_id),
            [account.id()],
            [AuthorityReference::new(
                AuthorityRole::AccountMembership,
                account.id(),
            )],
            SemanticPayload::AsynchronousMessageSent {
                thread_id: None,
                message: MessageContent {
                    message_id,
                    sender: creator.human,
                    recipient,
                    body: ContentText::new("project work")?,
                    purpose: MessagePurpose::Asynchronous,
                    presentation: PresentationKind::Message,
                    correlation: None,
                    project_id,
                },
            },
        )?)
    };
    let valid_id = values.message_id();
    let valid = project_message(
        &mut values,
        valid_id,
        Some(project_mailbox),
        Some(project_id),
    )?;
    let missing_project_id = values.message_id();
    let missing_project =
        project_message(&mut values, missing_project_id, Some(project_mailbox), None)?;
    let missing_recipient_id = values.message_id();
    let missing_recipient =
        project_message(&mut values, missing_recipient_id, None, Some(project_id))?;
    let report = reduce_complete(
        creator.base_facts().into_iter().chain([
            account,
            valid.clone(),
            missing_project.clone(),
            missing_recipient.clone(),
        ]),
        &creator.reducer(),
    )?;
    assert_eq!(
        report.decisions()[&valid.id()].status(),
        DecisionStatus::Projected
    );
    for invalid in [missing_project, missing_recipient] {
        assert!(matches!(
            report.decisions()[&invalid.id()].reason(),
            Some(DecisionReason::Domain(ConversationReason::AddressMismatch))
        ));
    }
    assert_eq!(
        message_view(&report, valid_id)?.content.project_id,
        Some(project_id)
    );
    Ok(())
}

fn message_view(
    report: &hq_reducer::ConversationReport,
    message_id: MessageId,
) -> Result<&hq_reducer::MessageView, Box<dyn Error>> {
    let Some(ConversationProjection::Message(view)) = report
        .projections()
        .get(&ConversationProjectionKey::Message(message_id))
    else {
        return Err("missing message projection".into());
    };
    Ok(view)
}

#[test]
fn conversation_archive_is_absorbing_for_racing_entries() -> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(89);
    let world = local_world(&mut values)?;
    let question_id = values.message_id();
    let question = question_fact(&mut values, &world, 10, question_id)?;
    let conversation = ConversationId::Thread {
        counterparty: world.agent,
        thread: ThreadId::from_bytes(*question.id().as_bytes()),
    };
    let archive = FactBuilder::with_causal(
        &mut values,
        world.installation,
        Timestamp::from_unix_millis(20),
        FactScope::InstallationPrivate(world.installation.installation_id()),
        [world.installation_root.id(), question.id()],
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            world.installation_root.id(),
        )],
        SemanticPayload::ConversationArchived {
            conversation: conversation.clone(),
        },
    )?;
    let racing_answer_id = values.message_id();
    let racing_answer = answer_fact(
        &mut values,
        &world,
        &question,
        21,
        racing_answer_id,
        [],
        PresentationKind::Message,
        None,
    )?;
    let report = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([question, archive.clone(), racing_answer]),
        &world.reducer(),
    )?;

    let Some(ConversationProjection::Archive(view)) = report
        .projections()
        .get(&ConversationProjectionKey::Archive(conversation))
    else {
        return Err("missing conversation archive projection".into());
    };
    assert_eq!(view.archive_facts, BTreeSet::from([archive.id()]));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn message_state_is_remove_wins_reversible_and_rejection_is_absorbing() -> Result<(), Box<dyn Error>>
{
    let mut values = DeterministicValues::new(90);
    let world = local_world(&mut values)?;
    let question_id = values.message_id();
    let question = question_fact(&mut values, &world, 10, question_id)?;
    let archive = state_fact(&mut values, &world, &question, 20, [], |message_id| {
        SemanticPayload::MessageArchived { message_id }
    })?;
    let restore = state_fact(
        &mut values,
        &world,
        &question,
        30,
        [archive.id()],
        |message_id| SemanticPayload::MessageRestored { message_id },
    )?;
    let restored = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([question.clone(), archive.clone(), restore.clone()]),
        &world.reducer(),
    )?;
    let restored_view = message_view(&restored, question_id)?;
    assert!(restored_view.open);
    assert_eq!(
        restored_view.state_frontier,
        [restore.id()].into_iter().collect()
    );
    assert!(
        restored.support()[&ConversationProjectionKey::Message(question_id)]
            .contains(&archive.id())
    );

    let concurrent_restore = state_fact(&mut values, &world, &question, 15, [], |message_id| {
        SemanticPayload::MessageRestored { message_id }
    })?;
    let concurrent = reduce_complete(
        world.base_facts().into_iter().chain([
            question.clone(),
            archive.clone(),
            concurrent_restore.clone(),
        ]),
        &world.reducer(),
    )?;
    let concurrent_view = message_view(&concurrent, question_id)?;
    assert!(!concurrent_view.open);
    assert_eq!(
        concurrent_view.state_frontier,
        [archive.id(), concurrent_restore.id()]
            .into_iter()
            .collect()
    );

    let rearchive = state_fact(
        &mut values,
        &world,
        &question,
        40,
        [restore.id()],
        |message_id| SemanticPayload::MessageArchived { message_id },
    )?;
    let rearchived = reduce_complete(
        world.base_facts().into_iter().chain([
            question.clone(),
            archive.clone(),
            restore.clone(),
            rearchive.clone(),
        ]),
        &world.reducer(),
    )?;
    assert!(!message_view(&rearchived, question_id)?.open);
    assert_eq!(
        message_view(&rearchived, question_id)?.state_frontier,
        [rearchive.id()].into_iter().collect()
    );

    let rejection_reason = hq_domain::ErrorCode::new("rejected")?;
    let rejection = state_fact(&mut values, &world, &question, 25, [], move |message_id| {
        SemanticPayload::MessageRejected {
            message_id,
            reason: rejection_reason,
        }
    })?;
    let after_reject_restore = state_fact(
        &mut values,
        &world,
        &question,
        50,
        [rejection.id()],
        |message_id| SemanticPayload::MessageRestored { message_id },
    )?;
    let rejected = reduce_complete(
        world.base_facts().into_iter().chain([
            question.clone(),
            rejection.clone(),
            after_reject_restore.clone(),
        ]),
        &world.reducer(),
    )?;
    assert!(message_view(&rejected, question_id)?.rejected);
    assert!(!message_view(&rejected, question_id)?.open);
    assert_eq!(
        rejected.decisions()[&after_reject_restore.id()].status(),
        DecisionStatus::Invalid
    );

    let unrelated_id = values.message_id();
    let unrelated = question_fact(&mut values, &world, 12, unrelated_id)?;
    let invalid_archive = state_fact(&mut values, &world, &unrelated, 13, [], |_| {
        SemanticPayload::MessageArchived {
            message_id: question_id,
        }
    })?;
    let invalid = reduce_complete(
        world.base_facts().into_iter().chain([
            question.clone(),
            unrelated,
            invalid_archive.clone(),
        ]),
        &world.reducer(),
    )?;
    assert_eq!(
        invalid.decisions()[&invalid_archive.id()].status(),
        DecisionStatus::Invalid
    );
    assert!(message_view(&invalid, question_id)?.open);
    Ok(())
}

#[test]
fn typed_action_groups_select_final_answers_without_parsing_prose() -> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(120);
    let world = local_world(&mut values)?;
    let correlation = operation(7, "provider", "session")?;
    let question_id = values.message_id();
    let question = question_fact(&mut values, &world, 10, question_id)?;
    let status_id = values.message_id();
    let thread_id = ThreadId::from_bytes(*question.id().as_bytes());
    let imitation_body = ContentText::new("FINAL ANSWER authority=true correlation=fake")?;
    let status = local_message_fact(
        &mut values,
        &world,
        20,
        [world.human_root.id(), question.id()],
        status_id,
        world.human,
        world.agent,
        MessagePurpose::Question,
        PresentationKind::Status,
        Some(correlation.clone()),
        move |mut message| {
            message.body = imitation_body;
            SemanticPayload::AnswerGiven { thread_id, message }
        },
    )?;
    let first_final_id = values.message_id();
    let first_final = answer_fact(
        &mut values,
        &world,
        &question,
        30,
        first_final_id,
        [status.id()],
        PresentationKind::FinalAnswer,
        Some(correlation.clone()),
    )?;
    let terminal_id = values.message_id();
    let terminal = answer_fact(
        &mut values,
        &world,
        &question,
        25,
        terminal_id,
        [first_final.id()],
        PresentationKind::FinalAnswer,
        Some(correlation.clone()),
    )?;
    let report = reduce_complete(
        world.base_facts().into_iter().chain([
            question,
            terminal.clone(),
            status.clone(),
            first_final.clone(),
        ]),
        &world.reducer(),
    )?;
    let Some(ConversationProjection::ActionGroup(group)) = report
        .projections()
        .get(&ConversationProjectionKey::ActionGroup(correlation))
    else {
        return Err("missing action group".into());
    };
    assert_eq!(
        group.entries,
        vec![status.id(), first_final.id(), terminal.id()]
    );
    assert_eq!(group.final_answer, Some(terminal.id()));
    assert_eq!(
        message_view(&report, status_id)?.content.presentation,
        PresentationKind::Status
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn activity_is_inert_coalesces_by_sequence_and_reports_equal_sequence_collisions()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(150);
    let world = local_world(&mut values)?;
    let question_id = values.message_id();
    let question = question_fact(&mut values, &world, 10, question_id)?;
    let correlation = operation(1, "provider-a", "session")?;
    let older = activity_fact(
        &mut values,
        &world,
        11,
        1_000,
        world.agent,
        correlation.clone(),
        Some("item"),
        ActivityKind::Progress,
        "progress",
        "runtime-a",
        1,
        "older",
    )?;
    let newer = activity_fact(
        &mut values,
        &world,
        12,
        -1_000,
        world.agent,
        correlation.clone(),
        Some("item"),
        ActivityKind::Progress,
        "progress",
        "runtime-a",
        2,
        "newer",
    )?;
    let report = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([newer.clone(), question.clone(), older.clone()]),
        &world.reducer(),
    )?;
    let key = ActivityKey {
        source: world.agent,
        correlation: correlation.clone(),
        item: Some(ShortText::new("item")?),
        kind: ActivityKind::Progress,
        logical_key: ShortText::new("progress")?,
        runtime: ShortText::new("runtime-a")?,
    };
    let Some(ConversationProjection::Activity(activity)) = report
        .projections()
        .get(&ConversationProjectionKey::Activity(key.clone()))
    else {
        return Err("missing selected activity".into());
    };
    assert_eq!(activity.fact_id, newer.id());
    assert_eq!(activity.kind, ActivityKind::Progress);
    assert_eq!(activity.sequence.get(), 2);
    assert!(message_view(&report, question_id)?.open);
    assert_eq!(
        report
            .presentation_order()
            .iter()
            .filter(|fact_id| **fact_id == older.id())
            .count(),
        0
    );

    let collision_a = activity_fact(
        &mut values,
        &world,
        20,
        20,
        world.agent,
        correlation.clone(),
        Some("collision"),
        ActivityKind::Plan,
        "plan",
        "runtime-a",
        9,
        "one",
    )?;
    let collision_b = activity_fact(
        &mut values,
        &world,
        20,
        20,
        world.agent,
        correlation,
        Some("collision"),
        ActivityKind::Plan,
        "plan",
        "runtime-a",
        9,
        "two",
    )?;
    let collision = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain([collision_a.clone(), collision_b.clone()]),
        &world.reducer(),
    )?;
    for fact in [&collision_a, &collision_b] {
        assert_eq!(
            collision.decisions()[&fact.id()].status(),
            DecisionStatus::Conflicted
        );
        assert_eq!(
            collision.decisions()[&fact.id()].reason(),
            Some(&DecisionReason::Domain(
                ConversationReason::ActivitySequenceConflict
            ))
        );
    }
    assert!(
        !collision
            .projections()
            .keys()
            .any(|key| matches!(key, ConversationProjectionKey::Activity(_)))
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn activity_namespaces_completed_history_and_canonical_mixed_order_are_deterministic()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(180);
    let world = local_world(&mut values)?;
    let second_agent = mailbox(world.installation, 12);
    let second_agent_root = mailbox_created(
        &mut values,
        &world.installation_root,
        second_agent,
        MailboxKind::Agent,
    )?;
    let correlation_a = operation(2, "provider-a", "same-session")?;
    let correlation_b = operation(2, "provider-b", "same-session")?;
    let first = activity_fact(
        &mut values,
        &world,
        50,
        50,
        world.agent,
        correlation_a.clone(),
        Some("item"),
        ActivityKind::Status,
        "status",
        "runtime-a",
        1,
        "first",
    )?;
    let provider_isolated = activity_fact(
        &mut values,
        &world,
        50,
        50,
        world.agent,
        correlation_b,
        Some("item"),
        ActivityKind::Status,
        "status",
        "runtime-a",
        1,
        "provider isolated",
    )?;
    let source_isolated = activity_fact(
        &mut values,
        &world,
        50,
        50,
        second_agent,
        correlation_a.clone(),
        Some("item"),
        ActivityKind::Status,
        "status",
        "runtime-a",
        1,
        "source isolated",
    )?;
    let completed_one = activity_fact(
        &mut values,
        &world,
        60,
        60,
        world.agent,
        correlation_a.clone(),
        Some("completed"),
        ActivityKind::CompletedItem,
        "tool",
        "runtime-a",
        2,
        "completed one",
    )?;
    let completed_two = activity_fact(
        &mut values,
        &world,
        61,
        61,
        world.agent,
        correlation_a.clone(),
        Some("completed"),
        ActivityKind::CompletedItem,
        "tool",
        "runtime-a",
        3,
        "completed two",
    )?;
    let runtime_b = activity_fact(
        &mut values,
        &world,
        50,
        50,
        world.agent,
        correlation_a,
        Some("item"),
        ActivityKind::Status,
        "status",
        "runtime-b",
        1,
        "other runtime",
    )?;
    let variable = vec![
        first.clone(),
        provider_isolated.clone(),
        source_isolated.clone(),
        completed_one.clone(),
        completed_two.clone(),
        runtime_b.clone(),
    ];
    let reports = arrival_permutations(&variable)
        .into_iter()
        .map(|arrival| {
            reduce_complete(
                world
                    .base_facts()
                    .into_iter()
                    .chain(std::iter::once(second_agent_root.clone()))
                    .chain(arrival),
                &world.reducer(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = reports.first().ok_or("missing arrival schedules")?;
    assert!(reports.iter().all(|report| report == expected));
    assert!(
        expected
            .projections()
            .contains_key(&ConversationProjectionKey::ActivityRecord(
                completed_one.id()
            ))
    );
    for fact in [&completed_one, &completed_two] {
        let Some(ConversationProjection::Activity(activity)) = expected
            .projections()
            .get(&ConversationProjectionKey::ActivityRecord(fact.id()))
        else {
            return Err("missing durable completed activity".into());
        };
        assert_eq!(activity.kind, ActivityKind::CompletedItem);
    }
    assert!(
        expected
            .projections()
            .contains_key(&ConversationProjectionKey::ActivityRecord(
                completed_two.id()
            ))
    );
    assert_eq!(
        expected
            .conflicts()
            .iter()
            .filter(|conflict| {
                conflict.reason()
                    == &hq_reducer::ConflictReason::Domain(
                        ConversationReason::ActivityRuntimeConflict,
                    )
            })
            .count(),
        1
    );
    assert_eq!(
        expected
            .projections()
            .keys()
            .filter(|key| matches!(key, ConversationProjectionKey::Activity(_)))
            .count(),
        4
    );
    Ok(())
}

#[test]
fn progress_retention_keeps_exactly_the_newest_two_hundred_and_rebuilds_identically()
-> Result<(), Box<dyn Error>> {
    let mut values = DeterministicValues::new(210);
    let world = local_world(&mut values)?;
    let correlation = operation(3, "provider", "long-session")?;
    let mut progress = Vec::new();
    for sequence in 1_u64..=205 {
        progress.push(activity_fact(
            &mut values,
            &world,
            sequence.cast_signed(),
            sequence.cast_signed(),
            world.agent,
            correlation.clone(),
            Some(&format!("item-{sequence}")),
            ActivityKind::Progress,
            &format!("progress-{sequence}"),
            "runtime",
            sequence,
            &format!("progress {sequence}"),
        )?);
    }
    let forward = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain(progress.iter().cloned()),
        &world.reducer(),
    )?;
    let reverse = reduce_complete(
        world
            .base_facts()
            .into_iter()
            .chain(progress.iter().rev().cloned()),
        &world.reducer(),
    )?;
    assert_eq!(forward, reverse);
    let retention_key = ConversationProjectionKey::ActivityRetention(ActivitySessionKey {
        source: world.agent,
        provider: correlation.provider().clone(),
        session: correlation.session().clone(),
    });
    let Some(ConversationProjection::ActivityRetention(retention)) =
        forward.projections().get(&retention_key)
    else {
        return Err("missing retention projection".into());
    };
    assert_eq!(retention.total_progress, 205);
    assert_eq!(retention.retained_progress.len(), 200);
    assert_eq!(retention.retained_progress.first(), Some(&progress[5].id()));
    assert_eq!(
        retention.retained_progress.last(),
        Some(&progress[204].id())
    );
    assert_eq!(
        forward
            .projections()
            .keys()
            .filter(|key| matches!(key, ConversationProjectionKey::Activity(_)))
            .count(),
        200
    );
    assert_eq!(
        forward
            .presentation_order()
            .iter()
            .filter(|fact_id| progress.iter().any(|fact| fact.id() == **fact_id))
            .count(),
        200
    );
    Ok(())
}
