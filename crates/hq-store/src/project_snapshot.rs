//! Representation-independent persisted project projection view.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::FactId;
use hq_reducer::{ProjectAggregateKey, ProjectProjection, ProjectProjectionKey, ProjectReport};

/// Full rebuildable project view, independent of its SQLite row layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectProjectionSnapshot {
    pub(crate) frontiers: BTreeMap<ProjectAggregateKey, BTreeSet<FactId>>,
    pub(crate) projections: BTreeMap<ProjectProjectionKey, ProjectProjection>,
    pub(crate) support: BTreeMap<ProjectProjectionKey, BTreeSet<FactId>>,
}

impl ProjectProjectionSnapshot {
    pub(crate) fn from_report(report: &ProjectReport) -> Self {
        Self {
            frontiers: report.frontiers().clone(),
            projections: report.projections().clone(),
            support: report.support().clone(),
        }
    }

    /// Returns every exact usable causal maximum by typed project aggregate.
    pub const fn frontiers(&self) -> &BTreeMap<ProjectAggregateKey, BTreeSet<FactId>> {
        &self.frontiers
    }

    /// Returns every typed project projection.
    pub const fn projections(&self) -> &BTreeMap<ProjectProjectionKey, ProjectProjection> {
        &self.projections
    }

    /// Returns one typed project projection.
    pub fn projection(&self, key: &ProjectProjectionKey) -> Option<&ProjectProjection> {
        self.projections.get(key)
    }

    /// Returns transitive usable support for every project projection.
    pub const fn support(&self) -> &BTreeMap<ProjectProjectionKey, BTreeSet<FactId>> {
        &self.support
    }
}
