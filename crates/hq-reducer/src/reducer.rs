use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
};

use hq_domain::{Fact, FactId};

use crate::{
    CausalGraph, DecisionReason, DecisionStatus, DomainDecision, FactDecision, FactSet,
    PresentationEntry, PresentationError, canonical_presentation_order,
};

/// One typed projection value with its direct semantic supporters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionContribution<K, V> {
    key: K,
    value: V,
    direct_support: BTreeSet<FactId>,
}

impl<K, V> ProjectionContribution<K, V> {
    /// Creates a projection contribution from an exact direct support set.
    pub fn new(key: K, value: V, direct_support: impl IntoIterator<Item = FactId>) -> Self {
        Self {
            key,
            value,
            direct_support: direct_support.into_iter().collect(),
        }
    }
}

/// Framework or domain reason for a normalized conflict observation.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConflictReason<R> {
    /// Unequal exact fact content carried one identity.
    IdentityCollision,
    /// Domain aggregate or global-cardinality policy found a conflict.
    Domain(R),
}

/// Explicit conflict and every normalized participant identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConflictObservation<R> {
    reason: ConflictReason<R>,
    participants: BTreeSet<FactId>,
}

impl<R> ConflictObservation<R> {
    /// Creates a normalized conflict observation.
    pub fn new(reason: ConflictReason<R>, participants: impl IntoIterator<Item = FactId>) -> Self {
        Self {
            reason,
            participants: participants.into_iter().collect(),
        }
    }

    /// Returns the closed conflict reason.
    pub const fn reason(&self) -> &ConflictReason<R> {
        &self.reason
    }

    /// Returns every conflict participant.
    pub const fn participants(&self) -> &BTreeSet<FactId> {
        &self.participants
    }
}

/// Read-only complete-set context supplied to a pure domain reducer.
pub struct ReductionContext<'a, R> {
    facts: &'a FactSet,
    graph: &'a CausalGraph,
    decisions: &'a BTreeMap<FactId, FactDecision<R>>,
}

impl<'a, R> ReductionContext<'a, R> {
    /// Returns the normalized complete fact set.
    pub const fn facts(&self) -> &'a FactSet {
        self.facts
    }

    /// Returns the complete structural graph.
    pub const fn graph(&self) -> &'a CausalGraph {
        self.graph
    }

    /// Returns the current deterministic fixed-point round snapshot.
    pub const fn decisions(&self) -> &'a BTreeMap<FactId, FactDecision<R>> {
        self.decisions
    }

    /// Reports whether a fact is usable in the current fixed-point round.
    pub fn is_projected(&self, fact_id: FactId) -> bool {
        self.decisions
            .get(&fact_id)
            .is_some_and(|decision| decision.status == DecisionStatus::Projected)
    }

    /// Tests usable reachability in the current fixed-point round.
    pub fn usably_reaches(&self, ancestor: FactId, descendant: FactId) -> bool {
        usably_reaches(self.graph, self.decisions, ancestor, descendant)
    }
}

/// Pure domain policy plugged into complete-batch causal reduction.
///
/// Classification is repeatedly evaluated from immutable complete-set snapshots until decisions
/// stabilize. Derivation hooks run only after that fixed point exists.
pub trait DomainReducer {
    /// Typed aggregate identity used for exact causal frontiers.
    type AggregateKey: Clone + fmt::Debug + Eq + Ord;
    /// Typed public projection identity.
    type ProjectionKey: Clone + fmt::Debug + Eq + Ord;
    /// Typed rebuildable projection value.
    type ProjectionValue: Clone + fmt::Debug + Eq;
    /// Closed domain reason enum.
    type Reason: Clone + fmt::Debug + Eq + Ord;

    /// Classifies a graph-ready fact using only explicit complete-set semantic inputs.
    fn classify(
        &self,
        fact: &Fact,
        context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason>;

    /// Returns every aggregate containing a fact. The default assigns none.
    fn aggregate_keys(
        &self,
        _fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<Self::AggregateKey> {
        Vec::new()
    }

    /// Derives typed public values and direct support after decisions stabilize.
    fn projections(
        &self,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ProjectionContribution<Self::ProjectionKey, Self::ProjectionValue>> {
        Vec::new()
    }

    /// Supplies additional normalized aggregate/global conflict observations.
    fn conflicts(
        &self,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<ConflictObservation<Self::Reason>> {
        Vec::new()
    }

    /// Selects projected entries for the canonical presentation traversal.
    fn presentation_entries(
        &self,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> Vec<PresentationEntry> {
        Vec::new()
    }
}

/// Empty aggregate key used by [`GraphOnlyReducer`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NoAggregateKey {}

/// Empty projection key and value used by [`GraphOnlyReducer`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NoProjection {}

/// Empty domain reason used by [`GraphOnlyReducer`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NoDomainReason {}

/// Permissive domain stage for graph-only reduction and boundary integration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GraphOnlyReducer;

impl DomainReducer for GraphOnlyReducer {
    type AggregateKey = NoAggregateKey;
    type ProjectionKey = NoProjection;
    type ProjectionValue = NoProjection;
    type Reason = NoDomainReason;

    fn classify(
        &self,
        _fact: &Fact,
        _context: &ReductionContext<'_, Self::Reason>,
    ) -> DomainDecision<Self::Reason> {
        DomainDecision::Projected
    }
}

/// Graph-only complete reduction report without domain projections.
pub type GraphReductionReport = DomainReductionReport<GraphOnlyReducer>;

/// Failure to produce a coherent normalized complete-batch report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReduceError {
    /// Domain classification did not reach a stable fixed point.
    NonConvergentDomainDecisions,
    /// One projection key was assigned unequal values.
    ProjectionValueConflict,
    /// A projection did not cite any supporting fact.
    EmptyProjectionSupport,
    /// Projection support cited a missing or unusable fact.
    InvalidProjectionSupport(FactId),
    /// Presentation selected a missing or unusable fact.
    InvalidPresentationFact(FactId),
    /// Canonical presentation traversal rejected its selected graph.
    Presentation(PresentationError),
}

impl fmt::Display for ReduceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonConvergentDomainDecisions => {
                formatter.write_str("domain decisions did not reach a fixed point")
            }
            Self::ProjectionValueConflict => {
                formatter.write_str("one projection key has unequal values")
            }
            Self::EmptyProjectionSupport => formatter.write_str("projection support is empty"),
            Self::InvalidProjectionSupport(_) => {
                formatter.write_str("projection support is absent or unusable")
            }
            Self::InvalidPresentationFact(_) => {
                formatter.write_str("presentation fact is absent or unusable")
            }
            Self::Presentation(error) => write!(formatter, "invalid presentation graph: {error}"),
        }
    }
}

impl Error for ReduceError {}

impl From<PresentationError> for ReduceError {
    fn from(value: PresentationError) -> Self {
        Self::Presentation(value)
    }
}

/// Representation-independent result of pure complete-batch reduction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReductionReport<A, K, V, R> {
    facts: FactSet,
    graph: CausalGraph,
    decisions: BTreeMap<FactId, FactDecision<R>>,
    dependency_order: Vec<FactId>,
    aggregate_members: BTreeMap<A, BTreeSet<FactId>>,
    frontiers: BTreeMap<A, BTreeSet<FactId>>,
    projections: BTreeMap<K, V>,
    support: BTreeMap<K, BTreeSet<FactId>>,
    conflicts: BTreeSet<ConflictObservation<R>>,
    presentation_order: Vec<FactId>,
}

/// Complete report type produced for a particular domain reducer implementation.
pub type DomainReductionReport<D> = ReductionReport<
    <D as DomainReducer>::AggregateKey,
    <D as DomainReducer>::ProjectionKey,
    <D as DomainReducer>::ProjectionValue,
    <D as DomainReducer>::Reason,
>;

impl<A, K, V, R> ReductionReport<A, K, V, R> {
    /// Returns the normalized immutable fact set.
    pub const fn facts(&self) -> &FactSet {
        &self.facts
    }

    /// Returns structural parent and reverse-dependency indexes.
    pub const fn graph(&self) -> &CausalGraph {
        &self.graph
    }

    /// Returns one normalized decision for every present identity.
    pub const fn decisions(&self) -> &BTreeMap<FactId, FactDecision<R>> {
        &self.decisions
    }

    /// Returns deterministic graph dependency order for known acyclic identities.
    pub fn dependency_order(&self) -> &[FactId] {
        &self.dependency_order
    }

    /// Returns every fact assigned to each typed aggregate, including unusable facts.
    pub const fn aggregate_members(&self) -> &BTreeMap<A, BTreeSet<FactId>> {
        &self.aggregate_members
    }

    /// Returns every exact usable causal maximum by typed aggregate.
    pub const fn frontiers(&self) -> &BTreeMap<A, BTreeSet<FactId>> {
        &self.frontiers
    }

    /// Returns normalized typed projections.
    pub const fn projections(&self) -> &BTreeMap<K, V> {
        &self.projections
    }

    /// Returns transitive usable fact support for each projection.
    pub const fn support(&self) -> &BTreeMap<K, BTreeSet<FactId>> {
        &self.support
    }

    /// Returns normalized framework and domain conflicts.
    pub const fn conflicts(&self) -> &BTreeSet<ConflictObservation<R>> {
        &self.conflicts
    }

    /// Returns the reducer-owned canonical presentation order.
    pub fn presentation_order(&self) -> &[FactId] {
        &self.presentation_order
    }

    /// Tests reachability restricted to facts whose decisions are projected.
    pub fn usably_reaches(&self, ancestor: FactId, descendant: FactId) -> bool {
        usably_reaches(&self.graph, &self.decisions, ancestor, descendant)
    }
}

/// Performs the authoritative pure complete-batch reduction for an explicit domain policy.
pub fn reduce_complete<D>(
    facts: impl IntoIterator<Item = Fact>,
    domain: &D,
) -> Result<DomainReductionReport<D>, ReduceError>
where
    D: DomainReducer,
{
    let facts = FactSet::from_facts(facts);
    let graph = facts.graph();
    let dependency_order = graph.dependency_order();
    let decisions = stabilize_decisions(&facts, &graph, domain)?;
    let context = ReductionContext {
        facts: &facts,
        graph: &graph,
        decisions: &decisions,
    };
    let aggregate_members = derive_aggregate_members(domain, &context);
    let frontiers = derive_frontiers(&aggregate_members, &context);
    let (projections, support) = derive_projections(domain, &context)?;
    let conflicts = derive_conflicts(domain, &context);
    let presentation_entries = domain.presentation_entries(&context);
    for entry in &presentation_entries {
        if !context.is_projected(entry.fact_id()) {
            return Err(ReduceError::InvalidPresentationFact(entry.fact_id()));
        }
    }
    let presentation_order = canonical_presentation_order(&graph, presentation_entries)?;

    Ok(ReductionReport {
        facts,
        graph,
        decisions,
        dependency_order,
        aggregate_members,
        frontiers,
        projections,
        support,
        conflicts,
        presentation_order,
    })
}

fn stabilize_decisions<D: DomainReducer>(
    facts: &FactSet,
    graph: &CausalGraph,
    domain: &D,
) -> Result<BTreeMap<FactId, FactDecision<D::Reason>>, ReduceError> {
    let mut decisions = facts
        .ids()
        .map(|fact_id| (fact_id, structural_decision(facts, graph, fact_id)))
        .collect::<BTreeMap<_, _>>();
    let maximum_rounds = decisions.len().saturating_mul(2).saturating_add(2);

    for _ in 0..maximum_rounds {
        let context = ReductionContext {
            facts,
            graph,
            decisions: &decisions,
        };
        let mut next = BTreeMap::new();
        for fact_id in facts.ids() {
            let structural = structural_decision(facts, graph, fact_id);
            let decision = if structural.status == DecisionStatus::Projected {
                let unusable_dependencies = graph
                    .parents(fact_id)
                    .iter()
                    .filter_map(|parent| {
                        context.decisions.get(parent).and_then(|decision| {
                            (decision.status != DecisionStatus::Projected)
                                .then_some((*parent, decision.status))
                        })
                    })
                    .collect::<BTreeMap<_, _>>();
                if unusable_dependencies.is_empty() {
                    let fact = facts
                        .get(fact_id)
                        .ok_or(ReduceError::NonConvergentDomainDecisions)?;
                    domain_decision(
                        domain.classify(fact, &context),
                        graph.children(fact_id).clone(),
                    )
                } else {
                    unresolved_decision(
                        BTreeSet::new(),
                        unusable_dependencies,
                        graph.children(fact_id).clone(),
                    )
                }
            } else {
                structural
            };
            next.insert(fact_id, decision);
        }
        if next == decisions {
            return Ok(next);
        }
        decisions = next;
    }
    Err(ReduceError::NonConvergentDomainDecisions)
}

fn structural_decision<R>(
    facts: &FactSet,
    graph: &CausalGraph,
    fact_id: FactId,
) -> FactDecision<R> {
    let reverse_dependants = graph.children(fact_id).clone();
    if facts.is_collision(fact_id) {
        return FactDecision {
            status: DecisionStatus::Conflicted,
            reason: Some(DecisionReason::IdentityCollision),
            missing_dependencies: BTreeSet::new(),
            unusable_dependencies: BTreeMap::new(),
            failed_authorities: BTreeSet::new(),
            conflict_participants: BTreeSet::from([fact_id]),
            reverse_dependants,
        };
    }
    if graph.cycle_members().contains(&fact_id) {
        return FactDecision {
            status: DecisionStatus::Invalid,
            reason: Some(DecisionReason::CausalCycle),
            missing_dependencies: BTreeSet::new(),
            unusable_dependencies: BTreeMap::new(),
            failed_authorities: BTreeSet::new(),
            conflict_participants: BTreeSet::new(),
            reverse_dependants,
        };
    }
    let missing = graph
        .parents(fact_id)
        .iter()
        .filter(|parent| !facts.contains(**parent))
        .copied()
        .collect::<BTreeSet<_>>();
    if missing.is_empty() {
        FactDecision {
            status: DecisionStatus::Projected,
            reason: None,
            missing_dependencies: BTreeSet::new(),
            unusable_dependencies: BTreeMap::new(),
            failed_authorities: BTreeSet::new(),
            conflict_participants: BTreeSet::new(),
            reverse_dependants,
        }
    } else {
        unresolved_decision(missing, BTreeMap::new(), reverse_dependants)
    }
}

fn unresolved_decision<R>(
    missing_dependencies: BTreeSet<FactId>,
    unusable_dependencies: BTreeMap<FactId, DecisionStatus>,
    reverse_dependants: BTreeSet<FactId>,
) -> FactDecision<R> {
    FactDecision {
        status: DecisionStatus::Unresolved,
        reason: None,
        missing_dependencies,
        unusable_dependencies,
        failed_authorities: BTreeSet::new(),
        conflict_participants: BTreeSet::new(),
        reverse_dependants,
    }
}

fn domain_decision<R>(
    decision: DomainDecision<R>,
    reverse_dependants: BTreeSet<FactId>,
) -> FactDecision<R> {
    let (status, reason, failed_authorities, conflict_participants) = match decision {
        DomainDecision::Projected => (
            DecisionStatus::Projected,
            None,
            BTreeSet::new(),
            BTreeSet::new(),
        ),
        DomainDecision::Unauthorized {
            reason,
            failed_authorities,
        } => (
            DecisionStatus::Unauthorized,
            Some(DecisionReason::Domain(reason)),
            failed_authorities,
            BTreeSet::new(),
        ),
        DomainDecision::Conflicted {
            reason,
            participants,
        } => (
            DecisionStatus::Conflicted,
            Some(DecisionReason::Domain(reason)),
            BTreeSet::new(),
            participants,
        ),
        DomainDecision::Invalid { reason } => (
            DecisionStatus::Invalid,
            Some(DecisionReason::Domain(reason)),
            BTreeSet::new(),
            BTreeSet::new(),
        ),
        DomainDecision::Unsupported { reason } => (
            DecisionStatus::Unsupported,
            Some(DecisionReason::Domain(reason)),
            BTreeSet::new(),
            BTreeSet::new(),
        ),
    };
    FactDecision {
        status,
        reason,
        missing_dependencies: BTreeSet::new(),
        unusable_dependencies: BTreeMap::new(),
        failed_authorities,
        conflict_participants,
        reverse_dependants,
    }
}

fn derive_aggregate_members<D: DomainReducer>(
    domain: &D,
    context: &ReductionContext<'_, D::Reason>,
) -> BTreeMap<D::AggregateKey, BTreeSet<FactId>> {
    let mut members = BTreeMap::<D::AggregateKey, BTreeSet<FactId>>::new();
    for fact in context.facts.facts() {
        for key in domain.aggregate_keys(fact, context) {
            members.entry(key).or_default().insert(fact.id());
        }
    }
    members
}

fn derive_frontiers<A>(
    aggregate_members: &BTreeMap<A, BTreeSet<FactId>>,
    context: &ReductionContext<'_, impl Sized>,
) -> BTreeMap<A, BTreeSet<FactId>>
where
    A: Clone + Ord,
{
    let members = aggregate_members
        .iter()
        .map(|(key, facts)| {
            (
                key.clone(),
                facts
                    .iter()
                    .copied()
                    .filter(|fact_id| context.is_projected(*fact_id))
                    .collect::<BTreeSet<_>>(),
            )
        })
        .filter(|(_, facts)| !facts.is_empty())
        .collect::<BTreeMap<_, _>>();
    members
        .into_iter()
        .map(|(key, candidates)| {
            let maxima = candidates
                .iter()
                .copied()
                .filter(|candidate| {
                    !candidates.iter().copied().any(|other| {
                        other != *candidate && context.usably_reaches(*candidate, other)
                    })
                })
                .collect();
            (key, maxima)
        })
        .collect()
}

type ProjectionMaps<K, V> = (BTreeMap<K, V>, BTreeMap<K, BTreeSet<FactId>>);

fn derive_projections<D: DomainReducer>(
    domain: &D,
    context: &ReductionContext<'_, D::Reason>,
) -> Result<ProjectionMaps<D::ProjectionKey, D::ProjectionValue>, ReduceError> {
    let mut projections = BTreeMap::new();
    let mut direct_support = BTreeMap::<D::ProjectionKey, BTreeSet<FactId>>::new();
    for contribution in domain.projections(context) {
        if contribution.direct_support.is_empty() {
            return Err(ReduceError::EmptyProjectionSupport);
        }
        if let Some(existing) = projections.get(&contribution.key) {
            if existing != &contribution.value {
                return Err(ReduceError::ProjectionValueConflict);
            }
        } else {
            projections.insert(contribution.key.clone(), contribution.value);
        }
        direct_support
            .entry(contribution.key)
            .or_default()
            .extend(contribution.direct_support);
    }

    let mut support = BTreeMap::new();
    for (key, direct) in direct_support {
        let mut transitive = BTreeSet::new();
        let mut pending = direct.into_iter().collect::<VecDeque<_>>();
        while let Some(fact_id) = pending.pop_front() {
            if !context.is_projected(fact_id) {
                return Err(ReduceError::InvalidProjectionSupport(fact_id));
            }
            if transitive.insert(fact_id) {
                pending.extend(context.graph.parents(fact_id).iter().copied());
            }
        }
        support.insert(key, transitive);
    }
    Ok((projections, support))
}

fn derive_conflicts<D: DomainReducer>(
    domain: &D,
    context: &ReductionContext<'_, D::Reason>,
) -> BTreeSet<ConflictObservation<D::Reason>> {
    let mut conflicts = context
        .facts
        .collisions()
        .into_iter()
        .map(|fact_id| ConflictObservation::new(ConflictReason::IdentityCollision, [fact_id]))
        .collect::<BTreeSet<_>>();
    conflicts.extend(context.decisions.values().filter_map(|decision| {
        match (&decision.reason, decision.status) {
            (Some(DecisionReason::Domain(reason)), DecisionStatus::Conflicted) => {
                Some(ConflictObservation::new(
                    ConflictReason::Domain(reason.clone()),
                    decision.conflict_participants.iter().copied(),
                ))
            }
            _ => None,
        }
    }));
    conflicts.extend(domain.conflicts(context));
    conflicts
}

fn usably_reaches<R>(
    graph: &CausalGraph,
    decisions: &BTreeMap<FactId, FactDecision<R>>,
    ancestor: FactId,
    descendant: FactId,
) -> bool {
    let is_usable = |fact_id| {
        decisions
            .get(&fact_id)
            .is_some_and(|decision| decision.status == DecisionStatus::Projected)
    };
    if !is_usable(ancestor) || !is_usable(descendant) {
        return false;
    }
    if ancestor == descendant {
        return true;
    }
    let mut visited = BTreeSet::new();
    let mut pending = graph
        .children(ancestor)
        .iter()
        .copied()
        .filter(|candidate| is_usable(*candidate))
        .collect::<VecDeque<_>>();
    while let Some(candidate) = pending.pop_front() {
        if candidate == descendant {
            return true;
        }
        if visited.insert(candidate) {
            pending.extend(
                graph
                    .children(candidate)
                    .iter()
                    .copied()
                    .filter(|child| is_usable(*child)),
            );
        }
    }
    false
}
