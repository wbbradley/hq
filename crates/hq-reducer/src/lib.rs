//! Deterministic, complete-batch causal reduction without runtime or adapter dependencies.

mod authority;
mod decision;
mod fact_set;
mod graph;
mod presentation;
mod reducer;

pub use authority::{
    AuthorityAggregateKey, AuthorityPolicy, AuthorityProjection, AuthorityProjectionKey,
    AuthorityReason, AuthorityReducer, AuthorityReport, CapabilityView, DeviceGrantView,
    InstallationView, MailboxView, MembershipState, MembershipView, PeerRouteCandidate,
    PeerRouteState, PeerRouteView,
};
pub use decision::{DecisionReason, DecisionStatus, DomainDecision, FactDecision};
pub use fact_set::FactSet;
pub use graph::CausalGraph;
pub use presentation::{
    PresentationEntry, PresentationError, PresentationFamily, PresentationItemId, PresentationKey,
    PresentationPublicId, canonical_presentation_order,
};
pub use reducer::{
    ConflictObservation, ConflictReason, DomainReducer, DomainReductionReport, GraphOnlyReducer,
    GraphReductionReport, NoAggregateKey, NoDomainReason, NoProjection, ProjectionContribution,
    ReduceError, ReductionContext, ReductionReport, reduce_complete,
};
