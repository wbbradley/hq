//! Representation-independent persisted named-agent projection view.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::FactId;
use hq_reducer::{AgentAggregateKey, AgentProjection, AgentProjectionKey, AgentReport};

/// Full rebuildable named-agent view, independent of its SQLite row layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProjectionSnapshot {
    pub(crate) frontiers: BTreeMap<AgentAggregateKey, BTreeSet<FactId>>,
    pub(crate) projections: BTreeMap<AgentProjectionKey, AgentProjection>,
    pub(crate) support: BTreeMap<AgentProjectionKey, BTreeSet<FactId>>,
}

impl AgentProjectionSnapshot {
    pub(crate) fn from_report(report: &AgentReport) -> Self {
        Self {
            frontiers: report.frontiers().clone(),
            projections: report.projections().clone(),
            support: report.support().clone(),
        }
    }

    /// Returns every exact usable causal maximum by typed agent aggregate.
    pub const fn frontiers(&self) -> &BTreeMap<AgentAggregateKey, BTreeSet<FactId>> {
        &self.frontiers
    }

    /// Returns every typed named-agent projection.
    pub const fn projections(&self) -> &BTreeMap<AgentProjectionKey, AgentProjection> {
        &self.projections
    }

    /// Returns one typed named-agent projection.
    pub fn projection(&self, key: &AgentProjectionKey) -> Option<&AgentProjection> {
        self.projections.get(key)
    }

    /// Returns transitive usable support for every named-agent projection.
    pub const fn support(&self) -> &BTreeMap<AgentProjectionKey, BTreeSet<FactId>> {
        &self.support
    }
}
