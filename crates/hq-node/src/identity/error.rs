//! Redacted failures for identity and local configuration persistence.

use std::{error::Error, fmt};

/// Stable identity-boundary failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityErrorClass {
    /// Neither an explicit root nor a usable environment-derived state root was available.
    PathUnavailable,
    /// A supplied state path was relative, empty, or otherwise unusable.
    InvalidPath,
    /// An existing state artifact had permissions broader than the private contract.
    UnsafePermissions,
    /// A state artifact was a symbolic link.
    SymbolicLink,
    /// A filesystem operation failed without exposing path or platform detail.
    FileSystem,
    /// Another local owner currently holds the state-directory lock.
    AlreadyOwned,
    /// Initialization or import would overwrite an existing identity.
    IdentityExists,
    /// No initialized identity exists.
    IdentityMissing,
    /// The identity file was malformed, truncated, trailing, or held an invalid key.
    IdentityMalformed,
    /// The operating system did not provide cryptographic randomness.
    EntropyUnavailable,
    /// A backup password was empty or exceeded its byte limit.
    PasswordInvalid,
    /// Export would overwrite an existing backup.
    BackupExists,
    /// The backup package or NIP-49 value was malformed or noncanonical.
    BackupMalformed,
    /// Backup authenticated decryption failed.
    BackupAuthenticationFailed,
    /// Local configuration was malformed or noncanonical.
    ConfigurationMalformed,
    /// A typed local configuration value violated its bound or vocabulary.
    ConfigurationInvalid,
}

/// Redacted identity-boundary error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdentityError {
    class: IdentityErrorClass,
}

impl IdentityError {
    pub(super) const fn new(class: IdentityErrorClass) -> Self {
        Self { class }
    }

    /// Returns the stable failure class.
    pub const fn class(self) -> IdentityErrorClass {
        self.class
    }
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.class {
            IdentityErrorClass::PathUnavailable => "state path is unavailable",
            IdentityErrorClass::InvalidPath => "state path is invalid",
            IdentityErrorClass::UnsafePermissions => "state artifact permissions are unsafe",
            IdentityErrorClass::SymbolicLink => "state artifact must not be a symbolic link",
            IdentityErrorClass::FileSystem => "identity filesystem operation failed",
            IdentityErrorClass::AlreadyOwned => "state directory is already owned",
            IdentityErrorClass::IdentityExists => "installation identity already exists",
            IdentityErrorClass::IdentityMissing => "installation identity is not initialized",
            IdentityErrorClass::IdentityMalformed => "installation identity is malformed",
            IdentityErrorClass::EntropyUnavailable => "cryptographic entropy is unavailable",
            IdentityErrorClass::PasswordInvalid => "backup password is invalid",
            IdentityErrorClass::BackupExists => "identity backup already exists",
            IdentityErrorClass::BackupMalformed => "identity backup is malformed",
            IdentityErrorClass::BackupAuthenticationFailed => {
                "identity backup authentication failed"
            }
            IdentityErrorClass::ConfigurationMalformed => "local configuration is malformed",
            IdentityErrorClass::ConfigurationInvalid => "local configuration value is invalid",
        };
        formatter.write_str(message)
    }
}

impl Error for IdentityError {}
