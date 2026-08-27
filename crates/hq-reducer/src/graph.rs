use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hq_domain::FactId;

use crate::FactSet;

/// Deterministic parent and reverse-dependency indexes over present and missing vertices.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CausalGraph {
    known: BTreeSet<FactId>,
    vertices: BTreeSet<FactId>,
    parents: BTreeMap<FactId, BTreeSet<FactId>>,
    children: BTreeMap<FactId, BTreeSet<FactId>>,
    cycle_members: BTreeSet<FactId>,
}

impl CausalGraph {
    pub(crate) fn from_fact_set(facts: &FactSet) -> Self {
        let known = facts.ids().collect::<BTreeSet<_>>();
        let mut vertices = known.clone();
        let mut parents = known
            .iter()
            .copied()
            .map(|fact_id| (fact_id, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        let mut children = parents.clone();

        for fact in facts.facts() {
            let fact_id = fact.id();
            for parent in fact.causal().parents().iter().copied() {
                vertices.insert(parent);
                parents.entry(parent).or_default();
                children.entry(parent).or_default().insert(fact_id);
                parents.entry(fact_id).or_default().insert(parent);
            }
        }
        for vertex in &vertices {
            parents.entry(*vertex).or_default();
            children.entry(*vertex).or_default();
        }

        let mut graph = Self {
            known,
            vertices,
            parents,
            children,
            cycle_members: BTreeSet::new(),
        };
        graph.cycle_members = graph.detect_cycle_members();
        graph
    }

    /// Returns all graph vertices, including absent required parent identities.
    pub const fn vertices(&self) -> &BTreeSet<FactId> {
        &self.vertices
    }

    /// Reports whether an identity is present in the fact set.
    pub fn is_known(&self, fact_id: FactId) -> bool {
        self.known.contains(&fact_id)
    }

    /// Returns the declared parents for a vertex.
    pub fn parents(&self, fact_id: FactId) -> &BTreeSet<FactId> {
        match self.parents.get(&fact_id) {
            Some(parents) => parents,
            None => empty_fact_ids(),
        }
    }

    /// Returns the direct reverse dependants for a vertex.
    pub fn children(&self, fact_id: FactId) -> &BTreeSet<FactId> {
        match self.children.get(&fact_id) {
            Some(children) => children,
            None => empty_fact_ids(),
        }
    }

    /// Returns exactly the present vertices that participate in a directed cycle.
    pub const fn cycle_members(&self) -> &BTreeSet<FactId> {
        &self.cycle_members
    }

    /// Tests reflexive structural reachability through declared parent edges.
    pub fn structurally_reaches(&self, ancestor: FactId, descendant: FactId) -> bool {
        if !self.vertices.contains(&ancestor) || !self.vertices.contains(&descendant) {
            return false;
        }
        if ancestor == descendant {
            return true;
        }
        self.reaches_non_reflexive(ancestor, descendant)
    }

    /// Returns roots plus every transitively reverse-dependent known fact.
    pub fn reverse_dependant_closure(
        &self,
        roots: impl IntoIterator<Item = FactId>,
    ) -> BTreeSet<FactId> {
        let mut closure = BTreeSet::new();
        let mut pending = roots.into_iter().collect::<VecDeque<_>>();
        while let Some(fact_id) = pending.pop_front() {
            if closure.insert(fact_id) {
                pending.extend(
                    self.children(fact_id)
                        .iter()
                        .copied()
                        .filter(|child| self.known.contains(child)),
                );
            }
        }
        closure
    }

    /// Returns a deterministic dependency order for known acyclic vertices.
    ///
    /// Cycle members are omitted. Edges from missing or cyclic parents do not prevent their known
    /// dependants from appearing; the reduction decision still records those blockers.
    pub fn dependency_order(&self) -> Vec<FactId> {
        let eligible = self
            .known
            .difference(&self.cycle_members)
            .copied()
            .collect::<BTreeSet<_>>();
        let mut indegree = eligible
            .iter()
            .copied()
            .map(|fact_id| {
                let count = self
                    .parents(fact_id)
                    .iter()
                    .filter(|parent| eligible.contains(parent))
                    .count();
                (fact_id, count)
            })
            .collect::<BTreeMap<_, _>>();
        let mut ready = indegree
            .iter()
            .filter_map(|(fact_id, degree)| (*degree == 0).then_some(*fact_id))
            .collect::<BTreeSet<_>>();
        let mut ordered = Vec::with_capacity(eligible.len());
        while let Some(fact_id) = ready.pop_first() {
            ordered.push(fact_id);
            for child in self.children(fact_id).intersection(&eligible) {
                if let Some(degree) = indegree.get_mut(child) {
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(*child);
                    }
                }
            }
        }
        ordered
    }

    fn detect_cycle_members(&self) -> BTreeSet<FactId> {
        self.known
            .iter()
            .copied()
            .filter(|candidate| self.reaches_non_reflexive(*candidate, *candidate))
            .collect()
    }

    fn reaches_non_reflexive(&self, ancestor: FactId, descendant: FactId) -> bool {
        let mut visited = BTreeSet::new();
        let mut pending = self
            .children(ancestor)
            .iter()
            .copied()
            .collect::<VecDeque<_>>();
        while let Some(candidate) = pending.pop_front() {
            if candidate == descendant {
                return true;
            }
            if visited.insert(candidate) {
                pending.extend(self.children(candidate).iter().copied());
            }
        }
        false
    }
}

fn empty_fact_ids() -> &'static BTreeSet<FactId> {
    static EMPTY: std::sync::OnceLock<BTreeSet<FactId>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeSet::new)
}
