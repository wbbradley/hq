use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use hq_domain::{Fact, FactId};

use crate::CausalGraph;

#[derive(Clone, Debug, Eq, PartialEq)]
enum StoredFact {
    Unique(Box<Fact>),
    Collision,
}

/// Immutable set of exact semantic facts, normalized by content identity.
///
/// Repeated equal facts collapse to one entry. Unequal facts carrying one identity produce an
/// absorbing collision entry, so merging sets remains commutative, associative, and idempotent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FactSet {
    entries: BTreeMap<FactId, StoredFact>,
}

impl FactSet {
    /// Builds a normalized fact set from any arrival order.
    pub fn from_facts(facts: impl IntoIterator<Item = Fact>) -> Self {
        let mut set = Self::default();
        for fact in facts {
            set.insert(fact);
        }
        set
    }

    /// Returns the set union, preserving any identity collision observed by either side.
    #[must_use]
    pub fn merge(&self, other: &Self) -> Self {
        let mut merged = self.clone();
        for (fact_id, stored) in &other.entries {
            match stored {
                StoredFact::Unique(fact) => merged.insert(fact.as_ref().clone()),
                StoredFact::Collision => {
                    merged.entries.insert(*fact_id, StoredFact::Collision);
                }
            }
        }
        merged
    }

    /// Returns the unique fact for an identity, excluding collided identities.
    pub fn get(&self, fact_id: FactId) -> Option<&Fact> {
        match self.entries.get(&fact_id) {
            Some(StoredFact::Unique(fact)) => Some(fact.as_ref()),
            Some(StoredFact::Collision) | None => None,
        }
    }

    /// Reports whether an identity is present, including an identity collision.
    pub fn contains(&self, fact_id: FactId) -> bool {
        self.entries.contains_key(&fact_id)
    }

    /// Iterates over all present identities in normalized order.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = FactId> + '_ {
        self.entries.keys().copied()
    }

    /// Iterates over unique, non-collided facts in normalized identity order.
    pub fn facts(&self) -> impl Iterator<Item = &Fact> {
        self.entries.values().filter_map(|stored| match stored {
            StoredFact::Unique(fact) => Some(fact.as_ref()),
            StoredFact::Collision => None,
        })
    }

    /// Returns every identity with unequal observed content.
    pub fn collisions(&self) -> BTreeSet<FactId> {
        self.entries
            .iter()
            .filter_map(|(fact_id, stored)| {
                matches!(stored, StoredFact::Collision).then_some(*fact_id)
            })
            .collect()
    }

    /// Reports whether unequal content was observed for an identity.
    pub fn is_collision(&self, fact_id: FactId) -> bool {
        matches!(self.entries.get(&fact_id), Some(StoredFact::Collision))
    }

    /// Builds the normalized causal graph for this set.
    pub fn graph(&self) -> CausalGraph {
        CausalGraph::from_fact_set(self)
    }

    fn insert(&mut self, fact: Fact) {
        match self.entries.entry(fact.id()) {
            Entry::Vacant(entry) => {
                entry.insert(StoredFact::Unique(Box::new(fact)));
            }
            Entry::Occupied(mut entry) => match entry.get() {
                StoredFact::Unique(existing) if existing.as_ref() == &fact => {}
                StoredFact::Unique(_) => {
                    entry.insert(StoredFact::Collision);
                }
                StoredFact::Collision => {}
            },
        }
    }
}

impl FromIterator<Fact> for FactSet {
    fn from_iter<T: IntoIterator<Item = Fact>>(iter: T) -> Self {
        Self::from_facts(iter)
    }
}
