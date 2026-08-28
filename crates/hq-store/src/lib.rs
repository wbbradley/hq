//! Durable persistence adapter boundary.
//!
//! `Store` deliberately exposes no SQLite handle or table-shaped operation:
//!
//! ```compile_fail
//! use hq_store::Store;
//!
//! fn bypass(store: &Store) {
//!     store.connection().execute("DELETE FROM canonical_facts", []).unwrap();
//! }
//! ```
//!
//! Only the complete protocol trust state can enter the immutable corpus:
//!
//! ```compile_fail
//! use hq_protocol::RawEventBytes;
//! use hq_store::Store;
//!
//! fn bypass(store: &Store, raw: RawEventBytes) {
//!     store.ingest_verified(raw, todo!()).unwrap();
//! }
//! ```

mod actor;
mod database;
mod error;
mod gateway;
mod harness;
mod operational;
mod paths;
mod project_saga;
mod relay;
mod snapshot;

pub use actor::{
    ApplicationStateHandle, HarnessStateHandle, IngestOutcome, ProjectSagaStateHandle,
    RelayStateHandle, RepairOutcome, ReplicationHandle, RevisionInvalidations, Store,
    VerifiedFactCorpus,
};
pub use error::{StoreError, StoreErrorClass};
pub use gateway::StoreGateway;
pub use harness::{
    HarnessLeaseOutcome, MAX_HARNESS_STATE_QUERY_ITEMS, StoredHarnessDelivery,
    StoredHarnessDeliveryState, StoredHarnessEventCheckpoint, StoredHarnessLease,
    StoredHarnessReadySession, StoredHarnessStateMutation, StoredHarnessStateSnapshot,
};
pub use hq_application::{
    AgentProjectionSnapshot, AuthoritativeSnapshot, AuthorityProjectionSnapshot, ConversationEntry,
    ConversationMessageEntry, ConversationProjectionSnapshot, DomainSnapshot,
    ProjectProjectionSnapshot,
};
pub use hq_reducer::ConversationKey;
pub use operational::{
    LocalMutationCommit, LocalMutationDecision, LocalMutationRequest, MAX_MUTATION_RESULT_BYTES,
    MAX_OUTBOX_QUERY_ITEMS, MutationReceipt, MutationResultBytes, MutationResultKind,
    OperationalValueError, OutboxIntent,
};
pub use project_saga::{
    MAX_PROJECT_COMMAND_BODY_BYTES, MAX_PROJECT_SAGA_QUERY_ITEMS, StoredProjectEffectState,
    StoredProjectSaga, StoredProjectSagaBegin, StoredProjectSagaState,
};
pub use relay::{
    MAX_RELAY_QUARANTINE_BYTES, MAX_RELAY_QUARANTINE_ITEMS, MAX_RELAY_QUARANTINE_SAMPLE_BYTES,
    MAX_RELAY_STAGING_BYTES, MAX_RELAY_STAGING_ITEMS, MAX_RELAY_STATE_QUERY_ITEMS,
    MAX_RELAY_WRAPPER_BYTES, StoredAttemptCursor, StoredAttemptDisposition, StoredCatchupCursor,
    StoredDesiredRelayPolicy, StoredInboundClaim, StoredLineageCursor, StoredOutboundCursor,
    StoredPreparedOutbound, StoredQuarantineEvidence, StoredRelayAttempt,
    StoredRelayAttemptFailure, StoredRelayPagePosition, StoredRelayPolicy, StoredRelayPolicyChange,
    StoredRelayStateMutation, StoredRelayStatePage, StoredRelayStateQuery,
    StoredRelayStateSnapshot, StoredStagedInput, StoredTimedDigestCursor,
};
pub use snapshot::{
    CompleteSnapshot, IndexedConflict, IndexedDecision, ReductionDomain, ReductionIndexSnapshot,
    ReductionReason,
};
