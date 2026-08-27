//! Typed durable operational state for managed-runtime supervision.

use hq_domain::{
    AgentId, CommandDigest, ContentText, MessageId, OperationId, ProviderId, ProviderSessionId,
};

/// Maximum harness rows returned by one bounded state query.
pub const MAX_HARNESS_STATE_QUERY_ITEMS: usize = 1_024;

/// Result of one exact-token lease transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessLeaseOutcome {
    /// This token acquired or renewed ownership.
    Acquired,
    /// Another live token owns the worker, or a stale token attempted mutation.
    Held,
    /// This exact token released ownership.
    Released,
}

/// Durable worker lease record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoredHarnessLease {
    /// Named agent whose runtime is exclusively owned.
    pub agent_id: AgentId,
    /// Opaque exact owner token.
    pub owner_token: [u8; 32],
    /// Injected absolute lease deadline.
    pub expires_at_millis: u64,
}

/// Durable acknowledged provider session used for exact worker revival.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredHarnessReadySession {
    /// Named agent bound to the ready session.
    pub agent_id: AgentId,
    /// Neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact acknowledged durable session.
    pub session_id: ProviderSessionId,
}

/// Monotonic durable provider-delivery state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredHarnessDeliveryState {
    /// Exact input is durable but no provider call has begun.
    Pending,
    /// Acceptance is unknown and reconciliation is mandatory.
    Uncertain,
    /// Exact input was authoritatively accepted.
    Accepted,
    /// Exact input was authoritatively rejected and must not be retried.
    Rejected,
}

/// Exact durable provider input retained for restart reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredHarnessDelivery {
    /// Named agent owning the delivery.
    pub agent_id: AgentId,
    /// Neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact durable provider session.
    pub session_id: ProviderSessionId,
    /// Stable provider submission identity.
    pub submission_id: MessageId,
    /// Digest of the complete exact neutral input.
    pub digest: CommandDigest,
    /// HQ operation correlation.
    pub operation_id: OperationId,
    /// Bounded exact neutral body.
    pub body: ContentText,
    /// Injected durable queue time used only for deterministic scanning.
    pub queued_at_millis: u64,
    /// Current monotonic delivery state.
    pub state: StoredHarnessDeliveryState,
}

/// Durable output/activity persistence checkpoint for one normalized event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredHarnessEventCheckpoint {
    /// Named agent owning the event.
    pub agent_id: AgentId,
    /// Stable normalized event identity.
    pub event_id: MessageId,
    /// Digest of the complete normalized event under this identity.
    pub digest: CommandDigest,
    /// Whether output persistence has committed.
    pub output_committed: bool,
    /// Whether activity persistence has committed.
    pub activity_committed: bool,
}

/// One atomic harness operational-state transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoredHarnessStateMutation {
    /// Acquire an absent/expired lease or renew the same exact token.
    ClaimLease {
        /// Named agent being claimed.
        agent_id: AgentId,
        /// Proposed exact owner token.
        owner_token: [u8; 32],
        /// Injected current time.
        now_millis: u64,
        /// New deadline, strictly after `now_millis`.
        expires_at_millis: u64,
    },
    /// Release only the cited exact owner token.
    ReleaseLease {
        /// Named agent being released.
        agent_id: AgentId,
        /// Exact token expected to own it.
        owner_token: [u8; 32],
    },
    /// Record exact acknowledged readiness under the live owner token.
    SetReadySession {
        /// Exact live worker token.
        owner_token: [u8; 32],
        /// Ready durable provider session.
        ready: StoredHarnessReadySession,
    },
    /// Queue one exact provider delivery idempotently.
    QueueDelivery(StoredHarnessDelivery),
    /// Advance one existing delivery monotonically.
    SetDeliveryState {
        /// Named agent owning the delivery.
        agent_id: AgentId,
        /// Stable submission identity.
        submission_id: MessageId,
        /// Exact live worker token authorizing this external-effect checkpoint.
        owner_token: [u8; 32],
        /// New monotonic state.
        state: StoredHarnessDeliveryState,
    },
    /// Insert or advance one exact event checkpoint.
    CheckpointEvent {
        /// Exact live worker token authorizing this persistence checkpoint.
        owner_token: [u8; 32],
        /// Monotonic event checkpoint.
        checkpoint: StoredHarnessEventCheckpoint,
    },
}

/// Bounded deterministic harness state snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoredHarnessStateSnapshot {
    /// Live and expired lease rows in agent identity order.
    pub leases: Vec<StoredHarnessLease>,
    /// Exact acknowledged durable sessions in agent identity order.
    pub ready_sessions: Vec<StoredHarnessReadySession>,
    /// Delivery rows in queue-time, agent, and submission order.
    pub deliveries: Vec<StoredHarnessDelivery>,
    /// Event checkpoints in agent and event identity order.
    pub events: Vec<StoredHarnessEventCheckpoint>,
}
