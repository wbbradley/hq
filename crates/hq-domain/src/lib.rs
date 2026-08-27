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
mod ids;
mod resource;
mod time;

pub use address::{InstallationAddress, MailboxAddress};
pub use bounded::{BoundedSet, BoundedText, BoundedVec, NonEmptyBoundedSet, ValidatedValueError};
pub use causal::{AuthorityReference, AuthorityRole, CausalReferences};
pub use correlation::{OperationCorrelation, ProviderId, ProviderSessionId};
pub use envelope::{Command, Outcome, Page, PageCursor, VersionedView};
pub use error::{DomainError, ErrorCategory, ErrorCode};
pub use ids::{
    AccountId, AgentId, CommandId, EncryptionPublicKey, FactId, InstallationId, MailboxId,
    MessageId, OperationId, ProjectId, ReceiptId, ResourceId, SigningPublicKey,
};
pub use resource::{ResourceLocator, ResourceScheme};
pub use time::{Revision, Timestamp};

/// Maximum payload size used only by the in-memory boundary skeleton.
pub const SKELETON_PAYLOAD_MAX_BYTES: usize = 4_096;

/// A verified fact used temporarily by the in-memory workspace skeleton.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fact {
    id: FactId,
    payload: BoundedText<SKELETON_PAYLOAD_MAX_BYTES>,
}

impl Fact {
    /// Creates a fact after an outer boundary has validated its input.
    pub const fn new(id: FactId, payload: BoundedText<SKELETON_PAYLOAD_MAX_BYTES>) -> Self {
        Self { id, payload }
    }

    /// Returns the fact identity.
    pub const fn id(&self) -> FactId {
        self.id
    }

    /// Returns the skeleton payload.
    pub fn payload(&self) -> &str {
        self.payload.as_str()
    }
}
