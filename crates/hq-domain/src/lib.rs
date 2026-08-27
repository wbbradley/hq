//! Pure, validated domain values and policies.
//!
//! Wire decoding, cryptographic verification, persistence, clocks, randomness, and filesystem
//! access construct these values from outside this crate. Opaque byte identities deliberately do
//! not provide text parsing or formatting. For example, distinct identifier types cannot be mixed:
//!
//! ```compile_fail
//! use hq_domain::{FactId, InstallationId};
//!
//! fn needs_fact(_: FactId) {}
//! needs_fact(InstallationId::from_bytes([0; 32]));
//! ```

mod address;
mod bounded;
mod causal;
mod correlation;
mod envelope;
mod error;
mod fact_catalog;
mod ids;
mod resource;
mod semantic_fact;
mod time;

pub use address::{InstallationAddress, MailboxAddress};
pub use bounded::{BoundedSet, BoundedText, BoundedVec, NonEmptyBoundedSet, ValidatedValueError};
pub use causal::{AuthorityReference, AuthorityRole, CausalReferences};
pub use correlation::{OperationCorrelation, ProviderId, ProviderSessionId};
pub use envelope::{Command, Outcome, Page, PageCursor, VersionedView};
pub use error::{DomainError, ErrorCategory, ErrorCode};
pub use fact_catalog::{FactKind, ProtocolClass, RetentionClass};
pub use ids::{
    AccountId, AgentId, AssignmentId, CommandDigest, CommandId, DispatchId, EncryptionPublicKey,
    FactId, GrantId, InstallationId, MailboxId, MessageId, OperationId, ProjectId, ReceiptId,
    ResourceId, SigningPublicKey, ThreadId,
};
pub use resource::{ResourceLocator, ResourceScheme};
pub use semantic_fact::*;
pub use time::{Revision, Timestamp};
