//! Bounded typed store actor and public corpus owner.

use std::{
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

use hq_domain::{CommandId, Page, PageCursor, Revision};
use hq_protocol::VerifiedSemanticFact;
use hq_reducer::{AuthorityPolicy, ConversationKey};

use crate::{
    AgentProjectionSnapshot, AuthoritativeSnapshot, AuthorityProjectionSnapshot, CompleteSnapshot,
    ConversationEntry, ConversationProjectionSnapshot, LocalMutationRequest, MutationReceipt,
    OutboxIntent, ProjectProjectionSnapshot, ReductionIndexSnapshot, StoreError, StoreErrorClass,
    StoredRelayStateMutation, StoredRelayStateSnapshot, database::Database,
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
    ApplyRelayState {
        mutation: StoredRelayStateMutation,
        reply: SyncSender<Result<(), StoreError>>,
    },
    LoadRelayState {
        limit: usize,
        reply: SyncSender<Result<StoredRelayStateSnapshot, StoreError>>,
    },
    Close {
        reply: SyncSender<()>,
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

impl RelayStateHandle {
    /// Applies one atomic durable relay synchronization transition.
    pub fn apply(&self, mutation: StoredRelayStateMutation) -> Result<(), StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::ApplyRelayState { mutation, reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
    }

    /// Loads one deterministic bounded page of durable relay synchronization state.
    pub fn load(&self, limit: usize) -> Result<StoredRelayStateSnapshot, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadRelayState { limit, reply })
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

    /// Atomically ingests one verified fact and every derived durable state package.
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
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::LoadAuthoritySnapshot { reply })
            .map_err(|_| StoreError::new(StoreErrorClass::ActorClosed))?;
        response
            .recv()
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?
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
            Request::CurrentRevision { reply } => {
                let _ = reply.send(database.current_revision());
            }
            Request::LoadMutationReceipt { command_id, reply } => {
                let _ = reply.send(database.load_mutation_receipt(command_id));
            }
            Request::LoadOutboxIntents { limit, reply } => {
                let _ = reply.send(database.load_outbox_intents(limit));
            }
            Request::ApplyRelayState { mutation, reply } => {
                let _ = reply.send(database.apply_relay_state(mutation));
            }
            Request::LoadRelayState { limit, reply } => {
                let _ = reply.send(database.load_relay_state(limit));
            }
            Request::Close { reply } => {
                let _ = reply.send(());
                break;
            }
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
