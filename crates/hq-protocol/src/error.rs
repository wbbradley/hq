//! Closed protocol failure taxonomy.

use std::{error::Error, fmt};

/// Stable class identifying the trust transition that rejected input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureClass {
    /// Complete event exceeded the raw byte limit.
    EventTooLarge,
    /// Decoded content exceeded the content byte limit.
    ContentTooLarge,
    /// Outer event was not valid UTF-8.
    OuterInvalidUtf8,
    /// Outer event used a valid but non-canonical spelling.
    OuterNonCanonical,
    /// Outer event members were missing, unknown, duplicated, or reordered.
    OuterMemberOrder,
    /// Outer event field had the wrong primitive shape or width.
    OuterFieldShape,
    /// Outer event had trailing bytes.
    OuterTrailingData,
    /// Event kind is not the provisional HQ carriage kind.
    WrongKind,
    /// HQ carriage event had one or more tags.
    NonemptyTags,
    /// Claimed event ID did not equal the exact NIP-01 preimage hash.
    EventIdMismatch,
    /// X-only public key could not be decoded.
    InvalidPublicKey,
    /// Signature bytes were not a canonical BIP-340 scalar pair.
    InvalidSignatureEncoding,
    /// Canonical BIP-340 signature did not verify.
    BadSignature,
    /// Signed content was not valid canonical JSON.
    ContentMalformed,
    /// Signed content used a non-canonical JSON spelling.
    ContentNonCanonical,
    /// Signed content exceeded the maximum JSON nesting depth.
    ContentTooDeep,
    /// Signed content exceeded a collection or object-member count.
    ContentTooManyItems,
    /// Signed millisecond time did not agree with the outer event seconds.
    AuthoredTimeMismatch,
    /// Signed scope and author identity disagree intrinsically.
    ScopeAuthorMismatch,
    /// An authority reference did not occur in the exact parent set.
    AuthorityNotParent,
    /// More than one authority reference used the same semantic role.
    DuplicateAuthorityRole,
    /// Canonical/control discriminator and family range were mixed.
    NamespaceConfusion,
    /// Frozen Go schema was presented at the Rust protocol boundary.
    LegacySchema,
    /// Signing key bytes did not represent a valid nonzero scalar.
    InvalidSecretKey,
    /// Signing or self-verification failed.
    SigningFailed,
}

/// Redacted protocol-boundary error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolError {
    class: FailureClass,
}

impl ProtocolError {
    pub(crate) const fn new(class: FailureClass) -> Self {
        Self { class }
    }

    /// Returns the stable failure class without attacker-controlled detail.
    pub const fn class(self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.class {
            FailureClass::EventTooLarge => "event exceeds the byte limit",
            FailureClass::ContentTooLarge => "content exceeds the byte limit",
            FailureClass::OuterInvalidUtf8 => "outer event is not valid UTF-8",
            FailureClass::OuterNonCanonical => "outer event is not canonical JSON",
            FailureClass::OuterMemberOrder => "outer event members are not the exact schema",
            FailureClass::OuterFieldShape => "outer event field has an invalid shape",
            FailureClass::OuterTrailingData => "outer event has trailing data",
            FailureClass::WrongKind => "event kind is not HQ carriage",
            FailureClass::NonemptyTags => "HQ carriage tags are not empty",
            FailureClass::EventIdMismatch => "event ID does not match its preimage",
            FailureClass::InvalidPublicKey => "event public key is invalid",
            FailureClass::InvalidSignatureEncoding => "event signature encoding is invalid",
            FailureClass::BadSignature => "event signature does not verify",
            FailureClass::ContentMalformed => "event content is malformed JSON",
            FailureClass::ContentNonCanonical => "event content is not canonical JSON",
            FailureClass::ContentTooDeep => "event content exceeds the JSON depth limit",
            FailureClass::ContentTooManyItems => "event content exceeds a collection limit",
            FailureClass::AuthoredTimeMismatch => {
                "content time does not agree with outer event time"
            }
            FailureClass::ScopeAuthorMismatch => "content scope and author disagree",
            FailureClass::AuthorityNotParent => "authority reference is not a declared parent",
            FailureClass::DuplicateAuthorityRole => "authority role occurs more than once",
            FailureClass::NamespaceConfusion => "protocol namespace and family disagree",
            FailureClass::LegacySchema => "legacy Go schema is not accepted",
            FailureClass::InvalidSecretKey => "signing key is invalid",
            FailureClass::SigningFailed => "event signing failed",
        };
        formatter.write_str(message)
    }
}

impl Error for ProtocolError {}
