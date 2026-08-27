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
    pub fn ordered_fact_ids(&self) -> &[FactId] {
        &self.ordered_fact_ids
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
    use hq_domain::{BoundedText, Fact, FactId, SKELETON_PAYLOAD_MAX_BYTES, ValidatedValueError};

    use super::summarize;

    #[test]
    fn summary_is_deterministic_and_deduplicated() -> Result<(), ValidatedValueError> {
        let fact_id = |value| {
            let mut bytes = [0; 32];
            bytes[31] = value;
            FactId::from_bytes(bytes)
        };
        let fact = |value, payload| {
            BoundedText::<SKELETON_PAYLOAD_MAX_BYTES>::new(payload)
                .map(|payload| Fact::new(fact_id(value), payload))
        };
        let facts = [fact(2, "second")?, fact(1, "first")?, fact(2, "second")?];

        assert_eq!(
            summarize(&facts).ordered_fact_ids(),
            [fact_id(1), fact_id(2)]
        );
        Ok(())
    }
}
