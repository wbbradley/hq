//! Typed durable coordination values that are not rebuildable projections.

use std::{error::Error, fmt, sync::Arc};

use hq_application::MailboxDraft;
use hq_domain::{CommandDigest, CommandId, FactId, InstallationId, OperationId, Revision};
use hq_protocol::{Bip340Signer, CanonicalEventPlan, MAX_EVENT_BYTES};
use hq_reducer::AuthorityPolicy;

use crate::CompleteSnapshot;

/// Maximum encoded mutation result retained for exact retry.
pub const MAX_MUTATION_RESULT_BYTES: usize = 65_536;
/// Maximum outbox intents returned by one store query.
pub const MAX_OUTBOX_QUERY_ITEMS: usize = 1_024;

pub(crate) type LocalDecisionCallback = Box<
    dyn FnOnce(&CompleteSnapshot, Option<&MailboxDraft>) -> LocalMutationDecision + Send + 'static,
>;

/// Bounded typed request for one retryable local fact-backed mutation.
pub struct LocalMutationRequest {
    command_id: CommandId,
    request_digest: CommandDigest,
    policy: AuthorityPolicy,
    signer: Arc<Bip340Signer>,
    draft_id: Option<OperationId>,
    decide: LocalDecisionCallback,
}

impl LocalMutationRequest {
    /// Creates a request whose one-shot decision runs against the transaction snapshot.
    pub fn new<D>(
        command_id: CommandId,
        request_digest: CommandDigest,
        policy: AuthorityPolicy,
        signer: Arc<Bip340Signer>,
        decide: D,
    ) -> Self
    where
        D: FnOnce(&CompleteSnapshot) -> LocalMutationDecision + Send + 'static,
    {
        Self {
            command_id,
            request_digest,
            policy,
            signer,
            draft_id: None,
            decide: Box::new(move |snapshot, _| decide(snapshot)),
        }
    }

    /// Creates a request whose successful commit atomically consumes the named draft.
    pub fn new_with_draft<D>(
        command_id: CommandId,
        request_digest: CommandDigest,
        policy: AuthorityPolicy,
        signer: Arc<Bip340Signer>,
        draft_id: OperationId,
        decide: D,
    ) -> Self
    where
        D: FnOnce(&CompleteSnapshot, Option<&MailboxDraft>) -> LocalMutationDecision
            + Send
            + 'static,
    {
        Self {
            command_id,
            request_digest,
            policy,
            signer,
            draft_id: Some(draft_id),
            decide: Box::new(decide),
        }
    }

    /// Returns the stable retry identity.
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    /// Returns the digest of the exact command input.
    pub const fn request_digest(&self) -> CommandDigest {
        self.request_digest
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        CommandId,
        CommandDigest,
        AuthorityPolicy,
        Arc<Bip340Signer>,
        Option<OperationId>,
        LocalDecisionCallback,
    ) {
        (
            self.command_id,
            self.request_digest,
            self.policy,
            self.signer,
            self.draft_id,
            self.decide,
        )
    }
}

impl fmt::Debug for LocalMutationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalMutationRequest")
            .field("command_id", &self.command_id)
            .field("request_digest", &self.request_digest)
            .field("policy", &self.policy)
            .field("draft_id", &self.draft_id)
            .finish_non_exhaustive()
    }
}

/// Committed local event plan and exact client result.
pub struct LocalMutationCommit {
    pub(crate) plan: CanonicalEventPlan,
    pub(crate) auxiliary_randomness: [u8; 32],
    pub(crate) result: MutationResultBytes,
}

/// Pure decision returned from a transaction-consistent local snapshot.
pub enum LocalMutationDecision {
    /// Author and commit one canonical fact.
    Commit(Box<LocalMutationCommit>),
    /// Persist a stable domain rejection without a canonical fact.
    Reject(MutationResultBytes),
}

impl LocalMutationDecision {
    /// Creates a committed decision from explicit signing inputs and exact result bytes.
    pub fn commit(
        plan: CanonicalEventPlan,
        auxiliary_randomness: [u8; 32],
        result: MutationResultBytes,
    ) -> Self {
        Self::Commit(Box::new(LocalMutationCommit {
            plan,
            auxiliary_randomness,
            result,
        }))
    }

    /// Creates a rejected decision with exact stable result bytes.
    pub const fn reject(result: MutationResultBytes) -> Self {
        Self::Reject(result)
    }

    pub(crate) fn into_parts(self) -> LocalMutationDecisionParts {
        match self {
            Self::Commit(commit) => LocalMutationDecisionParts::Commit(commit),
            Self::Reject(result) => LocalMutationDecisionParts::Reject(result),
        }
    }
}

pub(crate) enum LocalMutationDecisionParts {
    Commit(Box<LocalMutationCommit>),
    Reject(MutationResultBytes),
}

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

    pub(crate) fn from_application_encoding(bytes: Vec<u8>) -> Self {
        Self(bytes)
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

    #[test]
    fn local_request_debug_exposes_only_public_coordination_values() {
        let signer = Bip340Signer::from_secret_bytes({
            let mut secret = [0_u8; 32];
            secret[31] = 1;
            secret
        })
        .expect("fixture signer constructs");
        let request = LocalMutationRequest::new(
            CommandId::from_bytes([1; 32]),
            CommandDigest::from_bytes([2; 32]),
            AuthorityPolicy::new(
                InstallationId::from_bytes([3; 32]),
                hq_domain::MailboxId::from_bytes([4; 32]),
            ),
            Arc::new(signer),
            |_| {
                LocalMutationDecision::reject(
                    MutationResultBytes::new(Vec::new()).expect("unit result is valid"),
                )
            },
        );
        let debug = format!("{request:?}");
        assert!(debug.contains("command_id"));
        assert!(debug.contains("request_digest"));
        assert!(!debug.contains("signer"));
        assert!(!debug.contains("decide"));
    }
}
