//! Bounded typed store actor and public corpus owner.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    num::NonZeroUsize,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::{self, JoinHandle},
};

use hq_application::{
    CanonicalEvidence, MailboxDraft, MailboxDraftDeleteOutcome, MailboxDraftDeleteRequest,
    MailboxDraftSaveOutcome, MailboxDraftSaveRequest,
};
use hq_domain::{
    AgentId, CommandId, FactId, InstallationId, OperationId, Page, PageCursor, Revision,
};
use hq_protocol::VerifiedSemanticFact;
use hq_reducer::{AuthorityPolicy, ConversationKey};

use crate::{
    AgentProjectionSnapshot, AuthoritativeSnapshot, AuthorityProjectionSnapshot, CompleteSnapshot,
    ConversationEntry, ConversationProjectionSnapshot, HarnessLeaseOutcome, LocalMutationRequest,
    MutationReceipt, OutboxIntent, ProjectProjectionSnapshot, ReductionIndexSnapshot, StoreError,
    StoreErrorClass, StoredHarnessStateMutation, StoredHarnessStateSnapshot, StoredProjectSaga,
    StoredProjectSagaBegin, StoredRelayStateMutation, StoredRelayStatePage, StoredRelayStateQuery,
    StoredRelayStateSnapshot, database::Database,
};

/// Result of atomically ingesting immutable verified evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestOutcome {
    /// A previously unknown fact and all derived state committed at this revision.
    Inserted(Revision),
    /// The exact fact had already committed at this original revision.
    AlreadyPresent(Revision),
}

impl IngestOutcome {
    /// Returns the stable revision of the fact's canonical commit.
    pub const fn revision(self) -> Revision {
        match self {
            Self::Inserted(revision) | Self::AlreadyPresent(revision) => revision,
        }
    }

    /// Reports whether this call performed the canonical commit.
    pub const fn is_inserted(self) -> bool {
        matches!(self, Self::Inserted(_))
    }
}

/// Capacity-one coalesced post-commit revision observer.
pub struct RevisionInvalidations {
    wakes: Receiver<()>,
    latest: Arc<AtomicU64>,
}

impl RevisionInvalidations {
    /// Returns the latest committed revision when one or more wakes are pending.
    pub fn try_revision(&self) -> Option<Revision> {
        match self.wakes.try_recv() {
            Ok(()) => Some(Revision::new(self.latest.load(Ordering::Acquire))),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}

struct InvalidationEmitter {
    wakes: SyncSender<()>,
    latest: Arc<AtomicU64>,
}

impl InvalidationEmitter {
    fn publish(&self, revision: Revision) {
        self.latest.store(revision.value(), Ordering::Release);
        let _ = self.wakes.try_send(());
    }
}

/// Complete reducer-ready facts reconstructed from the immutable corpus.
pub struct VerifiedFactCorpus {
    facts: Vec<VerifiedSemanticFact>,
}

impl VerifiedFactCorpus {
    pub(crate) fn new(facts: Vec<VerifiedSemanticFact>) -> Self {
        Self { facts }
    }

    /// Returns the number of verified semantic facts.
    pub const fn len(&self) -> usize {
        self.facts.len()
    }

    /// Reports whether the corpus contains no facts.
    pub const fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// Iterates over facts in deterministic fact-identity order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &VerifiedSemanticFact> {
        self.facts.iter()
    }

    /// Consumes the owner and returns its complete verified facts.
    pub fn into_facts(self) -> Vec<VerifiedSemanticFact> {
        self.facts
    }
}

impl fmt::Debug for VerifiedFactCorpus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedFactCorpus")
            .field("fact_count", &self.facts.len())
            .finish()
    }
}

/// Successful complete repair and the exact persisted structural view it verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairOutcome {
    complete: CompleteSnapshot,
    persisted: ReductionIndexSnapshot,
    authority: AuthorityProjectionSnapshot,
    conversation: ConversationProjectionSnapshot,
    agent: AgentProjectionSnapshot,
    project: ProjectProjectionSnapshot,
}

impl RepairOutcome {
    pub(crate) fn new(
        complete: CompleteSnapshot,
        persisted: ReductionIndexSnapshot,
        authority: AuthorityProjectionSnapshot,
        conversation: ConversationProjectionSnapshot,
        agent: AgentProjectionSnapshot,
        project: ProjectProjectionSnapshot,
    ) -> Self {
        Self {
            complete,
            persisted,
            authority,
            conversation,
            agent,
            project,
        }
    }

    /// Returns all fresh complete-batch reports.
    pub const fn complete(&self) -> &CompleteSnapshot {
        &self.complete
    }

    /// Returns the normalized structural index read back before commit.
    pub const fn persisted(&self) -> &ReductionIndexSnapshot {
        &self.persisted
    }

    /// Returns the exact typed authority projection view read back before commit.
    pub const fn authority(&self) -> &AuthorityProjectionSnapshot {
        &self.authority
    }

    /// Returns the exact typed conversation/activity view read back before commit.
    pub const fn conversation(&self) -> &ConversationProjectionSnapshot {
        &self.conversation
    }

    /// Returns the exact typed named-agent view read back before commit.
    pub const fn agent(&self) -> &AgentProjectionSnapshot {
        &self.agent
    }

    /// Returns the exact typed project view read back before commit.
    pub const fn project(&self) -> &ProjectProjectionSnapshot {
        &self.project
    }

    /// Consumes the outcome into its complete and persisted snapshots.
    pub fn into_parts(
        self,
    ) -> (
        CompleteSnapshot,
        ReductionIndexSnapshot,
        AuthorityProjectionSnapshot,
        ConversationProjectionSnapshot,
        AgentProjectionSnapshot,
        ProjectProjectionSnapshot,
    ) {
        (
            self.complete,
            self.persisted,
            self.authority,
            self.conversation,
            self.agent,
            self.project,
        )
    }
}

enum Request {
    LocalMutation {
        request: LocalMutationRequest,
        reply: SyncSender<Result<MutationReceipt, StoreError>>,
    },
    LoadMailboxDrafts {
        reply: SyncSender<Result<Vec<MailboxDraft>, StoreError>>,
    },
    SaveMailboxDraft {
        request: MailboxDraftSaveRequest,
        reply: SyncSender<Result<MailboxDraftSaveOutcome, StoreError>>,
    },
    DeleteMailboxDraft {
        request: MailboxDraftDeleteRequest,
        reply: SyncSender<Result<MailboxDraftDeleteOutcome, StoreError>>,
    },
    Ingest {
        fact: Box<VerifiedSemanticFact>,
        policy: AuthorityPolicy,
        reply: SyncSender<Result<IngestOutcome, StoreError>>,
    },
    Load {
        reply: SyncSender<Result<VerifiedFactCorpus, StoreError>>,
    },
    CompleteSnapshot {
        policy: AuthorityPolicy,
        reply: SyncSender<Result<CompleteSnapshot, StoreError>>,
    },
    Repair {
        policy: AuthorityPolicy,
        reply: SyncSender<Result<RepairOutcome, StoreError>>,
    },
    LoadReductionIndex {
        reply: SyncSender<Result<ReductionIndexSnapshot, StoreError>>,
    },
    StateHealthSnapshot {
        repair: Option<AuthorityPolicy>,
        reply: SyncSender<Result<(Revision, ReductionIndexSnapshot), StoreError>>,
    },
    LoadAuthoritySnapshot {
        reply: SyncSender<Result<AuthorityProjectionSnapshot, StoreError>>,
    },
    LoadConversationSnapshot {
        reply: SyncSender<Result<ConversationProjectionSnapshot, StoreError>>,
    },
    LoadConversationEntries {
        key: ConversationKey,
        limit: usize,
        cursor: Option<PageCursor>,
        reply: SyncSender<Result<Page<ConversationEntry>, StoreError>>,
    },
    LoadAgentSnapshot {
        reply: SyncSender<Result<AgentProjectionSnapshot, StoreError>>,
    },
    LoadProjectSnapshot {
        reply: SyncSender<Result<ProjectProjectionSnapshot, StoreError>>,
    },
    AuthoritativeSnapshot {
        reply: SyncSender<Result<AuthoritativeSnapshot, StoreError>>,
    },
    CanonicalEvidence {
        roots: BTreeSet<FactId>,
        maximum_facts: usize,
        maximum_bytes: usize,
        reply: SyncSender<Result<Vec<CanonicalEvidence>, StoreError>>,
    },
    CurrentRevision {
        reply: SyncSender<Result<Revision, StoreError>>,
    },
    LoadMutationReceipt {
        command_id: CommandId,
        reply: SyncSender<Result<Option<MutationReceipt>, StoreError>>,
    },
    LoadOutboxIntents {
        limit: usize,
        reply: SyncSender<Result<Vec<OutboxIntent>, StoreError>>,
    },
    Relay(Box<RelayRequest>),
    Harness(Box<HarnessRequest>),
    ProjectSaga(Box<ProjectSagaRequest>),
    Close {
        reply: SyncSender<()>,
    },
}

enum RelayRequest {
    Apply {
        mutation: StoredRelayStateMutation,
        reply: SyncSender<Result<(), StoreError>>,
    },
    LoadPage {
        query: StoredRelayStateQuery,
        reply: SyncSender<Result<StoredRelayStatePage, StoreError>>,
    },
    LoadPrepared {
        fact_id: FactId,
        recipient: InstallationId,
        reply: SyncSender<Result<Option<crate::StoredPreparedOutbound>, StoreError>>,
    },
    LoadAttempt {
        url: String,
        wrapper_id: [u8; 32],
        reply: SyncSender<Result<Option<crate::StoredRelayAttempt>, StoreError>>,
    },
    LoadCursor {
        url: String,
        reply: SyncSender<Result<Option<crate::StoredCatchupCursor>, StoreError>>,
    },
}

enum HarnessRequest {
    Apply {
        mutation: Box<StoredHarnessStateMutation>,
        reply: SyncSender<Result<HarnessLeaseOutcome, StoreError>>,
    },
    Load {
        limit: usize,
        reply: SyncSender<Result<StoredHarnessStateSnapshot, StoreError>>,
    },
    Delivery {
        agent_id: AgentId,
        submission_id: hq_domain::MessageId,
        reply: SyncSender<Result<Option<crate::StoredHarnessDelivery>, StoreError>>,
    },
    DeliveryForOperation {
        agent_id: AgentId,
        operation_id: OperationId,
        reply: SyncSender<Result<Option<crate::StoredHarnessDelivery>, StoreError>>,
    },
    RunnableDeliveries {
        agent_id: AgentId,
        limit: usize,
        reply: SyncSender<Result<Vec<crate::StoredHarnessDelivery>, StoreError>>,
    },
    SessionOperation {
        operation_id: OperationId,
        reply: SyncSender<Result<Option<crate::HarnessSessionOperation>, StoreError>>,
    },
}

enum ProjectSagaRequest {
    Load {
        operation_id: OperationId,
        reply: SyncSender<Result<Option<StoredProjectSaga>, StoreError>>,
    },
    Begin {
        record: Box<StoredProjectSaga>,
        reply: SyncSender<Result<StoredProjectSagaBegin, StoreError>>,
    },
    Replace {
        record: Box<StoredProjectSaga>,
        reply: SyncSender<Result<(), StoreError>>,
    },
    LoadRunnable {
        limit: usize,
        reply: SyncSender<Result<Vec<StoredProjectSaga>, StoreError>>,
    },
}

/// Sole owner of one bounded SQLite worker and its request intake.
///
/// The value is intentionally not `Clone`; owned tasks clone only narrow request capabilities,
/// preserving one explicit worker owner and one shutdown point.
pub struct Store {
    requests: SyncSender<Request>,
    worker: Option<JoinHandle<()>>,
}

/// Cloneable relay-state request capability without store-worker ownership.
#[derive(Clone)]
pub struct RelayStateHandle {
    requests: SyncSender<Request>,
}

/// Cloneable harness-state request capability without store-worker ownership.
#[derive(Clone)]
pub struct HarnessStateHandle {
    requests: SyncSender<Request>,
}

/// Cloneable project-workflow state capability without store-worker ownership.
#[derive(Clone)]
pub struct ProjectSagaStateHandle {
    requests: SyncSender<Request>,
}

/// Cloneable application query and canonical-mutation capability without store-worker ownership.
#[derive(Clone)]
pub struct ApplicationStateHandle {
    requests: SyncSender<Request>,
}

/// Owned canonical-ingest and verified-authority query capability for background replication.
#[derive(Clone)]
pub struct ReplicationHandle {
    requests: SyncSender<Request>,
}

impl ReplicationHandle {
    /// Atomically ingests one already reverified semantic fact.
    pub fn ingest_verified(
        &self,
        fact: VerifiedSemanticFact,
        policy: AuthorityPolicy,
    ) -> Result<IngestOutcome, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Ingest {
                fact: Box::new(fact),
                policy,
                reply,
            })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the current verified authority projection used for route selection.
    pub fn load_authority_snapshot(&self) -> Result<AuthorityProjectionSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadAuthoritySnapshot { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }
}

impl ApplicationStateHandle {
    /// Loads every bounded installation-local mailbox draft in stable identity order.
    pub fn load_mailbox_drafts(&self) -> Result<Vec<MailboxDraft>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadMailboxDrafts { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Creates or optimistically replaces one complete local mailbox draft.
    pub fn save_mailbox_draft(
        &self,
        request: MailboxDraftSaveRequest,
    ) -> Result<MailboxDraftSaveOutcome, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::SaveMailboxDraft { request, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Idempotently and optimistically deletes one local mailbox draft.
    pub fn delete_mailbox_draft(
        &self,
        request: MailboxDraftDeleteRequest,
    ) -> Result<MailboxDraftDeleteOutcome, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::DeleteMailboxDraft { request, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads revision and the normalized structural index at one serialized store point.
    pub fn state_health_snapshot(&self) -> Result<(Revision, ReductionIndexSnapshot), StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::StateHealthSnapshot {
                repair: None,
                reply,
            })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Repairs rebuildable state and reports its revision from the same serialized store request.
    pub fn repair_health_snapshot(
        &self,
        policy: AuthorityPolicy,
    ) -> Result<(Revision, ReductionIndexSnapshot), StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::StateHealthSnapshot {
                repair: Some(policy),
                reply,
            })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Executes one retryable local fact-backed mutation in the serialized store worker.
    pub fn execute_local_mutation(
        &self,
        request: LocalMutationRequest,
    ) -> Result<MutationReceipt, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LocalMutation { request, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads revision and every application projection from one serialized store point.
    pub fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::AuthoritativeSnapshot { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one bounded exact transitive canonical evidence closure.
    pub fn canonical_evidence(
        &self,
        roots: &BTreeSet<FactId>,
        maximum_facts: usize,
        maximum_bytes: usize,
    ) -> Result<Vec<CanonicalEvidence>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::CanonicalEvidence {
                roots: roots.clone(),
                maximum_facts,
                maximum_bytes,
                reply,
            })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one bounded indexed conversation page.
    pub fn load_conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<ConversationEntry>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadConversationEntries {
                key: key.clone(),
                limit,
                cursor: cursor.cloned(),
                reply,
            })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }
}

impl fmt::Debug for ApplicationStateHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationStateHandle")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ReplicationHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationHandle")
            .finish_non_exhaustive()
    }
}

impl RelayStateHandle {
    /// Applies one atomic durable relay synchronization transition.
    pub fn apply(&self, mutation: StoredRelayStateMutation) -> Result<(), StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Relay(Box::new(RelayRequest::Apply {
                mutation,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one deterministic bounded keyset page of durable relay state.
    pub fn load_page(
        &self,
        query: StoredRelayStateQuery,
    ) -> Result<StoredRelayStatePage, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Relay(Box::new(RelayRequest::LoadPage {
                query,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the first deterministic bounded relay-state page.
    pub fn load(&self, limit: usize) -> Result<StoredRelayStateSnapshot, StoreError> {
        self.load_page(StoredRelayStateQuery::first(limit))
            .map(|page| page.state)
    }

    /// Loads one prepared lineage by stable outbox identity.
    pub fn prepared(
        &self,
        fact_id: FactId,
        recipient: InstallationId,
    ) -> Result<Option<crate::StoredPreparedOutbound>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Relay(Box::new(RelayRequest::LoadPrepared {
                fact_id,
                recipient,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one relay-local attempt by exact correlation identity.
    pub fn attempt(
        &self,
        url: String,
        wrapper_id: [u8; 32],
    ) -> Result<Option<crate::StoredRelayAttempt>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Relay(Box::new(RelayRequest::LoadAttempt {
                url,
                wrapper_id,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one retained cursor by exact relay identity.
    pub fn cursor(&self, url: String) -> Result<Option<crate::StoredCatchupCursor>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Relay(Box::new(RelayRequest::LoadCursor {
                url,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }
}

impl fmt::Debug for RelayStateHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayStateHandle")
            .finish_non_exhaustive()
    }
}

impl HarnessStateHandle {
    /// Applies one atomic durable managed-runtime coordination transition.
    pub fn apply(
        &self,
        mutation: StoredHarnessStateMutation,
    ) -> Result<HarnessLeaseOutcome, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Harness(Box::new(HarnessRequest::Apply {
                mutation: Box::new(mutation),
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one bounded deterministic snapshot of durable harness coordination state.
    pub fn load(&self, limit: usize) -> Result<StoredHarnessStateSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Harness(Box::new(HarnessRequest::Load {
                limit,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one exact durable managed-runtime delivery.
    pub fn delivery(
        &self,
        agent_id: AgentId,
        submission_id: hq_domain::MessageId,
    ) -> Result<Option<crate::StoredHarnessDelivery>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Harness(Box::new(HarnessRequest::Delivery {
                agent_id,
                submission_id,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the unique durable delivery associated with one provider operation.
    pub fn delivery_for_operation(
        &self,
        agent_id: AgentId,
        operation_id: OperationId,
    ) -> Result<Option<crate::StoredHarnessDelivery>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Harness(Box::new(
                HarnessRequest::DeliveryForOperation {
                    agent_id,
                    operation_id,
                    reply,
                },
            )))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one bounded runnable delivery prefix for one exact agent.
    pub fn runnable_deliveries(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> Result<Vec<crate::StoredHarnessDelivery>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Harness(Box::new(
                HarnessRequest::RunnableDeliveries {
                    agent_id,
                    limit,
                    reply,
                },
            )))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one exact managed-session control operation.
    pub fn session_operation(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<crate::HarnessSessionOperation>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Harness(Box::new(
                HarnessRequest::SessionOperation {
                    operation_id,
                    reply,
                },
            )))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }
}

impl fmt::Debug for HarnessStateHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HarnessStateHandle")
            .finish_non_exhaustive()
    }
}

impl ProjectSagaStateHandle {
    /// Loads one exact retained project workflow by stable operation identity.
    pub fn load(&self, operation_id: OperationId) -> Result<Option<StoredProjectSaga>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::ProjectSaga(Box::new(ProjectSagaRequest::Load {
                operation_id,
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Atomically begins one exact project workflow.
    pub fn begin(&self, record: StoredProjectSaga) -> Result<StoredProjectSagaBegin, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::ProjectSaga(Box::new(ProjectSagaRequest::Begin {
                record: Box::new(record),
                reply,
            })))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Atomically advances one exact project workflow checkpoint.
    pub fn replace(&self, record: StoredProjectSaga) -> Result<(), StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::ProjectSaga(Box::new(
                ProjectSagaRequest::Replace {
                    record: Box::new(record),
                    reply,
                },
            )))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one deterministic bounded set of runnable or reconcilable workflows.
    pub fn load_runnable(&self, limit: usize) -> Result<Vec<StoredProjectSaga>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::ProjectSaga(Box::new(
                ProjectSagaRequest::LoadRunnable { limit, reply },
            )))
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }
}

impl fmt::Debug for ProjectSagaStateHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSagaStateHandle")
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Opens a fresh or compatible database on a dedicated synchronous worker thread.
    pub fn open(path: impl AsRef<Path>, capacity: NonZeroUsize) -> Result<Self, StoreError> {
        Self::open_with_invalidations(path, capacity).map(|(store, _)| store)
    }

    /// Opens storage and returns its capacity-one coalesced post-commit observer.
    pub fn open_with_invalidations(
        path: impl AsRef<Path>,
        capacity: NonZeroUsize,
    ) -> Result<(Self, RevisionInvalidations), StoreError> {
        let path = path.as_ref().to_path_buf();
        let (requests, receiver) = mpsc::sync_channel(capacity.get());
        let (started, startup) = mpsc::sync_channel(1);
        let (wakes, wake_receiver) = mpsc::sync_channel(1);
        let latest = Arc::new(AtomicU64::new(0));
        let emitter = InvalidationEmitter {
            wakes,
            latest: Arc::clone(&latest),
        };
        let worker = thread::Builder::new()
            .name("hq-store".to_owned())
            .spawn(move || run(&path, &receiver, &started, &emitter))
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?;
        match startup.recv() {
            Ok(Ok(())) => Ok((
                Self {
                    requests,
                    worker: Some(worker),
                },
                RevisionInvalidations {
                    wakes: wake_receiver,
                    latest,
                },
            )),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(StoreError::new(StoreErrorClass::WorkerStopped))
            }
        }
    }

    /// Creates a relay-only request capability for owned synchronization tasks.
    pub fn relay_state_handle(&self) -> RelayStateHandle {
        RelayStateHandle {
            requests: self.requests.clone(),
        }
    }

    /// Creates a harness-state request capability for owned supervisor tasks.
    pub fn harness_state_handle(&self) -> HarnessStateHandle {
        HarnessStateHandle {
            requests: self.requests.clone(),
        }
    }

    /// Creates a project-workflow request capability for the owned saga worker.
    pub fn project_saga_state_handle(&self) -> ProjectSagaStateHandle {
        ProjectSagaStateHandle {
            requests: self.requests.clone(),
        }
    }

    /// Creates an application query/mutation capability without store shutdown ownership.
    pub fn application_state_handle(&self) -> ApplicationStateHandle {
        ApplicationStateHandle {
            requests: self.requests.clone(),
        }
    }

    /// Creates an owned background replication capability without store shutdown ownership.
    pub fn replication_handle(&self) -> ReplicationHandle {
        ReplicationHandle {
            requests: self.requests.clone(),
        }
    }

    /// Atomically ingests one verified fact and every derived durable state package.
    pub fn ingest_verified(
        &self,
        fact: VerifiedSemanticFact,
        policy: AuthorityPolicy,
    ) -> Result<IngestOutcome, StoreError> {
        self.replication_handle().ingest_verified(fact, policy)
    }

    /// Executes one retryable local fact-backed mutation in a single durable transaction.
    pub fn execute_local_mutation(
        &self,
        request: LocalMutationRequest,
    ) -> Result<MutationReceipt, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LocalMutation { request, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads and fully reverifies the immutable fact corpus.
    pub fn load_corpus(&self) -> Result<VerifiedFactCorpus, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Load { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Reverifies the corpus and runs every complete-batch reducer under one explicit policy.
    pub fn complete_snapshot(
        &self,
        policy: AuthorityPolicy,
    ) -> Result<CompleteSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::CompleteSnapshot { policy, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Rebuilds structural rows from the complete oracle and verifies them before commit.
    pub fn repair(&self, policy: AuthorityPolicy) -> Result<RepairOutcome, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Repair { policy, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the last successfully repaired normalized structural index without mutating it.
    pub fn load_reduction_index(&self) -> Result<ReductionIndexSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadReductionIndex { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the last successfully repaired typed authority projection view without mutation.
    pub fn load_authority_snapshot(&self) -> Result<AuthorityProjectionSnapshot, StoreError> {
        self.replication_handle().load_authority_snapshot()
    }

    /// Loads the last successfully repaired typed conversation/activity view without mutation.
    pub fn load_conversation_snapshot(&self) -> Result<ConversationProjectionSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadConversationSnapshot { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one bounded indexed page in reducer-derived conversation-local order.
    pub fn load_conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<ConversationEntry>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadConversationEntries {
                key: key.clone(),
                limit,
                cursor: cursor.cloned(),
                reply,
            })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the last successfully repaired typed named-agent view without mutation.
    pub fn load_agent_snapshot(&self) -> Result<AgentProjectionSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadAgentSnapshot { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the last successfully repaired typed project view without mutation.
    pub fn load_project_snapshot(&self) -> Result<ProjectProjectionSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadProjectSnapshot { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads revision and every application projection package from one serialized store point.
    pub fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::AuthoritativeSnapshot { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Returns the monotonic revision of the last committed relevant change.
    pub fn current_revision(&self) -> Result<Revision, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::CurrentRevision { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads the exact retained answer for one retryable command, when present.
    pub fn load_mutation_receipt(
        &self,
        command_id: CommandId,
    ) -> Result<Option<MutationReceipt>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadMutationReceipt { command_id, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads a bounded deterministic prefix of durable per-recipient outbox intents.
    pub fn load_outbox_intents(&self, limit: usize) -> Result<Vec<OutboxIntent>, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadOutboxIntents { limit, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Applies one atomic durable relay synchronization transition.
    pub fn apply_relay_state(&self, mutation: StoredRelayStateMutation) -> Result<(), StoreError> {
        self.relay_state_handle().apply(mutation)
    }

    /// Loads one deterministic bounded page of durable relay synchronization state.
    pub fn load_relay_state(&self, limit: usize) -> Result<StoredRelayStateSnapshot, StoreError> {
        self.relay_state_handle().load(limit)
    }

    /// Loads one deterministic bounded keyset page of durable relay state.
    pub fn load_relay_state_page(
        &self,
        query: StoredRelayStateQuery,
    ) -> Result<StoredRelayStatePage, StoreError> {
        self.relay_state_handle().load_page(query)
    }

    /// Applies one atomic durable harness supervision transition.
    pub fn apply_harness_state(
        &self,
        mutation: StoredHarnessStateMutation,
    ) -> Result<HarnessLeaseOutcome, StoreError> {
        self.harness_state_handle().apply(mutation)
    }

    /// Loads one bounded deterministic harness supervision snapshot.
    pub fn load_harness_state(
        &self,
        limit: usize,
    ) -> Result<StoredHarnessStateSnapshot, StoreError> {
        self.harness_state_handle().load(limit)
    }

    /// Atomically begins one exact durable project workflow.
    pub fn begin_project_saga(
        &self,
        record: StoredProjectSaga,
    ) -> Result<StoredProjectSagaBegin, StoreError> {
        self.project_saga_state_handle().begin(record)
    }

    /// Atomically advances one exact durable project workflow checkpoint.
    pub fn replace_project_saga(&self, record: StoredProjectSaga) -> Result<(), StoreError> {
        self.project_saga_state_handle().replace(record)
    }

    /// Loads one deterministic bounded project workflow recovery prefix.
    pub fn load_runnable_project_sagas(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredProjectSaga>, StoreError> {
        self.project_saga_state_handle().load_runnable(limit)
    }

    /// Stops intake, acknowledges worker shutdown, and joins the owning thread.
    pub fn close(mut self) -> Result<(), StoreError> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<(), StoreError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        let (reply, response) = mpsc::sync_channel(1);
        let sent = self.requests.send(Request::Close { reply }).is_ok();
        let acknowledged = sent && response.recv().is_ok();
        let joined = worker.join().is_ok();
        if acknowledged && joined {
            Ok(())
        } else {
            Err(StoreError::new(StoreErrorClass::WorkerStopped))
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl fmt::Debug for Store {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Store").finish_non_exhaustive()
    }
}

#[allow(clippy::too_many_lines)]
fn run(
    path: &Path,
    receiver: &Receiver<Request>,
    started: &SyncSender<Result<(), StoreError>>,
    invalidations: &InvalidationEmitter,
) {
    let database = Database::open(path);
    let Ok(mut database) = database else {
        let _ = started.send(database.map(|_| ()));
        return;
    };
    if started.send(Ok(())).is_err() {
        return;
    }
    while let Ok(request) = receiver.recv() {
        match request {
            Request::LocalMutation { request, reply } => {
                execute_local_mutation(&mut database, request, &reply, invalidations);
            }
            Request::LoadMailboxDrafts { reply } => {
                let _ = reply.send(database.load_mailbox_drafts());
            }
            Request::SaveMailboxDraft { request, reply } => {
                let _ = reply.send(database.save_mailbox_draft(&request));
            }
            Request::DeleteMailboxDraft { request, reply } => {
                let _ = reply.send(database.delete_mailbox_draft(request));
            }
            Request::Ingest {
                fact,
                policy,
                reply,
            } => {
                let result = database.ingest(&fact, policy);
                if let Ok(outcome) = result
                    && outcome.is_inserted()
                {
                    invalidations.publish(outcome.revision());
                }
                let _ = reply.send(result);
            }
            Request::Load { reply } => {
                let _ = reply.send(database.load().map(VerifiedFactCorpus::new));
            }
            Request::CompleteSnapshot { policy, reply } => {
                let _ = reply.send(database.complete_snapshot(policy));
            }
            Request::Repair { policy, reply } => {
                let _ = reply.send(database.repair(policy));
            }
            Request::LoadReductionIndex { reply } => {
                let _ = reply.send(database.load_reduction_index());
            }
            Request::StateHealthSnapshot { repair, reply } => {
                let _ = reply.send(load_state_health_snapshot(&mut database, repair));
            }
            Request::LoadAuthoritySnapshot { reply } => {
                let _ = reply.send(database.load_authority_snapshot());
            }
            Request::LoadConversationSnapshot { reply } => {
                let _ = reply.send(database.load_conversation_snapshot());
            }
            Request::LoadConversationEntries {
                key,
                limit,
                cursor,
                reply,
            } => {
                let _ =
                    reply.send(database.load_conversation_entries(&key, limit, cursor.as_ref()));
            }
            Request::LoadAgentSnapshot { reply } => {
                let _ = reply.send(database.load_agent_snapshot());
            }
            Request::LoadProjectSnapshot { reply } => {
                let _ = reply.send(database.load_project_snapshot());
            }
            Request::AuthoritativeSnapshot { reply } => {
                let _ = reply.send(database.load_authoritative_snapshot());
            }
            Request::CanonicalEvidence {
                roots,
                maximum_facts,
                maximum_bytes,
                reply,
            } => {
                reply_canonical_evidence(
                    &mut database,
                    &roots,
                    maximum_facts,
                    maximum_bytes,
                    &reply,
                );
            }
            Request::CurrentRevision { reply } => {
                let _ = reply.send(database.current_revision());
            }
            Request::LoadMutationReceipt { command_id, reply } => {
                let _ = reply.send(database.load_mutation_receipt(command_id));
            }
            Request::LoadOutboxIntents { limit, reply } => {
                let _ = reply.send(database.load_outbox_intents(limit));
            }
            Request::Relay(request) => handle_relay_request(&mut database, *request),
            Request::Harness(request) => handle_harness_request(&mut database, *request),
            Request::ProjectSaga(request) => handle_project_saga_request(&mut database, *request),
            Request::Close { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

fn load_state_health_snapshot(
    database: &mut Database,
    repair: Option<AuthorityPolicy>,
) -> Result<(Revision, ReductionIndexSnapshot), StoreError> {
    let index = match repair {
        Some(policy) => database.repair(policy)?.persisted().clone(),
        None => database.load_reduction_index()?,
    };
    Ok((database.current_revision()?, index))
}

fn execute_local_mutation(
    database: &mut Database,
    request: LocalMutationRequest,
    reply: &SyncSender<Result<MutationReceipt, StoreError>>,
    invalidations: &InvalidationEmitter,
) {
    match database.execute_local_mutation(request) {
        Ok((receipt, inserted)) => {
            if inserted {
                invalidations.publish(receipt.revision());
            }
            let _ = reply.send(Ok(receipt));
        }
        Err(error) => {
            let _ = reply.send(Err(error));
        }
    }
}

fn reply_canonical_evidence(
    database: &mut Database,
    roots: &BTreeSet<FactId>,
    maximum_facts: usize,
    maximum_bytes: usize,
    reply: &SyncSender<Result<Vec<CanonicalEvidence>, StoreError>>,
) {
    let result = database
        .load()
        .and_then(|facts| canonical_evidence_closure(&facts, roots, maximum_facts, maximum_bytes));
    let _ = reply.send(result);
}

fn canonical_evidence_closure(
    facts: &[VerifiedSemanticFact],
    roots: &BTreeSet<FactId>,
    maximum_facts: usize,
    maximum_bytes: usize,
) -> Result<Vec<CanonicalEvidence>, StoreError> {
    if roots.is_empty() || maximum_facts == 0 || maximum_bytes == 0 {
        return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
    }
    let by_id = facts
        .iter()
        .map(|fact| (fact.fact().id(), fact))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::new();
    let mut pending = roots.iter().copied().collect::<Vec<_>>();
    let mut total_bytes = 0_usize;
    while let Some(current) = pending.pop() {
        if !selected.insert(current) {
            continue;
        }
        let fact = by_id
            .get(&current)
            .ok_or_else(|| StoreError::new(StoreErrorClass::InvalidOperationalRequest))?;
        total_bytes = total_bytes
            .checked_add(fact.verified_event().exact_event_bytes().len())
            .ok_or_else(|| StoreError::new(StoreErrorClass::InvalidOperationalRequest))?;
        if selected.len() > maximum_facts || total_bytes > maximum_bytes {
            return Err(StoreError::new(StoreErrorClass::InvalidOperationalRequest));
        }
        pending.extend(fact.fact().causal().parents().iter().copied());
    }
    selected
        .into_iter()
        .map(|fact_id| {
            by_id
                .get(&fact_id)
                .map(|fact| CanonicalEvidence {
                    fact_id,
                    exact_event: fact.verified_event().exact_event_bytes().to_vec(),
                })
                .ok_or_else(|| StoreError::new(StoreErrorClass::InvalidOperationalRequest))
        })
        .collect()
}

fn handle_project_saga_request(database: &mut Database, request: ProjectSagaRequest) {
    match request {
        ProjectSagaRequest::Load {
            operation_id,
            reply,
        } => {
            let _ = reply.send(database.load_project_saga(operation_id));
        }
        ProjectSagaRequest::Begin { record, reply } => {
            let _ = reply.send(database.begin_project_saga(&record));
        }
        ProjectSagaRequest::Replace { record, reply } => {
            let _ = reply.send(database.replace_project_saga(&record));
        }
        ProjectSagaRequest::LoadRunnable { limit, reply } => {
            let _ = reply.send(database.load_runnable_project_sagas(limit));
        }
    }
}

fn handle_harness_request(database: &mut Database, request: HarnessRequest) {
    match request {
        HarnessRequest::Apply { mutation, reply } => {
            let _ = reply.send(database.apply_harness_state(*mutation));
        }
        HarnessRequest::Load { limit, reply } => {
            let _ = reply.send(database.load_harness_state(limit));
        }
        HarnessRequest::Delivery {
            agent_id,
            submission_id,
            reply,
        } => {
            let _ = reply.send(database.load_harness_delivery(agent_id, submission_id));
        }
        HarnessRequest::DeliveryForOperation {
            agent_id,
            operation_id,
            reply,
        } => {
            let _ =
                reply.send(database.load_harness_delivery_for_operation(agent_id, operation_id));
        }
        HarnessRequest::RunnableDeliveries {
            agent_id,
            limit,
            reply,
        } => {
            let _ = reply.send(database.load_runnable_harness_deliveries(agent_id, limit));
        }
        HarnessRequest::SessionOperation {
            operation_id,
            reply,
        } => {
            let _ = reply.send(database.load_harness_session_operation(operation_id));
        }
    }
}

fn handle_relay_request(database: &mut Database, request: RelayRequest) {
    match request {
        RelayRequest::Apply { mutation, reply } => {
            let _ = reply.send(database.apply_relay_state(mutation));
        }
        RelayRequest::LoadPage { query, reply } => {
            let _ = reply.send(database.load_relay_state(&query));
        }
        RelayRequest::LoadPrepared {
            fact_id,
            recipient,
            reply,
        } => {
            let _ = reply.send(database.load_prepared_relay_lineage(fact_id, recipient));
        }
        RelayRequest::LoadAttempt {
            url,
            wrapper_id,
            reply,
        } => {
            let _ = reply.send(database.load_relay_attempt(&url, wrapper_id));
        }
        RelayRequest::LoadCursor { url, reply } => {
            let _ = reply.send(database.load_relay_cursor(&url));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        num::NonZeroUsize,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
            mpsc::{TrySendError, sync_channel},
        },
        thread,
    };

    use super::{Request, Store};
    use crate::StoreErrorClass;

    #[test]
    fn bounded_mailbox_applies_backpressure_before_a_receiver_runs() {
        let (sender, _receiver) = sync_channel(1);
        sender.try_send(1).expect("first request fits");
        assert_eq!(sender.try_send(2), Err(TrySendError::Full(2)));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn dropped_one_shot_reply_does_not_stop_the_actor() {
        let (root, database) = test_path();
        let store = Store::open(&database, NonZeroUsize::MIN).expect("store opens");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::Load { reply })
            .expect("request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::Repair {
                policy: hq_reducer::AuthorityPolicy::new(
                    hq_domain::InstallationId::from_bytes([0x11; 32]),
                    hq_domain::MailboxId::from_bytes([0x33; 32]),
                ),
                reply,
            })
            .expect("repair request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::LoadAuthoritySnapshot { reply })
            .expect("authority request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::LoadConversationSnapshot { reply })
            .expect("conversation request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::LoadAgentSnapshot { reply })
            .expect("agent request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::LoadProjectSnapshot { reply })
            .expect("project request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::CurrentRevision { reply })
            .expect("revision request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::LoadMutationReceipt {
                command_id: hq_domain::CommandId::from_bytes([1; 32]),
                reply,
            })
            .expect("receipt request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::LoadOutboxIntents { limit: 1, reply })
            .expect("outbox request is accepted");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::Ingest {
                fact: Box::new(crate::database::tests::fixture()),
                policy: hq_reducer::AuthorityPolicy::new(
                    hq_domain::InstallationId::from_bytes([0x11; 32]),
                    hq_domain::MailboxId::from_bytes([0x33; 32]),
                ),
                reply,
            })
            .expect("ingest request is accepted");
        let plan = hq_protocol::CanonicalEventPlan::from_fact(
            crate::database::tests::root_fixture().fact(),
        );
        let signer = hq_protocol::Bip340Signer::from_secret_bytes({
            let mut secret = [0_u8; 32];
            secret[31] = 1;
            secret
        })
        .expect("fixture signer constructs");
        let (reply, response) = sync_channel(1);
        drop(response);
        store
            .requests
            .send(Request::LocalMutation {
                request: crate::LocalMutationRequest::new(
                    hq_domain::CommandId::from_bytes([0x21; 32]),
                    hq_domain::CommandDigest::from_bytes([0x22; 32]),
                    hq_reducer::AuthorityPolicy::new(
                        hq_domain::InstallationId::from_bytes([0x11; 32]),
                        hq_domain::MailboxId::from_bytes([0x33; 32]),
                    ),
                    Arc::new(signer),
                    move |_| {
                        crate::LocalMutationDecision::commit(
                            plan,
                            [6; 32],
                            crate::MutationResultBytes::new(b"root".to_vec())
                                .expect("result is bounded"),
                        )
                    },
                ),
                reply,
            })
            .expect("local mutation request is accepted");
        assert_eq!(
            store
                .load_corpus()
                .expect("actor survives dropped reply")
                .len(),
            2
        );
        store.close().expect("store closes");
        fs::remove_dir_all(root).expect("test state cleans up");
    }

    #[test]
    fn stopped_worker_closes_intake_and_shutdown_reports_failure() {
        let (requests, receiver) = sync_channel(1);
        drop(receiver);
        let worker = thread::spawn(|| {});
        let mut store = Store {
            requests,
            worker: Some(worker),
        };

        let error = store.load_corpus().expect_err("closed intake rejects");
        assert_eq!(error.class(), StoreErrorClass::ActorClosed);
        let shutdown = store
            .shutdown()
            .expect_err("missing acknowledgement rejects");
        assert_eq!(shutdown.class(), StoreErrorClass::WorkerStopped);
    }

    fn test_path() -> (std::path::PathBuf, std::path::PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "hq-rust-store-actor-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("test root creates");
        let database = root.join("state").join("hq.sqlite3");
        (root, database)
    }
}
