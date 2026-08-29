//! HQ envelope v1 over NIP-59.

use k256::schnorr::{Signature, SigningKey, VerifyingKey, signature::hazmat::PrehashVerifier};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{
    CLIENT_AUTH_KIND, EnvelopeError, FailureClass, GIFT_WRAP_KIND, HQ_RUMOR_KIND,
    MAX_GIFT_WRAP_BYTES, MAX_PLAINTEXT_BYTES, SEAL_KIND, nip44,
};

const TWO_DAYS_SECONDS: u64 = 172_800;
const MAX_RELAY_URL_BYTES: usize = 2_048;
const MAX_CHALLENGE_BYTES: usize = 1_024;

/// Fallible cryptographic random byte source.
pub trait RandomSource {
    /// Fills the target with independent cryptographic random bytes.
    fn fill(&mut self, target: &mut [u8]) -> Result<(), EnvelopeError>;
}

/// Operating-system cryptographic randomness.
pub struct SystemRandom;

impl RandomSource for SystemRandom {
    fn fill(&mut self, target: &mut [u8]) -> Result<(), EnvelopeError> {
        getrandom::fill(target).map_err(|_| EnvelopeError::new(FailureClass::Cryptography))
    }
}

/// Public metadata committed with exact wrapper bytes before publishing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedEnvelopeMetadata {
    /// Verified kind-1059 event ID.
    pub wrapper_id: [u8; 32],
    /// Fresh one-use outer signer public key.
    pub one_use_public_key: [u8; 32],
    /// Exact recipient root public key.
    pub recipient_public_key: [u8; 32],
    /// Embedded canonical event ID.
    pub canonical_event_id: [u8; 32],
    /// Digest of exact embedded canonical bytes.
    pub canonical_sha256: [u8; 32],
    /// Digest of exact publish bytes.
    pub wrapper_sha256: [u8; 32],
    /// Randomized seal timestamp.
    pub seal_created_at: u64,
    /// Independently randomized gift-wrap timestamp.
    pub gift_wrap_created_at: u64,
    /// Exact publish byte length.
    pub byte_len: usize,
}

/// Validated retry lineage whose exact publish bytes cannot be mutated.
pub struct PreparedEnvelope {
    /// Plain persistence and audit metadata.
    pub metadata: PreparedEnvelopeMetadata,
    exact_wire: Box<[u8]>,
}

impl PreparedEnvelope {
    /// Borrows the only bytes that may be published for this retry lineage.
    pub fn exact_wire(&self) -> &[u8] {
        &self.exact_wire
    }

    /// Converts the prepared lineage into a plain durable record.
    pub fn into_durable(self) -> DurableEnvelope {
        DurableEnvelope {
            metadata: self.metadata,
            exact_wire: self.exact_wire.into_vec(),
        }
    }

    /// Reconstructs a retry lineage after verifying all relay-visible metadata.
    pub fn restore(record: DurableEnvelope) -> Result<Self, EnvelopeError> {
        let outer = parse_signed(&record.exact_wire)?;
        verify_signed(&outer)?;
        let recipient = one_recipient(&outer.tags)?;
        let wire_digest: [u8; 32] = Sha256::digest(&record.exact_wire).into();
        if outer.kind != GIFT_WRAP_KIND
            || decode_hex::<32>(&outer.id)? != record.metadata.wrapper_id
            || decode_hex::<32>(&outer.pubkey)? != record.metadata.one_use_public_key
            || recipient != record.metadata.recipient_public_key
            || outer.created_at != record.metadata.gift_wrap_created_at
            || record.exact_wire.len() != record.metadata.byte_len
            || wire_digest != record.metadata.wrapper_sha256
        {
            return Err(EnvelopeError::new(FailureClass::IdentityAgreement));
        }
        Ok(Self {
            metadata: record.metadata,
            exact_wire: record.exact_wire.into_boxed_slice(),
        })
    }
}

/// Plain store-facing representation of a prepared retry lineage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableEnvelope {
    /// Integrity-checked public metadata.
    pub metadata: PreparedEnvelopeMetadata,
    /// Exact signed wrapper bytes.
    pub exact_wire: Vec<u8>,
}

/// Transport audit metadata produced while opening an envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenedEnvelopeMetadata {
    /// Verified outer wrapper ID.
    pub wrapper_id: [u8; 32],
    /// Verified outer wrapper timestamp used only for retained traversal.
    pub wrapper_created_at: u64,
    /// Verified one-use outer public key.
    pub one_use_public_key: [u8; 32],
    /// Verified sender root public key.
    pub sender_public_key: [u8; 32],
    /// Origin installation ID copied from the verified canonical DTO.
    pub origin_installation_id: [u8; 32],
    /// Verified canonical event ID copied from the embedded event.
    pub canonical_event_id: [u8; 32],
}

/// Successfully opened transport input, still carrying no domain authority.
pub struct OpenedEnvelope {
    /// Exact raw canonical bytes for the ordinary common ingest path.
    pub canonical_event: Box<[u8]>,
    /// Non-authoritative transport audit metadata.
    pub metadata: OpenedEnvelopeMetadata,
}

/// Bounded NIP-42 event inputs tied to one relay connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthInput {
    /// Exact active relay URL.
    pub relay_url: String,
    /// Exact latest challenge from that connection.
    pub challenge: String,
    /// Current Unix time in seconds.
    pub created_at: u64,
}

/// Installation-root envelope codec.
pub struct EnvelopeCodec {
    key: SigningKey,
}

impl EnvelopeCodec {
    /// Imports and zeroizes a 32-byte installation root secret.
    pub fn from_secret_bytes(mut secret: [u8; 32]) -> Result<Self, EnvelopeError> {
        let key = SigningKey::from_slice(&secret)
            .map_err(|_| EnvelopeError::new(FailureClass::InvalidPublicKey));
        secret.zeroize();
        key.map(|key| Self { key })
    }

    /// Returns the normalized x-only root public key.
    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes().into()
    }

    /// Creates one immutable retry lineage from an already verified canonical record.
    pub fn prepare(
        &self,
        canonical: &hq_protocol::VerifiedSemanticFact,
        recipient_public_key: [u8; 32],
        now: u64,
        random: &mut impl RandomSource,
    ) -> Result<PreparedEnvelope, EnvelopeError> {
        VerifyingKey::from_slice(&recipient_public_key)
            .map_err(|_| EnvelopeError::new(FailureClass::InvalidPublicKey))?;
        let verified = canonical.verified_event();
        if verified.public_key() != self.public_key() {
            return Err(EnvelopeError::new(FailureClass::IdentityAgreement));
        }
        let canonical_bytes = verified.exact_event_bytes();
        let canonical_raw = RawValue::from_string(
            std::str::from_utf8(canonical_bytes)
                .map_err(|_| EnvelopeError::new(FailureClass::Canonical))?
                .to_owned(),
        )
        .map_err(|_| EnvelopeError::new(FailureClass::Canonical))?;
        let envelope = EnvelopeWire {
            schema: 1,
            envelope_type: "hq.canonical",
            origin_installation_id: hex(canonical.fact().author().installation_id().as_bytes()),
            canonical_event_id: hex(&verified.event_id()),
            canonical_event: canonical_raw,
        };
        let envelope_json = serde_json::to_string(&envelope)
            .map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))?;
        let rumor = UnsignedEventWire::new(
            self.public_key(),
            verified.created_at(),
            HQ_RUMOR_KIND,
            recipient_tags(recipient_public_key),
            envelope_json,
        )?;
        let rumor_json = serde_json::to_vec(&rumor)
            .map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))?;
        let mut seal_key = nip44::conversation_key(&self.key, recipient_public_key)?;
        let seal_cipher_result = nip44::encrypt(&rumor_json, &seal_key, random_array(random)?);
        seal_key.zeroize();
        let seal_cipher = seal_cipher_result?;
        let seal_created_at = random_past(now, random)?;
        let seal = sign_event(
            &self.key,
            seal_created_at,
            SEAL_KIND,
            Vec::new(),
            seal_cipher,
            random_array(random)?,
        )?;
        let seal_json = serde_json::to_vec(&seal)
            .map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))?;
        let one_use = random_signing_key(random)?;
        let mut outer_key = nip44::conversation_key(&one_use, recipient_public_key)?;
        let outer_cipher_result = nip44::encrypt(&seal_json, &outer_key, random_array(random)?);
        outer_key.zeroize();
        let outer_cipher = outer_cipher_result?;
        let gift_wrap_created_at = random_past(now, random)?;
        let outer = sign_event(
            &one_use,
            gift_wrap_created_at,
            GIFT_WRAP_KIND,
            recipient_tags(recipient_public_key),
            outer_cipher,
            random_array(random)?,
        )?;
        let exact_wire = serde_json::to_vec(&outer)
            .map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))?;
        if exact_wire.is_empty() || exact_wire.len() > MAX_GIFT_WRAP_BYTES {
            return Err(EnvelopeError::new(FailureClass::Size));
        }
        let metadata = PreparedEnvelopeMetadata {
            wrapper_id: decode_hex(&outer.id)?,
            one_use_public_key: one_use.verifying_key().to_bytes().into(),
            recipient_public_key,
            canonical_event_id: verified.event_id(),
            canonical_sha256: Sha256::digest(canonical_bytes).into(),
            wrapper_sha256: Sha256::digest(&exact_wire).into(),
            seal_created_at,
            gift_wrap_created_at,
            byte_len: exact_wire.len(),
        };
        Ok(PreparedEnvelope {
            metadata,
            exact_wire: exact_wire.into_boxed_slice(),
        })
    }

    /// Opens one bounded wrapper and yields exact raw canonical bytes without domain authority.
    pub fn open(&self, raw: &[u8]) -> Result<OpenedEnvelope, EnvelopeError> {
        if raw.is_empty() || raw.len() > MAX_GIFT_WRAP_BYTES {
            return Err(EnvelopeError::new(FailureClass::Size));
        }
        let outer = parse_signed(raw)?;
        let outer_public = verify_signed(&outer)?;
        if outer.kind != GIFT_WRAP_KIND {
            return Err(EnvelopeError::new(FailureClass::LayerShape));
        }
        if one_recipient(&outer.tags)? != self.public_key() {
            return Err(EnvelopeError::new(FailureClass::Recipient));
        }
        let mut outer_key = nip44::conversation_key(&self.key, outer_public)?;
        let seal_json_result = nip44::decrypt(&outer.content, &outer_key);
        outer_key.zeroize();
        let mut seal_json = seal_json_result?;
        bound_plaintext(&seal_json)?;
        let seal = parse_signed(&seal_json)?;
        seal_json.zeroize();
        let sender = verify_signed(&seal)?;
        if seal.kind != SEAL_KIND || !seal.tags.is_empty() {
            return Err(EnvelopeError::new(FailureClass::LayerShape));
        }
        let mut seal_key = nip44::conversation_key(&self.key, sender)?;
        let rumor_json_result = nip44::decrypt(&seal.content, &seal_key);
        seal_key.zeroize();
        let mut rumor_json = rumor_json_result?;
        bound_plaintext(&rumor_json)?;
        let rumor: UnsignedEventWire = strict_json(&rumor_json)?;
        rumor_json.zeroize();
        rumor.verify_id()?;
        if rumor.kind != HQ_RUMOR_KIND
            || decode_hex::<32>(&rumor.pubkey)? != sender
            || one_recipient(&rumor.tags)? != self.public_key()
        {
            return Err(EnvelopeError::new(FailureClass::IdentityAgreement));
        }
        let envelope: EnvelopeOwned = strict_json(rumor.content.as_bytes())?;
        if envelope.schema != 1 || envelope.envelope_type != "hq.canonical" {
            return Err(EnvelopeError::new(FailureClass::EnvelopeVersion));
        }
        let canonical_event = envelope.canonical_event.get().as_bytes().to_vec();
        let verified = hq_protocol::RawEventBytes::new(canonical_event.clone())
            .and_then(hq_protocol::RawEventBytes::parse)
            .and_then(hq_protocol::ParsedOuterEvent::verify)
            .map_err(|_| EnvelopeError::new(FailureClass::Canonical))?;
        let canonical_id = verified.event_id();
        let canonical_public = verified.public_key();
        let supported = match verified
            .dispatch()
            .map_err(|_| EnvelopeError::new(FailureClass::Canonical))?
        {
            hq_protocol::DispatchOutcome::Supported(value) => value,
            hq_protocol::DispatchOutcome::Unsupported(_) => {
                return Err(EnvelopeError::new(FailureClass::Canonical));
            }
        };
        let semantic = supported
            .decode_v1()
            .and_then(hq_protocol::VerifiedSupportedRecord::into_semantic_fact)
            .map_err(|_| EnvelopeError::new(FailureClass::Canonical))?;
        let origin = *semantic.fact().author().installation_id().as_bytes();
        if decode_hex::<32>(&envelope.canonical_event_id)? != canonical_id
            || decode_hex::<32>(&envelope.origin_installation_id)? != origin
            || canonical_public != sender
        {
            return Err(EnvelopeError::new(FailureClass::IdentityAgreement));
        }
        Ok(OpenedEnvelope {
            canonical_event: canonical_event.into_boxed_slice(),
            metadata: OpenedEnvelopeMetadata {
                wrapper_id: decode_hex(&outer.id)?,
                wrapper_created_at: outer.created_at,
                one_use_public_key: outer_public,
                sender_public_key: sender,
                origin_installation_id: origin,
                canonical_event_id: canonical_id,
            },
        })
    }

    /// Creates exact signed NIP-42 event bytes from bounded connection inputs.
    pub fn authentication_event(
        &self,
        input: AuthInput,
        auxiliary_randomness: [u8; 32],
    ) -> Result<Vec<u8>, EnvelopeError> {
        validate_auth(&input.relay_url, MAX_RELAY_URL_BYTES)?;
        validate_auth(&input.challenge, MAX_CHALLENGE_BYTES)?;
        let event = sign_event(
            &self.key,
            input.created_at,
            CLIENT_AUTH_KIND,
            vec![
                vec!["relay".into(), input.relay_url],
                vec!["challenge".into(), input.challenge],
            ],
            String::new(),
            auxiliary_randomness,
        )?;
        serde_json::to_vec(&event).map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))
    }
}

pub(crate) fn verified_transport_event_id(raw: &[u8]) -> Result<[u8; 32], EnvelopeError> {
    let event = parse_signed(raw)?;
    verify_signed(&event)?;
    decode_hex(&event.id)
}

/// Enforces the durable one-use public-key uniqueness claim.
pub fn check_one_use_key_claim(
    existing_wrapper_id: [u8; 32],
    candidate_wrapper_id: [u8; 32],
) -> Result<(), EnvelopeError> {
    if existing_wrapper_id == candidate_wrapper_id {
        Ok(())
    } else {
        Err(EnvelopeError::new(FailureClass::OneUseKeyReuse))
    }
}

#[derive(Serialize)]
struct EnvelopeWire {
    schema: u8,
    #[serde(rename = "type")]
    envelope_type: &'static str,
    origin_installation_id: String,
    canonical_event_id: String,
    canonical_event: Box<RawValue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeOwned {
    schema: u8,
    #[serde(rename = "type")]
    envelope_type: String,
    origin_installation_id: String,
    canonical_event_id: String,
    canonical_event: Box<RawValue>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UnsignedEventWire {
    id: String,
    pubkey: String,
    created_at: u64,
    kind: u16,
    tags: Vec<Vec<String>>,
    content: String,
}

impl UnsignedEventWire {
    fn new(
        pubkey: [u8; 32],
        created_at: u64,
        kind: u16,
        tags: Vec<Vec<String>>,
        content: String,
    ) -> Result<Self, EnvelopeError> {
        let mut event = Self {
            id: String::new(),
            pubkey: hex(&pubkey),
            created_at,
            kind,
            tags,
            content,
        };
        event.id = hex(&event.computed_id()?);
        Ok(event)
    }
    fn computed_id(&self) -> Result<[u8; 32], EnvelopeError> {
        event_id(
            &self.pubkey,
            self.created_at,
            self.kind,
            &self.tags,
            &self.content,
        )
    }
    fn verify_id(&self) -> Result<(), EnvelopeError> {
        if decode_hex::<32>(&self.id)? == self.computed_id()? {
            Ok(())
        } else {
            Err(EnvelopeError::new(FailureClass::EventIdentity))
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedEventWire {
    id: String,
    pubkey: String,
    created_at: u64,
    kind: u16,
    tags: Vec<Vec<String>>,
    content: String,
    sig: String,
}

fn sign_event(
    key: &SigningKey,
    created_at: u64,
    kind: u16,
    tags: Vec<Vec<String>>,
    content: String,
    auxiliary_randomness: [u8; 32],
) -> Result<SignedEventWire, EnvelopeError> {
    let pubkey = hex(key.verifying_key().to_bytes().as_ref());
    let id = event_id(&pubkey, created_at, kind, &tags, &content)?;
    let signature = key
        .sign_raw(&id, &auxiliary_randomness)
        .map_err(|_| EnvelopeError::new(FailureClass::Cryptography))?;
    Ok(SignedEventWire {
        id: hex(&id),
        pubkey,
        created_at,
        kind,
        tags,
        content,
        sig: hex(signature.to_bytes().as_ref()),
    })
}

fn verify_signed(event: &SignedEventWire) -> Result<[u8; 32], EnvelopeError> {
    let claimed = decode_hex::<32>(&event.id)?;
    if claimed
        != event_id(
            &event.pubkey,
            event.created_at,
            event.kind,
            &event.tags,
            &event.content,
        )?
    {
        return Err(EnvelopeError::new(FailureClass::EventIdentity));
    }
    let public = decode_hex::<32>(&event.pubkey)?;
    let key = VerifyingKey::from_slice(&public)
        .map_err(|_| EnvelopeError::new(FailureClass::InvalidPublicKey))?;
    let signature = Signature::from_slice(&decode_hex::<64>(&event.sig)?)
        .map_err(|_| EnvelopeError::new(FailureClass::Signature))?;
    key.verify_prehash(&claimed, &signature)
        .map_err(|_| EnvelopeError::new(FailureClass::Signature))?;
    Ok(public)
}

fn event_id(
    pubkey: &str,
    created_at: u64,
    kind: u16,
    tags: &[Vec<String>],
    content: &str,
) -> Result<[u8; 32], EnvelopeError> {
    Ok(Sha256::digest(
        serde_json::to_vec(&(0_u8, pubkey, created_at, kind, tags, content))
            .map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))?,
    )
    .into())
}
fn parse_signed(raw: &[u8]) -> Result<SignedEventWire, EnvelopeError> {
    strict_json(raw)
}
fn strict_json<T: for<'de> Deserialize<'de>>(raw: &[u8]) -> Result<T, EnvelopeError> {
    let mut deserializer = serde_json::Deserializer::from_slice(raw);
    let value = T::deserialize(&mut deserializer)
        .map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))?;
    deserializer
        .end()
        .map_err(|_| EnvelopeError::new(FailureClass::MalformedJson))?;
    Ok(value)
}
fn recipient_tags(recipient: [u8; 32]) -> Vec<Vec<String>> {
    vec![vec!["p".into(), hex(&recipient)]]
}
fn one_recipient(tags: &[Vec<String>]) -> Result<[u8; 32], EnvelopeError> {
    if tags.len() != 1 || tags[0].len() != 2 || tags[0][0] != "p" {
        return Err(EnvelopeError::new(FailureClass::LayerShape));
    }
    decode_hex(&tags[0][1])
}
fn random_array(random: &mut impl RandomSource) -> Result<[u8; 32], EnvelopeError> {
    let mut bytes = [0; 32];
    random.fill(&mut bytes)?;
    Ok(bytes)
}
fn random_signing_key(random: &mut impl RandomSource) -> Result<SigningKey, EnvelopeError> {
    for _ in 0..128 {
        let mut bytes = random_array(random)?;
        let key = SigningKey::from_slice(&bytes);
        bytes.zeroize();
        if let Ok(key) = key {
            return Ok(key);
        }
    }
    Err(EnvelopeError::new(FailureClass::Cryptography))
}
fn random_past(now: u64, random: &mut impl RandomSource) -> Result<u64, EnvelopeError> {
    let bytes = random_array(random)?;
    let offset = u64::from_be_bytes(
        bytes[..8]
            .try_into()
            .map_err(|_| EnvelopeError::new(FailureClass::Cryptography))?,
    ) % (TWO_DAYS_SECONDS + 1);
    Ok(now.saturating_sub(offset))
}
fn validate_auth(value: &str, maximum: usize) -> Result<(), EnvelopeError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        Err(EnvelopeError::new(FailureClass::AuthenticationInput))
    } else {
        Ok(())
    }
}
fn bound_plaintext(bytes: &[u8]) -> Result<(), EnvelopeError> {
    if bytes.is_empty() || bytes.len() > MAX_PLAINTEXT_BYTES {
        Err(EnvelopeError::new(FailureClass::Size))
    } else {
        Ok(())
    }
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], EnvelopeError> {
    if value.len() != N * 2
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(EnvelopeError::new(FailureClass::MalformedJson));
    }
    let mut output = [0; N];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        output[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(output)
}

fn nibble(byte: u8) -> Result<u8, EnvelopeError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(EnvelopeError::new(FailureClass::MalformedJson)),
    }
}

/// Versioned public owner of the independently specified HQ envelope protocol.
pub mod v1 {
    pub use super::{
        AuthInput, DurableEnvelope, EnvelopeCodec, OpenedEnvelope, OpenedEnvelopeMetadata,
        PreparedEnvelope, PreparedEnvelopeMetadata, RandomSource, SystemRandom,
        check_one_use_key_claim,
    };
    pub use crate::{
        CLIENT_AUTH_KIND, EnvelopeError, FailureClass, GIFT_WRAP_KIND, HQ_RUMOR_KIND,
        MAX_GIFT_WRAP_BYTES, MAX_NIP44_PAYLOAD_BYTES, MAX_PLAINTEXT_BYTES,
        MAX_QUARANTINE_SAMPLE_BYTES, SEAL_KIND,
    };
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use hq_domain::{
        BoundedSet, CausalReferences, EncryptionPublicKey, FactScope, InstallationId,
        SemanticPayload, SigningPublicKey, Timestamp,
    };
    use hq_protocol::{Bip340Signer, CanonicalEventPlan, VerifiedSemanticFact};

    use super::*;

    struct SequenceRandom(u8);

    impl RandomSource for SequenceRandom {
        fn fill(&mut self, target: &mut [u8]) -> Result<(), EnvelopeError> {
            for byte in target {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
            Ok(())
        }
    }

    #[test]
    fn prepared_wrapper_round_trips_and_retries_exact_bytes() {
        let sender = codec(1);
        let recipient = codec(2);
        let canonical = canonical(1);
        let mut random = SequenceRandom(1);
        let first = sender
            .prepare(
                &canonical,
                recipient.public_key(),
                1_800_000_000,
                &mut random,
            )
            .expect("wrapper prepares");
        let first_bytes = first.exact_wire().to_vec();
        assert!(first.metadata.seal_created_at <= 1_800_000_000);
        assert!(first.metadata.seal_created_at >= 1_800_000_000 - TWO_DAYS_SECONDS);
        assert!(first.metadata.gift_wrap_created_at <= 1_800_000_000);
        assert_ne!(
            first.metadata.seal_created_at,
            first.metadata.gift_wrap_created_at
        );

        let restored = PreparedEnvelope::restore(first.into_durable()).expect("record restores");
        assert_eq!(restored.exact_wire(), first_bytes);
        let opened = recipient
            .open(restored.exact_wire())
            .expect("recipient opens");
        assert_eq!(
            opened.canonical_event.as_ref(),
            canonical.verified_event().exact_event_bytes()
        );
        assert_eq!(opened.metadata.sender_public_key, sender.public_key());

        let second = sender
            .prepare(
                &canonical,
                recipient.public_key(),
                1_800_000_000,
                &mut random,
            )
            .expect("second wrapper prepares");
        assert_ne!(
            restored.metadata.one_use_public_key,
            second.metadata.one_use_public_key
        );
        assert_ne!(restored.exact_wire(), second.exact_wire());
    }

    #[test]
    fn rejects_outer_tamper_wrong_recipient_signer_mismatch_and_size() {
        let sender = codec(1);
        let recipient = codec(2);
        let mut random = SequenceRandom(9);
        let prepared = sender
            .prepare(
                &canonical(1),
                recipient.public_key(),
                1_800_000_000,
                &mut random,
            )
            .expect("wrapper prepares");
        let mut tampered = prepared.exact_wire().to_vec();
        let content = tampered
            .iter()
            .position(|byte| *byte == b'A')
            .expect("base64 exists");
        tampered[content] = b'B';
        assert!(matches!(
            recipient.open(&tampered).map_err(EnvelopeError::class),
            Err(FailureClass::EventIdentity
                | FailureClass::Signature
                | FailureClass::MalformedJson)
        ));
        assert!(matches!(
            codec(3)
                .open(prepared.exact_wire())
                .map_err(EnvelopeError::class),
            Err(FailureClass::Recipient)
        ));
        assert!(matches!(
            sender
                .prepare(
                    &canonical(3),
                    recipient.public_key(),
                    1_800_000_000,
                    &mut random
                )
                .map_err(EnvelopeError::class),
            Err(FailureClass::IdentityAgreement)
        ));
        assert!(matches!(
            recipient
                .open(&vec![0; MAX_GIFT_WRAP_BYTES + 1])
                .map_err(EnvelopeError::class),
            Err(FailureClass::Size)
        ));
    }

    #[test]
    fn one_use_claim_is_idempotent_only_for_same_wrapper() {
        assert_eq!(check_one_use_key_claim([1; 32], [1; 32]), Ok(()));
        assert_eq!(
            check_one_use_key_claim([1; 32], [2; 32]).map_err(EnvelopeError::class),
            Err(FailureClass::OneUseKeyReuse)
        );
    }

    #[test]
    fn authentication_event_has_exact_nip42_inputs_and_bounds() {
        let codec = codec(4);
        let wire = codec
            .authentication_event(
                AuthInput {
                    relay_url: "wss://relay.example/".into(),
                    challenge: "challenge".into(),
                    created_at: 1_800_000_000,
                },
                [7; 32],
            )
            .expect("auth signs");
        let event = parse_signed(&wire).expect("auth parses");
        assert_eq!(event.kind, CLIENT_AUTH_KIND);
        assert_eq!(
            event.tags,
            vec![
                vec!["relay", "wss://relay.example/"],
                vec!["challenge", "challenge"]
            ]
        );
        assert_eq!(
            verify_signed(&event).expect("auth verifies"),
            codec.public_key()
        );
        assert_eq!(
            verified_transport_event_id(&wire).expect("transport event identity verifies"),
            decode_hex(&event.id).expect("event identity decodes")
        );
        assert_eq!(
            codec
                .authentication_event(
                    AuthInput {
                        relay_url: String::new(),
                        challenge: "x".into(),
                        created_at: 0
                    },
                    [0; 32]
                )
                .map_err(EnvelopeError::class),
            Err(FailureClass::AuthenticationInput)
        );
    }

    fn codec(value: u8) -> EnvelopeCodec {
        EnvelopeCodec::from_secret_bytes(secret(value)).expect("valid secret")
    }

    fn canonical(value: u8) -> VerifiedSemanticFact {
        let secret = secret(value);
        let signer = Bip340Signer::from_secret_bytes(secret).expect("valid signer");
        let installation = InstallationId::from_bytes([0x11; 32]);
        let public = signer.public_key();
        CanonicalEventPlan::new(
            installation,
            Timestamp::from_unix_millis(1_800_000_000_000),
            FactScope::InstallationPrivate(installation),
            CausalReferences::new(BoundedSet::new([]).expect("empty parent set"), [])
                .expect("empty causal set"),
            SemanticPayload::InstallationDeclared {
                installation_id: installation,
                signing_key: SigningPublicKey::from_bytes(public),
                encryption_key: EncryptionPublicKey::from_bytes(public),
                label: None,
            },
        )
        .sign(&signer, [value; 32])
        .expect("canonical signs")
    }

    fn secret(value: u8) -> [u8; 32] {
        let mut secret = [0; 32];
        secret[31] = value;
        secret
    }
}
