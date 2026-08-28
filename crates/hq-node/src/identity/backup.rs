//! Bounded canonical identity backup package and interoperable NIP-49 encryption.

use std::path::Path;

use bech32::{Bech32, Hrp, primitives::decode::CheckedHrpstring};
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, KeyInit, Payload, array::Array},
};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use super::{
    IdentityError, IdentityErrorClass, InstallationIdentity,
    atomic::{WriteMode, atomic_write},
    decode_hex, encode_hex, read_private_bounded,
};

const BACKUP_MAX_BYTES: u64 = 4_096;
const PASSWORD_MAX_BYTES: usize = 1_024;
const NIP49_VERSION: u8 = 2;
const NIP49_EXPORT_LOG_N: u8 = 16;
const NIP49_MIN_LOG_N: u8 = 16;
const NIP49_MAX_LOG_N: u8 = 18;
const NIP49_SECURITY_BYTE: u8 = 1;
const NIP49_DECODED_BYTES: usize = 91;
const NIP49_HRP: &str = "ncryptsec";

/// Owned normalized backup password that zeroizes its allocation on drop.
pub struct BackupPassword(Zeroizing<String>);

impl BackupPassword {
    /// NFKC-normalizes and bounds a nonempty password.
    pub fn new(password: String) -> Result<Self, IdentityError> {
        let mut supplied = Zeroizing::new(password);
        let normalized = Zeroizing::new(supplied.nfkc().collect::<String>());
        supplied.clear();
        if normalized.is_empty() || normalized.len() > PASSWORD_MAX_BYTES {
            return Err(IdentityError::new(IdentityErrorClass::PasswordInvalid));
        }
        Ok(Self(normalized))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl std::fmt::Debug for BackupPassword {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BackupPassword([REDACTED])")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupDto {
    format: String,
    version: u64,
    installation: String,
    public_key: String,
    ncryptsec: String,
}

pub(super) fn export(
    identity: &InstallationIdentity,
    password: &BackupPassword,
    path: &Path,
) -> Result<(), IdentityError> {
    let encrypted = nip49_encrypt(identity.secret_bytes(), password, NIP49_EXPORT_LOG_N)?;
    let public = identity.public_identity();
    let dto = BackupDto {
        format: "hq-identity-backup".to_owned(),
        version: 1,
        installation: encode_hex(public.installation_id.as_bytes()),
        public_key: encode_hex(&public.signing_public_key),
        ncryptsec: encrypted,
    };
    let encoded = serde_json::to_vec(&dto)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > BACKUP_MAX_BYTES {
        return Err(IdentityError::new(IdentityErrorClass::BackupMalformed));
    }
    atomic_write(
        path,
        &encoded,
        WriteMode::CreateNew(IdentityErrorClass::BackupExists),
    )
}

pub(super) fn import(
    path: &Path,
    password: &BackupPassword,
) -> Result<InstallationIdentity, IdentityError> {
    let bytes = read_private_bounded(path, BACKUP_MAX_BYTES, IdentityErrorClass::BackupMalformed)?;
    let dto: BackupDto = serde_json::from_slice(&bytes)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    let canonical = serde_json::to_vec(&dto)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    if canonical != bytes || dto.format != "hq-identity-backup" || dto.version != 1 {
        return Err(IdentityError::new(IdentityErrorClass::BackupMalformed));
    }
    let installation = decode_hex(&dto.installation, IdentityErrorClass::BackupMalformed)?;
    let expected_public = decode_hex(&dto.public_key, IdentityErrorClass::BackupMalformed)?;
    let secret = nip49_decrypt(&dto.ncryptsec, password)?;
    let identity = InstallationIdentity::from_parts(installation, *secret)?;
    if identity.public_identity().signing_public_key != expected_public {
        return Err(IdentityError::new(IdentityErrorClass::BackupMalformed));
    }
    Ok(identity)
}

fn nip49_encrypt(
    secret: &[u8; 32],
    password: &BackupPassword,
    log_n: u8,
) -> Result<String, IdentityError> {
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 24];
    getrandom::fill(&mut salt)
        .and_then(|()| getrandom::fill(&mut nonce))
        .map_err(|_| IdentityError::new(IdentityErrorClass::EntropyUnavailable))?;
    nip49_encrypt_with_material(secret, password, log_n, salt, nonce)
}

fn nip49_encrypt_with_material(
    secret: &[u8; 32],
    password: &BackupPassword,
    log_n: u8,
    salt: [u8; 16],
    nonce: [u8; 24],
) -> Result<String, IdentityError> {
    if !(NIP49_MIN_LOG_N..=NIP49_MAX_LOG_N).contains(&log_n) {
        return Err(IdentityError::new(IdentityErrorClass::BackupMalformed));
    }
    let key = derive_key(password, salt, log_n)?;
    let cipher = XChaCha20Poly1305::new(&Array(*key));
    let ciphertext = cipher
        .encrypt(
            &Array(nonce),
            Payload {
                msg: secret,
                aad: &[NIP49_SECURITY_BYTE],
            },
        )
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    let mut payload = Vec::with_capacity(NIP49_DECODED_BYTES);
    payload.push(NIP49_VERSION);
    payload.push(log_n);
    payload.extend_from_slice(&salt);
    payload.extend_from_slice(&nonce);
    payload.push(NIP49_SECURITY_BYTE);
    payload.extend_from_slice(&ciphertext);
    let hrp = Hrp::parse(NIP49_HRP)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    bech32::encode::<Bech32>(hrp, &payload)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))
}

fn nip49_decrypt(
    encoded: &str,
    password: &BackupPassword,
) -> Result<Zeroizing<[u8; 32]>, IdentityError> {
    let checked = CheckedHrpstring::new::<Bech32>(encoded)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    let expected_hrp = Hrp::parse(NIP49_HRP)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    if checked.hrp() != expected_hrp {
        return Err(IdentityError::new(IdentityErrorClass::BackupMalformed));
    }
    let payload = checked.byte_iter().collect::<Vec<_>>();
    if payload.len() != NIP49_DECODED_BYTES
        || payload[0] != NIP49_VERSION
        || !(NIP49_MIN_LOG_N..=NIP49_MAX_LOG_N).contains(&payload[1])
        || !matches!(payload[42], 0..=2)
    {
        return Err(IdentityError::new(IdentityErrorClass::BackupMalformed));
    }
    let canonical = bech32::encode::<Bech32>(expected_hrp, &payload)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    if canonical != encoded {
        return Err(IdentityError::new(IdentityErrorClass::BackupMalformed));
    }
    let log_n = payload[1];
    let salt: [u8; 16] = payload[2..18]
        .try_into()
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    let nonce: [u8; 24] = payload[18..42]
        .try_into()
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    let associated_data = [payload[42]];
    let key = derive_key(password, salt, log_n)?;
    let plaintext = Zeroizing::new(
        XChaCha20Poly1305::new(&Array(*key))
            .decrypt(
                &Array(nonce),
                Payload {
                    msg: &payload[43..],
                    aad: &associated_data,
                },
            )
            .map_err(|_| IdentityError::new(IdentityErrorClass::BackupAuthenticationFailed))?,
    );
    let secret: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    Ok(Zeroizing::new(secret))
}

fn derive_key(
    password: &BackupPassword,
    salt: [u8; 16],
    log_n: u8,
) -> Result<Zeroizing<[u8; 32]>, IdentityError> {
    let parameters = scrypt::Params::new(log_n, 8, 1)
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    let mut key = Zeroizing::new([0_u8; 32]);
    scrypt::scrypt(password.as_bytes(), &salt, &parameters, key.as_mut())
        .map_err(|_| IdentityError::new(IdentityErrorClass::BackupMalformed))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unicode_not_nfc)]

    use bech32::{Bech32, Bech32m, Hrp, primitives::decode::CheckedHrpstring};

    use super::{BackupPassword, IdentityErrorClass, NIP49_HRP, nip49_decrypt};

    const OFFICIAL: &str = "ncryptsec1qgg9947rlpvqu76pj5ecreduf9jxhselq2nae2kghhvd5g7dgjtcxfqtd67p9m0w57lspw8gsq6yphnm8623nsl8xn9j4jdzz84zm3frztj3z7s35vpzmqf6ksu8r89qk5z2zxfmu5gv8th8wclt0h4p";

    #[test]
    fn official_nip49_decryption_vector_matches_raw_secret() {
        let password = BackupPassword::new("nostr".to_owned()).expect("password is valid");
        let secret = nip49_decrypt(OFFICIAL, &password).expect("official vector decrypts");
        assert_eq!(
            *secret,
            [
                0x35, 0x01, 0x45, 0x41, 0x35, 0x01, 0x45, 0x41, 0x35, 0x01, 0x45, 0x41, 0x35, 0x01,
                0x45, 0x3f, 0xef, 0xb0, 0x22, 0x27, 0xe4, 0x49, 0xe5, 0x7c, 0xf4, 0xd3, 0xa3, 0xce,
                0x05, 0x37, 0x86, 0x83,
            ]
        );
    }

    #[test]
    fn backup_passwords_are_nfkc_normalized_before_derivation() {
        let source = BackupPassword::new("ÅΩẛ̣".to_owned()).expect("source password is valid");
        let normalized = BackupPassword::new("ÅΩṩ".to_owned()).expect("normalized password valid");
        assert_eq!(source.as_bytes(), normalized.as_bytes());
    }

    #[test]
    fn nip49_rejects_unreasonable_cost_wrong_layout_and_noncanonical_checksum() {
        let password = BackupPassword::new("nostr".to_owned()).expect("password is valid");
        for (index, value) in [(0, 1), (1, 19), (42, 3)] {
            let changed = mutate_payload(index, value, false);
            assert_eq!(
                nip49_decrypt(&changed, &password)
                    .expect_err("invalid NIP-49 header is rejected")
                    .class(),
                IdentityErrorClass::BackupMalformed
            );
        }
        let wrong_checksum = mutate_payload(0, 2, true);
        assert_eq!(
            nip49_decrypt(&wrong_checksum, &password)
                .expect_err("Bech32m is not NIP-49")
                .class(),
            IdentityErrorClass::BackupMalformed
        );
        assert_eq!(
            nip49_decrypt(&OFFICIAL.to_ascii_uppercase(), &password)
                .expect_err("uppercase spelling is noncanonical")
                .class(),
            IdentityErrorClass::BackupMalformed
        );
    }

    fn mutate_payload(index: usize, value: u8, bech32m: bool) -> String {
        let checked = CheckedHrpstring::new::<Bech32>(OFFICIAL).expect("official value parses");
        let mut payload = checked.byte_iter().collect::<Vec<_>>();
        payload[index] = value;
        let hrp = Hrp::parse(NIP49_HRP).expect("NIP-49 HRP parses");
        if bech32m {
            bech32::encode::<Bech32m>(hrp, &payload).expect("Bech32m fixture encodes")
        } else {
            bech32::encode::<Bech32>(hrp, &payload).expect("Bech32 fixture encodes")
        }
    }
}
