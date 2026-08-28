//! Strict offline-verifiable human pairing invitation bundles.

use std::{collections::BTreeMap, collections::BTreeSet, error::Error, fmt};

use hq_domain::{
    AccountId, FactId, GrantId, InstallationAddress, RelayHints, SemanticPayload, ShortText,
};
use serde::{Deserialize, Serialize};

use crate::{ProtocolNamespace, VerifiedSemanticFact, decode_semantic_event};

/// Exact artifact schema name.
pub const PAIRING_INVITATION_V1: &str = "hq-human-pairing-invitation-v1";
/// Inclusive complete artifact byte bound.
pub const MAX_PAIRING_INVITATION_BYTES: usize = 1_048_576;
/// Inclusive canonical evidence count bound.
pub const MAX_PAIRING_INVITATION_FACTS: usize = 64;

/// Public signed grant metadata projected from a verified invitation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingGrant {
    /// Exact signed grant fact.
    pub fact_id: FactId,
    /// Human account being joined.
    pub account_id: AccountId,
    /// Stable creator-selected grant identity.
    pub grant_id: GrantId,
    /// Exact invited installation and signing key.
    pub device: InstallationAddress,
    /// Optional signed device label.
    pub label: Option<ShortText>,
    /// Signed non-authority transport hints.
    pub relay_hints: RelayHints,
}

/// Closed invitation rejection without attacker-controlled detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingInvitationError {
    /// Artifact exceeds its byte or fact-count bound.
    TooLarge,
    /// Envelope JSON or fixed-width values are malformed.
    Malformed,
    /// Envelope bytes are not the one canonical spelling.
    NonCanonical,
    /// One embedded event fails canonical cryptographic verification.
    InvalidEvidence,
    /// The named root is not one signed human-device grant.
    InvalidGrant,
    /// A required transitive parent is absent.
    IncompleteEvidence,
    /// Evidence unrelated to the signed grant closure was included.
    ExtraneousEvidence,
}

impl fmt::Display for PairingInvitationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("human pairing invitation is invalid")
    }
}

impl Error for PairingInvitationError {}

/// Canonical invitation whose complete signed causal closure has been verified.
pub struct VerifiedPairingInvitation {
    canonical_bytes: Vec<u8>,
    grant: PairingGrant,
    facts: BTreeMap<FactId, VerifiedSemanticFact>,
}

impl VerifiedPairingInvitation {
    /// Builds and canonicalizes an invitation from exact signed canonical events.
    pub fn from_evidence(
        grant_fact: FactId,
        events: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<Self, PairingInvitationError> {
        validate(grant_fact, events.into_iter().collect())
    }

    /// Strictly decodes one canonical invitation artifact.
    pub fn decode(bytes: &[u8]) -> Result<Self, PairingInvitationError> {
        if bytes.len() > MAX_PAIRING_INVITATION_BYTES {
            return Err(PairingInvitationError::TooLarge);
        }
        let raw: RawInvitation =
            serde_json::from_slice(bytes).map_err(|_| PairingInvitationError::Malformed)?;
        if raw.schema != PAIRING_INVITATION_V1 {
            return Err(PairingInvitationError::Malformed);
        }
        let grant_fact = decode_hex32(&raw.grant_fact)
            .map(FactId::from_bytes)
            .ok_or(PairingInvitationError::Malformed)?;
        let invitation = validate(
            grant_fact,
            raw.facts.into_iter().map(String::into_bytes).collect(),
        )?;
        if invitation.canonical_bytes != bytes {
            return Err(PairingInvitationError::NonCanonical);
        }
        Ok(invitation)
    }

    /// Returns exact canonical artifact bytes suitable for one new file.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// Returns the passive signed grant metadata.
    pub const fn grant(&self) -> &PairingGrant {
        &self.grant
    }

    /// Iterates the complete verified canonical ancestry in fact-ID order.
    pub fn facts(&self) -> impl ExactSizeIterator<Item = &VerifiedSemanticFact> {
        self.facts.values()
    }

    /// Iterates exact signed event bytes in fact-ID order.
    pub fn exact_events(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.facts
            .values()
            .map(|fact| fact.verified_event().exact_event_bytes())
    }
}

impl fmt::Debug for VerifiedPairingInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedPairingInvitation")
            .field("grant", &self.grant)
            .field("fact_count", &self.facts.len())
            .field("byte_length", &self.canonical_bytes.len())
            .finish()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawInvitation {
    schema: String,
    grant_fact: String,
    facts: Vec<String>,
}

fn validate(
    grant_fact: FactId,
    events: Vec<Vec<u8>>,
) -> Result<VerifiedPairingInvitation, PairingInvitationError> {
    if events.is_empty() || events.len() > MAX_PAIRING_INVITATION_FACTS {
        return Err(PairingInvitationError::TooLarge);
    }
    let mut facts = BTreeMap::new();
    for event in events {
        let fact = decode_semantic_event(event)
            .map_err(|_| PairingInvitationError::InvalidEvidence)?
            .ok_or(PairingInvitationError::InvalidEvidence)?;
        if fact.namespace() != ProtocolNamespace::Canonical
            || facts.insert(fact.fact().id(), fact).is_some()
        {
            return Err(PairingInvitationError::InvalidEvidence);
        }
    }
    let root = facts
        .get(&grant_fact)
        .ok_or(PairingInvitationError::InvalidGrant)?;
    let grant = match root.fact().payload() {
        SemanticPayload::HumanDeviceGranted {
            account_id,
            grant_id,
            device,
            label,
            relay_hints,
        } => PairingGrant {
            fact_id: grant_fact,
            account_id: *account_id,
            grant_id: *grant_id,
            device: *device,
            label: label.clone(),
            relay_hints: relay_hints.clone(),
        },
        _ => return Err(PairingInvitationError::InvalidGrant),
    };

    let mut visited = BTreeSet::new();
    let mut pending = vec![grant_fact];
    while let Some(current) = pending.pop() {
        if !visited.insert(current) {
            continue;
        }
        let fact = facts
            .get(&current)
            .ok_or(PairingInvitationError::IncompleteEvidence)?;
        pending.extend(fact.fact().causal().parents().iter().copied());
    }
    if visited.len() != facts.len() {
        return Err(PairingInvitationError::ExtraneousEvidence);
    }

    let raw = RawInvitation {
        schema: PAIRING_INVITATION_V1.to_owned(),
        grant_fact: encode_hex(grant_fact.as_bytes()),
        facts: facts
            .values()
            .map(|fact| {
                std::str::from_utf8(fact.verified_event().exact_event_bytes())
                    .map(str::to_owned)
                    .map_err(|_| PairingInvitationError::InvalidEvidence)
            })
            .collect::<Result<_, _>>()?,
    };
    let canonical_bytes =
        serde_json::to_vec(&raw).map_err(|_| PairingInvitationError::Malformed)?;
    if canonical_bytes.len() > MAX_PAIRING_INVITATION_BYTES {
        return Err(PairingInvitationError::TooLarge);
    }
    Ok(VerifiedPairingInvitation {
        canonical_bytes,
        grant,
        facts,
    })
}

fn encode_hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex32(value: &str) -> Option<[u8; 32]> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut decoded = [0; 32];
    for (index, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        decoded[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(decoded)
}

const fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use hq_domain::{
        AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, EncryptionPublicKey,
        FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, RelayHints,
        SigningPublicKey, Timestamp,
    };

    use super::*;
    use crate::{Bip340Signer, CanonicalEventPlan};

    fn signer(last: u8) -> Bip340Signer {
        let mut secret = [0; 32];
        secret[31] = last;
        Bip340Signer::from_secret_bytes(secret).expect("signer")
    }

    fn causal(
        parents: impl IntoIterator<Item = FactId>,
        authorities: impl IntoIterator<Item = AuthorityReference>,
    ) -> CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES> {
        CausalReferences::new(BoundedSet::new(parents).expect("parents"), authorities)
            .expect("causal references")
    }

    fn invitation_events() -> (FactId, Vec<Vec<u8>>) {
        let creator_signer = signer(1);
        let creator_id = InstallationId::from_bytes([1; 32]);
        let creator = InstallationAddress::new(
            creator_id,
            SigningPublicKey::from_bytes(creator_signer.public_key()),
        );
        let installation = CanonicalEventPlan::new(
            creator_id,
            Timestamp::from_unix_millis(0),
            FactScope::InstallationPrivate(creator_id),
            causal([], []),
            SemanticPayload::InstallationDeclared {
                installation_id: creator_id,
                signing_key: creator.signing_key(),
                encryption_key: EncryptionPublicKey::from_bytes([2; 32]),
                label: None,
            },
        )
        .sign(&creator_signer, [0; 32])
        .expect("installation");
        let account_id = AccountId::from_bytes([3; 32]);
        let account = CanonicalEventPlan::new(
            creator_id,
            Timestamp::from_unix_millis(0),
            FactScope::InstallationPrivate(creator_id),
            causal(
                [installation.fact().id()],
                [AuthorityReference::new(
                    AuthorityRole::LocalInstallation,
                    installation.fact().id(),
                )],
            ),
            SemanticPayload::HumanAccountCreated {
                account_id,
                creator,
                label: None,
            },
        )
        .sign(&creator_signer, [0; 32])
        .expect("account");
        let target_signer = signer(2);
        let target = InstallationAddress::new(
            InstallationId::from_bytes([4; 32]),
            SigningPublicKey::from_bytes(target_signer.public_key()),
        );
        let grant = CanonicalEventPlan::new(
            creator_id,
            Timestamp::from_unix_millis(0),
            FactScope::AccountAddressed(account_id),
            causal(
                [account.fact().id()],
                [AuthorityReference::new(
                    AuthorityRole::AccountCreator,
                    account.fact().id(),
                )],
            ),
            SemanticPayload::HumanDeviceGranted {
                account_id,
                grant_id: GrantId::from_bytes([5; 32]),
                device: target,
                label: Some(ShortText::new("laptop").expect("label")),
                relay_hints: RelayHints::new([]).expect("hints"),
            },
        )
        .sign(&creator_signer, [0; 32])
        .expect("grant");
        let grant_id = grant.fact().id();
        let events = [grant, installation, account]
            .into_iter()
            .map(|fact| fact.verified_event().exact_event_bytes().to_vec())
            .collect();
        (grant_id, events)
    }

    #[test]
    fn invitation_canonicalizes_and_strictly_verifies_the_exact_grant_closure() {
        let (grant, events) = invitation_events();
        let invitation =
            VerifiedPairingInvitation::from_evidence(grant, events).expect("invitation");
        let decoded = VerifiedPairingInvitation::decode(invitation.canonical_bytes())
            .expect("canonical invitation decodes");
        assert_eq!(decoded.grant().fact_id, grant);
        assert_eq!(
            decoded.grant().label.as_ref().map(ShortText::as_str),
            Some("laptop")
        );
        assert_eq!(decoded.facts().len(), 3);
    }

    #[test]
    fn invitation_rejects_missing_extraneous_tampered_and_noncanonical_evidence() {
        let (grant, mut events) = invitation_events();
        events.pop();
        assert!(matches!(
            VerifiedPairingInvitation::from_evidence(grant, events),
            Err(PairingInvitationError::IncompleteEvidence)
        ));

        let (grant, mut events) = invitation_events();
        let unrelated_signer = signer(3);
        let unrelated_id = InstallationId::from_bytes([9; 32]);
        let unrelated = CanonicalEventPlan::new(
            unrelated_id,
            Timestamp::from_unix_millis(0),
            FactScope::InstallationPrivate(unrelated_id),
            causal([], []),
            SemanticPayload::InstallationDeclared {
                installation_id: unrelated_id,
                signing_key: SigningPublicKey::from_bytes(unrelated_signer.public_key()),
                encryption_key: EncryptionPublicKey::from_bytes([8; 32]),
                label: None,
            },
        )
        .sign(&unrelated_signer, [0; 32])
        .expect("unrelated canonical fact")
        .verified_event()
        .exact_event_bytes()
        .to_vec();
        events.push(unrelated);
        assert!(matches!(
            VerifiedPairingInvitation::from_evidence(grant, events),
            Err(PairingInvitationError::ExtraneousEvidence)
        ));

        let (grant, events) = invitation_events();
        let invitation =
            VerifiedPairingInvitation::from_evidence(grant, events).expect("invitation");
        let mut tampered = invitation.canonical_bytes().to_vec();
        let index = tampered.len() / 2;
        tampered[index] ^= 1;
        assert!(VerifiedPairingInvitation::decode(&tampered).is_err());

        let mut noncanonical = invitation.canonical_bytes().to_vec();
        noncanonical.push(b'\n');
        assert!(matches!(
            VerifiedPairingInvitation::decode(&noncanonical),
            Err(PairingInvitationError::NonCanonical)
        ));
    }
}
