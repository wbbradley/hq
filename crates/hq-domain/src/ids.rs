//! Opaque fixed-width identities and public keys.

macro_rules! opaque_bytes {
    ($name:ident, $documentation:literal) => {
        #[doc = $documentation]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; 32]);

        impl $name {
            /// Constructs the value from its opaque semantic bytes.
            pub const fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            /// Borrows the opaque semantic bytes.
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
        }
    };
}

opaque_bytes!(FactId, "Identity of an immutable semantic fact.");
opaque_bytes!(InstallationId, "Identity of an HQ installation.");
opaque_bytes!(MailboxId, "Identity of a mailbox.");
opaque_bytes!(AccountId, "Identity of a human account.");
opaque_bytes!(AgentId, "Identity of a durable named agent.");
opaque_bytes!(ProjectId, "Identity of a project.");
opaque_bytes!(MessageId, "Identity of a conversation message.");
opaque_bytes!(ResourceId, "Identity of a project resource.");
opaque_bytes!(CommandId, "Stable identity of a retryable command.");
opaque_bytes!(ReceiptId, "Identity of an idempotency receipt.");
opaque_bytes!(OperationId, "Identity of an external or managed operation.");
opaque_bytes!(GrantId, "Identity of a capability or membership grant.");
opaque_bytes!(AssignmentId, "Identity of a project assignment epoch.");
opaque_bytes!(ThreadId, "Identity of a causal conversation thread.");
opaque_bytes!(DispatchId, "Identity of a project input dispatch.");
opaque_bytes!(
    CommandDigest,
    "Digest identifying the exact input of a retryable command."
);
opaque_bytes!(
    SigningPublicKey,
    "Public key used to verify semantic authorship."
);
opaque_bytes!(
    EncryptionPublicKey,
    "Public key used to address encrypted transport."
);
