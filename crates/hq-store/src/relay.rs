//! Storage-owned durable relay synchronization records.
//!
//! These passive records deliberately contain no transport crate types. The node composition
//! boundary maps them to the consumer-owned relay port vocabulary.

use hq_application::{RelayAccess, RelayAuthentication};
use hq_domain::{CommandDigest, FactId, InstallationId, OperationId, Revision};

/// Maximum records returned for each collection in one relay-state query.
pub const MAX_RELAY_STATE_QUERY_ITEMS: usize = 1_024;
/// Maximum exact prepared or staged wrapper size.
pub const MAX_RELAY_WRAPPER_BYTES: usize = 256 * 1_024;
/// Maximum staged wrapper rows.
pub const MAX_RELAY_STAGING_ITEMS: usize = 1_024;
/// Maximum total staged exact wrapper bytes.
pub const MAX_RELAY_STAGING_BYTES: usize = 64 * 1_024 * 1_024;
/// Maximum quarantine rows.
pub const MAX_RELAY_QUARANTINE_ITEMS: usize = 1_024;
/// Maximum total quarantine sample bytes.
pub const MAX_RELAY_QUARANTINE_BYTES: usize = 4 * 1_024 * 1_024;
/// Maximum retained sample bytes for one quarantined wrapper.
pub const MAX_RELAY_QUARANTINE_SAMPLE_BYTES: usize = 4 * 1_024;

/// Keyset position for one independently ordered relay-state collection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum StoredRelayPagePosition<T> {
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
pub struct StoredOutboundCursor {
    /// Durable revision that created the intent.
    pub revision: Revision,
    /// Canonical fact identity.
    pub fact_id: FactId,
    /// Recipient installation identity.
    pub recipient: InstallationId,
}

/// Stable prepared-lineage page key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoredLineageCursor {
    /// Canonical fact identity.
    pub fact_id: FactId,
    /// Recipient installation identity.
    pub recipient: InstallationId,
}

/// Stable relay-attempt page key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAttemptCursor {
    /// Exact relay URL spelling.
    pub url: String,
    /// Prepared wrapper identity.
    pub wrapper_id: [u8; 32],
}

/// Stable FIFO page key used by staged and quarantine rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoredTimedDigestCursor {
    /// Collection-specific durable wall-clock time.
    pub millis: u64,
    /// Stable exact-input digest.
    pub digest: [u8; 32],
}

/// Independent keyset positions for one bounded relay-state query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRelayStateQuery {
    /// Maximum rows returned for each active collection.
    pub limit: usize,
    /// Policy collection position.
    pub policies: StoredRelayPagePosition<String>,
    /// Canonical outbox collection position.
    pub outbound: StoredRelayPagePosition<StoredOutboundCursor>,
    /// Prepared lineage collection position.
    pub prepared: StoredRelayPagePosition<StoredLineageCursor>,
    /// Relay attempt collection position.
    pub attempts: StoredRelayPagePosition<StoredAttemptCursor>,
    /// Catch-up cursor collection position.
    pub cursors: StoredRelayPagePosition<String>,
    /// Staging collection position.
    pub staged: StoredRelayPagePosition<StoredTimedDigestCursor>,
    /// Quarantine collection position.
    pub quarantine: StoredRelayPagePosition<StoredTimedDigestCursor>,
}

impl StoredRelayStateQuery {
    /// Starts every collection at its first row with the supplied bound.
    pub fn first(limit: usize) -> Self {
        Self {
            limit,
            policies: StoredRelayPagePosition::Start,
            outbound: StoredRelayPagePosition::Start,
            prepared: StoredRelayPagePosition::Start,
            attempts: StoredRelayPagePosition::Start,
            cursors: StoredRelayPagePosition::Start,
            staged: StoredRelayPagePosition::Start,
            quarantine: StoredRelayPagePosition::Start,
        }
    }
}

/// Desired relay policy before durable generation allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredDesiredRelayPolicy {
    /// Exact validated relay URL spelling.
    pub url: String,
    /// Allowed synchronization direction.
    pub access: RelayAccess,
    /// Authentication behavior.
    pub authentication: RelayAuthentication,
    /// Whether a session owner should exist.
    pub enabled: bool,
}

/// Idempotent relay policy operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRelayPolicyChange {
    /// Stable operation identity.
    pub operation_id: OperationId,
    /// Digest of the exact request.
    pub request_digest: CommandDigest,
    /// Desired policy fields.
    pub desired: StoredDesiredRelayPolicy,
}

/// One current durable relay policy generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRelayPolicy {
    /// Exact validated relay URL spelling.
    pub url: String,
    /// Allowed synchronization direction.
    pub access: RelayAccess,
    /// Authentication behavior.
    pub authentication: RelayAuthentication,
    /// Whether a session owner should exist.
    pub enabled: bool,
    /// Positive monotonic policy generation.
    pub generation: u64,
}

/// Prepared exact wrapper and its immutable uniqueness claims.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredPreparedOutbound {
    /// Canonical fact identity.
    pub fact_id: FactId,
    /// Recipient installation identity.
    pub recipient: InstallationId,
    /// Verified kind-1059 event ID.
    pub wrapper_id: [u8; 32],
    /// Fresh one-use outer signer public key.
    pub one_use_public_key: [u8; 32],
    /// Recipient root encryption public key.
    pub recipient_public_key: [u8; 32],
    /// Embedded canonical event ID.
    pub canonical_event_id: [u8; 32],
    /// Digest of exact embedded canonical bytes.
    pub canonical_sha256: [u8; 32],
    /// Digest of exact wrapper bytes.
    pub wrapper_sha256: [u8; 32],
    /// Randomized seal timestamp.
    pub seal_created_at: u64,
    /// Randomized gift-wrap timestamp.
    pub gift_wrap_created_at: u64,
    /// Exact signed wrapper bytes.
    pub exact_wire: Vec<u8>,
}

/// Relay-local outbound attempt disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredAttemptDisposition {
    /// Delivery is uncertain and exact retry remains eligible.
    Uncertain,
    /// Relay explicitly rejected the wrapper.
    Rejected,
    /// Relay accepted or already retained the wrapper.
    Accepted,
}

/// Closed redacted cause retained for a negative relay acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredRelayAttemptFailure {
    /// Relay requires successful connection authentication before retry.
    AuthenticationRequired,
    /// Relay requested bounded retry after rate limiting.
    RateLimited,
    /// Relay permanently rejected this wrapper for another reason.
    Permanent,
}

/// Durable relay-local attempt state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRelayAttempt {
    /// Exact relay URL spelling.
    pub url: String,
    /// Prepared wrapper identity.
    pub wrapper_id: [u8; 32],
    /// Positive saturating attempt count.
    pub attempts: u32,
    /// Current relay-local disposition.
    pub disposition: StoredAttemptDisposition,
    /// Redacted negative acknowledgement class, only for rejected state.
    pub failure: Option<StoredRelayAttemptFailure>,
    /// Last attempt wall-clock time.
    pub last_attempt_millis: u64,
    /// Earliest retry time, when eligible.
    pub retry_at_millis: Option<u64>,
}

/// Inclusive retained catch-up boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredCatchupCursor {
    /// Exact relay URL spelling.
    pub url: String,
    /// Policy generation for this traversal.
    pub generation: u64,
    /// Durable identity and upper wall-clock boundary of the active backward scan.
    pub scan_started_at_millis: u64,
    /// Latest scan-start boundary whose complete randomized-timestamp overlap was covered.
    pub covered_through_millis: Option<u64>,
    /// Oldest randomized wrapper timestamp observed so far.
    pub oldest_created_at: Option<u64>,
    /// Event-ID tie boundary paired with the timestamp.
    pub oldest_wrapper_id: Option<[u8; 32]>,
    /// Whether retained history is exhausted.
    pub exhausted: bool,
}

/// Successful atomic outer/logical inbound identity claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredInboundClaim {
    /// Outer wrapper ID.
    pub wrapper_id: [u8; 32],
    /// Verified origin installation identity.
    pub origin_installation_id: [u8; 32],
    /// Verified canonical event identity.
    pub canonical_event_id: [u8; 32],
    /// Digest of exact canonical evidence.
    pub canonical_sha256: [u8; 32],
    /// Local receive time.
    pub received_at_millis: u64,
}

/// Retryable exact outer input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredStagedInput {
    /// Digest of the exact outer bytes and stable staging identity.
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
pub struct StoredQuarantineEvidence {
    /// Digest of the complete rejected outer bytes.
    pub wrapper_sha256: [u8; 32],
    /// Verified outer ID, when validation reached it.
    pub wrapper_id: Option<[u8; 32]>,
    /// Stable redacted transport failure code.
    pub failure_code: u16,
    /// Local receive time.
    pub received_at_millis: u64,
    /// Complete rejected byte length.
    pub byte_len: usize,
    /// Bounded prefix of raw outer bytes only.
    pub raw_sample: Vec<u8>,
}

/// One atomic durable relay-state transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoredRelayStateMutation {
    /// Apply or replay a configuration operation.
    Configure(StoredRelayPolicyChange),
    /// Commit a prepared lineage and one-use-key claim together.
    Prepare(StoredPreparedOutbound),
    /// Record relay-local attempt state.
    Attempt(StoredRelayAttempt),
    /// Advance or replay a catch-up cursor.
    Cursor(StoredCatchupCursor),
    /// Claim outer/logical identities and optionally remove staged input atomically.
    ClaimInbound {
        /// Successful inbound identity claim.
        claim: StoredInboundClaim,
        /// Staged exact input completed by this claim, when retrying staging.
        remove_staged: Option<[u8; 32]>,
    },
    /// Add or advance one staged input.
    Stage(StoredStagedInput),
    /// Add diagnostic evidence, optionally remove staging, and evict atomically.
    Quarantine {
        /// Bounded permanent diagnostic evidence.
        evidence: StoredQuarantineEvidence,
        /// Staged exact input permanently rejected by this transition, when present.
        remove_staged: Option<[u8; 32]>,
    },
}

/// Bounded durable relay-state page reconstructed after wake or restart.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoredRelayStateSnapshot {
    /// Current relay policies.
    pub policies: Vec<StoredRelayPolicy>,
    /// Queued canonical intents.
    pub outbound: Vec<crate::OutboxIntent>,
    /// Prepared exact lineages.
    pub prepared: Vec<StoredPreparedOutbound>,
    /// Relay-local attempts.
    pub attempts: Vec<StoredRelayAttempt>,
    /// Catch-up cursors.
    pub cursors: Vec<StoredCatchupCursor>,
    /// Retryable staged input.
    pub staged: Vec<StoredStagedInput>,
    /// Bounded permanent diagnostics.
    pub quarantine: Vec<StoredQuarantineEvidence>,
}

/// One bounded relay-state page plus its independent collection continuations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRelayStatePage {
    /// Durable rows returned by this query.
    pub state: StoredRelayStateSnapshot,
    /// Next keyset query, or `None` after every collection is exhausted.
    pub next: Option<StoredRelayStateQuery>,
}
