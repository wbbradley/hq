//! Typed installation and mailbox addresses.

use crate::{InstallationId, MailboxId, SigningPublicKey};

/// Authenticated address of an installation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InstallationAddress {
    installation_id: InstallationId,
    signing_key: SigningPublicKey,
}

impl InstallationAddress {
    /// Creates an installation address from independently verified parts.
    pub const fn new(installation_id: InstallationId, signing_key: SigningPublicKey) -> Self {
        Self {
            installation_id,
            signing_key,
        }
    }

    /// Returns the installation identity.
    pub const fn installation_id(self) -> InstallationId {
        self.installation_id
    }

    /// Returns the signing public key.
    pub const fn signing_key(self) -> SigningPublicKey {
        self.signing_key
    }
}

/// Address of one mailbox owned by an installation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MailboxAddress {
    installation_id: InstallationId,
    mailbox_id: MailboxId,
}

impl MailboxAddress {
    /// Creates a mailbox address from typed identities.
    pub const fn new(installation_id: InstallationId, mailbox_id: MailboxId) -> Self {
        Self {
            installation_id,
            mailbox_id,
        }
    }

    /// Returns the owning installation identity.
    pub const fn installation_id(self) -> InstallationId {
        self.installation_id
    }

    /// Returns the mailbox identity.
    pub const fn mailbox_id(self) -> MailboxId {
        self.mailbox_id
    }
}
