//! Application use cases and inward-facing ports.

mod agent_admin;
mod authority_admin;
mod error;
mod harness;
mod human;
mod interactions;
mod mailbox;
mod messaging;
mod mutation;
mod ports;
mod project;
mod service;
mod snapshot;

use hq_domain::Fact;
use hq_reducer::{GraphOnlyReducer, GraphReductionReport, ReduceError, reduce_complete};

pub use hq_reducer::ConversationKey;

pub use agent_admin::{
    AgentNameClaimRequest, AgentRetirementPlanRequest, AgentSessionBindingRequest,
    AgentSessionContextRequest, AgentSessionRenameRequest, AgentSessionSelectionRequest,
    plan_agent_mailbox_creation, plan_agent_name_claim, plan_agent_retirement,
    plan_agent_session_binding, plan_agent_session_context, plan_agent_session_rename,
    plan_agent_session_selection,
};
pub use authority_admin::{
    MailboxGrantRequest, MailboxRevokeRequest, PeerRouteRequest, plan_mailbox_grant,
    plan_mailbox_revoke, plan_peer_route_block, plan_peer_route_set,
};
pub use error::{
    ApplicationError, ApplicationErrorClass, ApplicationErrorCode, ApplicationValueError,
};
pub use harness::{
    HarnessActivityFactRequest, HarnessAuthoringAuthority, HarnessOutputFactRequest,
    ProjectHarnessAuthoringAuthority, plan_harness_activity, plan_harness_output,
    plan_project_harness_activity, plan_project_harness_output,
};
pub use human::{
    HumanDeviceGrantRequest, HumanDeviceRevokeRequest, LocalFactInputs, LocalInstallationAuthority,
    plan_human_account_creation, plan_human_account_selection, plan_human_device_acceptance,
    plan_human_device_grant, plan_human_device_revoke, plan_human_mailbox_creation,
};
pub use interactions::{
    ControlInteractions, InteractionAnswerOutcome, InteractionAnswerRequest, InteractionChoice,
    InteractionId, InteractionKind, InteractionResponderLease, InteractionResponse,
    MAX_INTERACTION_CHOICES, MAX_PENDING_INTERACTIONS, PendingInteraction, QueryInteractions,
};
pub use mailbox::{
    MAX_MAILBOX_DRAFTS, MailboxCommandAction, MailboxCommandRequest, MailboxDraft,
    MailboxDraftDeleteOutcome, MailboxDraftDeleteRequest, MailboxDraftSaveOutcome,
    MailboxDraftSaveRequest, MailboxDraftTarget, plan_mailbox_command,
};
pub use messaging::{
    ContinueProjectMessageRequest, MessageAuthoringAuthority, MessageStateRequest,
    NewMessageRequest, ReplyRequest, ThreadCancellationRequest, plan_asynchronous_message,
    plan_message_archive, plan_message_restore, plan_project_message_continuation, plan_question,
    plan_reply, plan_thread_cancellation,
};
pub use mutation::{
    FactMutation, FactPlan, MAX_ENCODED_MUTATION_RESULT_BYTES, MutationAttempt, MutationDecision,
    MutationDecisionCallback, MutationDomain, MutationOutcome, MutationReceipt,
    decode_mutation_outcome, encode_mutation_outcome,
};
pub use ports::{
    AgentLaunchContext, AgentSessionRequest, AgentSessionResult, ApplicationPorts,
    CanonicalEvidence, CommitFacts, ConfigureRelays, ControlHarness, ControlMailbox, DomainHealth,
    EffectOutcome, EffectRequest, EvidenceIngestOutcome, HealthDomain, InspectResource,
    LaunchEnvironment, MAX_LAUNCH_ENVIRONMENT_BYTES, MAX_LAUNCH_ENVIRONMENT_ENTRIES,
    MAX_LAUNCH_ENVIRONMENT_NAME_BYTES, MAX_LAUNCH_ENVIRONMENT_VALUE_BYTES,
    MAX_PROVIDER_CATALOG_ITEMS, MAX_RELAY_STATUS_POLICIES, MAX_SUBSCRIPTION_TOPICS,
    ObserveRevisions, ProviderAvailability, ProviderCatalog, PublishWake, QueryDomain,
    QueryProviders, RelayAccess, RelayAuthentication, RelayConfiguration, RelayPolicyStatus,
    RelayStatus, ResourceCondition, ResourceInspectionRequest, ResourceInspectionResult,
    ResourceReleaseState, SessionControl, StateHealth, StateRepairReport, SubscriptionRequest,
    SubscriptionTopic, SynchronizationRequest, WakeDisposition,
};
pub use project::{
    AgentRetirementOutcome, AgentRetirementRequest, ControlProjects, ProjectCommandAction,
    ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage, ProjectCreationRequest,
    RetireAgents, WorktreeProvisioningRequest,
};
pub use service::{Application, MutationCompletion, PreparedSubscription};
pub use snapshot::{
    AgentProjectionSnapshot, AuthoritativeConversationView, AuthoritativeSnapshot,
    AuthorityProjectionSnapshot, ClientAgentLifecycle, ClientDeviceGrant, ClientMembershipState,
    ClientPeerRouteBlock, ClientPeerRouteCandidate, ClientPeerRouteState, ClientProjectAssignment,
    ClientProjectAssignmentPhase, ClientProjectLifecycle, ClientProjectOutputStatus,
    ClientProjectThread, ClientProjection, ClientRemoteCommandStage, ConversationContext,
    ConversationEntry, ConversationMessageEntry, ConversationPageSelection,
    ConversationParticipant, ConversationProjectionSnapshot, ConversationSummary, DomainSnapshot,
    IncompleteMessageSummary, MAX_CONVERSATION_PAGE_ITEMS, ProjectProjectionSnapshot,
    ProjectionSnapshot, SelectedConversationPage,
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
