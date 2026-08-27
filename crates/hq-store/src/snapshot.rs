//! Complete-batch oracle and representation-independent structural index.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::{AuthorityRole, FactId};
use hq_protocol::VerifiedSemanticFact;
use hq_reducer::{
    AgentReason, AgentReducer, AgentReport, AuthorityPolicy, AuthorityReason, AuthorityReducer,
    AuthorityReport, ConflictReason, ConversationReason, ConversationReducer, ConversationReport,
    DecisionReason, DecisionStatus, FactDecision, ProjectReason, ProjectReducer, ProjectReport,
    ReductionReport, reduce_complete,
};
use sha2::{Digest, Sha256};

use crate::{AgentProjectionSnapshot, AuthorityProjectionSnapshot, ConversationProjectionSnapshot};
use crate::{StoreError, StoreErrorClass};

/// One independently materialized reducer domain.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReductionDomain {
    /// Installation, mailbox, peer, capability, and account authority.
    Authority,
    /// Conversation, message state, presentation, and activity.
    Conversation,
    /// Named agents and provider sessions.
    Agent,
    /// Projects, resources, assignments, dispatch, output, and remote control.
    Project,
}

impl ReductionDomain {
    /// Every persisted reducer domain in stable order.
    pub const ALL: [Self; 4] = [
        Self::Authority,
        Self::Conversation,
        Self::Agent,
        Self::Project,
    ];
}

/// Framework or domain reason retained without formatting reducer prose.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReductionReason {
    /// Unequal semantic values reused one fact identity.
    IdentityCollision,
    /// The fact participates in a causal cycle.
    CausalCycle,
    /// Authority-domain reason.
    Authority(AuthorityReason),
    /// Conversation/activity-domain reason.
    Conversation(ConversationReason),
    /// Named-agent-domain reason.
    Agent(AgentReason),
    /// Project-domain reason.
    Project(ProjectReason),
}

/// One normalized fact decision and all diagnostic edges except global reverse dependencies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedDecision {
    pub(crate) status: DecisionStatus,
    pub(crate) reason: Option<ReductionReason>,
    pub(crate) missing_dependencies: BTreeSet<FactId>,
    pub(crate) unusable_dependencies: BTreeMap<FactId, DecisionStatus>,
    pub(crate) failed_authorities: BTreeSet<AuthorityRole>,
    pub(crate) conflict_participants: BTreeSet<FactId>,
}

impl IndexedDecision {
    /// Returns the normalized decision category.
    pub const fn status(&self) -> DecisionStatus {
        self.status
    }

    /// Returns the closed framework or domain reason.
    pub const fn reason(&self) -> Option<&ReductionReason> {
        self.reason.as_ref()
    }

    /// Returns missing required causal identities.
    pub const fn missing_dependencies(&self) -> &BTreeSet<FactId> {
        &self.missing_dependencies
    }

    /// Returns present but unusable dependencies and their outcomes.
    pub const fn unusable_dependencies(&self) -> &BTreeMap<FactId, DecisionStatus> {
        &self.unusable_dependencies
    }

    /// Returns typed historical-authority roles that failed.
    pub const fn failed_authorities(&self) -> &BTreeSet<AuthorityRole> {
        &self.failed_authorities
    }

    /// Returns every participant in the fact's conflict decision.
    pub const fn conflict_participants(&self) -> &BTreeSet<FactId> {
        &self.conflict_participants
    }
}

/// One normalized reducer conflict outside an individual decision.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IndexedConflict {
    pub(crate) reason: ReductionReason,
    pub(crate) participants: BTreeSet<FactId>,
}

impl IndexedConflict {
    /// Returns the closed conflict reason.
    pub const fn reason(&self) -> &ReductionReason {
        &self.reason
    }

    /// Returns every normalized participant.
    pub const fn participants(&self) -> &BTreeSet<FactId> {
        &self.participants
    }
}

/// Typed structural rows persisted by complete repair, independent of SQL layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReductionIndexSnapshot {
    pub(crate) policy: AuthorityPolicy,
    pub(crate) reverse_dependencies: BTreeMap<FactId, BTreeSet<FactId>>,
    pub(crate) decisions: BTreeMap<(ReductionDomain, FactId), IndexedDecision>,
    pub(crate) dependency_order: BTreeMap<ReductionDomain, Vec<FactId>>,
    pub(crate) presentation_order: BTreeMap<ReductionDomain, Vec<FactId>>,
    pub(crate) conflicts: BTreeMap<ReductionDomain, Vec<IndexedConflict>>,
}

impl ReductionIndexSnapshot {
    /// Returns the explicit local authority policy used for every report.
    pub const fn policy(&self) -> AuthorityPolicy {
        self.policy
    }

    /// Returns one fact decision for a reducer domain.
    pub fn decision(&self, domain: ReductionDomain, fact_id: FactId) -> Option<&IndexedDecision> {
        self.decisions.get(&(domain, fact_id))
    }

    /// Returns the complete global reverse-dependency index, including missing vertices.
    pub const fn reverse_dependencies(&self) -> &BTreeMap<FactId, BTreeSet<FactId>> {
        &self.reverse_dependencies
    }

    /// Returns deterministic dependency order for one domain.
    pub fn dependency_order(&self, domain: ReductionDomain) -> &[FactId] {
        self.dependency_order
            .get(&domain)
            .map_or(&[], Vec::as_slice)
    }

    /// Returns reducer-owned presentation order for one domain.
    pub fn presentation_order(&self, domain: ReductionDomain) -> &[FactId] {
        self.presentation_order
            .get(&domain)
            .map_or(&[], Vec::as_slice)
    }

    /// Returns normalized aggregate/global conflicts for one domain.
    pub fn conflicts(&self, domain: ReductionDomain) -> &[IndexedConflict] {
        self.conflicts.get(&domain).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"hq-reduction-index-v1\0");
        digest.update(self.policy.local_installation().as_bytes());
        digest.update(self.policy.local_human_mailbox().as_bytes());
        put_map_len(&mut digest, self.reverse_dependencies.len());
        for (parent, children) in &self.reverse_dependencies {
            digest.update(parent.as_bytes());
            put_ids(&mut digest, children.iter().copied());
        }
        put_map_len(&mut digest, self.decisions.len());
        for ((domain, fact_id), decision) in &self.decisions {
            put_i64(&mut digest, encode_domain(*domain));
            digest.update(fact_id.as_bytes());
            put_i64(&mut digest, encode_status(decision.status));
            if let Some(reason) = &decision.reason {
                let (code, parameter) = encode_reason(reason);
                put_i64(&mut digest, code);
                put_i64(&mut digest, parameter);
            } else {
                put_i64(&mut digest, 0);
                put_i64(&mut digest, 0);
            }
            put_ids(&mut digest, decision.missing_dependencies.iter().copied());
            put_map_len(&mut digest, decision.unusable_dependencies.len());
            for (dependency, status) in &decision.unusable_dependencies {
                digest.update(dependency.as_bytes());
                put_i64(&mut digest, encode_status(*status));
            }
            put_map_len(&mut digest, decision.failed_authorities.len());
            for role in &decision.failed_authorities {
                put_i64(&mut digest, encode_role(*role));
            }
            put_ids(&mut digest, decision.conflict_participants.iter().copied());
        }
        put_orders(&mut digest, &self.dependency_order);
        put_orders(&mut digest, &self.presentation_order);
        put_map_len(&mut digest, self.conflicts.len());
        for (domain, conflicts) in &self.conflicts {
            put_i64(&mut digest, encode_domain(*domain));
            put_map_len(&mut digest, conflicts.len());
            for conflict in conflicts {
                let (code, parameter) = encode_reason(&conflict.reason);
                put_i64(&mut digest, code);
                put_i64(&mut digest, parameter);
                put_ids(&mut digest, conflict.participants.iter().copied());
            }
        }
        digest.finalize().into()
    }
}

/// All authoritative complete-batch reports derived from one reverified corpus and policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteSnapshot {
    policy: AuthorityPolicy,
    authority: AuthorityReport,
    conversation: ConversationReport,
    agent: AgentReport,
    project: ProjectReport,
}

impl CompleteSnapshot {
    /// Returns the explicit local authority policy shared by every report.
    pub const fn policy(&self) -> AuthorityPolicy {
        self.policy
    }

    /// Returns the authority report.
    pub const fn authority(&self) -> &AuthorityReport {
        &self.authority
    }

    /// Clones the complete authority projection/frontier/support oracle.
    pub fn authority_projection_snapshot(&self) -> AuthorityProjectionSnapshot {
        AuthorityProjectionSnapshot::from_report(&self.authority)
    }

    /// Returns the conversation and activity report.
    pub const fn conversation(&self) -> &ConversationReport {
        &self.conversation
    }

    /// Clones the complete conversation projection/frontier/support oracle.
    pub fn conversation_projection_snapshot(&self) -> ConversationProjectionSnapshot {
        ConversationProjectionSnapshot::from_report(&self.conversation)
    }

    /// Returns the named-agent report.
    pub const fn agent(&self) -> &AgentReport {
        &self.agent
    }

    /// Clones the complete named-agent projection/frontier/support oracle.
    pub fn agent_projection_snapshot(&self) -> AgentProjectionSnapshot {
        AgentProjectionSnapshot::from_report(&self.agent)
    }

    /// Returns the project report.
    pub const fn project(&self) -> &ProjectReport {
        &self.project
    }

    /// Normalizes structural report data into the representation persisted by repair.
    pub fn normalized_index(&self) -> ReductionIndexSnapshot {
        normalize(self)
    }
}

pub(crate) fn build_complete_snapshot(
    facts: &[VerifiedSemanticFact],
    policy: AuthorityPolicy,
) -> Result<CompleteSnapshot, StoreError> {
    let semantic = facts
        .iter()
        .map(|fact| fact.fact().clone())
        .collect::<Vec<_>>();
    let authority = reduce_complete(semantic.clone(), &AuthorityReducer::new(policy));
    let conversation = reduce_complete(semantic.clone(), &ConversationReducer::new(policy));
    let agent = reduce_complete(semantic.clone(), &AgentReducer::new(policy));
    let project = reduce_complete(semantic, &ProjectReducer::new(policy));
    Ok(CompleteSnapshot {
        policy,
        authority: authority.map_err(reduction_error)?,
        conversation: conversation.map_err(reduction_error)?,
        agent: agent.map_err(reduction_error)?,
        project: project.map_err(reduction_error)?,
    })
}

fn normalize(snapshot: &CompleteSnapshot) -> ReductionIndexSnapshot {
    let mut index = ReductionIndexSnapshot {
        policy: snapshot.policy,
        reverse_dependencies: snapshot
            .authority
            .graph()
            .vertices()
            .iter()
            .copied()
            .map(|fact_id| {
                (
                    fact_id,
                    snapshot.authority.graph().children(fact_id).clone(),
                )
            })
            .collect(),
        decisions: BTreeMap::new(),
        dependency_order: BTreeMap::new(),
        presentation_order: BTreeMap::new(),
        conflicts: BTreeMap::new(),
    };
    normalize_report(
        &mut index,
        ReductionDomain::Authority,
        &snapshot.authority,
        ReductionReason::Authority,
    );
    normalize_report(
        &mut index,
        ReductionDomain::Conversation,
        &snapshot.conversation,
        ReductionReason::Conversation,
    );
    normalize_report(
        &mut index,
        ReductionDomain::Agent,
        &snapshot.agent,
        ReductionReason::Agent,
    );
    normalize_report(
        &mut index,
        ReductionDomain::Project,
        &snapshot.project,
        ReductionReason::Project,
    );
    index
}

fn normalize_report<A, K, V, R>(
    index: &mut ReductionIndexSnapshot,
    domain: ReductionDomain,
    report: &ReductionReport<A, K, V, R>,
    map_reason: impl Fn(R) -> ReductionReason + Copy,
) where
    R: Clone,
{
    for (fact_id, decision) in report.decisions() {
        index
            .decisions
            .insert((domain, *fact_id), normalize_decision(decision, map_reason));
    }
    index
        .dependency_order
        .insert(domain, report.dependency_order().to_vec());
    index
        .presentation_order
        .insert(domain, report.presentation_order().to_vec());
    index.conflicts.insert(
        domain,
        report
            .conflicts()
            .iter()
            .map(|conflict| IndexedConflict {
                reason: match conflict.reason() {
                    ConflictReason::IdentityCollision => ReductionReason::IdentityCollision,
                    ConflictReason::Domain(reason) => map_reason(reason.clone()),
                },
                participants: conflict.participants().clone(),
            })
            .collect(),
    );
}

fn normalize_decision<R: Clone>(
    decision: &FactDecision<R>,
    map_reason: impl Fn(R) -> ReductionReason,
) -> IndexedDecision {
    IndexedDecision {
        status: decision.status(),
        reason: decision.reason().map(|reason| match reason {
            DecisionReason::IdentityCollision => ReductionReason::IdentityCollision,
            DecisionReason::CausalCycle => ReductionReason::CausalCycle,
            DecisionReason::Domain(reason) => map_reason(reason.clone()),
        }),
        missing_dependencies: decision.missing_dependencies().clone(),
        unusable_dependencies: decision.unusable_dependencies().clone(),
        failed_authorities: decision.failed_authorities().clone(),
        conflict_participants: decision.conflict_participants().clone(),
    }
}

pub(crate) const fn encode_domain(domain: ReductionDomain) -> i64 {
    match domain {
        ReductionDomain::Authority => 1,
        ReductionDomain::Conversation => 2,
        ReductionDomain::Agent => 3,
        ReductionDomain::Project => 4,
    }
}

pub(crate) fn decode_domain(value: i64) -> Option<ReductionDomain> {
    match value {
        1 => Some(ReductionDomain::Authority),
        2 => Some(ReductionDomain::Conversation),
        3 => Some(ReductionDomain::Agent),
        4 => Some(ReductionDomain::Project),
        _ => None,
    }
}

pub(crate) const fn encode_status(status: DecisionStatus) -> i64 {
    match status {
        DecisionStatus::Projected => 1,
        DecisionStatus::Unresolved => 2,
        DecisionStatus::Unauthorized => 3,
        DecisionStatus::Conflicted => 4,
        DecisionStatus::Invalid => 5,
        DecisionStatus::Unsupported => 6,
    }
}

pub(crate) fn decode_status(value: i64) -> Option<DecisionStatus> {
    match value {
        1 => Some(DecisionStatus::Projected),
        2 => Some(DecisionStatus::Unresolved),
        3 => Some(DecisionStatus::Unauthorized),
        4 => Some(DecisionStatus::Conflicted),
        5 => Some(DecisionStatus::Invalid),
        6 => Some(DecisionStatus::Unsupported),
        _ => None,
    }
}

pub(crate) fn encode_reason(reason: &ReductionReason) -> (i64, i64) {
    match reason {
        ReductionReason::IdentityCollision => (1, 0),
        ReductionReason::CausalCycle => (2, 0),
        ReductionReason::Authority(reason) => encode_authority_reason(*reason, 1_000),
        ReductionReason::Conversation(ConversationReason::Authority(reason)) => {
            encode_authority_reason(*reason, 2_000)
        }
        ReductionReason::Conversation(reason) => (conversation_reason_code(reason), 0),
        ReductionReason::Agent(AgentReason::Authority(reason)) => {
            encode_authority_reason(*reason, 3_000)
        }
        ReductionReason::Agent(reason) => (agent_reason_code(reason), 0),
        ReductionReason::Project(ProjectReason::Authority(reason)) => {
            encode_authority_reason(*reason, 4_000)
        }
        ReductionReason::Project(reason) => (project_reason_code(reason), 0),
    }
}

pub(crate) fn decode_reason(code: i64, parameter: i64) -> Option<ReductionReason> {
    match code {
        1 if parameter == 0 => Some(ReductionReason::IdentityCollision),
        2 if parameter == 0 => Some(ReductionReason::CausalCycle),
        1_001..=1_011 => {
            decode_authority_reason(code, parameter, 1_000).map(ReductionReason::Authority)
        }
        2_001..=2_011 => decode_authority_reason(code, parameter, 2_000)
            .map(ConversationReason::Authority)
            .map(ReductionReason::Conversation),
        2_101..=2_108 if parameter == 0 => {
            decode_conversation_reason(code).map(ReductionReason::Conversation)
        }
        3_001..=3_011 => decode_authority_reason(code, parameter, 3_000)
            .map(AgentReason::Authority)
            .map(ReductionReason::Agent),
        3_101..=3_109 if parameter == 0 => decode_agent_reason(code).map(ReductionReason::Agent),
        4_001..=4_011 => decode_authority_reason(code, parameter, 4_000)
            .map(ProjectReason::Authority)
            .map(ReductionReason::Project),
        4_101..=4_114 if parameter == 0 => {
            decode_project_reason(code).map(ReductionReason::Project)
        }
        _ => None,
    }
}

pub(crate) const fn reason_belongs_to_domain(
    domain: ReductionDomain,
    reason: &ReductionReason,
) -> bool {
    matches!(
        (domain, reason),
        (
            _,
            ReductionReason::IdentityCollision | ReductionReason::CausalCycle
        ) | (ReductionDomain::Authority, ReductionReason::Authority(_))
            | (
                ReductionDomain::Conversation,
                ReductionReason::Conversation(_)
            )
            | (ReductionDomain::Agent, ReductionReason::Agent(_))
            | (ReductionDomain::Project, ReductionReason::Project(_))
    )
}

fn encode_authority_reason(reason: AuthorityReason, base: i64) -> (i64, i64) {
    let (offset, parameter) = match reason {
        AuthorityReason::ScopeMismatch => (1, 0),
        AuthorityReason::SignerMismatch => (2, 0),
        AuthorityReason::SubjectMismatch => (3, 0),
        AuthorityReason::MissingAuthority(role) => (4, encode_role(role)),
        AuthorityReason::AuthorityKindMismatch(role) => (5, encode_role(role)),
        AuthorityReason::RootHasParents => (6, 0),
        AuthorityReason::UniqueRootConflict => (7, 0),
        AuthorityReason::ConcurrentRegisterConflict => (8, 0),
        AuthorityReason::IncompleteRevocationFrontier => (9, 0),
        AuthorityReason::RevokedMailboxGrant => (10, 0),
        AuthorityReason::InactiveAccountMembership => (11, 0),
    };
    (base + offset, parameter)
}

fn decode_authority_reason(code: i64, parameter: i64, base: i64) -> Option<AuthorityReason> {
    match code - base {
        1 if parameter == 0 => Some(AuthorityReason::ScopeMismatch),
        2 if parameter == 0 => Some(AuthorityReason::SignerMismatch),
        3 if parameter == 0 => Some(AuthorityReason::SubjectMismatch),
        4 => decode_role(parameter).map(AuthorityReason::MissingAuthority),
        5 => decode_role(parameter).map(AuthorityReason::AuthorityKindMismatch),
        6 if parameter == 0 => Some(AuthorityReason::RootHasParents),
        7 if parameter == 0 => Some(AuthorityReason::UniqueRootConflict),
        8 if parameter == 0 => Some(AuthorityReason::ConcurrentRegisterConflict),
        9 if parameter == 0 => Some(AuthorityReason::IncompleteRevocationFrontier),
        10 if parameter == 0 => Some(AuthorityReason::RevokedMailboxGrant),
        11 if parameter == 0 => Some(AuthorityReason::InactiveAccountMembership),
        _ => None,
    }
}

fn conversation_reason_code(reason: &ConversationReason) -> i64 {
    match reason {
        ConversationReason::Authority(_) => 0,
        ConversationReason::AddressMismatch => 2_101,
        ConversationReason::ThreadMismatch => 2_102,
        ConversationReason::TargetNotAncestor => 2_103,
        ConversationReason::RejectedMessage => 2_104,
        ConversationReason::MessageIdentityConflict => 2_105,
        ConversationReason::ActivitySourceMismatch => 2_106,
        ConversationReason::ActivitySequenceConflict => 2_107,
        ConversationReason::ActivityRuntimeConflict => 2_108,
    }
}

fn decode_conversation_reason(code: i64) -> Option<ConversationReason> {
    match code {
        2_101 => Some(ConversationReason::AddressMismatch),
        2_102 => Some(ConversationReason::ThreadMismatch),
        2_103 => Some(ConversationReason::TargetNotAncestor),
        2_104 => Some(ConversationReason::RejectedMessage),
        2_105 => Some(ConversationReason::MessageIdentityConflict),
        2_106 => Some(ConversationReason::ActivitySourceMismatch),
        2_107 => Some(ConversationReason::ActivitySequenceConflict),
        2_108 => Some(ConversationReason::ActivityRuntimeConflict),
        _ => None,
    }
}

fn agent_reason_code(reason: &AgentReason) -> i64 {
    match reason {
        AgentReason::Authority(_) => 0,
        AgentReason::InvalidName => 3_101,
        AgentReason::AgentMailboxMismatch => 3_102,
        AgentReason::SubjectMismatch => 3_103,
        AgentReason::RetiredAgent => 3_104,
        AgentReason::NameConflict => 3_105,
        AgentReason::AgentClaimConflict => 3_106,
        AgentReason::SessionBindingConflict => 3_107,
        AgentReason::SelectionConflict => 3_108,
        AgentReason::RenameConflict => 3_109,
    }
}

fn decode_agent_reason(code: i64) -> Option<AgentReason> {
    match code {
        3_101 => Some(AgentReason::InvalidName),
        3_102 => Some(AgentReason::AgentMailboxMismatch),
        3_103 => Some(AgentReason::SubjectMismatch),
        3_104 => Some(AgentReason::RetiredAgent),
        3_105 => Some(AgentReason::NameConflict),
        3_106 => Some(AgentReason::AgentClaimConflict),
        3_107 => Some(AgentReason::SessionBindingConflict),
        3_108 => Some(AgentReason::SelectionConflict),
        3_109 => Some(AgentReason::RenameConflict),
        _ => None,
    }
}

fn project_reason_code(reason: &ProjectReason) -> i64 {
    match reason {
        ProjectReason::Authority(_) => 0,
        ProjectReason::SubjectMismatch => 4_101,
        ProjectReason::ProjectAuthorityMismatch => 4_102,
        ProjectReason::ProjectIdentityConflict => 4_103,
        ProjectReason::HomeLinearFork => 4_104,
        ProjectReason::StaleHead => 4_105,
        ProjectReason::InvalidTransition => 4_106,
        ProjectReason::ResourceInvariant => 4_107,
        ProjectReason::ResourceClaimConflict => 4_108,
        ProjectReason::AssignmentCardinalityConflict => 4_109,
        ProjectReason::AssignmentBindingMismatch => 4_110,
        ProjectReason::InputSequenceConflict => 4_111,
        ProjectReason::DispatchConflict => 4_112,
        ProjectReason::OutputConflict => 4_113,
        ProjectReason::RemoteCommandConflict => 4_114,
    }
}

fn decode_project_reason(code: i64) -> Option<ProjectReason> {
    match code {
        4_101 => Some(ProjectReason::SubjectMismatch),
        4_102 => Some(ProjectReason::ProjectAuthorityMismatch),
        4_103 => Some(ProjectReason::ProjectIdentityConflict),
        4_104 => Some(ProjectReason::HomeLinearFork),
        4_105 => Some(ProjectReason::StaleHead),
        4_106 => Some(ProjectReason::InvalidTransition),
        4_107 => Some(ProjectReason::ResourceInvariant),
        4_108 => Some(ProjectReason::ResourceClaimConflict),
        4_109 => Some(ProjectReason::AssignmentCardinalityConflict),
        4_110 => Some(ProjectReason::AssignmentBindingMismatch),
        4_111 => Some(ProjectReason::InputSequenceConflict),
        4_112 => Some(ProjectReason::DispatchConflict),
        4_113 => Some(ProjectReason::OutputConflict),
        4_114 => Some(ProjectReason::RemoteCommandConflict),
        _ => None,
    }
}

pub(crate) const fn encode_role(role: AuthorityRole) -> i64 {
    match role {
        AuthorityRole::LocalInstallation => 1,
        AuthorityRole::MailboxOwner => 2,
        AuthorityRole::MailboxGrant => 3,
        AuthorityRole::AccountCreator => 4,
        AuthorityRole::DeviceGrant => 5,
        AuthorityRole::AccountMembership => 6,
        AuthorityRole::PreviousState => 7,
        AuthorityRole::ProjectHome => 8,
        AuthorityRole::ActiveHuman => 9,
        AuthorityRole::Assignment => 10,
        AuthorityRole::Dispatch => 11,
        AuthorityRole::Request => 12,
        AuthorityRole::OutputBinding => 13,
    }
}

pub(crate) fn decode_role(value: i64) -> Option<AuthorityRole> {
    AuthorityRole::ALL
        .into_iter()
        .find(|role| encode_role(*role) == value)
}

fn put_orders(digest: &mut Sha256, orders: &BTreeMap<ReductionDomain, Vec<FactId>>) {
    put_map_len(digest, orders.len());
    for (domain, facts) in orders {
        put_i64(digest, encode_domain(*domain));
        put_ids(digest, facts.iter().copied());
    }
}

fn put_ids(digest: &mut Sha256, facts: impl IntoIterator<Item = FactId>) {
    let facts = facts.into_iter().collect::<Vec<_>>();
    put_map_len(digest, facts.len());
    for fact in facts {
        digest.update(fact.as_bytes());
    }
}

fn put_map_len(digest: &mut Sha256, length: usize) {
    digest.update(u64::try_from(length).unwrap_or(u64::MAX).to_be_bytes());
}

fn put_i64(digest: &mut Sha256, value: i64) {
    digest.update(value.to_be_bytes());
}

fn reduction_error(_: hq_reducer::ReduceError) -> StoreError {
    StoreError::new(StoreErrorClass::ReductionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_closed_reason_codec_round_trips() {
        let authority = [
            AuthorityReason::ScopeMismatch,
            AuthorityReason::SignerMismatch,
            AuthorityReason::SubjectMismatch,
            AuthorityReason::MissingAuthority(AuthorityRole::LocalInstallation),
            AuthorityReason::AuthorityKindMismatch(AuthorityRole::OutputBinding),
            AuthorityReason::RootHasParents,
            AuthorityReason::UniqueRootConflict,
            AuthorityReason::ConcurrentRegisterConflict,
            AuthorityReason::IncompleteRevocationFrontier,
            AuthorityReason::RevokedMailboxGrant,
            AuthorityReason::InactiveAccountMembership,
        ];
        let mut reasons = vec![
            ReductionReason::IdentityCollision,
            ReductionReason::CausalCycle,
        ];
        reasons.extend(authority.iter().copied().map(ReductionReason::Authority));
        reasons.extend(
            authority
                .iter()
                .copied()
                .map(ConversationReason::Authority)
                .map(ReductionReason::Conversation),
        );
        reasons.extend(
            [
                ConversationReason::AddressMismatch,
                ConversationReason::ThreadMismatch,
                ConversationReason::TargetNotAncestor,
                ConversationReason::RejectedMessage,
                ConversationReason::MessageIdentityConflict,
                ConversationReason::ActivitySourceMismatch,
                ConversationReason::ActivitySequenceConflict,
                ConversationReason::ActivityRuntimeConflict,
            ]
            .into_iter()
            .map(ReductionReason::Conversation),
        );
        reasons.extend(
            authority
                .iter()
                .copied()
                .map(AgentReason::Authority)
                .map(ReductionReason::Agent),
        );
        reasons.extend(
            [
                AgentReason::InvalidName,
                AgentReason::AgentMailboxMismatch,
                AgentReason::SubjectMismatch,
                AgentReason::RetiredAgent,
                AgentReason::NameConflict,
                AgentReason::AgentClaimConflict,
                AgentReason::SessionBindingConflict,
                AgentReason::SelectionConflict,
                AgentReason::RenameConflict,
            ]
            .into_iter()
            .map(ReductionReason::Agent),
        );
        reasons.extend(
            authority
                .iter()
                .copied()
                .map(ProjectReason::Authority)
                .map(ReductionReason::Project),
        );
        reasons.extend(
            [
                ProjectReason::SubjectMismatch,
                ProjectReason::ProjectAuthorityMismatch,
                ProjectReason::ProjectIdentityConflict,
                ProjectReason::HomeLinearFork,
                ProjectReason::StaleHead,
                ProjectReason::InvalidTransition,
                ProjectReason::ResourceInvariant,
                ProjectReason::ResourceClaimConflict,
                ProjectReason::AssignmentCardinalityConflict,
                ProjectReason::AssignmentBindingMismatch,
                ProjectReason::InputSequenceConflict,
                ProjectReason::DispatchConflict,
                ProjectReason::OutputConflict,
                ProjectReason::RemoteCommandConflict,
            ]
            .into_iter()
            .map(ReductionReason::Project),
        );

        for reason in reasons {
            let (code, parameter) = encode_reason(&reason);
            assert_eq!(decode_reason(code, parameter), Some(reason));
        }
    }

    #[test]
    fn closed_scalar_codecs_reject_values_outside_their_vocabularies() {
        for domain in ReductionDomain::ALL {
            assert_eq!(decode_domain(encode_domain(domain)), Some(domain));
        }
        for status in [
            DecisionStatus::Projected,
            DecisionStatus::Unresolved,
            DecisionStatus::Unauthorized,
            DecisionStatus::Conflicted,
            DecisionStatus::Invalid,
            DecisionStatus::Unsupported,
        ] {
            assert_eq!(decode_status(encode_status(status)), Some(status));
        }
        for role in AuthorityRole::ALL {
            assert_eq!(decode_role(encode_role(role)), Some(role));
        }
        assert_eq!(decode_domain(0), None);
        assert_eq!(decode_status(7), None);
        assert_eq!(decode_role(14), None);
        assert_eq!(decode_reason(1_004, 14), None);
        assert_eq!(decode_reason(2_101, 1), None);
    }
}
