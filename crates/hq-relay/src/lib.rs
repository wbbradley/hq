//! Encrypted Nostr relay transport boundary.

pub mod envelope;
mod envelope_port;
mod manager;
mod nip44;
mod ports;
mod session;
mod url;

use std::{error::Error, fmt};

pub use envelope::{
    AuthInput, DurableEnvelope, EnvelopeCodec, OpenedEnvelope, OpenedEnvelopeMetadata,
    PreparedEnvelope, PreparedEnvelopeMetadata, RandomSource, SystemRandom,
    check_one_use_key_claim,
};
pub use manager::{RelayManager, RelayManagerConfig, RelayManagerReport};
pub use ports::{
    AttemptCursor, AttemptDisposition, CanonicalIngest, CatchupCursor, DesiredRelayPolicy,
    InboundClaim, LogicalEnvelopeId, MAX_QUARANTINE_BYTES, MAX_QUARANTINE_ITEMS,
    MAX_RELAY_STATUS_BYTES, MAX_STAGING_BYTES, MAX_STAGING_ITEMS, MAX_STATE_QUERY_ITEMS,
    OpenedRelayEnvelope, OutboundCursor, OutboundIntent, OutboxKey, PreparedOutbound,
    PreparedRelayAuthentication, QuarantineEvidence, RejectedRelayEnvelope, RelayAttempt,
    RelayAttemptFailure, RelayClock, RelayConnection, RelayConnector, RelayEnvelopePort,
    RelayFrame, RelayOpenOutcome, RelayPagePosition, RelayPolicy, RelayPolicyChange,
    RelayPortError, RelayReceive, RelaySleeper, RelayStateMutation, RelayStatePage, RelayStatePort,
    RelayStateQuery, RelayStateSnapshot, ResolvedRoute, RouteResolver, StagedInput,
    TimedDigestCursor,
};
pub use session::{
    RelayJitter, RelaySession, RelaySessionConfig, RelaySessionDependencies, RelaySessionProgress,
    StableRelayJitter,
};
pub use url::{MAX_RELAY_URL_BYTES, RelayUrl, RelayUrlError};

/// Retained gift-wrap event kind from NIP-59.
pub const GIFT_WRAP_KIND: u16 = 1059;
/// Sender-authenticated seal event kind from NIP-59.
pub const SEAL_KIND: u16 = 13;
/// HQ's unsigned transport rumor kind.
pub const HQ_RUMOR_KIND: u16 = 7282;
/// Ephemeral client authentication event kind from NIP-42.
pub const CLIENT_AUTH_KIND: u16 = 22242;
/// Maximum complete untrusted gift-wrap input.
pub const MAX_GIFT_WRAP_BYTES: usize = 256 * 1024;
/// Maximum encoded NIP-44 payload accepted before base64 allocation.
pub const MAX_NIP44_PAYLOAD_BYTES: usize = 256 * 1024;
/// Maximum decrypted intermediate JSON or NIP-44 plaintext.
pub const MAX_PLAINTEXT_BYTES: usize = 192 * 1024;
/// Maximum raw outer bytes retained as quarantine evidence.
pub const MAX_QUARANTINE_SAMPLE_BYTES: usize = 4 * 1024;

/// Stable redacted failure at the encrypted transport boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureClass {
    /// An input or generated result exceeded its byte bound.
    Size,
    /// JSON did not have the strict expected object shape.
    MalformedJson,
    /// An event ID did not match its NIP-01 preimage.
    EventIdentity,
    /// A BIP-340 signature was invalid.
    Signature,
    /// An x-only public key or secret scalar was invalid.
    InvalidPublicKey,
    /// The event kind or tags did not match its layer.
    LayerShape,
    /// The marked recipient was not the local installation.
    Recipient,
    /// The NIP-44 version is not supported.
    UnsupportedEncryption,
    /// A NIP-44 payload was not valid padded base64 data.
    MalformedEncryption,
    /// The NIP-44 authentication code did not verify.
    Mac,
    /// Decrypted NIP-44 padding or UTF-8 was invalid.
    Padding,
    /// The HQ transport schema or type was unsupported.
    EnvelopeVersion,
    /// Embedded canonical bytes did not pass ordinary verification.
    Canonical,
    /// Seal, rumor, origin, and canonical identities disagreed.
    IdentityAgreement,
    /// A one-use public key was associated with another wrapper.
    OneUseKeyReuse,
    /// Relay authentication input was empty, oversized, or contained controls.
    AuthenticationInput,
    /// Randomness or an internal cryptographic primitive failed.
    Cryptography,
}

/// Redacted encrypted-transport error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvelopeError {
    class: FailureClass,
}

impl EnvelopeError {
    pub(crate) const fn new(class: FailureClass) -> Self {
        Self { class }
    }
    /// Returns the stable failure class.
    pub const fn class(self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.class {
            FailureClass::Size => "transport input exceeds its byte bound",
            FailureClass::MalformedJson => "transport event is malformed",
            FailureClass::EventIdentity => "transport event ID does not match",
            FailureClass::Signature => "transport event signature does not verify",
            FailureClass::InvalidPublicKey => "transport key is invalid",
            FailureClass::LayerShape => "transport layer has the wrong kind or tags",
            FailureClass::Recipient => "transport recipient does not match",
            FailureClass::UnsupportedEncryption => "encryption version is unsupported",
            FailureClass::MalformedEncryption => "encrypted payload is malformed",
            FailureClass::Mac => "encrypted payload authentication failed",
            FailureClass::Padding => "encrypted payload padding is invalid",
            FailureClass::EnvelopeVersion => "transport envelope version is unsupported",
            FailureClass::Canonical => "embedded canonical event is invalid",
            FailureClass::IdentityAgreement => "transport and canonical identities disagree",
            FailureClass::OneUseKeyReuse => "one-use wrapper key was reused",
            FailureClass::AuthenticationInput => "relay authentication input is invalid",
            FailureClass::Cryptography => "transport cryptography failed",
        })
    }
}

impl Error for EnvelopeError {}
