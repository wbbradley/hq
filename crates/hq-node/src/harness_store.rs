//! Node-only mapping between neutral harness records and storage-owned records.

use hq_harness::{
    HarnessDeliveryRecord, HarnessDeliveryState, HarnessError, HarnessErrorClass,
    HarnessEventCheckpoint, HarnessLeaseOutcome, HarnessOwnerToken, HarnessReadySession,
    HarnessSessionOperation, HarnessStateMutation, HarnessStatePort, HarnessStateSnapshot,
    HarnessWorkerLease,
};
use hq_store::{
    HarnessStateHandle, Store, StoreError, StoreErrorClass, StoredHarnessDelivery,
    StoredHarnessDeliveryState, StoredHarnessEventCheckpoint, StoredHarnessLease,
    StoredHarnessReadySession, StoredHarnessStateMutation, StoredHarnessStateSnapshot,
};

/// Neutral harness durable-state capability backed by the sole store actor.
#[derive(Clone, Debug)]
pub struct HarnessStoreAdapter {
    store: HarnessStateHandle,
}

impl HarnessStoreAdapter {
    /// Creates an owned harness state capability without exposing store shutdown ownership.
    pub fn new(store: &Store) -> Self {
        Self {
            store: store.harness_state_handle(),
        }
    }
}

impl HarnessStatePort for HarnessStoreAdapter {
    fn apply(&self, mutation: HarnessStateMutation) -> Result<HarnessLeaseOutcome, HarnessError> {
        let conflict = mutation_conflict(&mutation);
        self.store
            .apply(store_mutation(mutation))
            .map(map_lease_outcome)
            .map_err(|error| map_store_error(error, conflict))
    }

    fn load(&self, limit: usize) -> Result<HarnessStateSnapshot, HarnessError> {
        self.store
            .load(limit)
            .map_err(|error| map_store_error(error, HarnessErrorClass::PersistenceCollision))
            .and_then(map_snapshot)
    }

    fn session_operation(
        &self,
        operation_id: hq_domain::OperationId,
    ) -> Result<Option<HarnessSessionOperation>, HarnessError> {
        self.store
            .session_operation(operation_id)
            .map_err(|error| map_store_error(error, HarnessErrorClass::PersistenceCollision))
    }

    fn delivery(
        &self,
        agent_id: hq_domain::AgentId,
        submission_id: hq_domain::MessageId,
    ) -> Result<Option<HarnessDeliveryRecord>, HarnessError> {
        self.store
            .delivery(agent_id, submission_id)
            .map_err(|error| map_store_error(error, HarnessErrorClass::SubmissionIdentityConflict))
            .map(|stored| stored.map(map_delivery))
    }

    fn runnable_deliveries(
        &self,
        agent_id: hq_domain::AgentId,
        limit: usize,
    ) -> Result<Vec<HarnessDeliveryRecord>, HarnessError> {
        self.store
            .runnable_deliveries(agent_id, limit)
            .map_err(|error| map_store_error(error, HarnessErrorClass::PersistenceCollision))
            .map(|stored| stored.into_iter().map(map_delivery).collect())
    }
}

fn store_mutation(mutation: HarnessStateMutation) -> StoredHarnessStateMutation {
    match mutation {
        HarnessStateMutation::ClaimLease {
            agent_id,
            owner_token,
            now_millis,
            expires_at_millis,
        } => StoredHarnessStateMutation::ClaimLease {
            agent_id,
            owner_token: *owner_token.as_bytes(),
            now_millis,
            expires_at_millis,
        },
        HarnessStateMutation::ReleaseLease {
            agent_id,
            owner_token,
        } => StoredHarnessStateMutation::ReleaseLease {
            agent_id,
            owner_token: *owner_token.as_bytes(),
        },
        HarnessStateMutation::SetReadySession { owner_token, ready } => {
            StoredHarnessStateMutation::SetReadySession {
                owner_token: *owner_token.as_bytes(),
                ready: store_ready(ready),
            }
        }
        HarnessStateMutation::QueueSessionOperation(operation) => {
            StoredHarnessStateMutation::QueueSessionOperation(operation)
        }
        HarnessStateMutation::SetSessionOperationState {
            operation_id,
            state,
        } => StoredHarnessStateMutation::SetSessionOperationState {
            operation_id,
            state,
        },
        HarnessStateMutation::QueueDelivery(delivery) => {
            StoredHarnessStateMutation::QueueDelivery(store_delivery(delivery))
        }
        HarnessStateMutation::SetDeliveryState {
            agent_id,
            submission_id,
            owner_token,
            state,
        } => StoredHarnessStateMutation::SetDeliveryState {
            agent_id,
            submission_id,
            owner_token: *owner_token.as_bytes(),
            state: store_delivery_state(state),
        },
        HarnessStateMutation::CheckpointEvent {
            owner_token,
            checkpoint,
        } => StoredHarnessStateMutation::CheckpointEvent {
            owner_token: *owner_token.as_bytes(),
            checkpoint: store_checkpoint(&checkpoint),
        },
    }
}

fn map_snapshot(stored: StoredHarnessStateSnapshot) -> Result<HarnessStateSnapshot, HarnessError> {
    Ok(HarnessStateSnapshot {
        leases: stored
            .leases
            .into_iter()
            .map(map_lease)
            .collect::<Result<_, _>>()?,
        ready_sessions: stored.ready_sessions.into_iter().map(map_ready).collect(),
        session_operations: stored.session_operations,
        deliveries: stored.deliveries.into_iter().map(map_delivery).collect(),
        events: stored.events.iter().map(map_checkpoint).collect(),
    })
}

fn store_ready(ready: HarnessReadySession) -> StoredHarnessReadySession {
    StoredHarnessReadySession {
        agent_id: ready.agent_id,
        provider_id: ready.provider_id,
        session_id: ready.session_id,
    }
}

fn map_ready(stored: StoredHarnessReadySession) -> HarnessReadySession {
    HarnessReadySession {
        agent_id: stored.agent_id,
        provider_id: stored.provider_id,
        session_id: stored.session_id,
    }
}

fn store_delivery(delivery: HarnessDeliveryRecord) -> StoredHarnessDelivery {
    StoredHarnessDelivery {
        agent_id: delivery.agent_id,
        provider_id: delivery.provider_id,
        session_id: delivery.session_id,
        submission_id: delivery.submission.submission_id,
        digest: delivery.submission.digest,
        operation_id: delivery.submission.operation_id,
        body: delivery.submission.body,
        queued_at_millis: delivery.queued_at_millis,
        state: store_delivery_state(delivery.state),
    }
}

fn map_delivery(stored: StoredHarnessDelivery) -> HarnessDeliveryRecord {
    HarnessDeliveryRecord {
        agent_id: stored.agent_id,
        provider_id: stored.provider_id,
        session_id: stored.session_id,
        submission: hq_harness::HarnessSubmission {
            submission_id: stored.submission_id,
            digest: stored.digest,
            operation_id: stored.operation_id,
            body: stored.body,
        },
        queued_at_millis: stored.queued_at_millis,
        state: map_delivery_state(stored.state),
    }
}

const fn store_delivery_state(state: HarnessDeliveryState) -> StoredHarnessDeliveryState {
    match state {
        HarnessDeliveryState::Pending => StoredHarnessDeliveryState::Pending,
        HarnessDeliveryState::Uncertain => StoredHarnessDeliveryState::Uncertain,
        HarnessDeliveryState::Accepted => StoredHarnessDeliveryState::Accepted,
        HarnessDeliveryState::Rejected => StoredHarnessDeliveryState::Rejected,
    }
}

const fn map_delivery_state(state: StoredHarnessDeliveryState) -> HarnessDeliveryState {
    match state {
        StoredHarnessDeliveryState::Pending => HarnessDeliveryState::Pending,
        StoredHarnessDeliveryState::Uncertain => HarnessDeliveryState::Uncertain,
        StoredHarnessDeliveryState::Accepted => HarnessDeliveryState::Accepted,
        StoredHarnessDeliveryState::Rejected => HarnessDeliveryState::Rejected,
    }
}

fn store_checkpoint(checkpoint: &HarnessEventCheckpoint) -> StoredHarnessEventCheckpoint {
    StoredHarnessEventCheckpoint {
        agent_id: checkpoint.agent_id,
        event_id: checkpoint.event_id,
        digest: checkpoint.digest,
        output_committed: checkpoint.output_complete,
        activity_committed: checkpoint.activity_complete,
    }
}

fn map_checkpoint(stored: &StoredHarnessEventCheckpoint) -> HarnessEventCheckpoint {
    HarnessEventCheckpoint {
        agent_id: stored.agent_id,
        event_id: stored.event_id,
        digest: stored.digest,
        output_complete: stored.output_committed,
        activity_complete: stored.activity_committed,
    }
}

fn map_lease(stored: StoredHarnessLease) -> Result<HarnessWorkerLease, HarnessError> {
    Ok(HarnessWorkerLease {
        agent_id: stored.agent_id,
        owner_token: HarnessOwnerToken::from_bytes(stored.owner_token)
            .map_err(|_| HarnessError::new(HarnessErrorClass::PersistenceCollision))?,
        expires_at_millis: stored.expires_at_millis,
    })
}

const fn map_lease_outcome(stored: hq_store::HarnessLeaseOutcome) -> HarnessLeaseOutcome {
    match stored {
        hq_store::HarnessLeaseOutcome::Acquired => HarnessLeaseOutcome::Acquired,
        hq_store::HarnessLeaseOutcome::Held => HarnessLeaseOutcome::Held,
        hq_store::HarnessLeaseOutcome::Released => HarnessLeaseOutcome::Released,
    }
}

const fn mutation_conflict(mutation: &HarnessStateMutation) -> HarnessErrorClass {
    match mutation {
        HarnessStateMutation::ClaimLease { .. }
        | HarnessStateMutation::ReleaseLease { .. }
        | HarnessStateMutation::SetReadySession { .. } => HarnessErrorClass::OwnershipConflict,
        HarnessStateMutation::QueueSessionOperation(_)
        | HarnessStateMutation::SetSessionOperationState { .. } => {
            HarnessErrorClass::PersistenceCollision
        }
        HarnessStateMutation::QueueDelivery(_) | HarnessStateMutation::SetDeliveryState { .. } => {
            HarnessErrorClass::SubmissionIdentityConflict
        }
        HarnessStateMutation::CheckpointEvent { .. } => HarnessErrorClass::PersistenceCollision,
    }
}

const fn map_store_error(error: StoreError, conflict: HarnessErrorClass) -> HarnessError {
    let class = match error.class() {
        StoreErrorClass::HarnessStateConflict | StoreErrorClass::IdentityCollision => conflict,
        StoreErrorClass::InvalidOperationalRequest => HarnessErrorClass::InvalidInput,
        StoreErrorClass::ActorClosed
        | StoreErrorClass::WorkerStopped
        | StoreErrorClass::DatabaseUnavailable
        | StoreErrorClass::FileSystem => HarnessErrorClass::Unavailable,
        StoreErrorClass::InvalidPath
        | StoreErrorClass::SymbolicLink
        | StoreErrorClass::UnsafePermissions
        | StoreErrorClass::IncompatibleSchema
        | StoreErrorClass::CorruptDatabase
        | StoreErrorClass::InvalidEvidence
        | StoreErrorClass::MutationConflict
        | StoreErrorClass::RelayStateConflict
        | StoreErrorClass::ProjectSagaConflict
        | StoreErrorClass::RelayStagingFull
        | StoreErrorClass::RevisionExhausted
        | StoreErrorClass::OperationalStateCorrupt
        | StoreErrorClass::ReductionFailed
        | StoreErrorClass::NotRepaired
        | StoreErrorClass::RebuildableStateCorrupt => HarnessErrorClass::PersistenceCollision,
    };
    HarnessError::new(class)
}
