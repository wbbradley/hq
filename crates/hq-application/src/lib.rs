//! Application use cases and inward-facing ports.

mod error;
mod mutation;
mod ports;
mod project;
mod service;
mod snapshot;

use hq_domain::Fact;
use hq_reducer::{GraphOnlyReducer, GraphReductionReport, ReduceError, reduce_complete};

pub use hq_reducer::ConversationKey;

pub use error::{
    ApplicationError, ApplicationErrorClass, ApplicationErrorCode, ApplicationValueError,
};
pub use mutation::{
    FactMutation, FactPlan, MAX_ENCODED_MUTATION_RESULT_BYTES, MutationAttempt, MutationDecision,
    MutationDecisionCallback, MutationDomain, MutationOutcome, MutationReceipt,
    decode_mutation_outcome, encode_mutation_outcome,
};
pub use ports::{
    AgentSessionRequest, AgentSessionResult, ApplicationPorts, CommitFacts, ConfigureRelays,
    ControlHarness, EffectOutcome, EffectRequest, InspectResource, MAX_SUBSCRIPTION_TOPICS,
    ObserveRevisions, PublishWake, QueryDomain, RelayAccess, RelayAuthentication,
    RelayConfiguration, ResourceInspectionRequest, ResourceInspectionResult, SessionControl,
    SubscriptionRequest, SubscriptionTopic, SynchronizationRequest, WakeDisposition,
};
pub use project::{
    ControlProjects, ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest,
    ProjectCommandStage, WorktreeProvisioningRequest,
};
pub use service::{Application, MutationCompletion, PreparedSubscription};
pub use snapshot::{
    AgentProjectionSnapshot, AuthoritativeSnapshot, AuthorityProjectionSnapshot,
    ClientAgentLifecycle, ClientMembershipState, ClientPeerRouteState, ClientProjectLifecycle,
    ClientProjectOutputStatus, ClientProjection, ClientRemoteCommandStage, ConversationEntry,
    ConversationProjectionSnapshot, ConversationSummary, DomainSnapshot, ProjectProjectionSnapshot,
    ProjectionSnapshot,
};

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
