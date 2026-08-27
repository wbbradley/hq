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
mod agent_snapshot;
mod authority_snapshot;
mod conversation_query;
mod conversation_snapshot;
mod database;
mod error;
mod operational;
mod paths;
mod project_snapshot;
mod snapshot;

pub use actor::{IngestOutcome, RepairOutcome, RevisionInvalidations, Store, VerifiedFactCorpus};
pub use agent_snapshot::AgentProjectionSnapshot;
pub use authority_snapshot::AuthorityProjectionSnapshot;
pub use conversation_query::ConversationEntry;
pub use conversation_snapshot::ConversationProjectionSnapshot;
pub use error::{StoreError, StoreErrorClass};
pub use hq_reducer::ConversationKey;
pub use operational::{
    LocalMutationCommit, LocalMutationDecision, LocalMutationRequest, MAX_MUTATION_RESULT_BYTES,
    MAX_OUTBOX_QUERY_ITEMS, MutationReceipt, MutationResultBytes, MutationResultKind,
    OperationalValueError, OutboxIntent,
};
pub use project_snapshot::ProjectProjectionSnapshot;
pub use snapshot::{
    CompleteSnapshot, IndexedConflict, IndexedDecision, ReductionDomain, ReductionIndexSnapshot,
    ReductionReason,
};
