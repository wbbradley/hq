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
mod operational;
mod paths;
mod snapshot;

pub use actor::{IngestOutcome, RepairOutcome, RevisionInvalidations, Store, VerifiedFactCorpus};
pub use error::{StoreError, StoreErrorClass};
pub use gateway::StoreGateway;
pub use hq_application::{
    AgentProjectionSnapshot, AuthoritativeSnapshot, AuthorityProjectionSnapshot, ConversationEntry,
    ConversationProjectionSnapshot, DomainSnapshot, ProjectProjectionSnapshot,
};
pub use hq_reducer::ConversationKey;
pub use operational::{
    LocalMutationCommit, LocalMutationDecision, LocalMutationRequest, MAX_MUTATION_RESULT_BYTES,
    MAX_OUTBOX_QUERY_ITEMS, MutationReceipt, MutationResultBytes, MutationResultKind,
    OperationalValueError, OutboxIntent,
};
pub use snapshot::{
    CompleteSnapshot, IndexedConflict, IndexedDecision, ReductionDomain, ReductionIndexSnapshot,
    ReductionReason,
};
