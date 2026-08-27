//! Bounded typed store actor and public corpus owner.

use std::{
    fmt,
    num::NonZeroUsize,
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

use hq_protocol::VerifiedSemanticFact;
use hq_reducer::AuthorityPolicy;

use crate::{
    AgentProjectionSnapshot, AuthorityProjectionSnapshot, CompleteSnapshot,
    ConversationProjectionSnapshot, ReductionIndexSnapshot, StoreError, StoreErrorClass,
    database::Database,
};

/// Result of appending immutable verified evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    /// A previously unknown fact was durably inserted.
    Inserted,
    /// The exact fact evidence and normalized indexes were already present.
    AlreadyPresent,
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
}

impl RepairOutcome {
    pub(crate) fn new(
        complete: CompleteSnapshot,
        persisted: ReductionIndexSnapshot,
        authority: AuthorityProjectionSnapshot,
        conversation: ConversationProjectionSnapshot,
        agent: AgentProjectionSnapshot,
    ) -> Self {
        Self {
            complete,
            persisted,
            authority,
            conversation,
            agent,
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

    /// Consumes the outcome into its complete and persisted snapshots.
    pub fn into_parts(
        self,
    ) -> (
        CompleteSnapshot,
        ReductionIndexSnapshot,
        AuthorityProjectionSnapshot,
        ConversationProjectionSnapshot,
        AgentProjectionSnapshot,
    ) {
        (
            self.complete,
            self.persisted,
            self.authority,
            self.conversation,
            self.agent,
        )
    }
}

enum Request {
    Append {
        fact: Box<VerifiedSemanticFact>,
        reply: SyncSender<Result<AppendOutcome, StoreError>>,
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
    LoadAgentSnapshot {
        reply: SyncSender<Result<AgentProjectionSnapshot, StoreError>>,
    },
    Close {
        reply: SyncSender<()>,
    },
}

/// Sole owner of one bounded SQLite worker and its request intake.
///
/// The value is intentionally not `Clone`; callers that need shared access can place it in an
/// `Arc`, preserving one explicit owner and one shutdown point.
pub struct Store {
    requests: SyncSender<Request>,
    worker: Option<JoinHandle<()>>,
}

impl Store {
    /// Opens a fresh or compatible database on a dedicated synchronous worker thread.
    pub fn open(path: impl AsRef<Path>, capacity: NonZeroUsize) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let (requests, receiver) = mpsc::sync_channel(capacity.get());
        let (started, startup) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("hq-store".to_owned())
            .spawn(move || run(&path, &receiver, &started))
            .map_err(|_| StoreError::new(StoreErrorClass::WorkerStopped))?;
        match startup.recv() {
            Ok(Ok(())) => Ok(Self {
                requests,
                worker: Some(worker),
            }),
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

    /// Durably appends one complete verified semantic fact.
    pub fn append_verified(&self, fact: VerifiedSemanticFact) -> Result<AppendOutcome, StoreError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .send(Request::Append {
                fact: Box::new(fact),
                reply,
            })
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

fn run(path: &Path, receiver: &Receiver<Request>, started: &SyncSender<Result<(), StoreError>>) {
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
            Request::Append { fact, reply } => {
                let _ = reply.send(database.append(&fact));
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
            Request::LoadAgentSnapshot { reply } => {
                let _ = reply.send(database.load_agent_snapshot());
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
        assert!(
            store
                .load_corpus()
                .expect("actor survives dropped reply")
                .is_empty()
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
