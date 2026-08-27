use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{
    AgentId, Fact, FactId, MailboxAddress, MailboxId, MailboxKind, ProviderId, ProviderSessionId,
    RepositoryContext, SemanticPayload, ShortText,
};

use crate::{
    AuthorityPolicy, AuthorityReason, ConflictObservation, ConflictReason, DomainDecision,
    DomainReducer, ProjectionContribution, ReductionContext,
};

/// Provider-scoped durable session identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionIdentity {
    /// Provider namespace.
    pub provider: ProviderId,
    /// Provider-scoped session value.
    pub session: ProviderSessionId,
}

/// Typed aggregate identities for named-agent reduction.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AgentAggregateKey {
    /// Permanent reservation of one name.
    Name(ShortText),
    /// All claims and lifecycle facts for one agent identity.
    Agent(AgentId),
    /// Claims competing for one installation-qualified mailbox.
    Mailbox(MailboxAddress),
    /// Immutable binding of one provider-scoped session.
    Session(SessionIdentity),
    /// Durable session-selection register.
    Selection(AgentId),
    /// Mutable display-name register for one session.
    Rename {
        /// Agent identity.
        agent: AgentId,
        /// Provider-scoped session.
        session: SessionIdentity,
    },
    /// Grow-only repository-context history for one mailbox.
    Context(MailboxAddress),
}

/// Public projection identities for named agents and direct sessions.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AgentProjectionKey {
    /// Permanent name reservation.
    Name(ShortText),
    /// Named-agent lifecycle view.
    Agent(AgentId),
    /// Immutable provider-session binding.
    Session(SessionIdentity),
    /// Repository-context history.
    Context(MailboxAddress),
    /// Durable selected-session register.
    Selection(AgentId),
    /// Independent session display-name register.
    Rename {
        /// Agent identity.
        agent: AgentId,
        /// Provider-scoped session.
        session: SessionIdentity,
    },
    /// Permanent binding history usable without a name claim.
    DirectSession {
        /// Bound mailbox.
        mailbox: MailboxAddress,
        /// Provider-scoped session.
        session: SessionIdentity,
    },
}

/// One normalized name-claim subject.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NameClaimSubject {
    /// Stable agent identity.
    pub agent_id: AgentId,
    /// Installation-qualified agent mailbox.
    pub mailbox: MailboxAddress,
}

/// Permanent name reservation and every claimant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NameReservationView {
    /// Claim facts and their exact subjects.
    pub claims: BTreeMap<FactId, NameClaimSubject>,
    /// Whether incompatible subjects claim this name.
    pub conflicted: bool,
    /// Whether any valid claimant has retired.
    pub retired: bool,
}

/// Named-agent lifecycle derived only from canonical facts.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AgentLifecycle {
    /// One unique unretired claim subject exists.
    Active,
    /// Claim axes are ambiguous.
    Conflicted,
    /// At least one absorbing retirement exists.
    Retired,
}

/// Named-agent lifecycle derived only from canonical facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentView {
    /// Every claim fact for this identity.
    pub claims: BTreeSet<FactId>,
    /// Candidate permanent names.
    pub names: BTreeSet<ShortText>,
    /// Candidate agent mailboxes.
    pub mailboxes: BTreeSet<MailboxAddress>,
    /// Retirement facts.
    pub retirements: BTreeSet<FactId>,
    /// Normalized claim/retirement lifecycle.
    pub lifecycle: AgentLifecycle,
    /// Runnable requires an active claim and one unconflicted selected session.
    pub runnable: bool,
    /// Active durable selection when runnable.
    pub selected_session: Option<SessionIdentity>,
    /// A claimed name remains reserved even after retirement.
    pub name_reserved: bool,
}

/// Immutable provider-session binding history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionBindingView {
    /// Binding facts and their mailbox subjects.
    pub bindings: BTreeMap<FactId, MailboxAddress>,
    /// True when more than one mailbox claims this provider/session.
    pub conflicted: bool,
    /// Unique mailbox when unconflicted.
    pub mailbox: Option<MailboxAddress>,
}

/// Grow-only repository-context history and its exact causal frontier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextHistoryView {
    /// Every context fact and typed value.
    pub history: BTreeMap<FactId, RepositoryContext>,
    /// Every causal-maximal context fact.
    pub frontier: BTreeSet<FactId>,
}

/// One selected-session candidate.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SelectionCandidate {
    /// Provider-scoped session.
    pub session: SessionIdentity,
    /// Exact immutable repository context carried by selection.
    pub context: RepositoryContext,
}

/// Durable multivalue selection register.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionView {
    /// Causal-maximal selections and values.
    pub candidates: BTreeMap<FactId, SelectionCandidate>,
    /// Exact maximal frontier.
    pub frontier: BTreeSet<FactId>,
    /// Unique active selection when unconflicted and not retired.
    pub active: Option<SelectionCandidate>,
    /// Whether distinct maxima remain.
    pub conflicted: bool,
}

/// Independent mutable display-name register.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenameView {
    /// Causal-maximal rename facts and candidate name/clear values.
    pub candidates: BTreeMap<FactId, Option<ShortText>>,
    /// Exact maximal frontier.
    pub frontier: BTreeSet<FactId>,
    /// Whether the register has one resolved value.
    pub resolved: bool,
    /// Unique display name; `None` also represents an explicit resolved clear when `resolved=true`.
    pub display_name: Option<ShortText>,
}

/// Permanent direct-session binding independent of name lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectSessionView {
    /// Exact duplicate-safe binding history.
    pub binding_facts: BTreeSet<FactId>,
    /// Bound mailbox.
    pub mailbox: MailboxAddress,
    /// Named agent when one unique compatible claim exists.
    pub named_agent: Option<AgentId>,
    /// Global session binding conflict blocks use.
    pub conflicted: bool,
}

/// Public named-agent projection values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentProjection {
    /// Name reservation.
    Name(Box<NameReservationView>),
    /// Named-agent lifecycle.
    Agent(Box<AgentView>),
    /// Session binding.
    Session(Box<SessionBindingView>),
    /// Repository-context history.
    Context(Box<ContextHistoryView>),
    /// Selection register.
    Selection(Box<SelectionView>),
    /// Rename register.
    Rename(Box<RenameView>),
    /// Direct unnamed or named session history.
    DirectSession(Box<DirectSessionView>),
}

/// Closed validation and conflict reasons for named-agent reduction.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AgentReason {
    /// Historical authority policy rejected the fact.
    Authority(AuthorityReason),
    /// A name is not a lowercase ASCII slug.
    InvalidName,
    /// The named mailbox is absent, not an agent, or not cited exactly.
    AgentMailboxMismatch,
    /// Claim, agent, mailbox, session, or context subjects disagree.
    SubjectMismatch,
    /// A causally prior retirement forbids new lifecycle state.
    RetiredAgent,
    /// One permanent name has incompatible claimants.
    NameConflict,
    /// One agent identity or mailbox has incompatible claims.
    AgentClaimConflict,
    /// One provider/session is bound to several mailboxes.
    SessionBindingConflict,
    /// Several causal-maximal selected sessions remain.
    SelectionConflict,
    /// Several causal-maximal display names remain.
    RenameConflict,
}

/// Pure complete-batch named-agent policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentReducer {
    authority: AuthorityPolicy,
}

/// Complete normalized named-agent report.
pub type AgentReport = crate::DomainReductionReport<AgentReducer>;

impl AgentReducer {
    /// Creates the reducer from explicit installation-local authority policy.
    pub const fn new(authority: AuthorityPolicy) -> Self {
        Self { authority }
    }
}

impl DomainReducer for AgentReducer {
    type AggregateKey = AgentAggregateKey;
    type ProjectionKey = AgentProjectionKey;
    type ProjectionValue = AgentProjection;
    type Reason = AgentReason;

    fn classify(
        &self,
        fact: &Fact,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        let authority = crate::authority::classify_fact(self.authority, fact, context);
        if !matches!(authority, DomainDecision::Projected) {
            return map_authority_decision(authority);
        }
        match classify_agent_fact(fact, context) {
            Ok(()) => DomainDecision::Projected,
            Err(reason) => DomainDecision::Invalid { reason },
        }
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
        derive_conflicts(context)
    }
}

fn map_authority_decision(
    decision: DomainDecision<AuthorityReason>,
) -> DomainDecision<AgentReason> {
    match decision {
        DomainDecision::Projected => DomainDecision::Projected,
        DomainDecision::Unauthorized {
            reason,
            failed_authorities,
        } => DomainDecision::Unauthorized {
            reason: AgentReason::Authority(reason),
            failed_authorities,
        },
        DomainDecision::Conflicted {
            reason,
            participants,
        } => DomainDecision::Conflicted {
            reason: AgentReason::Authority(reason),
            participants,
        },
        DomainDecision::Invalid { reason } => DomainDecision::Invalid {
            reason: AgentReason::Authority(reason),
        },
        DomainDecision::Unsupported { reason } => DomainDecision::Unsupported {
            reason: AgentReason::Authority(reason),
        },
    }
}

fn classify_agent_fact(
    fact: &Fact,
    context: &ReductionContext<'_, AgentReason>,
) -> Result<(), AgentReason> {
    match fact.payload() {
        SemanticPayload::MailboxSessionBound { mailbox_id, .. }
        | SemanticPayload::MailboxContextRecorded { mailbox_id, .. } => {
            require_agent_mailbox(fact, *mailbox_id, context).map(|_| ())
        }
        SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id,
            name,
        } => {
            if !valid_agent_name(name.as_str()) {
                return Err(AgentReason::InvalidName);
            }
            require_agent_mailbox(fact, *mailbox_id, context)?;
            if retirement_before_claim(*agent_id, fact, context) {
                return Err(AgentReason::RetiredAgent);
            }
            Ok(())
        }
        SemanticPayload::AgentRetired {
            agent_id,
            mailbox_id,
        } => require_claim_parent(fact, *agent_id, *mailbox_id, context).map(|_| ()),
        SemanticPayload::ProviderSessionSelected {
            agent_id,
            mailbox_id,
            provider,
            session,
            context: selected_context,
        } => {
            require_claim_parent(fact, *agent_id, *mailbox_id, context)?;
            require_binding_parent(fact, *mailbox_id, provider, session, context)?;
            require_context_parent(fact, *mailbox_id, selected_context, context)?;
            reject_after_retirement(fact, *agent_id, context)
        }
        SemanticPayload::ProviderSessionRenamed {
            agent_id,
            provider,
            session,
            ..
        } => {
            let (_, mailbox_id) = require_claim_for_agent_parent(fact, *agent_id, context)?;
            require_binding_parent(fact, mailbox_id, provider, session, context)?;
            reject_after_retirement(fact, *agent_id, context)
        }
        _ => Ok(()),
    }
}

fn valid_agent_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn require_agent_mailbox<'a>(
    fact: &Fact,
    mailbox_id: MailboxId,
    context: &ReductionContext<'a, AgentReason>,
) -> Result<&'a Fact, AgentReason> {
    let address = MailboxAddress::new(fact.author().installation_id(), mailbox_id);
    context
        .facts()
        .facts()
        .find(|candidate| {
            context.is_projected(candidate.id())
                && fact.causal().parents().contains(&candidate.id())
                && candidate.author().installation_id() == address.installation_id()
                && matches!(
                    candidate.payload(),
                    SemanticPayload::MailboxCreated {
                        mailbox_id: candidate_id,
                        kind: MailboxKind::Agent,
                        ..
                    } if *candidate_id == mailbox_id
                )
        })
        .ok_or(AgentReason::AgentMailboxMismatch)
}

fn require_claim_parent<'a>(
    fact: &Fact,
    agent_id: AgentId,
    mailbox_id: MailboxId,
    context: &ReductionContext<'a, AgentReason>,
) -> Result<&'a Fact, AgentReason> {
    context
        .facts()
        .facts()
        .find(|candidate| {
            context.is_projected(candidate.id())
                && fact.causal().parents().contains(&candidate.id())
                && matches!(candidate.payload(), SemanticPayload::AgentNameClaimed { agent_id: candidate_agent, mailbox_id: candidate_mailbox, .. } if *candidate_agent == agent_id && *candidate_mailbox == mailbox_id)
        })
        .ok_or(AgentReason::SubjectMismatch)
}

fn require_claim_for_agent_parent(
    fact: &Fact,
    agent_id: AgentId,
    context: &ReductionContext<'_, AgentReason>,
) -> Result<(ShortText, MailboxId), AgentReason> {
    context
        .facts()
        .facts()
        .find_map(|candidate| {
            (context.is_projected(candidate.id())
                && fact.causal().parents().contains(&candidate.id()))
            .then(|| match candidate.payload() {
                SemanticPayload::AgentNameClaimed {
                    agent_id: candidate_agent,
                    mailbox_id,
                    name,
                } if *candidate_agent == agent_id => Some((name.clone(), *mailbox_id)),
                _ => None,
            })
            .flatten()
        })
        .ok_or(AgentReason::SubjectMismatch)
}

fn require_binding_parent<'a>(
    fact: &Fact,
    mailbox_id: MailboxId,
    provider: &ProviderId,
    session: &ProviderSessionId,
    context: &ReductionContext<'a, AgentReason>,
) -> Result<&'a Fact, AgentReason> {
    context
        .facts()
        .facts()
        .find(|candidate| {
            context.is_projected(candidate.id())
                && fact.causal().parents().contains(&candidate.id())
                && matches!(candidate.payload(), SemanticPayload::MailboxSessionBound { mailbox_id: candidate_mailbox, provider: candidate_provider, session: candidate_session } if *candidate_mailbox == mailbox_id && candidate_provider == provider && candidate_session == session)
        })
        .ok_or(AgentReason::SubjectMismatch)
}

fn require_context_parent<'a>(
    fact: &Fact,
    mailbox_id: MailboxId,
    selected_context: &RepositoryContext,
    context: &ReductionContext<'a, AgentReason>,
) -> Result<&'a Fact, AgentReason> {
    context
        .facts()
        .facts()
        .find(|candidate| {
            context.is_projected(candidate.id())
                && fact.causal().parents().contains(&candidate.id())
                && matches!(candidate.payload(), SemanticPayload::MailboxContextRecorded { mailbox_id: candidate_mailbox, context: candidate_context } if *candidate_mailbox == mailbox_id && candidate_context == selected_context)
        })
        .ok_or(AgentReason::SubjectMismatch)
}

fn reject_after_retirement(
    fact: &Fact,
    agent_id: AgentId,
    context: &ReductionContext<'_, AgentReason>,
) -> Result<(), AgentReason> {
    let retired_before = context.facts().facts().any(|candidate| {
        context.is_projected(candidate.id())
            && matches!(candidate.payload(), SemanticPayload::AgentRetired { agent_id: retired, .. } if *retired == agent_id)
            && context
                .graph()
                .structurally_reaches(candidate.id(), fact.id())
    });
    (!retired_before)
        .then_some(())
        .ok_or(AgentReason::RetiredAgent)
}

fn retirement_before_claim(
    agent_id: AgentId,
    fact: &Fact,
    context: &ReductionContext<'_, AgentReason>,
) -> bool {
    context.facts().facts().any(|candidate| {
        context.is_projected(candidate.id())
            && matches!(candidate.payload(), SemanticPayload::AgentRetired { agent_id: retired_agent, .. } if *retired_agent == agent_id)
            && context
                .graph()
                .structurally_reaches(candidate.id(), fact.id())
    })
}

fn session_identity(provider: &ProviderId, session: &ProviderSessionId) -> SessionIdentity {
    SessionIdentity {
        provider: provider.clone(),
        session: session.clone(),
    }
}

fn mailbox_subject(fact: &Fact, mailbox_id: MailboxId) -> MailboxAddress {
    MailboxAddress::new(fact.author().installation_id(), mailbox_id)
}

fn aggregate_keys(fact: &Fact) -> Vec<AgentAggregateKey> {
    match fact.payload() {
        SemanticPayload::MailboxSessionBound {
            mailbox_id: _,
            provider,
            session,
        } => vec![AgentAggregateKey::Session(session_identity(
            provider, session,
        ))],
        SemanticPayload::MailboxContextRecorded { mailbox_id, .. } => {
            vec![AgentAggregateKey::Context(mailbox_subject(
                fact,
                *mailbox_id,
            ))]
        }
        SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id,
            name,
        } => vec![
            AgentAggregateKey::Name(name.clone()),
            AgentAggregateKey::Agent(*agent_id),
            AgentAggregateKey::Mailbox(mailbox_subject(fact, *mailbox_id)),
        ],
        SemanticPayload::AgentRetired { agent_id, .. } => {
            vec![AgentAggregateKey::Agent(*agent_id)]
        }
        SemanticPayload::ProviderSessionSelected { agent_id, .. } => {
            vec![AgentAggregateKey::Selection(*agent_id)]
        }
        SemanticPayload::ProviderSessionRenamed {
            agent_id,
            provider,
            session,
            ..
        } => vec![AgentAggregateKey::Rename {
            agent: *agent_id,
            session: session_identity(provider, session),
        }],
        _ => Vec::new(),
    }
}

fn derive_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut projections = Vec::new();
    projections.extend(name_projections(context));
    projections.extend(session_projections(context));
    projections.extend(context_projections(context));
    projections.extend(selection_projections(context));
    projections.extend(rename_projections(context));
    projections.extend(agent_projections(context));
    projections.extend(direct_session_projections(context));
    projections
}

fn projected_facts<'facts, 'context>(
    context: &'context ReductionContext<'facts, AgentReason>,
) -> impl Iterator<Item = &'facts Fact> + 'context
where
    'facts: 'context,
{
    context
        .facts()
        .facts()
        .filter(|fact| context.is_projected(fact.id()))
}

fn name_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut groups = BTreeMap::<ShortText, Vec<&Fact>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::AgentNameClaimed { name, .. } = fact.payload() {
            groups.entry(name.clone()).or_default().push(fact);
        }
    }
    groups
        .into_iter()
        .map(|(name, facts)| {
            let claims = facts
                .iter()
                .filter_map(|fact| match fact.payload() {
                    SemanticPayload::AgentNameClaimed {
                        agent_id,
                        mailbox_id,
                        ..
                    } => Some((
                        fact.id(),
                        NameClaimSubject {
                            agent_id: *agent_id,
                            mailbox: mailbox_subject(fact, *mailbox_id),
                        },
                    )),
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
            let subjects = claims.values().cloned().collect::<BTreeSet<_>>();
            let retired = claims.values().any(|subject| {
                projected_facts(context).any(|candidate| matches!(candidate.payload(), SemanticPayload::AgentRetired { agent_id, mailbox_id } if *agent_id == subject.agent_id && mailbox_subject(candidate, *mailbox_id) == subject.mailbox))
            });
            ProjectionContribution::new(
                AgentProjectionKey::Name(name),
                AgentProjection::Name(Box::new(NameReservationView {
                    conflicted: subjects.len() > 1,
                    claims,
                    retired,
                })),
                facts.iter().map(|fact| fact.id()),
            )
        })
        .collect()
}

fn session_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut groups = BTreeMap::<SessionIdentity, Vec<&Fact>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::MailboxSessionBound {
            provider, session, ..
        } = fact.payload()
        {
            groups
                .entry(session_identity(provider, session))
                .or_default()
                .push(fact);
        }
    }
    groups
        .into_iter()
        .map(|(session, facts)| {
            let bindings = facts
                .iter()
                .filter_map(|fact| match fact.payload() {
                    SemanticPayload::MailboxSessionBound { mailbox_id, .. } => {
                        Some((fact.id(), mailbox_subject(fact, *mailbox_id)))
                    }
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
            let mailboxes = bindings.values().copied().collect::<BTreeSet<_>>();
            let mailbox = (mailboxes.len() == 1)
                .then(|| mailboxes.iter().next().copied())
                .flatten();
            ProjectionContribution::new(
                AgentProjectionKey::Session(session),
                AgentProjection::Session(Box::new(SessionBindingView {
                    bindings,
                    conflicted: mailboxes.len() > 1,
                    mailbox,
                })),
                facts.iter().map(|fact| fact.id()),
            )
        })
        .collect()
}

fn context_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut groups = BTreeMap::<MailboxAddress, BTreeMap<FactId, RepositoryContext>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::MailboxContextRecorded {
            mailbox_id,
            context: value,
        } = fact.payload()
        {
            groups
                .entry(mailbox_subject(fact, *mailbox_id))
                .or_default()
                .insert(fact.id(), value.clone());
        }
    }
    groups
        .into_iter()
        .map(|(mailbox, history)| {
            let members = history.keys().copied().collect::<BTreeSet<_>>();
            let frontier = maximal(&members, context);
            ProjectionContribution::new(
                AgentProjectionKey::Context(mailbox),
                AgentProjection::Context(Box::new(ContextHistoryView { history, frontier })),
                members,
            )
        })
        .collect()
}

fn selection_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut groups = BTreeMap::<AgentId, BTreeSet<FactId>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::ProviderSessionSelected { agent_id, .. } = fact.payload() {
            groups.entry(*agent_id).or_default().insert(fact.id());
        }
    }
    groups
        .into_iter()
        .map(|(agent_id, members)| {
            let frontier = maximal(&members, context);
            let candidates = frontier
                .iter()
                .filter_map(|fact_id| {
                    context
                        .facts()
                        .get(*fact_id)
                        .and_then(|fact| match fact.payload() {
                            SemanticPayload::ProviderSessionSelected {
                                provider,
                                session,
                                context,
                                ..
                            } => Some((
                                *fact_id,
                                SelectionCandidate {
                                    session: session_identity(provider, session),
                                    context: context.clone(),
                                },
                            )),
                            _ => None,
                        })
                })
                .collect::<BTreeMap<_, _>>();
            let retired = agent_retired(agent_id, context);
            let values = candidates.values().cloned().collect::<BTreeSet<_>>();
            let active = (values.len() == 1
                && !retired
                && values
                    .iter()
                    .next()
                    .is_some_and(|candidate| session_unconflicted(&candidate.session, context)))
            .then(|| values.iter().next().cloned())
            .flatten();
            ProjectionContribution::new(
                AgentProjectionKey::Selection(agent_id),
                AgentProjection::Selection(Box::new(SelectionView {
                    conflicted: values.len() > 1,
                    candidates,
                    frontier,
                    active,
                })),
                members,
            )
        })
        .collect()
}

fn rename_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut groups = BTreeMap::<(AgentId, SessionIdentity), BTreeSet<FactId>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::ProviderSessionRenamed {
            agent_id,
            provider,
            session,
            ..
        } = fact.payload()
        {
            groups
                .entry((*agent_id, session_identity(provider, session)))
                .or_default()
                .insert(fact.id());
        }
    }
    groups
        .into_iter()
        .map(|((agent, session), members)| {
            let frontier = maximal(&members, context);
            let candidates = frontier
                .iter()
                .filter_map(|fact_id| {
                    context
                        .facts()
                        .get(*fact_id)
                        .and_then(|fact| match fact.payload() {
                            SemanticPayload::ProviderSessionRenamed { display_name, .. } => {
                                Some((*fact_id, display_name.clone()))
                            }
                            _ => None,
                        })
                })
                .collect::<BTreeMap<_, _>>();
            let values = candidates.values().cloned().collect::<BTreeSet<_>>();
            let resolved = values.len() == 1 && !agent_retired(agent, context);
            let display_name = resolved
                .then(|| values.iter().next().cloned())
                .flatten()
                .flatten();
            ProjectionContribution::new(
                AgentProjectionKey::Rename {
                    agent,
                    session: session.clone(),
                },
                AgentProjection::Rename(Box::new(RenameView {
                    candidates,
                    frontier,
                    resolved,
                    display_name,
                })),
                members,
            )
        })
        .collect()
}

fn agent_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut groups = BTreeMap::<AgentId, Vec<&Fact>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::AgentNameClaimed { agent_id, .. } = fact.payload() {
            groups.entry(*agent_id).or_default().push(fact);
        }
    }
    groups
        .into_iter()
        .map(|(agent_id, facts)| {
            let claims = facts.iter().map(|fact| fact.id()).collect::<BTreeSet<_>>();
            let names = facts
                .iter()
                .filter_map(|fact| match fact.payload() {
                    SemanticPayload::AgentNameClaimed { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            let mailboxes = facts
                .iter()
                .filter_map(|fact| match fact.payload() {
                    SemanticPayload::AgentNameClaimed { mailbox_id, .. } => {
                        Some(mailbox_subject(fact, *mailbox_id))
                    }
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            let retirements = projected_facts(context)
                .filter(|fact| matches!(fact.payload(), SemanticPayload::AgentRetired { agent_id: retired, .. } if *retired == agent_id))
                .map(Fact::id)
                .collect::<BTreeSet<_>>();
            let retired = !retirements.is_empty();
            let global_conflict = facts.iter().any(|claim| claim_conflicted(claim, context));
            let lifecycle = if retired {
                AgentLifecycle::Retired
            } else if names.len() == 1 && mailboxes.len() == 1 && !global_conflict {
                AgentLifecycle::Active
            } else {
                AgentLifecycle::Conflicted
            };
            let selected = selection_value(agent_id, context);
            let runnable = lifecycle == AgentLifecycle::Active && selected.is_some();
            let support = claims
                .iter()
                .copied()
                .chain(retirements.iter().copied())
                .chain(selection_members(agent_id, context))
                .collect::<BTreeSet<_>>();
            ProjectionContribution::new(
                AgentProjectionKey::Agent(agent_id),
                AgentProjection::Agent(Box::new(AgentView {
                    claims,
                    names,
                    mailboxes,
                    retirements,
                    lifecycle,
                    runnable,
                    selected_session: runnable.then_some(selected).flatten(),
                    name_reserved: true,
                })),
                support,
            )
        })
        .collect()
}

fn direct_session_projections(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ProjectionContribution<AgentProjectionKey, AgentProjection>> {
    let mut groups = BTreeMap::<(MailboxAddress, SessionIdentity), BTreeSet<FactId>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::MailboxSessionBound {
            mailbox_id,
            provider,
            session,
        } = fact.payload()
        {
            groups
                .entry((
                    mailbox_subject(fact, *mailbox_id),
                    session_identity(provider, session),
                ))
                .or_default()
                .insert(fact.id());
        }
    }
    groups
        .into_iter()
        .map(|((mailbox, identity), binding_facts)| {
            let bindings = session_bindings(&identity, context);
            let mailboxes = bindings.values().copied().collect::<BTreeSet<_>>();
            let named = unique_agent_for_mailbox(mailbox, context);
            ProjectionContribution::new(
                AgentProjectionKey::DirectSession {
                    mailbox,
                    session: identity,
                },
                AgentProjection::DirectSession(Box::new(DirectSessionView {
                    binding_facts: binding_facts.clone(),
                    mailbox,
                    named_agent: named,
                    conflicted: mailboxes.len() > 1,
                })),
                binding_facts,
            )
        })
        .collect()
}

fn claim_conflicted(fact: &Fact, context: &ReductionContext<'_, AgentReason>) -> bool {
    let SemanticPayload::AgentNameClaimed {
        agent_id,
        mailbox_id,
        name,
    } = fact.payload()
    else {
        return false;
    };
    let subject = (*agent_id, mailbox_subject(fact, *mailbox_id), name.clone());
    projected_facts(context).any(|candidate| {
        matches!(candidate.payload(), SemanticPayload::AgentNameClaimed { agent_id: other_agent, mailbox_id: other_mailbox, name: other_name } if
            (*other_agent == *agent_id || mailbox_subject(candidate, *other_mailbox) == subject.1 || other_name == name)
            && (*other_agent, mailbox_subject(candidate, *other_mailbox), other_name.clone()) != subject)
    })
}

fn session_bindings(
    identity: &SessionIdentity,
    context: &ReductionContext<'_, AgentReason>,
) -> BTreeMap<FactId, MailboxAddress> {
    projected_facts(context)
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::MailboxSessionBound {
                mailbox_id,
                provider,
                session,
            } if session_identity(provider, session) == *identity => {
                Some((fact.id(), mailbox_subject(fact, *mailbox_id)))
            }
            _ => None,
        })
        .collect()
}

fn unique_agent_for_mailbox(
    mailbox: MailboxAddress,
    context: &ReductionContext<'_, AgentReason>,
) -> Option<AgentId> {
    let agents = projected_facts(context)
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::AgentNameClaimed {
                agent_id,
                mailbox_id,
                ..
            } if mailbox_subject(fact, *mailbox_id) == mailbox
                && !claim_conflicted(fact, context) =>
            {
                Some(*agent_id)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    (agents.len() == 1)
        .then(|| agents.iter().next().copied())
        .flatten()
}

fn selection_members<'a>(
    agent_id: AgentId,
    context: &'a ReductionContext<'a, AgentReason>,
) -> impl Iterator<Item = FactId> + 'a {
    projected_facts(context).filter_map(move |fact| {
        matches!(fact.payload(), SemanticPayload::ProviderSessionSelected { agent_id: selected, .. } if *selected == agent_id)
            .then_some(fact.id())
    })
}

fn selection_value(
    agent_id: AgentId,
    context: &ReductionContext<'_, AgentReason>,
) -> Option<SessionIdentity> {
    let members = selection_members(agent_id, context).collect::<BTreeSet<_>>();
    let frontier = maximal(&members, context);
    let values = frontier
        .iter()
        .filter_map(|fact_id| context.facts().get(*fact_id))
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::ProviderSessionSelected {
                provider, session, ..
            } => Some(session_identity(provider, session)),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    (values.len() == 1)
        .then(|| values.iter().next().cloned())
        .flatten()
        .filter(|session| session_unconflicted(session, context))
}

fn session_unconflicted(
    session: &SessionIdentity,
    context: &ReductionContext<'_, AgentReason>,
) -> bool {
    session_bindings(session, context)
        .into_values()
        .collect::<BTreeSet<_>>()
        .len()
        == 1
}

fn agent_retired(agent_id: AgentId, context: &ReductionContext<'_, AgentReason>) -> bool {
    projected_facts(context).any(|fact| {
        matches!(fact.payload(), SemanticPayload::AgentRetired { agent_id: retired, .. } if *retired == agent_id)
    })
}

fn maximal(
    members: &BTreeSet<FactId>,
    context: &ReductionContext<'_, AgentReason>,
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

#[allow(clippy::too_many_lines)]
fn derive_conflicts(
    context: &ReductionContext<'_, AgentReason>,
) -> Vec<ConflictObservation<AgentReason>> {
    let mut conflicts = Vec::new();
    let claims = projected_facts(context)
        .filter(|fact| matches!(fact.payload(), SemanticPayload::AgentNameClaimed { .. }))
        .collect::<Vec<_>>();
    for fact in &claims {
        let SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id,
            name,
        } = fact.payload()
        else {
            continue;
        };
        let same_name = claims
            .iter()
            .filter(|candidate| matches!(candidate.payload(), SemanticPayload::AgentNameClaimed { name: other, .. } if other == name))
            .map(|candidate| candidate.id())
            .collect::<BTreeSet<_>>();
        if claim_subjects(&same_name, context).len() > 1 {
            push_conflict(&mut conflicts, AgentReason::NameConflict, same_name);
        }
        let mailbox = mailbox_subject(fact, *mailbox_id);
        let same_subject_axis = claims
            .iter()
            .filter(|candidate| matches!(candidate.payload(), SemanticPayload::AgentNameClaimed { agent_id: other_agent, mailbox_id: other_mailbox, .. } if other_agent == agent_id || mailbox_subject(candidate, *other_mailbox) == mailbox))
            .map(|candidate| candidate.id())
            .collect::<BTreeSet<_>>();
        if claim_subjects(&same_subject_axis, context).len() > 1 {
            push_conflict(
                &mut conflicts,
                AgentReason::AgentClaimConflict,
                same_subject_axis,
            );
        }
    }
    let sessions = projected_facts(context)
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::MailboxSessionBound {
                provider, session, ..
            } => Some(session_identity(provider, session)),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for session in sessions {
        let bindings = session_bindings(&session, context);
        if bindings.values().copied().collect::<BTreeSet<_>>().len() > 1 {
            push_conflict(
                &mut conflicts,
                AgentReason::SessionBindingConflict,
                bindings.into_keys().collect(),
            );
        }
    }
    let agents = projected_facts(context)
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::ProviderSessionSelected { agent_id, .. } => Some(*agent_id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for agent in agents {
        let members = selection_members(agent, context).collect::<BTreeSet<_>>();
        let frontier = maximal(&members, context);
        let values = frontier
            .iter()
            .filter_map(|fact_id| context.facts().get(*fact_id))
            .filter_map(|fact| match fact.payload() {
                SemanticPayload::ProviderSessionSelected {
                    provider,
                    session,
                    context,
                    ..
                } => Some(SelectionCandidate {
                    session: session_identity(provider, session),
                    context: context.clone(),
                }),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if values.len() > 1 {
            push_conflict(&mut conflicts, AgentReason::SelectionConflict, frontier);
        }
    }
    let mut renames = BTreeMap::<(AgentId, SessionIdentity), BTreeSet<FactId>>::new();
    for fact in projected_facts(context) {
        if let SemanticPayload::ProviderSessionRenamed {
            agent_id,
            provider,
            session,
            ..
        } = fact.payload()
        {
            renames
                .entry((*agent_id, session_identity(provider, session)))
                .or_default()
                .insert(fact.id());
        }
    }
    for members in renames.into_values() {
        let frontier = maximal(&members, context);
        let values = frontier
            .iter()
            .filter_map(|fact_id| context.facts().get(*fact_id))
            .filter_map(|fact| match fact.payload() {
                SemanticPayload::ProviderSessionRenamed { display_name, .. } => {
                    Some(display_name.clone())
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if values.len() > 1 {
            push_conflict(&mut conflicts, AgentReason::RenameConflict, frontier);
        }
    }
    conflicts.sort();
    conflicts.dedup();
    conflicts
}

fn claim_subjects(
    facts: &BTreeSet<FactId>,
    context: &ReductionContext<'_, AgentReason>,
) -> BTreeSet<(AgentId, MailboxAddress, ShortText)> {
    facts
        .iter()
        .filter_map(|fact_id| context.facts().get(*fact_id))
        .filter_map(|fact| match fact.payload() {
            SemanticPayload::AgentNameClaimed {
                agent_id,
                mailbox_id,
                name,
            } => Some((*agent_id, mailbox_subject(fact, *mailbox_id), name.clone())),
            _ => None,
        })
        .collect()
}

fn push_conflict(
    conflicts: &mut Vec<ConflictObservation<AgentReason>>,
    reason: AgentReason,
    participants: BTreeSet<FactId>,
) {
    if participants.len() > 1 {
        conflicts.push(ConflictObservation::new(
            ConflictReason::Domain(reason),
            participants,
        ));
    }
}
