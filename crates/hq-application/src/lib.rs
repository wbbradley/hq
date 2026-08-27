//! Application use cases and inward-facing ports.

use hq_domain::Fact;
use hq_reducer::{GraphOnlyReducer, GraphReductionReport, ReduceError, reduce_complete};

/// Minimal in-memory use-case host for the workspace walking skeleton.
#[derive(Clone, Debug, Default)]
pub struct InMemoryApplication {
    facts: Vec<Fact>,
}

impl InMemoryApplication {
    /// Accepts a verified domain fact through the application boundary.
    pub fn submit(&mut self, fact: Fact) {
        self.facts.push(fact);
    }

    /// Reduces all accepted facts through the pure complete-batch causal kernel.
    pub fn summary(&self) -> Result<GraphReductionReport, ReduceError> {
        reduce_complete(self.facts.clone(), &GraphOnlyReducer)
    }
}
