//! Adapter from consumer-owned application ports to the store actor.

use std::{collections::BTreeSet, fmt, sync::Arc};

use hq_application::{
    ApplicationError, ApplicationErrorCode, AuthoritativeConversationView, CanonicalEvidence,
    CommitFacts, ControlMailbox, ConversationPageSelection, DomainHealth, DomainSnapshot,
    EvidenceIngestOutcome, FactMutation, HealthDomain, MailboxCommandRequest, MailboxDraft,
    MailboxDraftDeleteOutcome, MailboxDraftDeleteRequest, MailboxDraftSaveOutcome,
    MailboxDraftSaveRequest, MutationAttempt, MutationDecision, MutationOutcome,
    MutationReceipt as ApplicationMutationReceipt, QueryDomain, StateHealth, StateRepairReport,
    decode_mutation_outcome, encode_mutation_outcome, plan_mailbox_command,
};
use hq_domain::{FactId, MailboxAddress, OperationId, Page, PageCursor};
use hq_protocol::{Bip340Signer, CanonicalEventPlan, decode_semantic_event};
use hq_reducer::{AuthorityPolicy, ConversationKey, DecisionStatus};

use crate::{
    ApplicationStateHandle, IngestOutcome, LocalMutationDecision, LocalMutationRequest,
    MutationResultBytes, MutationResultKind, ReductionDomain, ReductionIndexSnapshot,
    ReplicationHandle, Store, StoreError, StoreErrorClass,
};

/// Application-facing store adapter configured with explicit local authoring capabilities.
#[derive(Clone)]
pub struct StoreGateway {
    store: ApplicationStateHandle,
    replication: ReplicationHandle,
    policy: AuthorityPolicy,
    signer: Arc<Bip340Signer>,
}

impl StoreGateway {
    /// Constructs a gateway without exposing signer bytes or persistence details to application code.
    pub fn new(store: &Store, policy: AuthorityPolicy, signer: Arc<Bip340Signer>) -> Self {
        Self {
            store: store.application_state_handle(),
            replication: store.replication_handle(),
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

    fn authoritative_conversation_view(
        &self,
        selection: Option<&ConversationPageSelection>,
    ) -> Result<AuthoritativeConversationView, ApplicationError> {
        self.store
            .authoritative_conversation_view(selection)
            .map_err(map_store_error)
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

    fn canonical_evidence(
        &self,
        roots: &BTreeSet<FactId>,
        maximum_facts: usize,
        maximum_bytes: usize,
    ) -> Result<Vec<CanonicalEvidence>, ApplicationError> {
        self.store
            .canonical_evidence(roots, maximum_facts, maximum_bytes)
            .map_err(map_store_error)
    }

    fn state_health(&self) -> Result<StateHealth, ApplicationError> {
        let (revision, index) = self
            .store
            .state_health_snapshot()
            .map_err(map_store_error)?;
        Ok(StateHealth {
            revision,
            domains: domain_health(&index),
        })
    }

    fn repair_state(
        &self,
        operation_id: OperationId,
    ) -> Result<StateRepairReport, ApplicationError> {
        let (revision, index) = self
            .store
            .repair_health_snapshot(self.policy)
            .map_err(map_store_error)?;
        Ok(StateRepairReport {
            operation_id,
            revision,
            domains: domain_health(&index),
        })
    }
}

fn domain_health(index: &ReductionIndexSnapshot) -> Vec<DomainHealth> {
    ReductionDomain::ALL
        .into_iter()
        .map(|domain| {
            let mut health = DomainHealth {
                domain: match domain {
                    ReductionDomain::Authority => HealthDomain::Authority,
                    ReductionDomain::Conversation => HealthDomain::Conversation,
                    ReductionDomain::Agent => HealthDomain::Agent,
                    ReductionDomain::Project => HealthDomain::Project,
                },
                projected: 0,
                unresolved: 0,
                unauthorized: 0,
                conflicted: 0,
                invalid: 0,
                unsupported: 0,
                conflicts: u64::try_from(index.conflicts(domain).len()).unwrap_or(u64::MAX),
            };
            for fact_id in index.presentation_order(domain) {
                let Some(decision) = index.decision(domain, *fact_id) else {
                    continue;
                };
                let count = match decision.status() {
                    DecisionStatus::Projected => &mut health.projected,
                    DecisionStatus::Unresolved => &mut health.unresolved,
                    DecisionStatus::Unauthorized => &mut health.unauthorized,
                    DecisionStatus::Conflicted => &mut health.conflicted,
                    DecisionStatus::Invalid => &mut health.invalid,
                    DecisionStatus::Unsupported => &mut health.unsupported,
                };
                *count = count.saturating_add(1);
            }
            health
        })
        .collect()
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
                to_local_decision(decide(&domain))
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

    fn ingest_canonical_evidence(
        &self,
        evidence: &[CanonicalEvidence],
    ) -> Result<Vec<EvidenceIngestOutcome>, ApplicationError> {
        let verified = evidence
            .iter()
            .map(|evidence| {
                let fact = decode_semantic_event(evidence.exact_event.clone())
                    .map_err(|_| ApplicationError::new(ApplicationErrorCode::InvalidRequest))?
                    .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::InvalidRequest))?;
                if fact.fact().id() != evidence.fact_id {
                    return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
                }
                Ok(fact)
            })
            .collect::<Result<Vec<_>, _>>()?;
        verified
            .into_iter()
            .map(|fact| {
                let fact_id = fact.fact().id();
                self.replication
                    .ingest_verified(fact, self.policy)
                    .map(|outcome| EvidenceIngestOutcome {
                        fact_id,
                        revision: outcome.revision(),
                        inserted: matches!(outcome, IngestOutcome::Inserted(_)),
                    })
                    .map_err(map_store_error)
            })
            .collect()
    }
}

impl ControlMailbox for StoreGateway {
    fn mailbox_drafts(&self) -> Result<Vec<MailboxDraft>, ApplicationError> {
        self.store.load_mailbox_drafts().map_err(map_store_error)
    }

    fn save_mailbox_draft(
        &self,
        request: MailboxDraftSaveRequest,
    ) -> Result<MailboxDraftSaveOutcome, ApplicationError> {
        self.store
            .save_mailbox_draft(request)
            .map_err(map_store_error)
    }

    fn delete_mailbox_draft(
        &self,
        request: MailboxDraftDeleteRequest,
    ) -> Result<MailboxDraftDeleteOutcome, ApplicationError> {
        self.store
            .delete_mailbox_draft(request)
            .map_err(map_store_error)
    }

    fn control_mailbox(
        &self,
        request: MailboxCommandRequest,
    ) -> Result<MutationAttempt, ApplicationError> {
        let command_id = request.command_id;
        let request_digest = request.request_digest;
        let policy = self.policy;
        let signer = Arc::clone(&self.signer);
        let local = policy.local_installation();
        let human = MailboxAddress::new(local, policy.local_human_mailbox());
        let store_request = if let Some(draft_id) = request.draft_id {
            LocalMutationRequest::new_with_draft(
                command_id,
                request_digest,
                policy,
                signer,
                draft_id,
                move |snapshot, draft| {
                    let domain = DomainSnapshot::from_reports(
                        snapshot.authority(),
                        snapshot.conversation(),
                        snapshot.agent(),
                        snapshot.project(),
                    );
                    to_local_decision(plan_mailbox_command(&domain, local, human, &request, draft))
                },
            )
        } else {
            LocalMutationRequest::new(
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
                    to_local_decision(plan_mailbox_command(&domain, local, human, &request, None))
                },
            )
        };
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

fn to_local_decision(decision: MutationDecision) -> LocalMutationDecision {
    match decision {
        MutationDecision::Commit(plan) => {
            let (author, authored_at, scope, causal, payload, randomness) = plan.into_parts();
            let result = MutationResultBytes::from_application_encoding(encode_mutation_outcome(
                &MutationOutcome::Committed,
            ));
            LocalMutationDecision::commit(
                CanonicalEventPlan::new(author, authored_at, scope, causal, payload),
                randomness,
                result,
            )
        }
        MutationDecision::Reject(error) => {
            let outcome = MutationOutcome::Rejected(error);
            LocalMutationDecision::reject(MutationResultBytes::from_application_encoding(
                encode_mutation_outcome(&outcome),
            ))
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
        StoreErrorClass::RelayStagingFull | StoreErrorClass::MailboxDraftsFull => {
            ApplicationErrorCode::IntakeFull
        }
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
