//! Consumer-owned relay synchronization ports and passive records.

use std::{error::Error, fmt, num::NonZeroU64, os::fd::BorrowedFd, time::Duration};

use hq_application::{RelayAccess, RelayAuthentication};
use hq_domain::{CommandDigest, FactId, InstallationId, OperationId, Revision};

use crate::{DurableEnvelope, FailureClass, RelayUrl};

/// Maximum records returned for each collection in one durable relay-state query.
pub const MAX_STATE_QUERY_ITEMS: usize = 1_024;
/// Maximum staged exact wrappers retained before backpressure.
pub const MAX_STAGING_ITEMS: usize = 1_024;
/// Maximum total staged wrapper bytes.
pub const MAX_STAGING_BYTES: usize = 64 * 1024 * 1024;
/// Maximum quarantine evidence rows.
pub const MAX_QUARANTINE_ITEMS: usize = 1_024;
/// Maximum total quarantine sample bytes.
pub const MAX_QUARANTINE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum relay-supplied acknowledgement, notice, or challenge text.
pub const MAX_RELAY_STATUS_BYTES: usize = 1_024;

/// Keyset position for one independently ordered relay-state collection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RelayPagePosition<T> {
    /// Begin at the first row.
    #[default]
    Start,
    /// Continue strictly after this stable key.
    After(T),
    /// Skip a collection that is already exhausted.
    Done,
}

/// Stable outbox page key including its primary ordering revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutboundCursor {
    /// Durable revision that created the intent.
    pub revision: Revision,
    /// Stable canonical/recipient identity.
    pub key: OutboxKey,
}

/// Stable relay-attempt page key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptCursor {
    /// Exact relay identity.
    pub url: RelayUrl,
    /// Prepared outer wrapper ID.
    pub wrapper_id: [u8; 32],
}

/// Stable FIFO page key used by staged and quarantine rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimedDigestCursor {
    /// Collection-specific durable wall-clock time.
    pub millis: u64,
    /// Stable exact-input digest.
    pub digest: [u8; 32],
}

/// Independent keyset positions for one bounded relay-state query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayStateQuery {
    /// Maximum rows returned for each active collection.
    pub limit: usize,
    /// Policy collection position.
    pub policies: RelayPagePosition<RelayUrl>,
    /// Canonical outbox collection position.
    pub outbound: RelayPagePosition<OutboundCursor>,
    /// Prepared lineage collection position.
    pub prepared: RelayPagePosition<OutboxKey>,
    /// Relay attempt collection position.
    pub attempts: RelayPagePosition<AttemptCursor>,
    /// Catch-up cursor collection position.
    pub cursors: RelayPagePosition<RelayUrl>,
    /// Staging collection position.
    pub staged: RelayPagePosition<TimedDigestCursor>,
    /// Quarantine collection position.
    pub quarantine: RelayPagePosition<TimedDigestCursor>,
}

impl RelayStateQuery {
    /// Starts every collection at its first row with the supplied bound.
    pub fn first(limit: usize) -> Self {
        Self {
            limit,
            policies: RelayPagePosition::Start,
            outbound: RelayPagePosition::Start,
            prepared: RelayPagePosition::Start,
            attempts: RelayPagePosition::Start,
            cursors: RelayPagePosition::Start,
            staged: RelayPagePosition::Start,
            quarantine: RelayPagePosition::Start,
        }
    }
}

/// Stable relay-port failure without external prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayPortError {
    /// Supplied value or query bound was invalid.
    InvalidInput,
    /// Stable operation or immutable identity was reused unequally.
    Conflict,
    /// Durable state failed integrity checks.
    Corrupt,
    /// A local dependency is temporarily unavailable.
    Unavailable,
    /// The owned relay connection failed or closed.
    Connection,
    /// Bounded staging cannot accept another exact wrapper.
    Backpressure,
}

impl fmt::Display for RelayPortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "relay port input is invalid",
            Self::Conflict => "relay state identity conflicts",
            Self::Corrupt => "relay durable state is corrupt",
            Self::Unavailable => "relay dependency is unavailable",
            Self::Connection => "relay connection is unavailable",
            Self::Backpressure => "relay staging is full",
        })
    }
}

impl Error for RelayPortError {}

/// One durable relay configuration generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayPolicy {
    /// Exact relay identity.
    pub url: RelayUrl,
    /// Enabled read/write direction.
    pub access: RelayAccess,
    /// Connection authentication policy.
    pub authentication: RelayAuthentication,
    /// Whether a session owner should exist.
    pub enabled: bool,
    /// Positive durable policy generation.
    pub generation: NonZeroU64,
}

/// Desired relay policy fields before durable generation allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesiredRelayPolicy {
    /// Exact relay identity.
    pub url: RelayUrl,
    /// Enabled read/write direction.
    pub access: RelayAccess,
    /// Connection authentication policy.
    pub authentication: RelayAuthentication,
    /// Whether a session owner should exist.
    pub enabled: bool,
}

/// Idempotent request to replace one relay policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayPolicyChange {
    /// Stable external operation identity.
    pub operation_id: OperationId,
    /// Digest of the exact requested policy.
    pub request_digest: CommandDigest,
    /// Desired policy fields; persistence assigns or reuses the generation.
    pub desired: DesiredRelayPolicy,
}

/// Stable per-recipient canonical outbox identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OutboxKey {
    /// Exact canonical event ID.
    pub fact_id: FactId,
    /// Recipient installation identity.
    pub recipient: InstallationId,
}

/// Queued canonical work awaiting route resolution or preparation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundIntent {
    /// Stable outbox identity.
    pub key: OutboxKey,
    /// Exact signed canonical event bytes.
    pub exact_canonical_bytes: Vec<u8>,
    /// Revision that created the intent.
    pub revision: Revision,
}

/// Verified routing material resolved independently of relay observations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRoute {
    /// Recipient root encryption public key.
    pub recipient_public_key: [u8; 32],
    /// Ordered unique eligible relay URLs.
    pub relays: Vec<RelayUrl>,
}

/// Prepared exact wrapper committed before its first publish.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedOutbound {
    /// Stable canonical/recipient lineage.
    pub key: OutboxKey,
    /// Immutable envelope metadata and exact wrapper bytes.
    pub envelope: DurableEnvelope,
}

/// Relay-local result retained after an outbound attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptDisposition {
    /// Write may or may not have reached the relay; exact retry remains eligible.
    Uncertain,
    /// Relay explicitly rejected the event.
    Rejected,
    /// Relay positively accepted or reported an already-retained duplicate.
    Accepted,
}

/// Closed redacted cause retained for a negative relay acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayAttemptFailure {
    /// Relay requires successful connection authentication before retry.
    AuthenticationRequired,
    /// Relay requested bounded retry after rate limiting.
    RateLimited,
    /// Relay permanently rejected this wrapper for another reason.
    Permanent,
}

/// Durable relay-local outbound attempt state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayAttempt {
    /// Exact relay identity.
    pub url: RelayUrl,
    /// Prepared outer wrapper ID.
    pub wrapper_id: [u8; 32],
    /// Saturating number of attempts.
    pub attempts: u32,
    /// Current relay-local disposition.
    pub disposition: AttemptDisposition,
    /// Redacted negative acknowledgement class, only for rejected state.
    pub failure: Option<RelayAttemptFailure>,
    /// Last monotonic scheduling time represented as Unix milliseconds for persistence.
    pub last_attempt_millis: u64,
    /// Earliest retry time, when retry remains eligible.
    pub retry_at_millis: Option<u64>,
}

/// Inclusive retained catch-up boundary for one policy generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatchupCursor {
    /// Exact relay identity.
    pub url: RelayUrl,
    /// Policy generation under which the cursor was observed.
    pub generation: NonZeroU64,
    /// Durable identity and upper wall-clock boundary of the active backward scan.
    pub scan_started_at_millis: u64,
    /// Latest scan-start boundary whose complete randomized-timestamp overlap was covered.
    pub covered_through_millis: Option<u64>,
    /// Oldest randomized wrapper timestamp observed so far.
    pub oldest_created_at: Option<u64>,
    /// Event-ID tie boundary paired with the oldest timestamp.
    pub oldest_wrapper_id: Option<[u8; 32]>,
    /// Whether a stable short/empty retained page exhausted older history.
    pub exhausted: bool,
}

/// Logical identity opened from one wrapper.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LogicalEnvelopeId {
    /// Verified origin installation identity.
    pub origin_installation_id: [u8; 32],
    /// Verified canonical event ID.
    pub canonical_event_id: [u8; 32],
}

/// Successful canonical-ingest identity claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InboundClaim {
    /// Outer wrapper ID.
    pub wrapper_id: [u8; 32],
    /// Opened logical identity.
    pub logical_id: LogicalEnvelopeId,
    /// Digest of exact canonical evidence.
    pub canonical_sha256: [u8; 32],
    /// Local receive time.
    pub received_at_millis: u64,
}

/// Transient exact outer input retained for retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedInput {
    /// SHA-256 digest of the exact outer bytes and stable staging identity.
    pub wrapper_sha256: [u8; 32],
    /// Exact bounded outer bytes.
    pub exact_outer: Vec<u8>,
    /// First local receive time.
    pub first_received_millis: u64,
    /// Saturating retry count.
    pub attempts: u32,
    /// Earliest retry time.
    pub retry_at_millis: u64,
}

/// Permanently rejected bounded diagnostic evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineEvidence {
    /// Digest of complete rejected outer bytes.
    pub wrapper_sha256: [u8; 32],
    /// Verified outer ID when validation reached that transition.
    pub wrapper_id: Option<[u8; 32]>,
    /// Redacted envelope failure class.
    pub failure: FailureClass,
    /// Local receive time.
    pub received_at_millis: u64,
    /// Complete rejected byte length.
    pub byte_len: usize,
    /// Bounded prefix of raw outer bytes only.
    pub raw_sample: Vec<u8>,
}

/// Atomic durable transition requested by the relay owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelayStateMutation {
    /// Apply or idempotently replay one policy operation.
    Configure(RelayPolicyChange),
    /// Commit an exact prepared lineage and its one-use key claim.
    Prepare(PreparedOutbound),
    /// Record relay-local attempt state.
    Attempt(RelayAttempt),
    /// Advance or replay one catch-up cursor.
    Cursor(CatchupCursor),
    /// Claim equal outer/logical identities and optionally remove staged input atomically.
    ClaimInbound {
        /// Successful inbound identity claim.
        claim: InboundClaim,
        /// Staged exact input completed by this claim, when retrying staging.
        remove_staged: Option<[u8; 32]>,
    },
    /// Add or replace one transient staged input.
    Stage(StagedInput),
    /// Add quarantine evidence, optionally remove staging, and evict atomically.
    Quarantine {
        /// Bounded permanent diagnostic evidence.
        evidence: QuarantineEvidence,
        /// Staged exact input permanently rejected by this transition, when present.
        remove_staged: Option<[u8; 32]>,
    },
}

/// Bounded durable state page used to reconstruct relay work after restart.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelayStateSnapshot {
    /// Current durable policies.
    pub policies: Vec<RelayPolicy>,
    /// Queued canonical intents.
    pub outbound: Vec<OutboundIntent>,
    /// Already prepared exact lineages.
    pub prepared: Vec<PreparedOutbound>,
    /// Relay-local attempts.
    pub attempts: Vec<RelayAttempt>,
    /// Catch-up cursors.
    pub cursors: Vec<CatchupCursor>,
    /// Transient staged inputs.
    pub staged: Vec<StagedInput>,
    /// Bounded permanent diagnostics.
    pub quarantine: Vec<QuarantineEvidence>,
}

/// One bounded relay-state page plus independent collection continuations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayStatePage {
    /// Durable rows returned by this query.
    pub state: RelayStateSnapshot,
    /// Next keyset query, or `None` after every collection is exhausted.
    pub next: Option<RelayStateQuery>,
}

/// Durable relay state capability implemented by the node's store adapter.
pub trait RelayStatePort: Send + Sync {
    /// Loads one deterministic bounded keyset page.
    fn load_page(&self, query: RelayStateQuery) -> Result<RelayStatePage, RelayPortError>;
    /// Loads a deterministic state page with each collection bounded by `limit`.
    fn load_state(&self, limit: usize) -> Result<RelayStateSnapshot, RelayPortError> {
        self.load_page(RelayStateQuery::first(limit))
            .map(|page| page.state)
    }
    /// Loads one prepared lineage by its stable outbox identity.
    fn prepared(&self, key: OutboxKey) -> Result<Option<PreparedOutbound>, RelayPortError>;
    /// Loads one relay-local attempt by its exact correlation identity.
    fn attempt(
        &self,
        url: &RelayUrl,
        wrapper_id: [u8; 32],
    ) -> Result<Option<RelayAttempt>, RelayPortError>;
    /// Loads one retained cursor by exact relay identity.
    fn cursor(&self, url: &RelayUrl) -> Result<Option<CatchupCursor>, RelayPortError>;
    /// Applies one atomic durable transition.
    fn apply(&self, mutation: RelayStateMutation) -> Result<(), RelayPortError>;
}

/// Signed route resolution independent of relay observations.
pub trait RouteResolver: Send + Sync {
    /// Resolves current verified routing material for one queued intent.
    fn resolve(&self, key: OutboxKey) -> Result<ResolvedRoute, RelayPortError>;
}

/// Common exact canonical ingest capability.
pub trait CanonicalIngest: Send + Sync {
    /// Re-verifies and atomically ingests exact canonical bytes.
    fn ingest(&self, exact_canonical_bytes: Vec<u8>) -> Result<(), RelayPortError>;
}

/// Injected time source for deterministic session behavior.
pub trait RelayClock: Send + Sync {
    /// Returns wall-clock Unix milliseconds for durable observations.
    fn unix_millis(&self) -> u64;
    /// Returns a monotonic tick for deadlines and backoff.
    fn monotonic_millis(&self) -> u64;
}

/// Injected bounded sleeper.
pub trait RelaySleeper: Send + Sync {
    /// Waits for a deterministic duration or cancellation owned by the caller.
    fn sleep(&self, duration: Duration) -> Result<(), RelayPortError>;
}

/// Successfully opened non-authoritative transport envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenedRelayEnvelope {
    /// Exact raw canonical bytes for the common ingest path.
    pub exact_canonical_bytes: Vec<u8>,
    /// Verified outer wrapper identity.
    pub wrapper_id: [u8; 32],
    /// Verified outer wrapper timestamp used only for catch-up traversal.
    pub wrapper_created_at: u64,
    /// Opened logical deduplication identity.
    pub logical_id: LogicalEnvelopeId,
    /// Digest of exact canonical evidence.
    pub canonical_sha256: [u8; 32],
}

/// Permanent result of opening one untrusted outer wrapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RejectedRelayEnvelope {
    /// Redacted envelope failure class.
    pub failure: FailureClass,
    /// Verified outer identity when validation reached it.
    pub wrapper_id: Option<[u8; 32]>,
}

/// Open result separates permanent bad input from transient local dependency failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelayOpenOutcome {
    /// Wrapper opened and reverified successfully.
    Opened(OpenedRelayEnvelope),
    /// Wrapper is permanently invalid and may be quarantined.
    Rejected(RejectedRelayEnvelope),
}

/// Exact signed connection-authentication event and its correlation identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedRelayAuthentication {
    /// Signed kind-22242 event identity.
    pub event_id: [u8; 32],
    /// Exact signed event bytes sent on this connection only.
    pub exact_event: Vec<u8>,
}

/// Installation-root envelope operations injected into session ownership.
pub trait RelayEnvelopePort: Send + Sync {
    /// Returns the local root public key used by the `p` subscription filter.
    fn local_public_key(&self) -> [u8; 32];
    /// Creates one exact immutable lineage from verified queued canonical bytes.
    fn prepare(
        &self,
        intent: &OutboundIntent,
        recipient_public_key: [u8; 32],
        now_seconds: u64,
    ) -> Result<PreparedOutbound, RelayPortError>;
    /// Opens one exact outer wrapper without granting canonical authority.
    fn open(&self, exact_outer: &[u8]) -> Result<RelayOpenOutcome, RelayPortError>;
    /// Signs one connection-local NIP-42 challenge.
    fn authenticate(
        &self,
        url: &RelayUrl,
        challenge: &str,
        now_seconds: u64,
    ) -> Result<PreparedRelayAuthentication, RelayPortError>;
}

/// Bounded receive result from one exclusively owned relay connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelayReceive {
    /// One complete typed relay frame arrived.
    Frame(RelayFrame),
    /// No complete frame is currently buffered on the ready connection.
    Pending,
    /// The peer closed the connection cleanly.
    Closed,
}

/// Minimal owned relay frame vocabulary used by real and scripted connections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelayFrame {
    /// Publish exact signed event bytes.
    Event(Vec<u8>),
    /// Open one named subscription with an exact bounded filter JSON object.
    Request {
        /// Connection-local subscription identity.
        subscription: String,
        /// Exact bounded NIP-01 filter JSON.
        filter: String,
    },
    /// Close one named subscription.
    Close(String),
    /// Relay write acknowledgement.
    Ok {
        /// Event identity being acknowledged.
        event_id: [u8; 32],
        /// Whether the relay accepted or already retained it.
        accepted: bool,
        /// Bounded relay-supplied status text.
        message: String,
    },
    /// End of stored events for one subscription.
    EndOfStoredEvents(String),
    /// Relay authentication challenge or signed response event.
    Auth(String),
    /// Relay closed one subscription.
    Closed {
        /// Connection-local subscription identity.
        subscription: String,
        /// Bounded relay-supplied status text.
        message: String,
    },
    /// Bounded relay notice.
    Notice(String),
    /// Received event for one subscription.
    SubscriptionEvent {
        /// Connection-local subscription identity.
        subscription: String,
        /// Exact bounded signed event bytes.
        exact_event: Vec<u8>,
    },
}

/// One exclusively owned relay connection.
pub trait RelayConnection: Send {
    /// Borrows the descriptor whose readable or closed state permits a nonblocking receive.
    fn readiness(&self) -> BorrowedFd<'_>;
    /// Sends one typed frame.
    fn send(&mut self, frame: RelayFrame) -> Result<(), RelayPortError>;
    /// Receives one currently buffered frame without blocking.
    fn receive(&mut self) -> Result<RelayReceive, RelayPortError>;
    /// Idempotently closes the connection.
    fn close(&mut self) -> Result<(), RelayPortError>;
}

/// Factory for real or scripted exclusively owned connections.
pub trait RelayConnector: Send + Sync {
    /// Opens one connection for the exact relay URL.
    fn connect(&self, url: &RelayUrl) -> Result<Box<dyn RelayConnection>, RelayPortError>;
}
