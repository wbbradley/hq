//! Exact NIP-01 event carriage and cryptographic trust states.

use std::fmt;

use k256::{
    elliptic_curve::zeroize::Zeroize,
    schnorr::{Signature, SigningKey, VerifyingKey, signature::hazmat::PrehashVerifier},
};
use sha2::{Digest, Sha256};

use crate::{FailureClass, ProtocolError, json::JsonCursor};

/// Maximum bytes accepted for a complete outer event.
pub const MAX_EVENT_BYTES: usize = 4_194_304;
/// Maximum decoded bytes accepted for signed protocol content.
pub const MAX_CONTENT_BYTES: usize = 1_048_576;
/// Provisional regular Nostr kind carrying HQ protocol records.
pub const HQ_EVENT_KIND: u16 = 6000;
const MAX_CREATED_AT: u64 = (i64::MAX as u64) / 1000;

/// Bounded attacker-controlled event bytes before parsing.
///
/// Unverified bytes have no dispatch API:
///
/// ```compile_fail
/// use hq_protocol::RawEventBytes;
///
/// let raw = RawEventBytes::new(Vec::new()).unwrap();
/// raw.dispatch();
/// ```
pub struct RawEventBytes {
    bytes: Box<[u8]>,
}

impl RawEventBytes {
    /// Bounds and takes ownership of untrusted event bytes.
    pub fn new(bytes: Vec<u8>) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(ProtocolError::new(FailureClass::EventTooLarge));
        }
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
        })
    }

    /// Returns the exact received bytes without interpreting them.
    pub fn exact_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Strictly parses the exact HQ NIP-01 outer object.
    pub fn parse(self) -> Result<ParsedOuterEvent, ProtocolError> {
        ParsedOuterEvent::parse(self)
    }
}

impl fmt::Debug for RawEventBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawEventBytes")
            .field("length", &self.bytes.len())
            .finish()
    }
}

/// Strictly parsed outer fields whose identity and signature remain untrusted.
pub struct ParsedOuterEvent {
    raw: RawEventBytes,
    claimed_id: [u8; 32],
    public_key: [u8; 32],
    created_at: u64,
    kind: u16,
    tags_empty: bool,
    content: Box<[u8]>,
    signature: [u8; 64],
}

impl ParsedOuterEvent {
    fn parse(raw: RawEventBytes) -> Result<Self, ProtocolError> {
        if std::str::from_utf8(raw.exact_bytes()).is_err() {
            return Err(ProtocolError::new(FailureClass::OuterInvalidUtf8));
        }
        let mut cursor = JsonCursor::outer(raw.exact_bytes());
        cursor.expect(b"{\"id\":", FailureClass::OuterMemberOrder)?;
        let claimed_id = cursor.parse_hex()?;
        cursor.expect(b",\"pubkey\":", FailureClass::OuterMemberOrder)?;
        let public_key = cursor.parse_hex()?;
        cursor.expect(b",\"created_at\":", FailureClass::OuterMemberOrder)?;
        let created_at = cursor.parse_u64()?;
        if created_at > MAX_CREATED_AT {
            return Err(ProtocolError::new(FailureClass::OuterFieldShape));
        }
        cursor.expect(b",\"kind\":", FailureClass::OuterMemberOrder)?;
        let kind = u16::try_from(cursor.parse_u64()?)
            .map_err(|_| ProtocolError::new(FailureClass::OuterFieldShape))?;
        cursor.expect(b",\"tags\":", FailureClass::OuterMemberOrder)?;
        let tags_empty = if cursor.remaining().starts_with(b"[]") {
            cursor.expect(b"[]", FailureClass::OuterFieldShape)?;
            true
        } else {
            if cursor.peek() != Some(b'[') {
                return Err(ProtocolError::new(FailureClass::OuterFieldShape));
            }
            cursor.validate_value(1)?;
            false
        };
        cursor.expect(b",\"content\":", FailureClass::OuterMemberOrder)?;
        let content = cursor.parse_string(MAX_CONTENT_BYTES)?.into_boxed_slice();
        cursor.expect(b",\"sig\":", FailureClass::OuterMemberOrder)?;
        let signature = cursor.parse_hex()?;
        cursor.expect(b"}", FailureClass::OuterFieldShape)?;
        cursor.finish_outer()?;
        Ok(Self {
            raw,
            claimed_id,
            public_key,
            created_at,
            kind,
            tags_empty,
            content,
            signature,
        })
    }

    /// Returns the decoded content candidate before identity verification.
    pub fn content_bytes(&self) -> &[u8] {
        &self.content
    }

    /// Returns the claimed NIP-01 creation timestamp.
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Reconstructs identity and verifies the canonical BIP-340 signature.
    pub fn verify(self) -> Result<CryptographicallyVerifiedEvent, ProtocolError> {
        if self.kind != HQ_EVENT_KIND {
            return Err(ProtocolError::new(FailureClass::WrongKind));
        }
        if !self.tags_empty {
            return Err(ProtocolError::new(FailureClass::NonemptyTags));
        }
        let preimage = encode_event_preimage(self.public_key, self.created_at, &self.content)?;
        let event_id: [u8; 32] = Sha256::digest(&preimage).into();
        if !constant_time_equal(&event_id, &self.claimed_id) {
            return Err(ProtocolError::new(FailureClass::EventIdMismatch));
        }
        let key = VerifyingKey::from_slice(&self.public_key)
            .map_err(|_| ProtocolError::new(FailureClass::InvalidPublicKey))?;
        let signature = Signature::from_slice(&self.signature)
            .map_err(|_| ProtocolError::new(FailureClass::InvalidSignatureEncoding))?;
        key.verify_prehash(&event_id, &signature)
            .map_err(|_| ProtocolError::new(FailureClass::BadSignature))?;
        Ok(CryptographicallyVerifiedEvent {
            raw: self.raw.bytes,
            event_id,
            public_key: self.public_key,
            created_at: self.created_at,
            event_preimage: preimage.into_boxed_slice(),
            content: self.content,
        })
    }
}

impl fmt::Debug for ParsedOuterEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedOuterEvent")
            .field("claimed_id", &Hex(&self.claimed_id))
            .field("created_at", &self.created_at)
            .field("kind", &self.kind)
            .field("tags_empty", &self.tags_empty)
            .field("content_length", &self.content.len())
            .finish_non_exhaustive()
    }
}

/// Event whose exact NIP-01 ID and BIP-340 signature have been verified.
pub struct CryptographicallyVerifiedEvent {
    raw: Box<[u8]>,
    event_id: [u8; 32],
    public_key: [u8; 32],
    created_at: u64,
    event_preimage: Box<[u8]>,
    content: Box<[u8]>,
}

impl CryptographicallyVerifiedEvent {
    /// Returns the exact received or locally produced outer event bytes.
    pub fn exact_event_bytes(&self) -> &[u8] {
        &self.raw
    }

    /// Returns the verified event ID.
    pub const fn event_id(&self) -> [u8; 32] {
        self.event_id
    }

    /// Returns the verified x-only signing public key.
    pub const fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// Returns the verified NIP-01 creation timestamp.
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Returns the exact reconstructed NIP-01 event-ID preimage bytes.
    pub fn event_preimage_bytes(&self) -> &[u8] {
        &self.event_preimage
    }

    /// Returns the exact decoded event content bytes.
    pub fn content_bytes(&self) -> &[u8] {
        &self.content
    }
}

impl fmt::Debug for CryptographicallyVerifiedEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CryptographicallyVerifiedEvent")
            .field("event_id", &Hex(&self.event_id))
            .field("public_key", &Hex(&self.public_key))
            .field("created_at", &self.created_at)
            .field("content_length", &self.content.len())
            .finish_non_exhaustive()
    }
}

/// Zeroizing BIP-340 signer for exact HQ event IDs.
pub struct Bip340Signer {
    key: SigningKey,
}

impl Bip340Signer {
    /// Imports one 32-byte secret scalar, zeroizing the supplied copy before return.
    pub fn from_secret_bytes(mut secret: [u8; 32]) -> Result<Self, ProtocolError> {
        let key = SigningKey::from_slice(&secret)
            .map_err(|_| ProtocolError::new(FailureClass::InvalidSecretKey));
        secret.zeroize();
        key.map(|key| Self { key })
    }

    /// Returns the x-only public key without exposing secret material.
    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes().into()
    }

    /// Signs exact content using caller-supplied BIP-340 auxiliary randomness.
    pub fn sign(
        &self,
        created_at: u64,
        content: &[u8],
        auxiliary_randomness: [u8; 32],
    ) -> Result<CryptographicallyVerifiedEvent, ProtocolError> {
        if created_at > MAX_CREATED_AT {
            return Err(ProtocolError::new(FailureClass::OuterFieldShape));
        }
        if content.len() > MAX_CONTENT_BYTES {
            return Err(ProtocolError::new(FailureClass::ContentTooLarge));
        }
        let public_key = self.public_key();
        let preimage = encode_event_preimage(public_key, created_at, content)?;
        let event_id: [u8; 32] = Sha256::digest(&preimage).into();
        let signature = self
            .key
            .sign_raw(&event_id, &auxiliary_randomness)
            .map_err(|_| ProtocolError::new(FailureClass::SigningFailed))?;
        self.key
            .verifying_key()
            .verify_prehash(&event_id, &signature)
            .map_err(|_| ProtocolError::new(FailureClass::SigningFailed))?;
        let event = encode_signed_event(
            event_id,
            public_key,
            created_at,
            content,
            signature.to_bytes(),
        )?;
        RawEventBytes::new(event)?.parse()?.verify()
    }
}

/// Verifies a canonical BIP-340 signature over an exact message byte array.
pub fn verify_bip340(public_key: [u8; 32], message: [u8; 32], signature: [u8; 64]) -> bool {
    let Ok(key) = VerifyingKey::from_slice(&public_key) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(&signature) else {
        return false;
    };
    key.verify_prehash(&message, &signature).is_ok()
}

pub(crate) fn encode_event_preimage(
    public_key: [u8; 32],
    created_at: u64,
    content: &[u8],
) -> Result<Vec<u8>, ProtocolError> {
    let mut encoded = Vec::with_capacity(content.len().saturating_mul(2).saturating_add(128));
    encoded.extend_from_slice(b"[0,\"");
    append_hex(&mut encoded, &public_key);
    encoded.extend_from_slice(b"\",");
    append_decimal(&mut encoded, created_at);
    encoded.extend_from_slice(b",6000,[],");
    append_json_string(&mut encoded, content)?;
    encoded.push(b']');
    Ok(encoded)
}

fn encode_signed_event(
    event_id: [u8; 32],
    public_key: [u8; 32],
    created_at: u64,
    content: &[u8],
    signature: [u8; 64],
) -> Result<Vec<u8>, ProtocolError> {
    let mut encoded = Vec::with_capacity(content.len().saturating_mul(2).saturating_add(384));
    encoded.extend_from_slice(b"{\"id\":\"");
    append_hex(&mut encoded, &event_id);
    encoded.extend_from_slice(b"\",\"pubkey\":\"");
    append_hex(&mut encoded, &public_key);
    encoded.extend_from_slice(b"\",\"created_at\":");
    append_decimal(&mut encoded, created_at);
    encoded.extend_from_slice(b",\"kind\":6000,\"tags\":[],\"content\":");
    append_json_string(&mut encoded, content)?;
    encoded.extend_from_slice(b",\"sig\":\"");
    append_hex(&mut encoded, &signature);
    encoded.extend_from_slice(b"\"}");
    if encoded.len() > MAX_EVENT_BYTES {
        return Err(ProtocolError::new(FailureClass::EventTooLarge));
    }
    Ok(encoded)
}

fn append_json_string(output: &mut Vec<u8>, value: &[u8]) -> Result<(), ProtocolError> {
    let text = std::str::from_utf8(value)
        .map_err(|_| ProtocolError::new(FailureClass::ContentMalformed))?;
    output.push(b'\"');
    for character in text.chars() {
        match character {
            '\0' => return Err(ProtocolError::new(FailureClass::ContentNonCanonical)),
            '\"' => output.extend_from_slice(b"\\\""),
            '\\' => output.extend_from_slice(b"\\\\"),
            '\u{08}' => output.extend_from_slice(b"\\b"),
            '\u{0c}' => output.extend_from_slice(b"\\f"),
            '\n' => output.extend_from_slice(b"\\n"),
            '\r' => output.extend_from_slice(b"\\r"),
            '\t' => output.extend_from_slice(b"\\t"),
            '\u{01}'..='\u{1f}' => {
                output.extend_from_slice(b"\\u00");
                let value = character as u8;
                output.push(HEX[usize::from(value >> 4)]);
                output.push(HEX[usize::from(value & 0x0f)]);
            }
            _ => {
                let mut buffer = [0_u8; 4];
                output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    output.push(b'\"');
    Ok(())
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn append_hex(output: &mut Vec<u8>, value: &[u8]) {
    output.reserve(value.len().saturating_mul(2));
    for byte in value {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
}

fn append_decimal(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(value.to_string().as_bytes());
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

struct Hex<'a>(&'a [u8]);

impl fmt::Debug for Hex<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}
