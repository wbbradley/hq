//! Deterministic causal reduction without runtime or adapter dependencies.

use std::collections::BTreeSet;

use hq_domain::{Fact, FactId};

/// Minimal deterministic projection proving the pure reducer boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FactSummary {
    ordered_fact_ids: Vec<FactId>,
}

impl FactSummary {
    /// Returns the number of unique fact identities in the input.
    pub fn unique_fact_count(&self) -> usize {
        self.ordered_fact_ids.len()
    }

    /// Returns the skeleton fact identities in deterministic order.
    pub fn ordered_fact_ids(&self) -> Vec<u64> {
        self.ordered_fact_ids
            .iter()
            .copied()
            .map(FactId::value)
            .collect()
    }
}

/// Builds the in-memory skeleton projection without I/O or ambient inputs.
pub fn summarize(facts: &[Fact]) -> FactSummary {
    let ordered_fact_ids = facts
        .iter()
        .map(Fact::id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    FactSummary { ordered_fact_ids }
}

#[cfg(test)]
mod tests {
    use hq_domain::{Fact, FactId};

    use super::summarize;

    #[test]
    fn summary_is_deterministic_and_deduplicated() {
        let facts = [
            Fact::new(FactId::new(2), "second"),
            Fact::new(FactId::new(1), "first"),
            Fact::new(FactId::new(2), "second"),
        ];

        assert_eq!(summarize(&facts).ordered_fact_ids(), [1, 2]);
    }
}
