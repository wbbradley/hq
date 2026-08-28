//! Stateless orchestration over transport-independent application ports.

use std::collections::BTreeSet;

use hq_domain::{FactId, OperationId, Page, PageCursor};
use hq_reducer::ConversationKey;

use crate::{
    AgentRetirementOutcome, AgentRetirementRequest, AgentSessionRequest, AgentSessionResult,
    ApplicationError, ApplicationPorts, AuthoritativeSnapshot, CanonicalEvidence,
    ConversationEntry, EffectOutcome, EffectRequest, EvidenceIngestOutcome, FactMutation,
    MutationAttempt, MutationOutcome, ProjectCommandOutcome, ProjectCommandRequest,
    RelayConfiguration, RelayStatus, ResourceInspectionRequest, ResourceInspectionResult,
    StateHealth, StateRepairReport, SubscriptionRequest, SynchronizationRequest, WakeDisposition,
};

/// Durable mutation attempt plus separate post-commit scheduling evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationCompletion {
    attempt: MutationAttempt,
    wake: Option<Result<WakeDisposition, ApplicationError>>,
}

impl MutationCompletion {
    /// Returns the authoritative completed or uncertain mutation attempt.
    pub const fn attempt(&self) -> &MutationAttempt {
        &self.attempt
    }

    /// Returns post-commit relay scheduling, only for a committed receipt.
    pub const fn wake(&self) -> Option<&Result<WakeDisposition, ApplicationError>> {
        self.wake.as_ref()
    }
}

/// A pending observer paired with the snapshot that its eventual acknowledgement names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSubscription {
    request: SubscriptionRequest,
    snapshot: AuthoritativeSnapshot,
}

impl PreparedSubscription {
    /// Returns the stable pending-registration identity.
    pub const fn operation_id(&self) -> OperationId {
        self.request.operation_id()
    }

    /// Returns the requested invalidation topics.
    pub const fn request(&self) -> &SubscriptionRequest {
        &self.request
    }

    /// Returns the authoritative snapshot loaded after pending registration.
    pub const fn snapshot(&self) -> &AuthoritativeSnapshot {
        &self.snapshot
    }
}

/// Transport-independent, stateless HQ use-case coordinator.
#[derive(Clone, Debug)]
pub struct Application<P> {
    ports: P,
}

impl<P> Application<P> {
    /// Constructs a service around one node-composed capability bundle.
    pub const fn new(ports: P) -> Self {
        Self { ports }
    }

    /// Returns the composed ports for ownership, tests, and orderly shutdown coordination.
    pub const fn ports(&self) -> &P {
        &self.ports
    }

    /// Consumes the service into its capability bundle.
    pub fn into_ports(self) -> P {
        self.ports
    }
}

impl<P> Application<P>
where
    P: ApplicationPorts,
{
    /// Loads one complete authoritative projection refresh.
    pub fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError> {
        self.ports.authoritative_snapshot()
    }

    /// Loads one bounded exact canonical evidence closure for offline transfer.
    pub fn canonical_evidence(
        &self,
        roots: &BTreeSet<FactId>,
        maximum_facts: usize,
        maximum_bytes: usize,
    ) -> Result<Vec<CanonicalEvidence>, ApplicationError> {
        self.ports
            .canonical_evidence(roots, maximum_facts, maximum_bytes)
    }

    /// Reverifies and idempotently imports exact canonical evidence.
    pub fn ingest_canonical_evidence(
        &self,
        evidence: &[CanonicalEvidence],
    ) -> Result<Vec<EvidenceIngestOutcome>, ApplicationError> {
        self.ports.ingest_canonical_evidence(evidence)
    }

    /// Executes, routes, or reconciles one exact project command.
    pub fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.ports.control_project(request)
    }

    /// Executes or reconciles one exact node-owned named-agent retirement.
    pub fn retire_agent(
        &self,
        request: AgentRetirementRequest,
    ) -> Result<AgentRetirementOutcome, ApplicationError> {
        self.ports.retire_agent(request)
    }

    /// Loads one indexed conversation page without a complete-history scan or sort.
    pub fn conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<ConversationEntry>, ApplicationError> {
        self.ports.conversation_entries(key, limit, cursor)
    }

    /// Executes or reconciles one fact-backed identity, account, mailbox, conversation, agent, or
    /// project mutation, then independently prompts post-commit relay work.
    pub fn execute_mutation(
        &self,
        request: FactMutation,
    ) -> Result<MutationCompletion, ApplicationError> {
        let attempt = self.ports.commit_facts(request)?;
        let wake = match &attempt {
            MutationAttempt::Completed(receipt)
                if matches!(receipt.outcome(), MutationOutcome::Committed) =>
            {
                Some(self.ports.publish_wake(receipt.revision()))
            }
            MutationAttempt::Completed(_) | MutationAttempt::Uncertain { .. } => None,
        };
        Ok(MutationCompletion { attempt, wake })
    }

    /// Applies or reconciles one stable relay configuration operation.
    pub fn configure_relay(
        &self,
        request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.ports.configure_relay(request)
    }

    /// Prompts or reconciles explicit synchronization without changing prior commit outcomes.
    pub fn synchronize(
        &self,
        request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.ports.synchronize(request)
    }

    /// Loads one bounded authoritative relay and delivery health observation.
    pub fn relay_status(&self) -> Result<RelayStatus, ApplicationError> {
        self.ports.relay_status()
    }

    /// Loads current normalized domain health without mutating it.
    pub fn state_health(&self) -> Result<StateHealth, ApplicationError> {
        self.ports.state_health()
    }

    /// Explicitly reverifies the corpus and atomically replaces rebuildable state.
    pub fn repair_state(
        &self,
        operation_id: hq_domain::OperationId,
    ) -> Result<StateRepairReport, ApplicationError> {
        self.ports.repair_state(operation_id)
    }

    /// Starts, resumes, or stops one neutral named-agent runtime operation.
    pub fn control_agent_session(
        &self,
        request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        self.ports.control_harness(request)
    }

    /// Inspects an external project resource without treating database intent as observation.
    pub fn inspect_resource(
        &self,
        request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        self.ports.inspect_resource(request)
    }

    /// Registers a pending observer before reading the snapshot its acknowledgement will name.
    pub fn prepare_subscription(
        &self,
        request: SubscriptionRequest,
    ) -> Result<PreparedSubscription, ApplicationError> {
        self.ports.register_subscription(&request)?;
        match self.ports.authoritative_snapshot() {
            Ok(snapshot) => Ok(PreparedSubscription { request, snapshot }),
            Err(error) => {
                let _ = self.ports.cancel_subscription(request.operation_id());
                Err(error)
            }
        }
    }

    /// Activates a pending observer after its snapshot acknowledgement has been written.
    pub fn activate_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError> {
        self.ports.activate_subscription(operation_id)
    }

    /// Cancels a pending or active observer idempotently.
    pub fn cancel_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError> {
        self.ports.cancel_subscription(operation_id)
    }
}
