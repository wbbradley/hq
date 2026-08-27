//! Deterministic, complete-batch causal reduction without runtime or adapter dependencies.

mod agent;
mod authority;
mod conversation;
mod decision;
mod fact_set;
mod graph;
mod presentation;
mod project;
mod reducer;

pub use agent::{
    AgentAggregateKey, AgentLifecycle, AgentProjection, AgentProjectionKey, AgentReason,
    AgentReducer, AgentReport, AgentView, ContextHistoryView, DirectSessionView, NameClaimSubject,
    NameReservationView, RenameView, SelectionCandidate, SelectionView, SessionBindingView,
    SessionIdentity,
};
pub use authority::{
    AuthorityAggregateKey, AuthorityPolicy, AuthorityProjection, AuthorityProjectionKey,
    AuthorityReason, AuthorityReducer, AuthorityReport, CapabilityView, DeviceGrantView,
    InstallationView, MailboxView, MembershipState, MembershipView, PeerRouteCandidate,
    PeerRouteState, PeerRouteView,
};
pub use conversation::{
    ActionGroupView, ActivityKey, ActivityRetentionView, ActivitySessionKey, ActivityView,
    CausalRelation, ConversationAggregateKey, ConversationProjection, ConversationProjectionKey,
    ConversationReason, ConversationReducer, ConversationReport, IncompleteMessageObservation,
    MessageView, ThreadView, incomplete_addressed_observations,
};
pub use decision::{DecisionReason, DecisionStatus, DomainDecision, FactDecision};
pub use fact_set::FactSet;
pub use graph::CausalGraph;
pub use presentation::{
    PresentationEntry, PresentationError, PresentationFamily, PresentationItemId, PresentationKey,
    PresentationPublicId, canonical_presentation_order,
};
pub use project::{
    PathResourcePolicy, ProjectAggregateKey, ProjectAssignmentPhase, ProjectAssignmentView,
    ProjectDispatchView, ProjectInputView, ProjectLifecycle, ProjectOutputStatus,
    ProjectOutputView, ProjectProjection, ProjectProjectionKey, ProjectReason, ProjectReducer,
    ProjectReport, ProjectView, RemoteCommandStage, RemoteCommandView, ResourceConflictPolicy,
};
pub use reducer::{
    ConflictObservation, ConflictReason, DomainReducer, DomainReductionReport, GraphOnlyReducer,
    GraphReductionReport, NoAggregateKey, NoDomainReason, NoProjection, ProjectionContribution,
    ReduceError, ReductionContext, ReductionReport, reduce_complete,
};
