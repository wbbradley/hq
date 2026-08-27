//! Representation-independent persisted authority projection view.

use std::collections::{BTreeMap, BTreeSet};

use hq_domain::FactId;
use hq_reducer::{
    AuthorityAggregateKey, AuthorityProjection, AuthorityProjectionKey, AuthorityReport,
};

/// Full rebuildable authority view, independent of its SQLite row layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityProjectionSnapshot {
    pub(crate) frontiers: BTreeMap<AuthorityAggregateKey, BTreeSet<FactId>>,
    pub(crate) projections: BTreeMap<AuthorityProjectionKey, AuthorityProjection>,
    pub(crate) support: BTreeMap<AuthorityProjectionKey, BTreeSet<FactId>>,
}

impl AuthorityProjectionSnapshot {
    pub(crate) fn from_report(report: &AuthorityReport) -> Self {
        Self {
            frontiers: report.frontiers().clone(),
            projections: report.projections().clone(),
            support: report.support().clone(),
        }
    }

    /// Returns every exact usable causal maximum by typed authority aggregate.
    pub const fn frontiers(&self) -> &BTreeMap<AuthorityAggregateKey, BTreeSet<FactId>> {
        &self.frontiers
    }

    /// Returns every typed authority projection.
    pub const fn projections(&self) -> &BTreeMap<AuthorityProjectionKey, AuthorityProjection> {
        &self.projections
    }

    /// Returns one typed authority projection.
    pub fn projection(&self, key: AuthorityProjectionKey) -> Option<&AuthorityProjection> {
        self.projections.get(&key)
    }

    /// Returns transitive usable support for every authority projection.
    pub const fn support(&self) -> &BTreeMap<AuthorityProjectionKey, BTreeSet<FactId>> {
        &self.support
    }
}
