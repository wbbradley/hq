//! Adapter from consumer-owned application ports to the store actor.

use std::{fmt, sync::Arc};

use hq_application::{
    ApplicationError, ApplicationErrorCode, CommitFacts, DomainSnapshot, FactMutation,
    MutationAttempt, MutationDecision, MutationOutcome,
    MutationReceipt as ApplicationMutationReceipt, QueryDomain, decode_mutation_outcome,
    encode_mutation_outcome,
};
use hq_domain::{Page, PageCursor};
use hq_protocol::{Bip340Signer, CanonicalEventPlan};
use hq_reducer::{AuthorityPolicy, ConversationKey};

use crate::{
    ApplicationStateHandle, LocalMutationDecision, LocalMutationRequest, MutationResultBytes,
    MutationResultKind, Store, StoreError, StoreErrorClass,
};

/// Application-facing store adapter configured with explicit local authoring capabilities.
#[derive(Clone)]
pub struct StoreGateway {
    store: ApplicationStateHandle,
    policy: AuthorityPolicy,
    signer: Arc<Bip340Signer>,
}

impl StoreGateway {
    /// Constructs a gateway without exposing signer bytes or persistence details to application code.
    pub fn new(store: &Store, policy: AuthorityPolicy, signer: Arc<Bip340Signer>) -> Self {
        Self {
            store: store.application_state_handle(),
            policy,
            signer,
        }
    }
}

impl fmt::Debug for StoreGateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreGateway")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl QueryDomain for StoreGateway {
    fn authoritative_snapshot(
        &self,
    ) -> Result<hq_application::AuthoritativeSnapshot, ApplicationError> {
        self.store.authoritative_snapshot().map_err(map_store_error)
    }

    fn conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<hq_application::ConversationEntry>, ApplicationError> {
        self.store
            .load_conversation_entries(key, limit, cursor)
            .map_err(map_store_error)
    }
}

impl CommitFacts for StoreGateway {
    fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
        let (command_id, request_digest, decide) = request.into_parts();
        let policy = self.policy;
        let signer = Arc::clone(&self.signer);
        let store_request = LocalMutationRequest::new(
            command_id,
            request_digest,
            policy,
            signer,
            move |snapshot| {
                let domain = DomainSnapshot::from_reports(
                    snapshot.authority(),
                    snapshot.conversation(),
                    snapshot.agent(),
                    snapshot.project(),
                );
                match decide(&domain) {
                    MutationDecision::Commit(plan) => {
                        let (author, authored_at, scope, causal, payload, randomness) =
                            plan.into_parts();
                        let result = MutationResultBytes::from_application_encoding(
                            encode_mutation_outcome(&MutationOutcome::Committed),
                        );
                        LocalMutationDecision::commit(
                            CanonicalEventPlan::new(author, authored_at, scope, causal, payload),
                            randomness,
                            result,
                        )
                    }
                    MutationDecision::Reject(error) => {
                        let outcome = MutationOutcome::Rejected(error);
                        LocalMutationDecision::reject(
                            MutationResultBytes::from_application_encoding(
                                encode_mutation_outcome(&outcome),
                            ),
                        )
                    }
                }
            },
        );

        match self.store.execute_local_mutation(store_request) {
            Ok(receipt) => decode_receipt(&receipt).map(MutationAttempt::Completed),
            Err(error) if error.class() == StoreErrorClass::WorkerStopped => {
                Ok(MutationAttempt::Uncertain {
                    command_id,
                    request_digest,
                })
            }
            Err(error) => Err(map_store_error(error)),
        }
    }
}

fn decode_receipt(
    receipt: &crate::MutationReceipt,
) -> Result<ApplicationMutationReceipt, ApplicationError> {
    let outcome = decode_mutation_outcome(receipt.result().as_bytes())
        .map_err(|_| ApplicationError::new(ApplicationErrorCode::StateCorrupt))?;
    let kind_matches = matches!(
        (receipt.result_kind(), &outcome),
        (MutationResultKind::Committed, MutationOutcome::Committed)
            | (MutationResultKind::Rejected, MutationOutcome::Rejected(_))
    );
    if !kind_matches {
        return Err(ApplicationError::new(ApplicationErrorCode::StateCorrupt));
    }
    Ok(ApplicationMutationReceipt::new(
        receipt.command_id(),
        receipt.request_digest(),
        receipt.revision(),
        outcome,
    ))
}

fn map_store_error(error: StoreError) -> ApplicationError {
    let code = match error.class() {
        StoreErrorClass::MutationConflict => ApplicationErrorCode::CommandIdentityConflict,
        StoreErrorClass::IdentityCollision
        | StoreErrorClass::RelayStateConflict
        | StoreErrorClass::HarnessStateConflict
        | StoreErrorClass::ProjectSagaConflict => ApplicationErrorCode::StateIdentityConflict,
        StoreErrorClass::InvalidOperationalRequest => ApplicationErrorCode::InvalidRequest,
        StoreErrorClass::RelayStagingFull => ApplicationErrorCode::IntakeFull,
        StoreErrorClass::ActorClosed
        | StoreErrorClass::WorkerStopped
        | StoreErrorClass::DatabaseUnavailable
        | StoreErrorClass::FileSystem
        | StoreErrorClass::NotRepaired => ApplicationErrorCode::AdapterUnavailable,
        StoreErrorClass::CorruptDatabase
        | StoreErrorClass::InvalidEvidence
        | StoreErrorClass::OperationalStateCorrupt
        | StoreErrorClass::RebuildableStateCorrupt => ApplicationErrorCode::StateCorrupt,
        StoreErrorClass::InvalidPath
        | StoreErrorClass::SymbolicLink
        | StoreErrorClass::UnsafePermissions
        | StoreErrorClass::IncompatibleSchema
        | StoreErrorClass::RevisionExhausted
        | StoreErrorClass::ReductionFailed => ApplicationErrorCode::InvariantViolation,
    };
    ApplicationError::new(code)
}
