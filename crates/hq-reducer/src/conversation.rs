use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{
    AccountId, ActivityKind, ActivityStatus, ConversationId, Fact, FactId, FactScope,
    MailboxAddress, MessageContent, MessageId, MessagePurpose, OperationCorrelation,
    PresentationKind, ProjectActivityAttribution, SemanticPayload, ShortText, ThreadId, Timestamp,
};

use crate::{
    AuthorityPolicy, AuthorityReason, ConflictObservation, ConflictReason, DomainDecision,
    DomainReducer, DomainReductionReport, PresentationEntry, PresentationFamily,
    PresentationItemId, PresentationKey, PresentationPublicId, ProjectionContribution,
    ReductionContext, canonical_presentation_order,
};

/// Complete report produced by [`ConversationReducer`].
pub type ConversationReport = DomainReductionReport<ConversationReducer>;

/// Compatibility name for the domain-owned conversation identity.
pub type ConversationKey = ConversationId;

/// Derives exact conversation-local order with the one canonical presentation comparator.
pub fn conversation_orders(
    report: &ConversationReport,
    policy: AuthorityPolicy,
) -> Result<BTreeMap<ConversationKey, Vec<FactId>>, crate::PresentationError> {
    let mut entries = BTreeMap::<ConversationKey, Vec<PresentationEntry>>::new();
    for (key, projection) in report.projections() {
        match (key, projection) {
            (ConversationProjectionKey::Message(_), ConversationProjection::Message(message)) => {
                let Some(conversation) = message_conversation_key(message, policy) else {
                    continue;
                };
                if let Some(fact) = report.facts().get(message.fact_id)
                    && let Some(entry) = presentation_entry(fact)
                {
                    entries.entry(conversation).or_default().push(entry);
                }
            }
            (
                ConversationProjectionKey::Activity(_)
                | ConversationProjectionKey::ActivityRecord(_),
                ConversationProjection::Activity(activity),
            ) => {
                let Some(fact) = report.facts().get(activity.fact_id) else {
                    continue;
                };
                let SemanticPayload::HarnessActivityRecorded {
                    project,
                    source,
                    correlation,
                    ..
                } = fact.payload()
                else {
                    continue;
                };
                if let Some(entry) = presentation_entry(fact) {
                    let key = project.as_ref().map_or_else(
                        || ConversationKey::ProviderSession {
                            counterparty: *source,
                            provider: correlation.provider().clone(),
                            session: correlation.session().clone(),
                        },
                        |project| ConversationKey::ProjectThread {
                            project_id: project.project_id,
                            thread: project.thread_id,
                        },
                    );
                    entries.entry(key).or_default().push(entry);
                }
            }
            _ => {}
        }
    }
    entries
        .into_iter()
        .map(|(key, selected)| {
            canonical_presentation_order(report.graph(), selected).map(|order| (key, order))
        })
        .collect()
}

fn message_conversation_key(
    message: &MessageView,
    policy: AuthorityPolicy,
) -> Option<ConversationKey> {
    if let Some(project_id) = message.content.project_id {
        return Some(ConversationKey::ProjectThread {
            project_id,
            thread: message.thread_id,
        });
    }
    let local = MailboxAddress::new(policy.local_installation(), policy.local_human_mailbox());
    let counterparty = if message.content.sender == local {
        message.content.recipient?
    } else if message.content.recipient == Some(local) || message.content.recipient.is_none() {
        message.content.sender
    } else {
        return None;
    };
    Some(match &message.content.correlation {
        Some(correlation) => ConversationKey::ProviderSession {
            counterparty,
            provider: correlation.provider().clone(),
            session: correlation.session().clone(),
        },
        None => ConversationKey::Thread {
            counterparty,
            thread: message.thread_id,
        },
    })
}

/// Addressed content visible for diagnosis while causal history is incomplete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncompleteMessageObservation {
    /// Canonical fact identity.
    pub fact_id: FactId,
    /// Typed content; never parsed for behavior.
    pub content: MessageContent,
    /// Required causal identities that are absent.
    pub missing_dependencies: BTreeSet<FactId>,
    /// Present causal identities that are currently unusable.
    pub unusable_dependencies: BTreeSet<FactId>,
}

/// Derives inert addressed observations without admitting them to semantic projections.
pub fn incomplete_addressed_observations(
    report: &ConversationReport,
) -> Vec<IncompleteMessageObservation> {
    report
        .facts()
        .facts()
        .filter_map(|fact| {
            let decision = report.decisions().get(&fact.id())?;
            (decision.status() == crate::DecisionStatus::Unresolved
                && matches!(
                    fact.scope(),
                    FactScope::PeerAddressed(_) | FactScope::AccountAddressed(_)
                ))
            .then_some(())?;
            let content = message_content(fact)?.clone();
            Some(IncompleteMessageObservation {
                fact_id: fact.id(),
                content,
                missing_dependencies: decision.missing_dependencies().clone(),
                unusable_dependencies: decision.unusable_dependencies().keys().copied().collect(),
            })
        })
        .collect()
}

/// Typed aggregate identities owned by conversation and activity reduction.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConversationAggregateKey {
    /// Every fact carrying one stable public message identity.
    MessageIdentity(MessageId),
    /// Question root, answers, and cancellations for one causal thread.
    Thread(ThreadId),
    /// Archive, restore, and rejection history for one message.
    MessageState(MessageId),
    /// Permanent lifecycle state for one exact conversation.
    ConversationState(ConversationId),
    /// Snapshot/history facts for one full activity writer namespace.
    Activity(ActivityKey),
}

/// Full activity namespace used for deterministic coalescing.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ActivityKey {
    /// Exact source mailbox, including its installation.
    pub source: MailboxAddress,
    /// Provider/session/operation correlation.
    pub correlation: OperationCorrelation,
    /// Optional operation-scoped item identity.
    pub item: Option<ShortText>,
    /// Activity family.
    pub kind: ActivityKind,
    /// Harness-neutral logical key.
    pub logical_key: ShortText,
    /// Runtime-lifetime namespace.
    pub runtime: ShortText,
}

/// Activity retention namespace, intentionally independent of operation and runtime lifetimes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ActivitySessionKey {
    /// Exact source mailbox.
    pub source: MailboxAddress,
    /// Provider namespace.
    pub provider: hq_domain::ProviderId,
    /// Provider-scoped durable session.
    pub session: hq_domain::ProviderSessionId,
}

/// Public projection identities produced by conversation reduction.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConversationProjectionKey {
    /// One question or asynchronous causal thread.
    Thread(ThreadId),
    /// One unambiguous stable public message.
    Message(MessageId),
    /// Permanent lifecycle state for one exact conversation.
    Archive(ConversationId),
    /// One provider operation action group.
    ActionGroup(OperationCorrelation),
    /// One selected activity writer key.
    Activity(ActivityKey),
    /// One durable completed activity fact.
    ActivityRecord(FactId),
    /// Deterministically retained progress facts for one source/provider session.
    ActivityRetention(ActivitySessionKey),
}

/// Exact causal relation between independent thread facts.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CausalRelation {
    /// The answer is an ancestor of the cancellation.
    Before,
    /// The cancellation is an ancestor of the answer.
    After,
    /// Neither usable fact reaches the other.
    Concurrent,
}

/// Normalized question/answer/cancellation state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadView {
    /// Canonical root fact.
    pub root_fact: FactId,
    /// Stable root message identity.
    pub root_message: MessageId,
    /// Every valid answer fact.
    pub answers: BTreeSet<FactId>,
    /// Every valid cancellation fact.
    pub cancellations: BTreeSet<FactId>,
    /// Relation for every answer/cancellation pair.
    pub relations: BTreeMap<(FactId, FactId), CausalRelation>,
    /// Answers in canonical ready order.
    pub ready_answers: Vec<FactId>,
    /// Whether at least one cancellation exists.
    pub cancelled: bool,
}

/// Normalized durable message and its reversible local state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageView {
    /// Canonical message-bearing fact.
    pub fact_id: FactId,
    /// Signed semantic authoring time of the message fact.
    pub authored_at: Timestamp,
    /// Account scope for account-addressed messages.
    pub account_id: Option<AccountId>,
    /// Derived causal thread identity.
    pub thread_id: ThreadId,
    /// Typed immutable content.
    pub content: MessageContent,
    /// Whether the message remains in open/action views.
    pub open: bool,
    /// Whether an absorbing rejection exists.
    pub rejected: bool,
    /// Exact causal-maximal state facts.
    pub state_frontier: BTreeSet<FactId>,
    /// Peer-authored usable children proving receipt.
    pub peer_received_by: BTreeSet<FactId>,
}

/// Permanent archive evidence for one complete conversation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationArchiveView {
    /// Every usable archive fact naming the exact conversation.
    pub archive_facts: BTreeSet<FactId>,
}

/// Typed provider-operation grouping and final-answer selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionGroupView {
    /// Every correlated message in canonical presentation order.
    pub entries: Vec<FactId>,
    /// Canonically last typed final-answer candidate.
    pub final_answer: Option<FactId>,
}

/// One selected or durable activity record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityView {
    /// Selected fact for a snapshot, or the durable completed item.
    pub fact_id: FactId,
    /// Exact source mailbox participating in provider correlation.
    pub source: MailboxAddress,
    /// Exact provider, session, and operation identity.
    pub correlation: OperationCorrelation,
    /// Optional bounded provider item identity.
    pub item: Option<ShortText>,
    /// Closed activity family retained independently from display content.
    pub kind: ActivityKind,
    /// Positive writer sequence.
    pub sequence: std::num::NonZeroU64,
    /// Stable coalescing/history key within the operation.
    pub logical_key: ShortText,
    /// Bounded runtime identity.
    pub runtime: ShortText,
    /// Provider event occurrence time.
    pub occurred_at: Timestamp,
    /// Typed activity state.
    pub status: ActivityStatus,
    /// Bounded display content.
    pub content: hq_domain::ContentText,
    /// Whether authoring truncated content.
    pub truncated: bool,
    /// Structured completed-item presentation when this is a durable completed item.
    pub completed: Option<hq_domain::CompletedItemPresentation>,
}

/// Deterministic disposable progress budget over permanent canonical activity facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityRetentionView {
    /// Selected progress winners retained in canonical presentation order.
    pub retained_progress: Vec<FactId>,
    /// Total selected progress winners before applying the budget.
    pub total_progress: usize,
}

/// Public values produced by conversation and activity reduction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationProjection {
    /// Thread state.
    Thread(Box<ThreadView>),
    /// Message state.
    Message(Box<MessageView>),
    /// Permanent whole-conversation archive state.
    Archive(ConversationArchiveView),
    /// Provider operation group.
    ActionGroup(ActionGroupView),
    /// Selected activity value.
    Activity(Box<ActivityView>),
    /// Progress-retention summary.
    ActivityRetention(ActivityRetentionView),
}

/// Closed conversation and activity rejection/conflict reasons.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConversationReason {
    /// Historical authority policy rejected the fact.
    Authority(AuthorityReason),
    /// Message sender, author, scope, recipient, or typed purpose disagree.
    AddressMismatch,
    /// A child names or cites the wrong root/thread family.
    ThreadMismatch,
    /// A state action does not causally descend from its target.
    TargetNotAncestor,
    /// A restore attempts to reopen a causally prior rejection.
    RejectedMessage,
    /// An archive does not descend from any fact in the named conversation.
    ConversationMismatch,
    /// Unequal canonical facts reuse one stable public message identity.
    MessageIdentityConflict,
    /// Activity source does not equal the full author installation/mailbox identity.
    ActivitySourceMismatch,
    /// Equal activity writer key and sequence carry unequal semantic values.
    ActivitySequenceConflict,
    /// Concurrent activity facts reuse one logical key across runtime lifetimes.
    ActivityRuntimeConflict,
}

/// Pure complete-batch conversation/activity policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConversationReducer {
    authority: AuthorityPolicy,
}

impl ConversationReducer {
    /// Creates the reducer from explicit installation-local authority policy.
    pub const fn new(authority: AuthorityPolicy) -> Self {
        Self { authority }
    }
}

impl DomainReducer for ConversationReducer {
    type AggregateKey = ConversationAggregateKey;
    type ProjectionKey = ConversationProjectionKey;
    type ProjectionValue = ConversationProjection;
    type Reason = ConversationReason;

    fn classify(
        &self,
        fact: &Fact,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        let authority = crate::authority::classify_fact(self.authority, fact, context);
        if !matches!(authority, DomainDecision::Projected) {
            return map_authority_decision(authority);
        }
        classify_conversation(fact, context, self.authority)
    }

    fn aggregate_keys(
        &self,
        fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<Self::AggregateKey> {
        aggregate_keys(fact)
    }

    fn projections(
        &self,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ProjectionContribution<Self::ProjectionKey, Self::ProjectionValue>> {
        derive_projections(context)
    }

    fn conflicts(
        &self,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ConflictObservation<Self::Reason>> {
        let mut conflicts = message_conflicts(context);
        conflicts.extend(activity_runtime_conflicts(context));
        conflicts
    }

    fn presentation_entries(
        &self,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<PresentationEntry> {
        let mut entries = context
            .facts()
            .facts()
            .filter(|fact| context.is_projected(fact.id()))
            .filter(|fact| message_content(fact).is_some())
            .filter_map(presentation_entry)
            .collect::<Vec<_>>();
        entries.extend(
            selected_activity(context)
                .into_iter()
                .filter_map(|(_, fact_id)| context.facts().get(fact_id))
                .filter_map(presentation_entry),
        );
        entries
    }
}

fn map_authority_decision(
    decision: DomainDecision<AuthorityReason>,
) -> DomainDecision<ConversationReason> {
    match decision {
        DomainDecision::Projected => DomainDecision::Projected,
        DomainDecision::Unauthorized {
            reason,
            failed_authorities,
        } => DomainDecision::Unauthorized {
            reason: ConversationReason::Authority(reason),
            failed_authorities,
        },
        DomainDecision::Conflicted {
            reason,
            participants,
        } => DomainDecision::Conflicted {
            reason: ConversationReason::Authority(reason),
            participants,
        },
        DomainDecision::Invalid { reason } => DomainDecision::Invalid {
            reason: ConversationReason::Authority(reason),
        },
        DomainDecision::Unsupported { reason } => DomainDecision::Unsupported {
            reason: ConversationReason::Authority(reason),
        },
    }
}

fn classify_conversation(
    fact: &Fact,
    context: &ReductionContext<'_, ConversationReason>,
    authority: AuthorityPolicy,
) -> DomainDecision<ConversationReason> {
    let result = match fact.payload() {
        SemanticPayload::QuestionAsked(message) => {
            validate_message_purpose(message, MessagePurpose::Question)
                .and_then(|()| validate_message_address(fact, message))
                .and_then(|()| validate_message_identity(message.message_id, context))
        }
        SemanticPayload::AsynchronousMessageSent { thread_id, message } => {
            validate_message_purpose(message, MessagePurpose::Asynchronous)
                .and_then(|()| validate_message_address(fact, message))
                .and_then(|()| {
                    thread_id.map_or(Ok(()), |thread_id| {
                        validate_asynchronous_continuation(fact, thread_id, message, context)
                    })
                })
                .and_then(|()| validate_message_identity(message.message_id, context))
        }
        SemanticPayload::ProjectOutputRecorded {
            project_id,
            dispatch_id,
            binding,
            thread_id,
            message,
            ..
        } => validate_message_purpose(message, MessagePurpose::ProjectOutput)
            .and_then(|()| validate_message_address(fact, message))
            .and_then(|()| {
                validate_project_runtime_parent(
                    fact,
                    *project_id,
                    *dispatch_id,
                    binding,
                    *thread_id,
                    message.sender,
                    context,
                )
            })
            .and_then(|()| validate_message_identity(message.message_id, context)),
        SemanticPayload::AnswerGiven { thread_id, message } => {
            validate_message_purpose(message, MessagePurpose::Question)
                .and_then(|()| validate_message_address(fact, message))
                .and_then(|()| validate_answer(fact, *thread_id, message, context))
                .and_then(|()| validate_message_identity(message.message_id, context))
        }
        SemanticPayload::ThreadCancelled { thread_id, .. } => {
            validate_cancellation(fact, *thread_id, context)
        }
        SemanticPayload::MessageArchived { message_id }
        | SemanticPayload::MessageRestored { message_id }
        | SemanticPayload::MessageRejected { message_id, .. } => {
            validate_message_state(fact, *message_id, context)
        }
        SemanticPayload::ConversationArchived { conversation } => {
            validate_conversation_archive(fact, conversation, context, authority)
        }
        SemanticPayload::HarnessActivityRecorded {
            project,
            source,
            correlation,
            ..
        } => validate_activity_source(fact, *source, context)
            .and_then(|()| {
                project.as_ref().map_or(Ok(()), |project| {
                    validate_project_activity(fact, project, *source, correlation, context)
                })
            })
            .and_then(|()| validate_activity_sequence(fact, context)),
        _ => Ok(()),
    };
    match result {
        Ok(()) => DomainDecision::Projected,
        Err(reason @ ConversationReason::MessageIdentityConflict) => {
            let participants = message_content(fact)
                .map(|message| message_identity_participants(message.message_id, context))
                .or_else(|| {
                    state_target(fact)
                        .map(|message_id| message_identity_participants(message_id, context))
                })
                .unwrap_or_default();
            DomainDecision::Conflicted {
                reason,
                participants,
            }
        }
        Err(reason @ ConversationReason::ActivitySequenceConflict) => {
            let participants = activity_sequence_participants(fact, context);
            DomainDecision::Conflicted {
                reason,
                participants,
            }
        }
        Err(reason) => DomainDecision::Invalid { reason },
    }
}

fn validate_message_identity(
    message_id: MessageId,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    (message_identity_participants(message_id, context).len() == 1)
        .then_some(())
        .ok_or(ConversationReason::MessageIdentityConflict)
}

fn validate_activity_source(
    fact: &Fact,
    source: MailboxAddress,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    let valid_mailbox = context.facts().facts().any(|candidate| {
        context.is_projected(candidate.id())
            && candidate.author().installation_id() == source.installation_id()
            && matches!(
                candidate.payload(),
                SemanticPayload::MailboxCreated {
                    mailbox_id,
                    kind: hq_domain::MailboxKind::Agent,
                    ..
                } if *mailbox_id == source.mailbox_id()
            )
    });
    (source.installation_id() == fact.author().installation_id() && valid_mailbox)
        .then_some(())
        .ok_or(ConversationReason::ActivitySourceMismatch)
}

fn validate_activity_sequence(
    fact: &Fact,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    (activity_sequence_participants(fact, context).len() <= 1)
        .then_some(())
        .ok_or(ConversationReason::ActivitySequenceConflict)
}

fn validate_project_activity(
    fact: &Fact,
    project: &ProjectActivityAttribution,
    source: MailboxAddress,
    correlation: &OperationCorrelation,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    if project.binding.provider != *correlation.provider()
        || project.binding.session != *correlation.session()
    {
        return Err(ConversationReason::ActivitySourceMismatch);
    }
    validate_project_runtime_parent(
        fact,
        project.project_id,
        project.dispatch_id,
        &project.binding,
        project.thread_id,
        source,
        context,
    )
}

fn validate_project_runtime_parent(
    fact: &Fact,
    project_id: hq_domain::ProjectId,
    dispatch_id: hq_domain::DispatchId,
    binding: &hq_domain::AssignmentBinding,
    thread_id: ThreadId,
    source: MailboxAddress,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    let exact_dispatch_parent = fact.causal().parents().iter().any(|parent| {
        context.facts().get(*parent).is_some_and(|candidate| {
            context.is_projected(candidate.id())
                && matches!(
                    candidate.payload(),
                    SemanticPayload::ProjectInputDispatched {
                        project_id: candidate_project,
                        dispatch_id: candidate_dispatch,
                        binding: candidate_binding,
                        thread_id: candidate_thread,
                        ..
                    } if *candidate_project == project_id
                        && *candidate_dispatch == dispatch_id
                        && candidate_binding == binding
                        && *candidate_thread == thread_id
                )
        })
    });
    let source_claimed_for_agent = context.facts().facts().any(|candidate| {
        context.is_projected(candidate.id())
            && candidate.author().installation_id() == source.installation_id()
            && matches!(
                candidate.payload(),
                SemanticPayload::AgentNameClaimed {
                    agent_id,
                    mailbox_id,
                    ..
                } if *agent_id == binding.agent_id
                    && *mailbox_id == source.mailbox_id()
            )
    });
    (exact_dispatch_parent && source_claimed_for_agent)
        .then_some(())
        .ok_or(ConversationReason::ActivitySourceMismatch)
}

fn activity_sequence_participants(
    fact: &Fact,
    context: &ReductionContext<'_, impl Sized>,
) -> BTreeSet<FactId> {
    let Some(key) = activity_key(fact) else {
        return BTreeSet::new();
    };
    let Some(SemanticPayload::HarnessActivityRecorded { sequence, .. }) =
        context.facts().get(fact.id()).map(Fact::payload)
    else {
        return BTreeSet::new();
    };
    let candidates = context
        .facts()
        .facts()
        .filter(|candidate| activity_key(candidate).as_ref() == Some(&key))
        .filter(|candidate| {
            matches!(candidate.payload(), SemanticPayload::HarnessActivityRecorded { sequence: candidate_sequence, .. } if candidate_sequence == sequence)
        })
        .collect::<Vec<_>>();
    let unequal = candidates
        .iter()
        .any(|candidate| candidate.payload() != fact.payload());
    if unequal {
        candidates.into_iter().map(Fact::id).collect()
    } else {
        BTreeSet::new()
    }
}

fn validate_message_address(
    fact: &Fact,
    message: &MessageContent,
) -> Result<(), ConversationReason> {
    if message.sender.installation_id() != fact.author().installation_id() {
        return Err(ConversationReason::AddressMismatch);
    }
    let valid = match fact.scope() {
        FactScope::InstallationPrivate(installation) => {
            *installation == fact.author().installation_id()
                && message
                    .recipient
                    .is_some_and(|recipient| recipient.installation_id() == *installation)
        }
        FactScope::PeerAddressed(target) => message.recipient == Some(*target),
        FactScope::AccountAddressed(_) => {
            message.recipient.is_some() == message.project_id.is_some()
        }
        FactScope::RemoteControl { .. } => false,
    };
    valid
        .then_some(())
        .ok_or(ConversationReason::AddressMismatch)
}

fn validate_message_purpose(
    message: &MessageContent,
    expected: MessagePurpose,
) -> Result<(), ConversationReason> {
    (message.purpose == expected)
        .then_some(())
        .ok_or(ConversationReason::AddressMismatch)
}

fn validate_answer(
    fact: &Fact,
    thread_id: ThreadId,
    answer: &MessageContent,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    let root = question_root(thread_id, context).ok_or(ConversationReason::ThreadMismatch)?;
    if !fact.causal().parents().contains(&root.id()) {
        return Err(ConversationReason::ThreadMismatch);
    }
    let SemanticPayload::QuestionAsked(question) = root.payload() else {
        return Err(ConversationReason::ThreadMismatch);
    };
    if question.purpose != MessagePurpose::Question
        || answer.purpose != MessagePurpose::Question
        || !same_scope(root.scope(), fact.scope())
    {
        return Err(ConversationReason::ThreadMismatch);
    }
    let reversed = match root.scope() {
        FactScope::AccountAddressed(_) => answer.recipient.is_none(),
        _ => question.recipient == Some(answer.sender) && answer.recipient == Some(question.sender),
    };
    reversed
        .then_some(())
        .ok_or(ConversationReason::AddressMismatch)
}

fn validate_asynchronous_continuation(
    fact: &Fact,
    thread_id: ThreadId,
    message: &MessageContent,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    let root = asynchronous_root(thread_id, context).ok_or(ConversationReason::ThreadMismatch)?;
    let SemanticPayload::AsynchronousMessageSent {
        thread_id: None,
        message: root_message,
    } = root.payload()
    else {
        return Err(ConversationReason::ThreadMismatch);
    };
    (fact.causal().parents().contains(&root.id())
        && same_scope(root.scope(), fact.scope())
        && root_message.purpose == MessagePurpose::Asynchronous
        && root_message.project_id.is_some()
        && message.purpose == MessagePurpose::Asynchronous
        && message.sender == root_message.sender
        && message.recipient == root_message.recipient
        && message.project_id == root_message.project_id)
        .then_some(())
        .ok_or(ConversationReason::ThreadMismatch)
}

fn validate_cancellation(
    fact: &Fact,
    thread_id: ThreadId,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    let root = question_root(thread_id, context).ok_or(ConversationReason::ThreadMismatch)?;
    let SemanticPayload::QuestionAsked(question) = root.payload() else {
        return Err(ConversationReason::ThreadMismatch);
    };
    (fact.causal().parents().contains(&root.id())
        && same_scope(root.scope(), fact.scope())
        && fact.author().installation_id() == question.sender.installation_id())
    .then_some(())
    .ok_or(ConversationReason::ThreadMismatch)
}

fn validate_message_state(
    fact: &Fact,
    message_id: MessageId,
    context: &ReductionContext<'_, ConversationReason>,
) -> Result<(), ConversationReason> {
    let targets = message_identity_participants(message_id, context);
    if targets.len() != 1 {
        return Err(ConversationReason::MessageIdentityConflict);
    }
    let target = *targets
        .iter()
        .next()
        .ok_or(ConversationReason::TargetNotAncestor)?;
    if !context.graph().structurally_reaches(target, fact.id()) || !context.is_projected(target) {
        return Err(ConversationReason::TargetNotAncestor);
    }
    if matches!(fact.payload(), SemanticPayload::MessageRestored { .. })
        && context.facts().facts().any(|candidate| {
            context.is_projected(candidate.id())
                && matches!(candidate.payload(), SemanticPayload::MessageRejected { message_id: candidate_id, .. } if *candidate_id == message_id)
                && context
                    .graph()
                    .structurally_reaches(candidate.id(), fact.id())
        })
    {
        return Err(ConversationReason::RejectedMessage);
    }
    Ok(())
}

fn validate_conversation_archive(
    fact: &Fact,
    conversation: &ConversationId,
    context: &ReductionContext<'_, ConversationReason>,
    authority: AuthorityPolicy,
) -> Result<(), ConversationReason> {
    let local_human = MailboxAddress::new(
        authority.local_installation(),
        authority.local_human_mailbox(),
    );
    context
        .facts()
        .facts()
        .filter(|candidate| context.is_projected(candidate.id()))
        .any(|candidate| {
            fact_conversation_id(candidate, local_human).as_ref() == Some(conversation)
                && context
                    .graph()
                    .structurally_reaches(candidate.id(), fact.id())
                && same_scope(candidate.scope(), fact.scope())
        })
        .then_some(())
        .ok_or(ConversationReason::ConversationMismatch)
}

fn fact_conversation_id(fact: &Fact, local_human: MailboxAddress) -> Option<ConversationId> {
    if let Some(message) = message_content(fact) {
        if let Some(project_id) = message.project_id {
            return Some(ConversationId::ProjectThread {
                project_id,
                thread: thread_id(fact),
            });
        }
        let counterparty = if message.sender == local_human {
            message.recipient?
        } else if message.recipient == Some(local_human) || message.recipient.is_none() {
            message.sender
        } else {
            return None;
        };
        return Some(match &message.correlation {
            Some(correlation) => ConversationId::ProviderSession {
                counterparty,
                provider: correlation.provider().clone(),
                session: correlation.session().clone(),
            },
            None => ConversationId::Thread {
                counterparty,
                thread: thread_id(fact),
            },
        });
    }
    match fact.payload() {
        SemanticPayload::HarnessActivityRecorded {
            project,
            source,
            correlation,
            ..
        } => Some(project.as_ref().map_or_else(
            || ConversationId::ProviderSession {
                counterparty: *source,
                provider: correlation.provider().clone(),
                session: correlation.session().clone(),
            },
            |project| ConversationId::ProjectThread {
                project_id: project.project_id,
                thread: project.thread_id,
            },
        )),
        _ => None,
    }
}

fn same_scope(left: &FactScope, right: &FactScope) -> bool {
    match (left, right) {
        (FactScope::PeerAddressed(_), FactScope::PeerAddressed(_)) => true,
        _ => left == right,
    }
}

fn thread_id(fact: &Fact) -> ThreadId {
    match fact.payload() {
        SemanticPayload::AsynchronousMessageSent {
            thread_id: Some(thread_id),
            ..
        }
        | SemanticPayload::AnswerGiven { thread_id, .. }
        | SemanticPayload::ProjectOutputRecorded { thread_id, .. } => *thread_id,
        _ => ThreadId::from_bytes(*fact.id().as_bytes()),
    }
}

fn question_root<'a>(
    thread: ThreadId,
    context: &ReductionContext<'a, ConversationReason>,
) -> Option<&'a Fact> {
    context.facts().facts().find(|fact| {
        context.is_projected(fact.id())
            && matches!(fact.payload(), SemanticPayload::QuestionAsked(_))
            && thread_id(fact) == thread
    })
}

fn asynchronous_root<'a>(
    thread: ThreadId,
    context: &ReductionContext<'a, ConversationReason>,
) -> Option<&'a Fact> {
    context.facts().facts().find(|fact| {
        context.is_projected(fact.id())
            && matches!(
                fact.payload(),
                SemanticPayload::AsynchronousMessageSent {
                    thread_id: None,
                    ..
                }
            )
            && ThreadId::from_bytes(*fact.id().as_bytes()) == thread
    })
}

fn message_content(fact: &Fact) -> Option<&MessageContent> {
    match fact.payload() {
        SemanticPayload::QuestionAsked(message)
        | SemanticPayload::AsynchronousMessageSent { message, .. }
        | SemanticPayload::AnswerGiven { message, .. }
        | SemanticPayload::ProjectOutputRecorded { message, .. } => Some(message),
        _ => None,
    }
}

fn message_identity_participants(
    message_id: MessageId,
    context: &ReductionContext<'_, impl Sized>,
) -> BTreeSet<FactId> {
    context
        .facts()
        .facts()
        .filter(|fact| {
            message_content(fact).is_some_and(|message| message.message_id == message_id)
        })
        .map(Fact::id)
        .collect()
}

fn state_target(fact: &Fact) -> Option<MessageId> {
    match fact.payload() {
        SemanticPayload::MessageArchived { message_id }
        | SemanticPayload::MessageRestored { message_id }
        | SemanticPayload::MessageRejected { message_id, .. } => Some(*message_id),
        _ => None,
    }
}

fn activity_key(fact: &Fact) -> Option<ActivityKey> {
    match fact.payload() {
        SemanticPayload::HarnessActivityRecorded {
            source,
            correlation,
            item,
            kind,
            logical_key,
            runtime,
            ..
        } => Some(ActivityKey {
            source: *source,
            correlation: correlation.clone(),
            item: item.clone(),
            kind: *kind,
            logical_key: logical_key.clone(),
            runtime: runtime.clone(),
        }),
        _ => None,
    }
}

fn activity_session_key(fact: &Fact) -> Option<ActivitySessionKey> {
    match fact.payload() {
        SemanticPayload::HarnessActivityRecorded {
            source,
            correlation,
            ..
        } => Some(ActivitySessionKey {
            source: *source,
            provider: correlation.provider().clone(),
            session: correlation.session().clone(),
        }),
        _ => None,
    }
}

fn aggregate_keys(fact: &Fact) -> Vec<ConversationAggregateKey> {
    let mut keys = Vec::new();
    if let Some(message) = message_content(fact) {
        keys.push(ConversationAggregateKey::MessageIdentity(
            message.message_id,
        ));
    }
    match fact.payload() {
        SemanticPayload::QuestionAsked(_) | SemanticPayload::AsynchronousMessageSent { .. } => {
            keys.push(ConversationAggregateKey::Thread(thread_id(fact)));
        }
        SemanticPayload::AnswerGiven { thread_id, .. }
        | SemanticPayload::ThreadCancelled { thread_id, .. } => {
            keys.push(ConversationAggregateKey::Thread(*thread_id));
        }
        _ => {}
    }
    if let Some(target) = state_target(fact) {
        keys.push(ConversationAggregateKey::MessageState(target));
    }
    if let SemanticPayload::ConversationArchived { conversation } = fact.payload() {
        keys.push(ConversationAggregateKey::ConversationState(
            conversation.clone(),
        ));
    }
    if let Some(key) = activity_key(fact) {
        keys.push(ConversationAggregateKey::Activity(key));
    }
    keys
}

fn derive_projections(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ProjectionContribution<ConversationProjectionKey, ConversationProjection>> {
    let mut result = Vec::new();
    result.extend(thread_projections(context));
    result.extend(message_projections(context));
    result.extend(conversation_archive_projections(context));
    result.extend(action_group_projections(context));
    result.extend(activity_projections(context));
    result
}

fn conversation_archive_projections(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ProjectionContribution<ConversationProjectionKey, ConversationProjection>> {
    let mut archives = BTreeMap::<ConversationId, BTreeSet<FactId>>::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        if let SemanticPayload::ConversationArchived { conversation } = fact.payload() {
            archives
                .entry(conversation.clone())
                .or_default()
                .insert(fact.id());
        }
    }
    archives
        .into_iter()
        .map(|(conversation, archive_facts)| {
            ProjectionContribution::new(
                ConversationProjectionKey::Archive(conversation),
                ConversationProjection::Archive(ConversationArchiveView {
                    archive_facts: archive_facts.clone(),
                }),
                archive_facts,
            )
        })
        .collect()
}

fn thread_projections(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ProjectionContribution<ConversationProjectionKey, ConversationProjection>> {
    context
        .facts()
        .facts()
        .filter(|fact| {
            context.is_projected(fact.id())
                && matches!(
                    fact.payload(),
                    SemanticPayload::QuestionAsked(_)
                        | SemanticPayload::AsynchronousMessageSent {
                            thread_id: None,
                            ..
                        }
                )
        })
        .filter_map(|root| {
            let root_message = message_content(root)?;
            let id = thread_id(root);
            let answers = context
                .facts()
                .facts()
                .filter(|fact| context.is_projected(fact.id()))
                .filter(|fact| match fact.payload() {
                    SemanticPayload::AnswerGiven { thread_id, .. }
                    | SemanticPayload::AsynchronousMessageSent {
                        thread_id: Some(thread_id),
                        ..
                    } => *thread_id == id,
                    _ => false,
                })
                .map(Fact::id)
                .collect::<BTreeSet<_>>();
            let cancellations = context
                .facts()
                .facts()
                .filter(|fact| context.is_projected(fact.id()))
                .filter(|fact| matches!(fact.payload(), SemanticPayload::ThreadCancelled { thread_id, .. } if *thread_id == id))
                .map(Fact::id)
                .collect::<BTreeSet<_>>();
            let relations = answers
                .iter()
                .flat_map(|answer| {
                    cancellations.iter().map(move |cancel| {
                        let relation = if context.usably_reaches(*answer, *cancel) {
                            CausalRelation::Before
                        } else if context.usably_reaches(*cancel, *answer) {
                            CausalRelation::After
                        } else {
                            CausalRelation::Concurrent
                        };
                        ((*answer, *cancel), relation)
                    })
                })
                .collect();
            let entries = answers
                .iter()
                .filter_map(|fact_id| context.facts().get(*fact_id))
                .filter_map(presentation_entry)
                .collect::<Vec<_>>();
            let ready_answers =
                canonical_presentation_order(context.graph(), entries).unwrap_or_default();
            let support = std::iter::once(root.id())
                .chain(answers.iter().copied())
                .chain(cancellations.iter().copied())
                .collect::<BTreeSet<_>>();
            Some(ProjectionContribution::new(
                ConversationProjectionKey::Thread(id),
                ConversationProjection::Thread(Box::new(ThreadView {
                    root_fact: root.id(),
                    root_message: root_message.message_id,
                    answers,
                    cancellations: cancellations.clone(),
                    relations,
                    ready_answers,
                    cancelled: !cancellations.is_empty(),
                })),
                support,
            ))
        })
        .collect()
}

fn message_projections(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ProjectionContribution<ConversationProjectionKey, ConversationProjection>> {
    context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
        .filter_map(|fact| message_content(fact).map(|message| (fact, message)))
        .filter(|(_, message)| {
            message_identity_participants(message.message_id, context).len() == 1
        })
        .map(|(fact, message)| {
            let states = context
                .facts()
                .facts()
                .filter(|state| {
                    context.is_projected(state.id())
                        && state_target(state) == Some(message.message_id)
                })
                .map(Fact::id)
                .collect::<BTreeSet<_>>();
            let frontier = maximal(&states, context);
            let rejected = states.iter().any(|state| {
                matches!(
                    context.facts().get(*state).map(Fact::payload),
                    Some(SemanticPayload::MessageRejected { .. })
                )
            });
            let archived = frontier.iter().any(|state| {
                matches!(
                    context.facts().get(*state).map(Fact::payload),
                    Some(SemanticPayload::MessageArchived { .. })
                )
            });
            let peer_received_by = context
                .facts()
                .facts()
                .filter(|child| {
                    context.is_projected(child.id())
                        && child.author().installation_id() != fact.author().installation_id()
                        && child.causal().parents().contains(&fact.id())
                })
                .map(Fact::id)
                .collect::<BTreeSet<_>>();
            let support = std::iter::once(fact.id())
                .chain(states.iter().copied())
                .chain(peer_received_by.iter().copied())
                .collect::<BTreeSet<_>>();
            ProjectionContribution::new(
                ConversationProjectionKey::Message(message.message_id),
                ConversationProjection::Message(Box::new(MessageView {
                    fact_id: fact.id(),
                    authored_at: fact.authored_at(),
                    account_id: match fact.scope() {
                        FactScope::AccountAddressed(account_id) => Some(*account_id),
                        FactScope::InstallationPrivate(_)
                        | FactScope::PeerAddressed(_)
                        | FactScope::RemoteControl { .. } => None,
                    },
                    thread_id: thread_id(fact),
                    content: message.clone(),
                    open: !rejected && !archived,
                    rejected,
                    state_frontier: frontier,
                    peer_received_by,
                })),
                support,
            )
        })
        .collect()
}

fn action_group_projections(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ProjectionContribution<ConversationProjectionKey, ConversationProjection>> {
    let mut groups = BTreeMap::<OperationCorrelation, Vec<&Fact>>::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        if let Some(correlation) =
            message_content(fact).and_then(|message| message.correlation.as_ref())
        {
            groups.entry(correlation.clone()).or_default().push(fact);
        }
    }
    groups
        .into_iter()
        .map(|(correlation, facts)| {
            let entries = facts
                .iter()
                .filter_map(|fact| presentation_entry(fact))
                .collect::<Vec<_>>();
            let ordered =
                canonical_presentation_order(context.graph(), entries).unwrap_or_default();
            let final_answer = ordered.iter().rev().copied().find(|fact_id| {
                context.facts().get(*fact_id).is_some_and(|fact| {
                    message_content(fact).is_some_and(|message| {
                        message.presentation == PresentationKind::FinalAnswer
                    })
                })
            });
            ProjectionContribution::new(
                ConversationProjectionKey::ActionGroup(correlation),
                ConversationProjection::ActionGroup(ActionGroupView {
                    entries: ordered.clone(),
                    final_answer,
                }),
                ordered,
            )
        })
        .collect()
}

fn activity_projections(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ProjectionContribution<ConversationProjectionKey, ConversationProjection>> {
    let selected = selected_activity(context);
    let mut result = selected
        .iter()
        .filter_map(|(key, fact_id)| {
            let winner = context.facts().get(*fact_id)?;
            let SemanticPayload::HarnessActivityRecorded {
                source,
                correlation,
                item,
                sequence,
                logical_key,
                runtime,
                occurred_at,
                status,
                content,
                truncated,
                kind,
                completed,
                ..
            } = winner.payload()
            else {
                return None;
            };
            let support: BTreeSet<FactId> = if *kind == ActivityKind::CompletedItem {
                [winner.id()].into_iter().collect()
            } else {
                context
                    .facts()
                    .facts()
                    .filter(|fact| context.is_projected(fact.id()))
                    .filter(|fact| activity_key(fact).as_ref() == Some(key))
                    .map(Fact::id)
                    .collect()
            };
            Some(ProjectionContribution::new(
                if *kind == ActivityKind::CompletedItem {
                    ConversationProjectionKey::ActivityRecord(winner.id())
                } else {
                    ConversationProjectionKey::Activity(key.clone())
                },
                ConversationProjection::Activity(Box::new(ActivityView {
                    fact_id: winner.id(),
                    source: *source,
                    correlation: correlation.clone(),
                    item: item.clone(),
                    kind: *kind,
                    sequence: *sequence,
                    logical_key: logical_key.clone(),
                    runtime: runtime.clone(),
                    occurred_at: *occurred_at,
                    status: status.clone(),
                    content: content.clone(),
                    truncated: *truncated,
                    completed: completed.clone(),
                })),
                support,
            ))
        })
        .collect::<Vec<_>>();
    let mut sessions = BTreeMap::<ActivitySessionKey, Vec<FactId>>::new();
    for (_, fact_id) in activity_winners(context) {
        let Some(fact) = context.facts().get(fact_id) else {
            continue;
        };
        if matches!(
            fact.payload(),
            SemanticPayload::HarnessActivityRecorded {
                kind: ActivityKind::Progress,
                ..
            }
        ) && let Some(session) = activity_session_key(fact)
        {
            sessions.entry(session).or_default().push(fact_id);
        }
    }
    for (session, facts) in sessions {
        let entries = facts
            .iter()
            .filter_map(|fact_id| context.facts().get(*fact_id))
            .filter_map(presentation_entry)
            .collect::<Vec<_>>();
        let ordered = canonical_presentation_order(context.graph(), entries).unwrap_or_default();
        let retained_progress = ordered
            .iter()
            .skip(ordered.len().saturating_sub(200))
            .copied()
            .collect::<Vec<_>>();
        result.push(ProjectionContribution::new(
            ConversationProjectionKey::ActivityRetention(session),
            ConversationProjection::ActivityRetention(ActivityRetentionView {
                retained_progress: retained_progress.clone(),
                total_progress: ordered.len(),
            }),
            retained_progress,
        ));
    }
    result
}

fn activity_winners(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<(ActivityKey, FactId)> {
    let mut groups = BTreeMap::<ActivityKey, Vec<&Fact>>::new();
    let mut completed = Vec::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        let Some(key) = activity_key(fact) else {
            continue;
        };
        if matches!(
            fact.payload(),
            SemanticPayload::HarnessActivityRecorded {
                kind: ActivityKind::CompletedItem,
                ..
            }
        ) {
            completed.push((key, fact.id()));
        } else {
            groups.entry(key).or_default().push(fact);
        }
    }
    groups
        .into_iter()
        .filter_map(|(key, facts)| {
            facts
                .into_iter()
                .max_by_key(|fact| match fact.payload() {
                    SemanticPayload::HarnessActivityRecorded { sequence, .. } => *sequence,
                    _ => std::num::NonZeroU64::MIN,
                })
                .map(|winner| (key, winner.id()))
        })
        .chain(completed)
        .collect()
}

fn selected_activity(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<(ActivityKey, FactId)> {
    let mut selected = activity_winners(context);

    let mut progress_by_session = BTreeMap::<ActivitySessionKey, Vec<usize>>::new();
    for (index, (_, fact_id)) in selected.iter().enumerate() {
        let Some(fact) = context.facts().get(*fact_id) else {
            continue;
        };
        if matches!(
            fact.payload(),
            SemanticPayload::HarnessActivityRecorded {
                kind: ActivityKind::Progress,
                ..
            }
        ) && let Some(session) = activity_session_key(fact)
        {
            progress_by_session.entry(session).or_default().push(index);
        }
    }
    let mut remove = BTreeSet::new();
    for indexes in progress_by_session.into_values() {
        if indexes.len() <= 200 {
            continue;
        }
        let entries = indexes
            .iter()
            .filter_map(|index| context.facts().get(selected[*index].1))
            .filter_map(presentation_entry)
            .collect::<Vec<_>>();
        let retained = canonical_presentation_order(context.graph(), entries)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .take(200)
            .collect::<BTreeSet<_>>();
        for index in indexes {
            if !retained.contains(&selected[index].1) {
                remove.insert(index);
            }
        }
    }
    selected = selected
        .into_iter()
        .enumerate()
        .filter_map(|(index, value)| (!remove.contains(&index)).then_some(value))
        .collect();
    selected
}

fn maximal(
    members: &BTreeSet<FactId>,
    context: &ReductionContext<'_, ConversationReason>,
) -> BTreeSet<FactId> {
    members
        .iter()
        .copied()
        .filter(|candidate| {
            !members
                .iter()
                .copied()
                .any(|other| other != *candidate && context.usably_reaches(*candidate, other))
        })
        .collect()
}

fn message_conflicts(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ConflictObservation<ConversationReason>> {
    let mut messages = BTreeMap::<MessageId, BTreeSet<FactId>>::new();
    for fact in context.facts().facts() {
        if let Some(message) = message_content(fact) {
            messages
                .entry(message.message_id)
                .or_default()
                .insert(fact.id());
        }
    }
    messages
        .into_values()
        .filter(|participants| participants.len() > 1)
        .map(|participants| {
            ConflictObservation::new(
                ConflictReason::Domain(ConversationReason::MessageIdentityConflict),
                participants,
            )
        })
        .collect()
}

fn activity_runtime_conflicts(
    context: &ReductionContext<'_, ConversationReason>,
) -> Vec<ConflictObservation<ConversationReason>> {
    #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
    struct LogicalKey {
        source: MailboxAddress,
        correlation: OperationCorrelation,
        item: Option<ShortText>,
        kind: ActivityKind,
        logical_key: ShortText,
    }
    let mut groups = BTreeMap::<LogicalKey, Vec<&Fact>>::new();
    for fact in context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
    {
        if let Some(key) = activity_key(fact) {
            groups
                .entry(LogicalKey {
                    source: key.source,
                    correlation: key.correlation,
                    item: key.item,
                    kind: key.kind,
                    logical_key: key.logical_key,
                })
                .or_default()
                .push(fact);
        }
    }
    groups
        .into_values()
        .filter_map(|facts| {
            let ids = facts.iter().map(|fact| fact.id()).collect::<BTreeSet<_>>();
            let frontier = maximal(&ids, context);
            let runtimes = frontier
                .iter()
                .filter_map(|fact_id| context.facts().get(*fact_id))
                .filter_map(activity_key)
                .map(|key| key.runtime)
                .collect::<BTreeSet<_>>();
            (runtimes.len() > 1).then(|| {
                ConflictObservation::new(
                    ConflictReason::Domain(ConversationReason::ActivityRuntimeConflict),
                    frontier,
                )
            })
        })
        .collect()
}

fn presentation_entry(fact: &Fact) -> Option<PresentationEntry> {
    if let Some(message) = message_content(fact) {
        let correlation = message.correlation.as_ref();
        return Some(PresentationEntry::new(
            fact.id(),
            PresentationKey::new(
                fact.authored_at(),
                fact.authored_at(),
                PresentationFamily::Message,
                fact.author().installation_id(),
                Some(message.sender.mailbox_id()),
                correlation.map(|value| value.provider().clone()),
                correlation.map(|value| value.session().clone()),
                correlation.map(OperationCorrelation::operation),
                Some(PresentationItemId::Message(message.message_id)),
                None,
                None,
                Some(PresentationPublicId::Message(message.message_id)),
            ),
        ));
    }
    match fact.payload() {
        SemanticPayload::HarnessActivityRecorded {
            source,
            correlation,
            item,
            runtime,
            sequence,
            occurred_at,
            ..
        } => Some(PresentationEntry::new(
            fact.id(),
            PresentationKey::new(
                fact.authored_at(),
                *occurred_at,
                PresentationFamily::Activity,
                fact.author().installation_id(),
                Some(source.mailbox_id()),
                Some(correlation.provider().clone()),
                Some(correlation.session().clone()),
                Some(correlation.operation()),
                Some(
                    item.clone()
                        .map(PresentationItemId::Activity)
                        .unwrap_or(PresentationItemId::Fact(fact.id())),
                ),
                Some(runtime.clone()),
                Some(*sequence),
                None,
            ),
        )),
        _ => None,
    }
}
