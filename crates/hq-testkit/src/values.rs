//! Explicit deterministic byte, identity, key, and clock streams.

use hq_domain::{
    AgentId, AssignmentId, CommandDigest, CommandId, DispatchId, EncryptionPublicKey, FactId,
    GrantId, InstallationId, MailboxId, MessageId, OperationId, ProjectId, ReceiptId, ResourceId,
    SigningPublicKey, ThreadId, Timestamp,
};

/// Reproducible source of opaque test bytes and typed identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeterministicValues {
    state: u64,
}

impl DeterministicValues {
    /// Creates a stream from an explicit seed.
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Clones the current stream position for reproducible branching.
    #[must_use]
    pub fn fork(&self) -> Self {
        self.clone()
    }

    /// Returns the next deterministic 32-byte block.
    pub fn bytes(&mut self) -> [u8; 32] {
        let mut bytes = [0; 32];
        for offset in (0..32).step_by(8) {
            bytes[offset..offset + 8].copy_from_slice(&self.next_u64().to_be_bytes());
        }
        bytes
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}

macro_rules! typed_value_methods {
    ($(($method:ident, $type:ident)),+ $(,)?) => {
        impl DeterministicValues {
            $(
                #[doc = concat!("Returns the next deterministic `", stringify!($type), "`.")]
                pub fn $method(&mut self) -> $type {
                    $type::from_bytes(self.bytes())
                }
            )+
        }
    };
}

typed_value_methods!(
    (fact_id, FactId),
    (installation_id, InstallationId),
    (mailbox_id, MailboxId),
    (agent_id, AgentId),
    (project_id, ProjectId),
    (message_id, MessageId),
    (resource_id, ResourceId),
    (command_id, CommandId),
    (receipt_id, ReceiptId),
    (operation_id, OperationId),
    (grant_id, GrantId),
    (assignment_id, AssignmentId),
    (thread_id, ThreadId),
    (dispatch_id, DispatchId),
    (command_digest, CommandDigest),
    (signing_key, SigningPublicKey),
    (encryption_key, EncryptionPublicKey),
);

/// Explicit deterministic clock for tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeterministicClock {
    next_millis: i64,
    step_millis: i64,
}

impl DeterministicClock {
    /// Creates a clock with an explicit first value and signed step.
    pub const fn new(first_millis: i64, step_millis: i64) -> Self {
        Self {
            next_millis: first_millis,
            step_millis,
        }
    }

    /// Returns the next explicit timestamp and advances by the configured step.
    pub fn tick(&mut self) -> Timestamp {
        let timestamp = Timestamp::from_unix_millis(self.next_millis);
        self.next_millis = self.next_millis.wrapping_add(self.step_millis);
        timestamp
    }
}
