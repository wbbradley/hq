//! Pure fact-backed command decisions and stable replay results.

use std::fmt;

use hq_domain::{
    CausalReferences, CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode, FactKind,
    FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, Revision, SemanticPayload,
    Timestamp,
};

use crate::{ApplicationValueError, DomainSnapshot};

/// Semantic reducer package affected by a canonical fact family.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MutationDomain {
    /// Installation, account, mailbox, peer, and capability authority.
    Authority,
    /// Messages, message state, and durable activity.
    Conversation,
    /// Durable named agents and provider-session selection.
    Agent,
    /// Projects, resources, assignments, dispatch, output, and remote control.
    Project,
}

impl MutationDomain {
    /// Classifies one closed semantic fact family.
    pub const fn for_kind(kind: FactKind) -> Self {
        match kind {
            FactKind::InstallationDeclared
            | FactKind::MailboxCreated
            | FactKind::MailboxSessionBound
            | FactKind::MailboxContextRecorded
            | FactKind::PeerRouteSet
            | FactKind::PeerRouteBlocked
            | FactKind::MailboxAccessGranted
            | FactKind::MailboxAccessRevoked
            | FactKind::MailboxActionObserved
            | FactKind::HumanAccountCreated
            | FactKind::HumanAccountSelected
            | FactKind::HumanDeviceGranted
            | FactKind::HumanDeviceAccepted
            | FactKind::HumanDeviceRevoked => Self::Authority,
            FactKind::QuestionAsked
            | FactKind::AsynchronousMessageSent
            | FactKind::AnswerGiven
            | FactKind::ThreadCancelled
            | FactKind::MessageArchived
            | FactKind::MessageRestored
            | FactKind::MessageRejected
            | FactKind::HarnessActivityRecorded => Self::Conversation,
            FactKind::AgentNameClaimed
            | FactKind::AgentRetired
            | FactKind::ProviderSessionSelected
            | FactKind::ProviderSessionRenamed => Self::Agent,
            FactKind::ProjectCreated
            | FactKind::ProjectOpened
            | FactKind::ProjectClosingStarted
            | FactKind::ProjectClosed
            | FactKind::ProjectArchived
            | FactKind::ProjectUnarchived
            | FactKind::ProjectMetadataUpdated
            | FactKind::ProjectResourceAdded
            | FactKind::ProjectResourceRemoved
            | FactKind::ProjectResourceReplaced
            | FactKind::ProjectPrimaryResourceChanged
            | FactKind::ProjectResourceHealthObserved
            | FactKind::ProjectAssignmentConfiguring
            | FactKind::ProjectAssignmentRunnable
            | FactKind::ProjectAssignmentBlocked
            | FactKind::ProjectAssignmentEnded
            | FactKind::ProjectInputAccepted
            | FactKind::ProjectInputDispatched
            | FactKind::ProjectOutputRecorded
            | FactKind::RemoteProjectCommandRequested
            | FactKind::RemoteProjectCommandReceipt
            | FactKind::RemoteProjectCommandOutcome => Self::Project,
        }
    }
}

/// Deterministic semantic inputs for one locally authored canonical fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactPlan {
    author: InstallationId,
    authored_at: Timestamp,
    scope: FactScope,
    causal: CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>,
    payload: SemanticPayload,
    auxiliary_randomness: [u8; 32],
}

impl FactPlan {
    /// Constructs a plan from explicit time, causal, semantic, and signing inputs.
    pub const fn new(
        author: InstallationId,
        authored_at: Timestamp,
        scope: FactScope,
        causal: CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>,
        payload: SemanticPayload,
        auxiliary_randomness: [u8; 32],
    ) -> Self {
        Self {
            author,
            authored_at,
            scope,
            causal,
            payload,
            auxiliary_randomness,
        }
    }

    /// Returns the affected semantic reducer package.
    pub const fn domain(&self) -> MutationDomain {
        MutationDomain::for_kind(self.payload.kind())
    }

    /// Returns the author installation identity.
    pub const fn author(&self) -> InstallationId {
        self.author
    }

    /// Returns the explicit authored time.
    pub const fn authored_at(&self) -> Timestamp {
        self.authored_at
    }

    /// Returns the semantic audience.
    pub const fn scope(&self) -> &FactScope {
        &self.scope
    }

    /// Returns the complete causal and historical-authority references.
    pub const fn causal(&self) -> &CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES> {
        &self.causal
    }

    /// Returns the semantic fact payload.
    pub const fn payload(&self) -> &SemanticPayload {
        &self.payload
    }

    /// Consumes the plan into adapter-owned authoring inputs.
    pub fn into_parts(
        self,
    ) -> (
        InstallationId,
        Timestamp,
        FactScope,
        CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>,
        SemanticPayload,
        [u8; 32],
    ) {
        (
            self.author,
            self.authored_at,
            self.scope,
            self.causal,
            self.payload,
            self.auxiliary_randomness,
        )
    }
}

/// Pure decision made against the transaction-consistent application snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationDecision {
    /// Author and commit one canonical fact.
    Commit(Box<FactPlan>),
    /// Persist a stable domain rejection without a canonical fact.
    Reject(DomainError),
}

impl MutationDecision {
    /// Creates a committed decision.
    pub fn commit(plan: FactPlan) -> Self {
        Self::Commit(Box::new(plan))
    }

    /// Creates a stable domain rejection.
    pub const fn reject(error: DomainError) -> Self {
        Self::Reject(error)
    }
}

/// One-shot pure decision callback owned by a retryable fact mutation.
pub type MutationDecisionCallback =
    Box<dyn FnOnce(&DomainSnapshot) -> MutationDecision + Send + 'static>;

/// Stable retry identity plus a pure transaction-snapshot decision.
pub struct FactMutation {
    command_id: CommandId,
    request_digest: CommandDigest,
    decide: MutationDecisionCallback,
}

impl FactMutation {
    /// Constructs a mutation without invoking its decision callback.
    pub fn new<D>(command_id: CommandId, request_digest: CommandDigest, decide: D) -> Self
    where
        D: FnOnce(&DomainSnapshot) -> MutationDecision + Send + 'static,
    {
        Self {
            command_id,
            request_digest,
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

    /// Consumes the request into values needed by a commit adapter.
    pub fn into_parts(self) -> (CommandId, CommandDigest, MutationDecisionCallback) {
        (self.command_id, self.request_digest, self.decide)
    }
}

impl fmt::Debug for FactMutation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FactMutation")
            .field("command_id", &self.command_id)
            .field("request_digest", &self.request_digest)
            .finish_non_exhaustive()
    }
}

/// Stable semantic result retained for exact mutation replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationOutcome {
    /// The command committed a canonical fact or durable semantic no-op.
    Committed,
    /// Domain policy rejected the command without a canonical fact.
    Rejected(DomainError),
}

/// Complete typed answer for one retryable command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationReceipt {
    command_id: CommandId,
    request_digest: CommandDigest,
    revision: Revision,
    outcome: MutationOutcome,
}

impl MutationReceipt {
    /// Constructs a receipt from already validated durable values.
    pub const fn new(
        command_id: CommandId,
        request_digest: CommandDigest,
        revision: Revision,
        outcome: MutationOutcome,
    ) -> Self {
        Self {
            command_id,
            request_digest,
            revision,
            outcome,
        }
    }

    /// Returns the stable retry identity.
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    /// Returns the exact request digest.
    pub const fn request_digest(&self) -> CommandDigest {
        self.request_digest
    }

    /// Returns the durable transaction revision.
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// Returns the stable semantic outcome.
    pub const fn outcome(&self) -> &MutationOutcome {
        &self.outcome
    }
}

/// Result of a mutation submission whose response may be lost after durable commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationAttempt {
    /// A retained committed or rejected receipt is authoritative.
    Completed(MutationReceipt),
    /// The adapter cannot prove whether the stable command committed; retry must reconcile it.
    Uncertain {
        /// Stable command identity to retry.
        command_id: CommandId,
        /// Exact digest that must be reused.
        request_digest: CommandDigest,
    },
}

/// Maximum bytes in the application-owned stable receipt encoding.
pub const MAX_ENCODED_MUTATION_RESULT_BYTES: usize = 128;

/// Encodes a mutation outcome for exact durable replay.
pub fn encode_mutation_outcome(outcome: &MutationOutcome) -> Vec<u8> {
    match outcome {
        MutationOutcome::Committed => vec![1, 0],
        MutationOutcome::Rejected(error) => {
            let code = error.code().as_str().as_bytes();
            let mut encoded = Vec::with_capacity(5 + code.len());
            encoded.extend_from_slice(&[1, 1, encode_category(error.category())]);
            encoded.extend_from_slice(&u16::try_from(code.len()).unwrap_or(u16::MAX).to_be_bytes());
            encoded.extend_from_slice(code);
            encoded
        }
    }
}

/// Strictly decodes one canonical application-owned mutation outcome.
pub fn decode_mutation_outcome(bytes: &[u8]) -> Result<MutationOutcome, ApplicationValueError> {
    match bytes {
        [1, 0] => Ok(MutationOutcome::Committed),
        [1, 1, category, length_high, length_low, rest @ ..] => {
            let length = usize::from(u16::from_be_bytes([*length_high, *length_low]));
            if rest.len() != length || bytes.len() > MAX_ENCODED_MUTATION_RESULT_BYTES {
                return Err(ApplicationValueError::InvalidEncoding);
            }
            let code = std::str::from_utf8(rest)
                .map_err(|_| ApplicationValueError::InvalidEncoding)
                .and_then(|value| {
                    ErrorCode::new(value).map_err(|_| ApplicationValueError::InvalidEncoding)
                })?;
            let error = DomainError::new(decode_category(*category)?, code);
            let outcome = MutationOutcome::Rejected(error);
            if encode_mutation_outcome(&outcome) != bytes {
                return Err(ApplicationValueError::InvalidEncoding);
            }
            Ok(outcome)
        }
        _ => Err(ApplicationValueError::InvalidEncoding),
    }
}

const fn encode_category(category: ErrorCategory) -> u8 {
    match category {
        ErrorCategory::InvalidInput => 1,
        ErrorCategory::Conflict => 2,
        ErrorCategory::Unauthorized => 3,
        ErrorCategory::Unresolved => 4,
        ErrorCategory::NotFound => 5,
        ErrorCategory::InvariantViolation => 6,
    }
}

const fn decode_category(encoded: u8) -> Result<ErrorCategory, ApplicationValueError> {
    match encoded {
        1 => Ok(ErrorCategory::InvalidInput),
        2 => Ok(ErrorCategory::Conflict),
        3 => Ok(ErrorCategory::Unauthorized),
        4 => Ok(ErrorCategory::Unresolved),
        5 => Ok(ErrorCategory::NotFound),
        6 => Ok(ErrorCategory::InvariantViolation),
        _ => Err(ApplicationValueError::InvalidEncoding),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn stable_outcome_codec_round_trips_every_error_category() {
        for category in [
            ErrorCategory::InvalidInput,
            ErrorCategory::Conflict,
            ErrorCategory::Unauthorized,
            ErrorCategory::Unresolved,
            ErrorCategory::NotFound,
            ErrorCategory::InvariantViolation,
        ] {
            let outcome = MutationOutcome::Rejected(DomainError::new(
                category,
                ErrorCode::new("stable_code").expect("fixture code validates"),
            ));
            let encoded = encode_mutation_outcome(&outcome);
            assert!(encoded.len() <= MAX_ENCODED_MUTATION_RESULT_BYTES);
            assert_eq!(decode_mutation_outcome(&encoded), Ok(outcome));
        }
        assert_eq!(
            decode_mutation_outcome(&encode_mutation_outcome(&MutationOutcome::Committed)),
            Ok(MutationOutcome::Committed)
        );
    }

    #[test]
    fn stable_outcome_codec_rejects_unknown_trailing_and_noncanonical_bytes() {
        for invalid in [
            Vec::new(),
            vec![2, 0],
            vec![1, 0, 0],
            vec![1, 1, 99, 0, 1, b'x'],
            vec![1, 1, 1, 0, 2, b'x'],
            vec![1, 1, 1, 0, 1, 0xff],
        ] {
            assert_eq!(
                decode_mutation_outcome(&invalid),
                Err(ApplicationValueError::InvalidEncoding)
            );
        }
    }
}
