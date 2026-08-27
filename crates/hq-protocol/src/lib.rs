//! Strict protocol transitions into verified domain values.

mod dispatch;
mod dto;
mod error;
mod json;
mod signed_event;

pub use dispatch::{
    DispatchOutcome, ProtocolNamespace, SupportedContentBytes, UnsupportedReason,
    VerifiedUnsupportedRecord,
};
pub use dto::{VerifiedSemanticFact, VerifiedSupportedRecord};
pub use error::{FailureClass, ProtocolError};
pub use signed_event::{
    Bip340Signer, CryptographicallyVerifiedEvent, HQ_EVENT_KIND, MAX_CONTENT_BYTES,
    MAX_EVENT_BYTES, ParsedOuterEvent, RawEventBytes, verify_bip340,
};

use std::{error::Error, fmt};

use hq_domain::{
    BoundedSet, CausalReferences, EncryptionPublicKey, Fact, FactId, FactScope,
    InstallationAddress, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, SemanticPayload,
    ShortText, SigningPublicKey, Timestamp,
};

/// A pre-serialization frame used only by the workspace walking skeleton.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InMemoryFrame {
    fact_id: u64,
    payload: String,
}

impl InMemoryFrame {
    /// Creates a frame at the untrusted side of the protocol boundary.
    pub fn new(fact_id: u64, payload: impl Into<String>) -> Self {
        Self {
            fact_id,
            payload: payload.into(),
        }
    }

    /// Validates the frame and converts it into a domain fact.
    pub fn decode(self) -> Result<Fact, DecodeError> {
        if self.payload.is_empty() {
            return Err(DecodeError::EmptyPayload);
        }

        let label = ShortText::new(self.payload).map_err(|_| DecodeError::PayloadTooLong)?;
        let mut id_bytes = [0; 32];
        id_bytes[24..].copy_from_slice(&self.fact_id.to_be_bytes());
        let installation_id = InstallationId::from_bytes(id_bytes);
        let signing_key = SigningPublicKey::from_bytes(id_bytes);
        let author = InstallationAddress::new(installation_id, signing_key);
        let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
            BoundedSet::new([]).map_err(|_| DecodeError::InvalidSkeleton)?,
            [],
        )
        .map_err(|_| DecodeError::InvalidSkeleton)?;
        Fact::new(
            FactId::from_bytes(id_bytes),
            author,
            Timestamp::from_unix_millis(0),
            FactScope::InstallationPrivate(installation_id),
            causal,
            SemanticPayload::InstallationDeclared {
                installation_id,
                signing_key,
                encryption_key: EncryptionPublicKey::from_bytes(id_bytes),
                label: Some(label),
            },
        )
        .map_err(|_| DecodeError::InvalidSkeleton)
    }
}

/// Validation failures at the in-memory protocol boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The frame contained no semantic payload.
    EmptyPayload,
    /// The frame exceeded the walking-skeleton payload limit.
    PayloadTooLong,
    /// The fixed walking-skeleton fixture violated a domain invariant.
    InvalidSkeleton,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPayload => formatter.write_str("fact payload is empty"),
            Self::PayloadTooLong => formatter.write_str("fact payload is too long"),
            Self::InvalidSkeleton => formatter.write_str("walking skeleton is invalid"),
        }
    }
}

impl Error for DecodeError {}
