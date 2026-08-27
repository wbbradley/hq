//! Typed durable coordination values that are not rebuildable projections.

use std::{error::Error, fmt};

use hq_domain::{CommandDigest, CommandId, FactId, InstallationId, Revision};
use hq_protocol::MAX_EVENT_BYTES;

/// Maximum encoded mutation result retained for exact retry.
pub const MAX_MUTATION_RESULT_BYTES: usize = 65_536;
/// Maximum outbox intents returned by one store query.
pub const MAX_OUTBOX_QUERY_ITEMS: usize = 1_024;

/// Validation failure for bounded operational bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationalValueError {
    /// Exact bytes required a non-empty value.
    Empty,
    /// Exact bytes exceeded their inclusive storage limit.
    TooLong {
        /// Inclusive byte limit.
        maximum: usize,
        /// Actual byte length.
        actual: usize,
    },
}

impl fmt::Display for OperationalValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("exact operational bytes must not be empty"),
            Self::TooLong { maximum, actual } => {
                write!(formatter, "value has {actual} bytes; maximum is {maximum}")
            }
        }
    }
}

impl Error for OperationalValueError {}

/// Exact bounded result bytes returned for a retryable mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationResultBytes(Vec<u8>);

impl MutationResultBytes {
    /// Validates and owns an exact encoded result. Unit results may be empty.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, OperationalValueError> {
        let bytes = bytes.into();
        if bytes.len() > MAX_MUTATION_RESULT_BYTES {
            return Err(OperationalValueError::TooLong {
                maximum: MAX_MUTATION_RESULT_BYTES,
                actual: bytes.len(),
            });
        }
        Ok(Self(bytes))
    }

    /// Borrows the exact encoded result.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the exact encoded result.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Stable semantic class of a retained mutation result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationResultKind {
    /// The command committed a domain change or durable no-op.
    Committed,
    /// Domain policy rejected the command without a canonical change.
    Rejected,
}

/// Exact durable answer bound to one command identity and request digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationReceipt {
    command_id: CommandId,
    request_digest: CommandDigest,
    result_kind: MutationResultKind,
    result: MutationResultBytes,
    revision: Revision,
}

impl MutationReceipt {
    /// Creates a complete typed receipt from already validated values.
    pub const fn new(
        command_id: CommandId,
        request_digest: CommandDigest,
        result_kind: MutationResultKind,
        result: MutationResultBytes,
        revision: Revision,
    ) -> Self {
        Self {
            command_id,
            request_digest,
            result_kind,
            result,
            revision,
        }
    }

    /// Returns the stable command identity.
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    /// Returns the digest of the exact command input.
    pub const fn request_digest(&self) -> CommandDigest {
        self.request_digest
    }

    /// Returns the semantic result class.
    pub const fn result_kind(&self) -> MutationResultKind {
        self.result_kind
    }

    /// Borrows the exact encoded result.
    pub const fn result(&self) -> &MutationResultBytes {
        &self.result
    }

    /// Returns the revision allocated by the receipt's transaction.
    pub const fn revision(&self) -> Revision {
        self.revision
    }
}

/// Durable canonical delivery intent for one recipient installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxIntent {
    fact_id: FactId,
    recipient: InstallationId,
    exact_canonical_bytes: Vec<u8>,
    revision: Revision,
}

impl OutboxIntent {
    pub(crate) fn new(
        fact_id: FactId,
        recipient: InstallationId,
        exact_canonical_bytes: Vec<u8>,
        revision: Revision,
    ) -> Result<Self, OperationalValueError> {
        if exact_canonical_bytes.is_empty() {
            return Err(OperationalValueError::Empty);
        }
        if exact_canonical_bytes.len() > MAX_EVENT_BYTES {
            return Err(OperationalValueError::TooLong {
                maximum: MAX_EVENT_BYTES,
                actual: exact_canonical_bytes.len(),
            });
        }
        Ok(Self {
            fact_id,
            recipient,
            exact_canonical_bytes,
            revision,
        })
    }

    /// Returns the canonical fact identity.
    pub const fn fact_id(&self) -> FactId {
        self.fact_id
    }

    /// Returns the exact recipient installation.
    pub const fn recipient(&self) -> InstallationId {
        self.recipient
    }

    /// Borrows the exact canonical signed bytes retained for retry.
    pub fn exact_canonical_bytes(&self) -> &[u8] {
        &self.exact_canonical_bytes
    }

    /// Returns the revision that created the intent.
    pub const fn revision(&self) -> Revision {
        self.revision
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn mutation_result_bytes_accept_boundaries_and_reject_oversize() {
        assert!(MutationResultBytes::new(Vec::new()).is_ok());
        assert!(MutationResultBytes::new(vec![0; MAX_MUTATION_RESULT_BYTES]).is_ok());
        assert_eq!(
            MutationResultBytes::new(vec![0; MAX_MUTATION_RESULT_BYTES + 1]),
            Err(OperationalValueError::TooLong {
                maximum: MAX_MUTATION_RESULT_BYTES,
                actual: MAX_MUTATION_RESULT_BYTES + 1,
            })
        );
    }

    #[test]
    fn outbox_intent_requires_bounded_exact_event_bytes() {
        let error = OutboxIntent::new(
            FactId::from_bytes([1; 32]),
            InstallationId::from_bytes([2; 32]),
            Vec::new(),
            Revision::new(1),
        )
        .expect_err("empty canonical evidence rejects");
        assert_eq!(error, OperationalValueError::Empty);
    }
}
