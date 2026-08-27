//! Secure installation identity, state ownership, backup, and local defaults.

mod atomic;
mod backup;
mod config;
mod error;
mod paths;

use std::{fmt, fs::File, io::Read, path::Path, sync::Arc};

use hq_domain::{InstallationId, SigningPublicKey};
use hq_protocol::Bip340Signer;
use zeroize::Zeroizing;

pub use backup::BackupPassword;
pub use config::LocalConfiguration;
pub use error::{IdentityError, IdentityErrorClass};
pub use hq_relay::RelayUrl as RelayEndpoint;
pub use paths::{StateDirectoryOwner, StatePaths};

use atomic::{WriteMode, atomic_write};
use config::MAX_CONFIGURATION_BYTES;
use paths::{ensure_private_file, file_system, reject_symlink};

const IDENTITY_MAGIC: [u8; 8] = *b"HQIDV1\0\0";
const IDENTITY_BYTES: usize = 73;
const IDENTITY_MAX_BYTES: u64 = IDENTITY_BYTES as u64;

/// Public identity metadata safe for diagnostics and client inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicIdentity {
    installation_id: InstallationId,
    signing_public_key: [u8; 32],
    fingerprint: String,
}

impl PublicIdentity {
    /// Returns the stable installation identity.
    pub const fn installation_id(&self) -> InstallationId {
        self.installation_id
    }

    /// Returns the x-only secp256k1 public key.
    pub const fn signing_public_key(&self) -> [u8; 32] {
        self.signing_public_key
    }

    /// Returns a short public-key fingerprint for safe display.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the typed signing public key used by semantic facts.
    pub const fn typed_signing_public_key(&self) -> SigningPublicKey {
        SigningPublicKey::from_bytes(self.signing_public_key)
    }
}

/// Non-cloneable root identity with zeroizing secret ownership and signer access.
///
/// Secret bytes have no public accessor:
///
/// ```compile_fail
/// use hq_node::InstallationIdentity;
///
/// fn leak(identity: &InstallationIdentity) {
///     let _ = identity.secret_bytes();
/// }
/// ```
pub struct InstallationIdentity {
    public: PublicIdentity,
    secret: Zeroizing<[u8; 32]>,
    signer: Arc<Bip340Signer>,
}

impl InstallationIdentity {
    fn from_parts(installation: [u8; 32], secret: [u8; 32]) -> Result<Self, IdentityError> {
        if installation == [0; 32] {
            return Err(IdentityError::new(IdentityErrorClass::IdentityMalformed));
        }
        let signer = Bip340Signer::from_secret_bytes(secret)
            .map_err(|_| IdentityError::new(IdentityErrorClass::IdentityMalformed))?;
        let signing_public_key = signer.public_key();
        let fingerprint = encode_hex(&signing_public_key[..8]);
        Ok(Self {
            public: PublicIdentity {
                installation_id: InstallationId::from_bytes(installation),
                signing_public_key,
                fingerprint,
            },
            secret: Zeroizing::new(secret),
            signer: Arc::new(signer),
        })
    }

    /// Returns safe public inspection metadata.
    pub fn public_identity(&self) -> PublicIdentity {
        self.public.clone()
    }

    /// Borrows the signer without exposing root secret bytes.
    pub fn signer(&self) -> &Bip340Signer {
        self.signer.as_ref()
    }

    pub(crate) fn signer_handle(&self) -> Arc<Bip340Signer> {
        Arc::clone(&self.signer)
    }

    fn secret_bytes(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl fmt::Debug for InstallationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstallationIdentity")
            .field("installation_id", &self.public.installation_id)
            .field("public_key_fingerprint", &self.public.fingerprint)
            .finish_non_exhaustive()
    }
}

impl StateDirectoryOwner {
    /// Generates and durably initializes a fresh identity with operating-system entropy.
    pub fn initialize(&self) -> Result<InstallationIdentity, IdentityError> {
        reject_identity_destination(self.paths.identity_file())?;
        let identity = generate_identity(|bytes| {
            getrandom::fill(bytes)
                .map_err(|_| IdentityError::new(IdentityErrorClass::EntropyUnavailable))
        })?;
        write_identity(self.paths.identity_file(), &identity)?;
        Ok(identity)
    }

    /// Loads and validates the complete private identity file.
    pub fn load_identity(&self) -> Result<InstallationIdentity, IdentityError> {
        let bytes = Zeroizing::new(
            read_private_bounded(
                self.paths.identity_file(),
                IDENTITY_MAX_BYTES,
                IdentityErrorClass::IdentityMalformed,
            )
            .map_err(|error| {
                if error.class() == IdentityErrorClass::FileSystem
                    && !self.paths.identity_file().exists()
                {
                    IdentityError::new(IdentityErrorClass::IdentityMissing)
                } else {
                    error
                }
            })?,
        );
        decode_identity(&bytes)
    }

    /// Exports only identity authority in a new encrypted package created without overwrite.
    pub fn export_identity(
        &self,
        identity: &InstallationIdentity,
        password: &BackupPassword,
        destination: &Path,
    ) -> Result<(), IdentityError> {
        backup::export(identity, password, destination)
    }

    /// Imports encrypted identity authority into an otherwise uninitialized owned state root.
    pub fn import_identity(
        &self,
        source: &Path,
        password: &BackupPassword,
    ) -> Result<InstallationIdentity, IdentityError> {
        reject_identity_destination(self.paths.identity_file())?;
        let identity = backup::import(source, password)?;
        write_identity(self.paths.identity_file(), &identity)?;
        Ok(identity)
    }

    /// Loads unsigned local defaults, returning explicit defaults when the file is absent.
    pub fn load_configuration(&self) -> Result<LocalConfiguration, IdentityError> {
        match std::fs::symlink_metadata(self.paths.configuration_file()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LocalConfiguration::default());
            }
            Ok(_) => {}
            Err(_) => return Err(IdentityError::new(IdentityErrorClass::FileSystem)),
        }
        let bytes = read_private_bounded(
            self.paths.configuration_file(),
            MAX_CONFIGURATION_BYTES,
            IdentityErrorClass::ConfigurationMalformed,
        )?;
        config::decode(&bytes)
    }

    /// Durably and atomically replaces only unsigned local defaults.
    pub fn store_configuration(
        &self,
        configuration: &LocalConfiguration,
    ) -> Result<(), IdentityError> {
        reject_symlink(self.paths.configuration_file())?;
        let bytes = config::encode(configuration)?;
        atomic_write(self.paths.configuration_file(), &bytes, WriteMode::Replace)
    }
}

fn reject_identity_destination(path: &Path) -> Result<(), IdentityError> {
    reject_symlink(path)?;
    if path.exists() {
        Err(IdentityError::new(IdentityErrorClass::IdentityExists))
    } else {
        Ok(())
    }
}

fn write_identity(path: &Path, identity: &InstallationIdentity) -> Result<(), IdentityError> {
    let mut encoded = Zeroizing::new(Vec::with_capacity(IDENTITY_BYTES));
    encoded.extend_from_slice(&IDENTITY_MAGIC);
    encoded.push(1);
    encoded.extend_from_slice(identity.public.installation_id.as_bytes());
    encoded.extend_from_slice(identity.secret_bytes());
    atomic_write(
        path,
        &encoded,
        WriteMode::CreateNew(IdentityErrorClass::IdentityExists),
    )
}

fn decode_identity(bytes: &[u8]) -> Result<InstallationIdentity, IdentityError> {
    if bytes.len() != IDENTITY_BYTES || bytes[..8] != IDENTITY_MAGIC || bytes[8] != 1 {
        return Err(IdentityError::new(IdentityErrorClass::IdentityMalformed));
    }
    let installation = bytes[9..41]
        .try_into()
        .map_err(|_| IdentityError::new(IdentityErrorClass::IdentityMalformed))?;
    let secret = Zeroizing::new(
        bytes[41..73]
            .try_into()
            .map_err(|_| IdentityError::new(IdentityErrorClass::IdentityMalformed))?,
    );
    InstallationIdentity::from_parts(installation, *secret)
}

fn generate_identity(
    mut fill: impl FnMut(&mut [u8]) -> Result<(), IdentityError>,
) -> Result<InstallationIdentity, IdentityError> {
    for _ in 0..16 {
        let mut installation = [0_u8; 32];
        let mut secret = Zeroizing::new([0_u8; 32]);
        fill(&mut installation)?;
        fill(secret.as_mut())?;
        if let Ok(identity) = InstallationIdentity::from_parts(installation, *secret) {
            return Ok(identity);
        }
    }
    Err(IdentityError::new(IdentityErrorClass::EntropyUnavailable))
}

fn read_private_bounded(
    path: &Path,
    maximum: u64,
    malformed: IdentityErrorClass,
) -> Result<Vec<u8>, IdentityError> {
    reject_symlink(path)?;
    let file = File::open(path).map_err(file_system)?;
    ensure_private_file(&file)?;
    if file.metadata().map_err(file_system)?.len() > maximum {
        return Err(IdentityError::new(malformed));
    }
    let capacity = usize::try_from(maximum)
        .map_err(|_| IdentityError::new(malformed))?
        .saturating_add(1);
    let mut bytes = Vec::new();
    file.take(u64::try_from(capacity).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(file_system)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        return Err(IdentityError::new(malformed));
    }
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex(value: &str, class: IdentityErrorClass) -> Result<[u8; 32], IdentityError> {
    if value.len() != 64 {
        return Err(IdentityError::new(class));
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let high = nibble(pair[0]).ok_or_else(|| IdentityError::new(class))?;
        let low = nibble(pair[1]).ok_or_else(|| IdentityError::new(class))?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
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

    use super::{IdentityError, IdentityErrorClass, generate_identity};

    #[test]
    fn identity_generation_propagates_entropy_failure_without_a_partial_value() {
        let error =
            generate_identity(|_| Err(IdentityError::new(IdentityErrorClass::EntropyUnavailable)))
                .expect_err("entropy failure stops generation");
        assert_eq!(error.class(), IdentityErrorClass::EntropyUnavailable);
    }
}
