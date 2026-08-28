//! Application use cases and inward-facing ports.

mod authority_admin;
mod error;
mod human;
mod mutation;
mod ports;
mod project;
mod service;
mod snapshot;

use hq_domain::Fact;
use hq_reducer::{GraphOnlyReducer, GraphReductionReport, ReduceError, reduce_complete};

pub use hq_reducer::ConversationKey;

pub use authority_admin::{
    MailboxGrantRequest, MailboxRevokeRequest, PeerRouteRequest, plan_mailbox_grant,
    plan_mailbox_revoke, plan_peer_route_block, plan_peer_route_set,
};
pub use error::{
    ApplicationError, ApplicationErrorClass, ApplicationErrorCode, ApplicationValueError,
};
pub use human::{
    HumanDeviceGrantRequest, HumanDeviceRevokeRequest, LocalFactInputs, LocalInstallationAuthority,
    plan_human_account_creation, plan_human_account_selection, plan_human_device_acceptance,
    plan_human_device_grant, plan_human_device_revoke, plan_human_mailbox_creation,
};
pub use mutation::{
    FactMutation, FactPlan, MAX_ENCODED_MUTATION_RESULT_BYTES, MutationAttempt, MutationDecision,
    MutationDecisionCallback, MutationDomain, MutationOutcome, MutationReceipt,
    decode_mutation_outcome, encode_mutation_outcome,
};
pub use ports::{
    AgentSessionRequest, AgentSessionResult, ApplicationPorts, CanonicalEvidence, CommitFacts,
    ConfigureRelays, ControlHarness, EffectOutcome, EffectRequest, EvidenceIngestOutcome,
    InspectResource, MAX_SUBSCRIPTION_TOPICS, ObserveRevisions, PublishWake, QueryDomain,
    RelayAccess, RelayAuthentication, RelayConfiguration, ResourceInspectionRequest,
    ResourceInspectionResult, SessionControl, SubscriptionRequest, SubscriptionTopic,
    SynchronizationRequest, WakeDisposition,
};
pub use project::{
    ControlProjects, ProjectCommandAction, ProjectCommandOutcome, ProjectCommandRequest,
    ProjectCommandStage, WorktreeProvisioningRequest,
};
pub use service::{Application, MutationCompletion, PreparedSubscription};
pub use snapshot::{
    AgentProjectionSnapshot, AuthoritativeSnapshot, AuthorityProjectionSnapshot,
    ClientAgentLifecycle, ClientDeviceGrant, ClientMembershipState, ClientPeerRouteBlock,
    ClientPeerRouteCandidate, ClientPeerRouteState, ClientProjectLifecycle,
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
