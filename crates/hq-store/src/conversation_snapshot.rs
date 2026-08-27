//! Representation-independent persisted conversation and activity projection view.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::FactId;
use hq_reducer::{
    ConversationAggregateKey, ConversationProjection, ConversationProjectionKey, ConversationReport,
};

/// Full rebuildable conversation/activity view, independent of its SQLite row layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationProjectionSnapshot {
    pub(crate) frontiers: BTreeMap<ConversationAggregateKey, BTreeSet<FactId>>,
    pub(crate) projections: BTreeMap<ConversationProjectionKey, ConversationProjection>,
    pub(crate) support: BTreeMap<ConversationProjectionKey, BTreeSet<FactId>>,
}

impl ConversationProjectionSnapshot {
    pub(crate) fn from_report(report: &ConversationReport) -> Self {
        Self {
            frontiers: report.frontiers().clone(),
            projections: report.projections().clone(),
            support: report.support().clone(),
        }
    }

    /// Returns every exact usable causal maximum by typed conversation aggregate.
    pub const fn frontiers(&self) -> &BTreeMap<ConversationAggregateKey, BTreeSet<FactId>> {
        &self.frontiers
    }

    /// Returns every typed conversation or activity projection.
    pub const fn projections(
        &self,
    ) -> &BTreeMap<ConversationProjectionKey, ConversationProjection> {
        &self.projections
    }

    /// Returns one typed conversation or activity projection.
    pub fn projection(&self, key: &ConversationProjectionKey) -> Option<&ConversationProjection> {
        self.projections.get(key)
    }

    /// Returns transitive usable support for every conversation projection.
    pub const fn support(&self) -> &BTreeMap<ConversationProjectionKey, BTreeSet<FactId>> {
        &self.support
    }
}
