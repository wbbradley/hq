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
//!     store.append_verified(raw).unwrap();
//! }
//! ```

mod actor;
mod agent_snapshot;
mod authority_snapshot;
mod conversation_snapshot;
mod database;
mod error;
mod paths;
mod project_snapshot;
mod snapshot;

pub use actor::{AppendOutcome, RepairOutcome, Store, VerifiedFactCorpus};
pub use agent_snapshot::AgentProjectionSnapshot;
pub use authority_snapshot::AuthorityProjectionSnapshot;
pub use conversation_snapshot::ConversationProjectionSnapshot;
pub use error::{StoreError, StoreErrorClass};
pub use project_snapshot::ProjectProjectionSnapshot;
pub use snapshot::{
    CompleteSnapshot, IndexedConflict, IndexedDecision, ReductionDomain, ReductionIndexSnapshot,
    ReductionReason,
};
