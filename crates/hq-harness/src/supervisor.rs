//! Exact-owner managed-runtime supervision over consumer-owned durable and persistence ports.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    num::{NonZeroU64, NonZeroUsize},
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hq_domain::{
    ActivityKind, ActivityStatus, AgentId, AssignmentId, CommandDigest, DispatchId, MessageId,
    OperationId, ProjectId, ProviderId, ProviderSessionId, ThreadId,
};
use sha2::{Digest, Sha256};

use crate::{
    HarnessActivity, HarnessBufferPush, HarnessBufferedEvent, HarnessCancellationOutcome,
    HarnessDrainOutcome, HarnessEnvironment, HarnessError, HarnessErrorClass, HarnessEvent,
    HarnessEventBuffer, HarnessEventPoll, HarnessInstanceRequest, HarnessInteractiveAnswer,
    HarnessInteractiveRequest, HarnessOutput, HarnessOutputKind, HarnessRegistry, HarnessSession,
    HarnessSessionRequest, HarnessSnapshotKey, HarnessSubmission, HarnessSubmissionLookup,
    HarnessSubmissionOutcome,
};

/// Maximum state rows inspected by one supervisor repair pass.
pub const MAX_HARNESS_SUPERVISOR_STATE_ITEMS: usize = 1_024;

/// Opaque stable identity for one active interactive-response capability.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HarnessResponderId([u8; 32]);

impl HarnessResponderId {
    /// Constructs a responder identity, rejecting the absent identity.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, HarnessError> {
        if bytes == [0; 32] {
            Err(HarnessError::new(HarnessErrorClass::InvalidInput))
        } else {
            Ok(Self(bytes))
        }
    }

    /// Borrows the exact opaque bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for HarnessResponderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HarnessResponderId([redacted])")
    }
}

/// Opaque stable capability identifying one exact logical worker owner.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HarnessOwnerToken([u8; 32]);

impl HarnessOwnerToken {
    /// Constructs a token from injected stable bytes, rejecting the absent identity.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, HarnessError> {
        if bytes == [0; 32] {
            Err(HarnessError::new(HarnessErrorClass::InvalidInput))
        } else {
            Ok(Self(bytes))
        }
    }

    /// Borrows the exact opaque bytes for record-only persistence adapters.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for HarnessOwnerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HarnessOwnerToken([redacted])")
    }
}

/// Result of one exact-token durable lease transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessLeaseOutcome {
    /// This exact token acquired or renewed ownership.
    Acquired,
    /// Another live token owns the worker, or this token is stale.
    Held,
    /// This exact token released ownership.
    Released,
}

/// Passive durable lease record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HarnessWorkerLease {
    /// Named agent whose logical worker is owned.
    pub agent_id: AgentId,
    /// Exact opaque owner capability.
    pub owner_token: HarnessOwnerToken,
    /// Injected absolute deadline.
    pub expires_at_millis: u64,
}

/// Passive acknowledged durable-session record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessReadySession {
    /// Named agent bound to the session.
    pub agent_id: AgentId,
    /// Neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact acknowledged provider session.
    pub session_id: ProviderSessionId,
}

/// Exact provider-neutral lifecycle action retained for request reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessSessionOperationKind {
    /// Create a fresh durable provider session.
    Start,
    /// Resume exactly the cited durable provider session.
    Resume(ProviderSessionId),
    /// Stop only the current local runtime owner.
    Stop,
}

/// Monotonic durable disposition of one managed-session control operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessSessionOperationState {
    /// Exact request identity is durable but no provider call has begun.
    Prepared,
    /// The provider boundary may have been crossed and requires observation.
    Uncertain,
    /// The exact provider session was acknowledged ready.
    Ready(ProviderSessionId),
    /// The local runtime was authoritatively stopped.
    Stopped,
    /// The operation was authoritatively rejected.
    Rejected,
}

/// Passive durable managed-session operation without environment or path data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessSessionOperation {
    /// Stable external-operation identity.
    pub operation_id: hq_domain::OperationId,
    /// Digest of the complete exact request, including sensitive launch inputs.
    pub request_digest: CommandDigest,
    /// Named agent whose local runtime is controlled.
    pub agent_id: AgentId,
    /// Neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact requested lifecycle behavior.
    pub kind: HarnessSessionOperationKind,
    /// Current monotonic disposition.
    pub state: HarnessSessionOperationState,
}

/// Monotonic durable delivery state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessDeliveryState {
    /// Exact input is durable but no provider call has begun.
    Pending,
    /// Acceptance is unknown and reconciliation is mandatory.
    Uncertain,
    /// Exact input was authoritatively accepted.
    Accepted,
    /// Exact input was authoritatively rejected and must not be retried.
    Rejected,
}

/// Immutable project provenance captured before provider I/O.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessProjectDelivery {
    /// Target project captured when the input was dispatched.
    pub project_id: ProjectId,
    /// Stable canonical dispatch identity.
    pub dispatch_id: DispatchId,
    /// Assignment captured by the dispatch.
    pub assignment_id: AssignmentId,
    /// Immutable project conversation thread selected for the dispatch.
    pub thread_id: ThreadId,
    /// Authoritative positive project-input sequence.
    pub sequence: NonZeroU64,
}

/// Passive exact durable provider-delivery record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessDeliveryRecord {
    /// Named agent owning the delivery.
    pub agent_id: AgentId,
    /// Neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact durable provider session.
    pub session_id: ProviderSessionId,
    /// Complete exact neutral submission.
    pub submission: HarnessSubmission,
    /// Exact project provenance, absent only when an older writer did not retain it.
    pub project: Option<HarnessProjectDelivery>,
    /// Injected durable queue time.
    pub queued_at_millis: u64,
    /// Current monotonic state.
    pub state: HarnessDeliveryState,
}

/// Passive normalized persistence checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessEventCheckpoint {
    /// Named agent owning the event.
    pub agent_id: AgentId,
    /// Stable event identity.
    pub event_id: MessageId,
    /// Digest of the complete normalized event.
    pub digest: CommandDigest,
    /// Output is absent or has committed.
    pub output_complete: bool,
    /// Activity is absent or has committed.
    pub activity_complete: bool,
}

/// One exact atomic supervisor-state transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessStateMutation {
    /// Acquire or renew an absent, expired, or same-token lease.
    ClaimLease {
        /// Named agent being claimed.
        agent_id: AgentId,
        /// Proposed exact owner token.
        owner_token: HarnessOwnerToken,
        /// Injected current time.
        now_millis: u64,
        /// New deadline.
        expires_at_millis: u64,
    },
    /// Release only the exact cited owner token.
    ReleaseLease {
        /// Named agent being released.
        agent_id: AgentId,
        /// Exact token expected to own it.
        owner_token: HarnessOwnerToken,
    },
    /// Record acknowledged durable readiness under the exact live token.
    SetReadySession {
        /// Exact live token.
        owner_token: HarnessOwnerToken,
        /// Acknowledged ready session.
        ready: HarnessReadySession,
    },
    /// Insert one exact prepared managed-session operation idempotently.
    QueueSessionOperation(HarnessSessionOperation),
    /// Advance one existing managed-session operation monotonically.
    SetSessionOperationState {
        /// Stable external-operation identity.
        operation_id: hq_domain::OperationId,
        /// New monotonic disposition.
        state: HarnessSessionOperationState,
    },
    /// Queue one exact provider input idempotently.
    QueueDelivery(HarnessDeliveryRecord),
    /// Advance one delivery under the exact live token.
    SetDeliveryState {
        /// Named agent owning the delivery.
        agent_id: AgentId,
        /// Stable submission identity.
        submission_id: MessageId,
        /// Exact live token.
        owner_token: HarnessOwnerToken,
        /// New monotonic state.
        state: HarnessDeliveryState,
    },
    /// Insert or advance one event checkpoint under the exact live token.
    CheckpointEvent {
        /// Exact live token.
        owner_token: HarnessOwnerToken,
        /// Monotonic checkpoint.
        checkpoint: HarnessEventCheckpoint,
    },
}

/// Bounded deterministic state loaded for startup or missed-wake repair.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HarnessStateSnapshot {
    /// Lease rows in agent order.
    pub leases: Vec<HarnessWorkerLease>,
    /// Ready sessions in agent order.
    pub ready_sessions: Vec<HarnessReadySession>,
    /// Managed-session operations in stable identity order.
    pub session_operations: Vec<HarnessSessionOperation>,
    /// Deliveries in durable queue order.
    pub deliveries: Vec<HarnessDeliveryRecord>,
    /// Event checkpoints in stable identity order.
    pub events: Vec<HarnessEventCheckpoint>,
}

/// Consumer-owned durable operational state capability.
pub trait HarnessStatePort: Send + Sync {
    /// Applies one exact atomic mutation.
    fn apply(&self, mutation: HarnessStateMutation) -> Result<HarnessLeaseOutcome, HarnessError>;

    /// Loads one bounded deterministic repair snapshot.
    fn load(&self, limit: usize) -> Result<HarnessStateSnapshot, HarnessError>;

    /// Loads one exact managed-session control operation for response-loss replay.
    fn session_operation(
        &self,
        operation_id: hq_domain::OperationId,
    ) -> Result<Option<HarnessSessionOperation>, HarnessError>;

    /// Loads one exact durable delivery for idempotent client replay.
    fn delivery(
        &self,
        agent_id: AgentId,
        submission_id: MessageId,
    ) -> Result<Option<HarnessDeliveryRecord>, HarnessError>;

    /// Loads the unique durable delivery associated with one provider operation.
    fn delivery_for_operation(
        &self,
        agent_id: AgentId,
        operation_id: hq_domain::OperationId,
    ) -> Result<Option<HarnessDeliveryRecord>, HarnessError>;

    /// Loads one bounded durable runnable prefix for one exact agent.
    fn runnable_deliveries(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> Result<Vec<HarnessDeliveryRecord>, HarnessError>;
}

/// Consumer-owned canonical persistence capability for normalized values.
pub trait HarnessPersistencePort: Send + Sync {
    /// Idempotently persists one exact output or rejects an unequal stable identity.
    fn persist_output(
        &self,
        agent_id: AgentId,
        provider_id: &ProviderId,
        session_id: &ProviderSessionId,
        delivery: Option<&HarnessDeliveryRecord>,
        output: &HarnessOutput,
    ) -> Result<(), HarnessError>;

    /// Idempotently persists one exact activity or rejects an unequal stable identity.
    fn persist_activity(
        &self,
        agent_id: AgentId,
        provider_id: &ProviderId,
        session_id: &ProviderSessionId,
        delivery: Option<&HarnessDeliveryRecord>,
        activity: &HarnessActivity,
    ) -> Result<(), HarnessError>;
}

/// Injected time source for reproducible leases and queue order.
pub trait HarnessClock: Send + Sync {
    /// Returns current Unix milliseconds without ambient reads in state-machine tests.
    fn now_millis(&self) -> u64;
}

/// Injected stable owner-token source.
pub trait HarnessTokenSource: Send + Sync {
    /// Returns one fresh exact owner capability.
    fn next_token(&self) -> Result<HarnessOwnerToken, HarnessError>;
}

/// Bounded synchronous supervisor configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessSupervisorConfig {
    /// Maximum simultaneously owned named-agent workers.
    pub max_workers: usize,
    /// Maximum state rows loaded per repair pass.
    pub state_query_items: usize,
    /// Lease duration renewed at each worker reconciliation.
    pub lease_duration: Duration,
    /// Maximum normalized events pending per worker.
    pub event_capacity: NonZeroUsize,
    /// Maximum adapter drain wait during ordered shutdown.
    pub drain_wait: Duration,
    /// Delay between bounded component-owned event polling passes.
    pub event_poll_interval: Duration,
}

impl Default for HarnessSupervisorConfig {
    fn default() -> Self {
        Self {
            max_workers: 64,
            state_query_items: 256,
            lease_duration: Duration::from_secs(30),
            event_capacity: NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
            drain_wait: Duration::from_secs(2),
            event_poll_interval: Duration::from_millis(10),
        }
    }
}

/// Shared capabilities consumed by the sole supervisor owner.
#[derive(Clone)]
pub struct HarnessSupervisorDependencies {
    /// Immutable provider registry composed by the node.
    pub registry: Arc<HarnessRegistry>,
    /// Durable operational coordination state.
    pub state: Arc<dyn HarnessStatePort>,
    /// Canonical normalized persistence capability.
    pub persistence: Arc<dyn HarnessPersistencePort>,
    /// Injected lease and queue clock.
    pub clock: Arc<dyn HarnessClock>,
    /// Injected exact owner-token source.
    pub tokens: Arc<dyn HarnessTokenSource>,
}

/// Memory-only request to start or exactly resume one logical worker.
pub struct HarnessLaunchRequest {
    /// Named agent to own.
    pub agent_id: AgentId,
    /// Optional project binding.
    pub project_id: Option<ProjectId>,
    /// Optional validated launch directory passed without filesystem interpretation.
    pub launch_directory: Option<hq_domain::ResourceLocator>,
    /// Registered neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact start or resume request.
    pub session: HarnessSessionRequest,
    /// Copied memory-only launch environment.
    pub environment: HarnessEnvironment,
}

/// Definite or explicitly reconcilable managed-session control result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessSessionControlOutcome {
    /// The exact provider session is live and acknowledged.
    Ready(ProviderSessionId),
    /// The local runtime is authoritatively absent.
    Stopped,
    /// The request was authoritatively rejected.
    Rejected,
    /// Completion is unknown and exact operation replay is required.
    Uncertain,
}

impl fmt::Debug for HarnessLaunchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HarnessLaunchRequest")
            .field("agent_id", &self.agent_id)
            .field("project_id", &self.project_id)
            .field("launch_directory", &self.launch_directory)
            .field("provider_id", &self.provider_id)
            .field("session", &self.session)
            .field("environment", &self.environment)
            .finish()
    }
}

/// Bounded shutdown evidence for the complete owned worker set.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HarnessSupervisorReport {
    /// Workers whose complete lifetime was released.
    pub workers_released: usize,
    /// Workers requiring forced adapter termination after drain.
    pub workers_forced: usize,
    /// Stable failures retained up to the configured worker bound.
    pub failures: Vec<HarnessErrorClass>,
}

/// One bounded polling pass over every currently live exact worker.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HarnessEventPumpReport {
    /// Provider events accepted from source-ordered streams.
    pub events_polled: usize,
    /// Older replaceable snapshots removed before canonical persistence.
    pub snapshots_replaced: usize,
    /// Interactive requests retained for later exact response.
    pub interactive_requests: usize,
    /// Interactive requests terminally failed closed because no responder was active.
    pub interactive_requests_failed_closed: usize,
    /// Workers whose provider stream closed normally in this pass.
    pub workers_closed: usize,
    /// Workers whose provider poll failed in this pass.
    pub workers_failed: usize,
    /// Normalized persistence items still owned after this pass.
    pub pending_values: usize,
    /// Live workers remaining after this pass.
    pub live_workers: usize,
    /// Bounded stable failure classes observed without provider prose.
    pub failures: Vec<HarnessErrorClass>,
}

struct HarnessWorker {
    token: HarnessOwnerToken,
    project_id: Option<ProjectId>,
    provider_id: ProviderId,
    session_id: ProviderSessionId,
    session: Box<dyn HarnessSession>,
    events: HarnessEventBuffer,
    staged: Option<HarnessEvent>,
    requests: VecDeque<HarnessInteractiveRequest>,
}

/// Complete memory-only provider request with its exact live owner context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessPendingInteraction {
    /// Named agent awaiting the response.
    pub agent_id: AgentId,
    /// Optional project supplied when the worker launched.
    pub project_id: Option<ProjectId>,
    /// Neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact live provider session.
    pub session_id: ProviderSessionId,
    /// Source-ordered normalized request.
    pub request: HarnessInteractiveRequest,
}

/// Sole synchronous owner of named-agent provider workers and recovery checkpoints.
pub struct HarnessSupervisor {
    config: HarnessSupervisorConfig,
    dependencies: HarnessSupervisorDependencies,
    workers: Mutex<BTreeMap<AgentId, HarnessWorker>>,
    responders: Mutex<BTreeSet<HarnessResponderId>>,
    accepting: AtomicBool,
}

impl HarnessSupervisor {
    /// Constructs an empty supervisor after validating every explicit bound.
    pub fn new(
        config: HarnessSupervisorConfig,
        dependencies: HarnessSupervisorDependencies,
    ) -> Result<Self, HarnessError> {
        if config.max_workers == 0
            || config.max_workers > MAX_HARNESS_SUPERVISOR_STATE_ITEMS
            || config.state_query_items == 0
            || config.state_query_items > MAX_HARNESS_SUPERVISOR_STATE_ITEMS
            || config.event_capacity.get() > MAX_HARNESS_SUPERVISOR_STATE_ITEMS
            || config.lease_duration.is_zero()
            || config.drain_wait.is_zero()
            || config.event_poll_interval.is_zero()
        {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        Ok(Self {
            config,
            dependencies,
            workers: Mutex::new(BTreeMap::new()),
            responders: Mutex::new(BTreeSet::new()),
            accepting: AtomicBool::new(true),
        })
    }

    /// Performs or reconciles one exact durable managed-session operation.
    pub fn control_session(
        &self,
        operation: &HarnessSessionOperation,
        launch: Option<HarnessLaunchRequest>,
    ) -> Result<HarnessSessionControlOutcome, HarnessError> {
        validate_session_control(operation, launch.as_ref())?;
        self.dependencies
            .state
            .apply(HarnessStateMutation::QueueSessionOperation(
                operation.clone(),
            ))?;
        let retained = self
            .dependencies
            .state
            .session_operation(operation.operation_id)?
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        if !same_session_operation_identity(operation, &retained) {
            return Err(HarnessError::new(HarnessErrorClass::PersistenceCollision));
        }
        if let Some(outcome) = terminal_session_outcome(&retained.state) {
            return Ok(outcome);
        }
        if retained.state == HarnessSessionOperationState::Uncertain
            && let Some(outcome) = self.observe_uncertain_session_operation(&retained)?
        {
            return Ok(outcome);
        }
        if retained.state == HarnessSessionOperationState::Uncertain {
            return Ok(HarnessSessionControlOutcome::Uncertain);
        }
        self.set_session_operation_state(
            retained.operation_id,
            HarnessSessionOperationState::Uncertain,
        )?;
        match retained.kind {
            HarnessSessionOperationKind::Start | HarnessSessionOperationKind::Resume(_) => {
                let request =
                    launch.ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
                let result = if matches!(retained.kind, HarnessSessionOperationKind::Resume(_)) {
                    self.recover(request)
                } else {
                    self.launch(request)
                };
                match result {
                    Ok(session) => {
                        self.set_session_operation_state(
                            retained.operation_id,
                            HarnessSessionOperationState::Ready(session.clone()),
                        )?;
                        Ok(HarnessSessionControlOutcome::Ready(session))
                    }
                    Err(error) if definitive_session_rejection(error.class) => {
                        self.set_session_operation_state(
                            retained.operation_id,
                            HarnessSessionOperationState::Rejected,
                        )?;
                        Ok(HarnessSessionControlOutcome::Rejected)
                    }
                    Err(_) => Ok(HarnessSessionControlOutcome::Uncertain),
                }
            }
            HarnessSessionOperationKind::Stop => match self.stop(retained.agent_id) {
                Ok(report) if report.failures.is_empty() => {
                    self.set_session_operation_state(
                        retained.operation_id,
                        HarnessSessionOperationState::Stopped,
                    )?;
                    Ok(HarnessSessionControlOutcome::Stopped)
                }
                Err(error) if error.class == HarnessErrorClass::Unavailable => {
                    self.set_session_operation_state(
                        retained.operation_id,
                        HarnessSessionOperationState::Stopped,
                    )?;
                    Ok(HarnessSessionControlOutcome::Stopped)
                }
                Ok(_) | Err(_) => Ok(HarnessSessionControlOutcome::Uncertain),
            },
        }
    }

    fn observe_uncertain_session_operation(
        &self,
        operation: &HarnessSessionOperation,
    ) -> Result<Option<HarnessSessionControlOutcome>, HarnessError> {
        let workers = self.lock_workers()?;
        let worker = workers
            .get(&operation.agent_id)
            .map(|worker| (worker.provider_id.clone(), worker.session_id.clone()));
        drop(workers);
        match &operation.kind {
            HarnessSessionOperationKind::Start
                if worker
                    .as_ref()
                    .is_some_and(|(provider, _)| provider == &operation.provider_id) =>
            {
                let session = worker
                    .map(|(_, session)| session)
                    .ok_or_else(|| HarnessError::new(HarnessErrorClass::PersistenceCollision))?;
                self.set_session_operation_state(
                    operation.operation_id,
                    HarnessSessionOperationState::Ready(session.clone()),
                )?;
                Ok(Some(HarnessSessionControlOutcome::Ready(session)))
            }
            HarnessSessionOperationKind::Resume(expected)
                if worker.as_ref().is_some_and(|(provider, session)| {
                    provider == &operation.provider_id && session == expected
                }) =>
            {
                self.set_session_operation_state(
                    operation.operation_id,
                    HarnessSessionOperationState::Ready(expected.clone()),
                )?;
                Ok(Some(HarnessSessionControlOutcome::Ready(expected.clone())))
            }
            HarnessSessionOperationKind::Stop if worker.is_none() => {
                self.set_session_operation_state(
                    operation.operation_id,
                    HarnessSessionOperationState::Stopped,
                )?;
                Ok(Some(HarnessSessionControlOutcome::Stopped))
            }
            HarnessSessionOperationKind::Stop
                if worker
                    .as_ref()
                    .is_some_and(|(provider, _)| provider == &operation.provider_id) =>
            {
                let report = self.stop(operation.agent_id)?;
                if report.failures.is_empty() {
                    self.set_session_operation_state(
                        operation.operation_id,
                        HarnessSessionOperationState::Stopped,
                    )?;
                    Ok(Some(HarnessSessionControlOutcome::Stopped))
                } else {
                    Ok(Some(HarnessSessionControlOutcome::Uncertain))
                }
            }
            HarnessSessionOperationKind::Start
            | HarnessSessionOperationKind::Resume(_)
            | HarnessSessionOperationKind::Stop => Ok(None),
        }
    }

    fn set_session_operation_state(
        &self,
        operation_id: hq_domain::OperationId,
        state: HarnessSessionOperationState,
    ) -> Result<(), HarnessError> {
        self.dependencies
            .state
            .apply(HarnessStateMutation::SetSessionOperationState {
                operation_id,
                state,
            })
            .map(|_| ())
    }

    /// Claims and opens one exact logical worker, returning acknowledged readiness.
    pub fn launch(&self, request: HarnessLaunchRequest) -> Result<ProviderSessionId, HarnessError> {
        self.ensure_accepting()?;
        let mut workers = self.lock_workers()?;
        if workers.contains_key(&request.agent_id) || workers.len() == self.config.max_workers {
            return Err(HarnessError::new(HarnessErrorClass::OwnershipConflict));
        }
        let token = self.dependencies.tokens.next_token()?;
        let now = self.dependencies.clock.now_millis();
        let expires = lease_deadline(now, self.config.lease_duration)?;
        let outcome = self
            .dependencies
            .state
            .apply(HarnessStateMutation::ClaimLease {
                agent_id: request.agent_id,
                owner_token: token,
                now_millis: now,
                expires_at_millis: expires,
            })?;
        if outcome != HarnessLeaseOutcome::Acquired {
            return Err(HarnessError::new(HarnessErrorClass::OwnershipConflict));
        }
        let opened = self.dependencies.registry.open_session(
            &request.provider_id,
            HarnessInstanceRequest {
                agent_id: request.agent_id,
                project_id: request.project_id,
                launch_directory: request.launch_directory,
                environment: request.environment,
            },
            request.session,
        );
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => {
                let _ = self
                    .dependencies
                    .state
                    .apply(HarnessStateMutation::ReleaseLease {
                        agent_id: request.agent_id,
                        owner_token: token,
                    });
                return Err(error);
            }
        };
        self.dependencies
            .state
            .apply(HarnessStateMutation::SetReadySession {
                owner_token: token,
                ready: HarnessReadySession {
                    agent_id: request.agent_id,
                    provider_id: request.provider_id.clone(),
                    session_id: opened.session_id.clone(),
                },
            })?;
        let ready = opened.session_id.clone();
        workers.insert(
            request.agent_id,
            HarnessWorker {
                token,
                project_id: request.project_id,
                provider_id: request.provider_id,
                session_id: opened.session_id,
                session: opened.session,
                events: HarnessEventBuffer::new(self.config.event_capacity),
                staged: None,
                requests: VecDeque::with_capacity(self.config.event_capacity.get()),
            },
        );
        Ok(ready)
    }

    /// Exactly resumes one worker and immediately repairs its durable pending work.
    pub fn recover(
        &self,
        request: HarnessLaunchRequest,
    ) -> Result<ProviderSessionId, HarnessError> {
        if !matches!(&request.session, HarnessSessionRequest::Resume { .. }) {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        let agent_id = request.agent_id;
        let session_id = self.launch(request)?;
        if let Err(error) = self.wake_agent(agent_id) {
            let _ = self.stop(agent_id);
            return Err(error);
        }
        Ok(session_id)
    }

    /// Durably queues one exact input and reconciles it immediately when its worker is live.
    pub fn deliver(&self, mut delivery: HarnessDeliveryRecord) -> Result<(), HarnessError> {
        self.ensure_accepting()?;
        if delivery.state != HarnessDeliveryState::Pending {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        delivery.queued_at_millis = self.dependencies.clock.now_millis();
        self.dependencies
            .state
            .apply(HarnessStateMutation::QueueDelivery(delivery.clone()))?;
        let delivery = self
            .dependencies
            .state
            .delivery(delivery.agent_id, delivery.submission.submission_id)?
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        let mut workers = self.lock_workers()?;
        let worker = workers
            .get_mut(&delivery.agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        renew_worker(&self.config, &self.dependencies, delivery.agent_id, worker)?;
        reconcile_delivery(&self.dependencies, worker, &delivery)
    }

    /// Loads one exact durable delivery disposition without creating a second queue.
    pub fn delivery(
        &self,
        agent_id: AgentId,
        submission_id: MessageId,
    ) -> Result<Option<HarnessDeliveryRecord>, HarnessError> {
        self.dependencies.state.delivery(agent_id, submission_id)
    }

    /// Reconciles all bounded pending/uncertain work for currently live exact workers.
    pub fn wake(&self) -> Result<usize, HarnessError> {
        let agents: Vec<_> = self.lock_workers()?.keys().copied().collect();
        let mut reconciled = 0_usize;
        for agent_id in agents {
            reconciled = reconciled.saturating_add(self.wake_agent(agent_id)?);
        }
        Ok(reconciled)
    }

    fn wake_agent(&self, agent_id: AgentId) -> Result<usize, HarnessError> {
        let deliveries = self
            .dependencies
            .state
            .runnable_deliveries(agent_id, self.config.state_query_items)?;
        let mut workers = self.lock_workers()?;
        let worker = workers
            .get_mut(&agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        for delivery in &deliveries {
            renew_worker(&self.config, &self.dependencies, agent_id, worker)?;
            reconcile_delivery(&self.dependencies, worker, delivery)?;
        }
        Ok(deliveries.len())
    }

    /// Accepts normalized work into one worker's bounded FIFO/coalescing buffer and drains it.
    pub fn persist_event(
        &self,
        agent_id: AgentId,
        event: HarnessBufferedEvent,
    ) -> Result<(), HarnessError> {
        let mut workers = self.lock_workers()?;
        let worker = workers
            .get_mut(&agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        renew_worker(&self.config, &self.dependencies, agent_id, worker)?;
        worker
            .events
            .push(event)
            .map_err(|_| HarnessError::new(HarnessErrorClass::Backpressure))?;
        drain_events(&self.dependencies, agent_id, worker)
    }

    /// Retries every already accepted normalized item without admitting new work.
    pub fn flush(&self, agent_id: AgentId) -> Result<(), HarnessError> {
        let mut workers = self.lock_workers()?;
        let worker = workers
            .get_mut(&agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        renew_worker(&self.config, &self.dependencies, agent_id, worker)?;
        drain_events(&self.dependencies, agent_id, worker)
    }

    /// Polls every live worker once without blocking and retains every accepted value.
    pub fn poll_events(&self) -> Result<HarnessEventPumpReport, HarnessError> {
        let responders = self.lock_responders()?;
        let responder_available = !responders.is_empty();
        let mut workers = self.lock_workers()?;
        let agents: Vec<_> = workers.keys().copied().collect();
        let mut report = HarnessEventPumpReport::default();
        let mut terminal = Vec::new();
        for agent_id in agents {
            let worker = workers
                .get_mut(&agent_id)
                .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
            let pass = pump_worker(
                &self.config,
                &self.dependencies,
                agent_id,
                worker,
                responder_available,
            );
            report.events_polled = report.events_polled.saturating_add(pass.events_polled);
            report.snapshots_replaced = report
                .snapshots_replaced
                .saturating_add(pass.snapshots_replaced);
            report.interactive_requests = report
                .interactive_requests
                .saturating_add(pass.interactive_requests);
            report.interactive_requests_failed_closed = report
                .interactive_requests_failed_closed
                .saturating_add(pass.interactive_requests_failed_closed);
            for failure in pass.failures {
                retain_pump_failure(&mut report, failure, self.config.max_workers);
            }
            if let Some(outcome) = pass.terminal {
                terminal.push((agent_id, outcome));
            }
        }
        for (agent_id, outcome) in terminal {
            let Some(worker) = workers.remove(&agent_id) else {
                continue;
            };
            match outcome {
                WorkerTerminal::Closed => {
                    report.workers_closed = report.workers_closed.saturating_add(1);
                }
                WorkerTerminal::Failed(class) => {
                    report.workers_failed = report.workers_failed.saturating_add(1);
                    retain_pump_failure(&mut report, class, self.config.max_workers);
                }
            }
            let stopped = stop_worker(&self.config, &self.dependencies, agent_id, worker);
            for failure in stopped.failures {
                retain_pump_failure(&mut report, failure, self.config.max_workers);
            }
        }
        report.pending_values = workers
            .values()
            .map(|worker| {
                worker.events.len()
                    + usize::from(matches!(
                        worker.staged.as_ref(),
                        Some(HarnessEvent::Output(_) | HarnessEvent::Activity(_))
                    ))
            })
            .sum();
        report.live_workers = workers.len();
        drop(responders);
        Ok(report)
    }

    /// Activates one exact interactive responder capability idempotently.
    pub fn register_responder(
        &self,
        responder_id: HarnessResponderId,
    ) -> Result<bool, HarnessError> {
        self.ensure_accepting()?;
        Ok(self.lock_responders()?.insert(responder_id))
    }

    /// Removes one responder and fails closed all retained requests when it was the last.
    pub fn unregister_responder(
        &self,
        responder_id: HarnessResponderId,
    ) -> Result<usize, HarnessError> {
        let mut responders = self.lock_responders()?;
        if !responders.remove(&responder_id) || !responders.is_empty() {
            return Ok(0);
        }
        let mut failed_closed = 0usize;
        for worker in self.lock_workers()?.values_mut() {
            failed_closed = failed_closed.saturating_add(fail_closed_retained_requests(worker)?);
        }
        Ok(failed_closed)
    }

    /// Drains provider streams after intake closure for at most one explicit bounded wait.
    pub fn drain_event_streams(
        &self,
        wait: Duration,
    ) -> Result<HarnessEventPumpReport, HarnessError> {
        if wait.is_zero() {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        let deadline = Instant::now()
            .checked_add(wait)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
        let mut aggregate = HarnessEventPumpReport::default();
        loop {
            let pass = self.poll_events()?;
            merge_pump_report(&mut aggregate, pass, self.config.max_workers);
            if aggregate.live_workers == 0 || Instant::now() >= deadline {
                return Ok(aggregate);
            }
            std::thread::sleep(
                self.config
                    .event_poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    /// Loads one bounded source-ordered view of retained interactive requests.
    pub fn interactive_requests(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> Result<Vec<HarnessInteractiveRequest>, HarnessError> {
        if limit == 0 || limit > MAX_HARNESS_SUPERVISOR_STATE_ITEMS {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        let workers = self.lock_workers()?;
        let worker = workers
            .get(&agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        Ok(worker.requests.iter().take(limit).cloned().collect())
    }

    /// Loads one bounded stable view of pending requests across every live worker.
    pub fn pending_interactions(
        &self,
        limit: usize,
    ) -> Result<Vec<HarnessPendingInteraction>, HarnessError> {
        if limit == 0 || limit > MAX_HARNESS_SUPERVISOR_STATE_ITEMS {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        let workers = self.lock_workers()?;
        Ok(workers
            .iter()
            .flat_map(|(agent_id, worker)| {
                worker
                    .requests
                    .iter()
                    .map(move |request| HarnessPendingInteraction {
                        agent_id: *agent_id,
                        project_id: worker.project_id,
                        provider_id: worker.provider_id.clone(),
                        session_id: worker.session_id.clone(),
                        request: request.clone(),
                    })
            })
            .take(limit)
            .collect())
    }

    /// Answers one structured request through its sole live session owner.
    pub fn answer(
        &self,
        agent_id: AgentId,
        answer: HarnessInteractiveAnswer,
    ) -> Result<(), HarnessError> {
        self.ensure_accepting()?;
        let mut workers = self.lock_workers()?;
        let worker = workers
            .get_mut(&agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        renew_worker(&self.config, &self.dependencies, agent_id, worker)?;
        let index = worker
            .requests
            .iter()
            .position(|request| request.request_id == answer.request_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
        worker.session.answer_interactive(answer)?;
        let _ = worker.requests.remove(index);
        Ok(())
    }

    /// Cancels one exact operation through its sole live session owner.
    pub fn cancel(
        &self,
        agent_id: AgentId,
        operation_id: hq_domain::OperationId,
    ) -> Result<HarnessCancellationOutcome, HarnessError> {
        self.ensure_accepting()?;
        let mut workers = self.lock_workers()?;
        let worker = workers
            .get_mut(&agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        renew_worker(&self.config, &self.dependencies, agent_id, worker)?;
        worker.session.cancel_operation(operation_id)
    }

    /// Closes new launch, delivery, answer, and cancellation intake for every worker.
    pub fn stop_intake(&self) -> Result<(), HarnessError> {
        self.accepting.store(false, Ordering::Release);
        let mut first_error = None;
        for worker in self.lock_workers()?.values_mut() {
            if let Err(error) = worker.session.stop_intake()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Stops and releases one exact worker without affecting siblings.
    pub fn stop(&self, agent_id: AgentId) -> Result<HarnessSupervisorReport, HarnessError> {
        let worker = self
            .lock_workers()?
            .remove(&agent_id)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        Ok(stop_worker(
            &self.config,
            &self.dependencies,
            agent_id,
            worker,
        ))
    }

    /// Stops intake, drains, force-stops when necessary, and releases every exact owner.
    pub fn shutdown(&self) -> Result<HarnessSupervisorReport, HarnessError> {
        let intake_failure = self.stop_intake().err();
        let workers = std::mem::take(&mut *self.lock_workers()?);
        let mut report = HarnessSupervisorReport::default();
        if let Some(error) = intake_failure {
            report.failures.push(error.class);
        }
        for (agent_id, worker) in workers {
            merge_report(
                &mut report,
                stop_worker(&self.config, &self.dependencies, agent_id, worker),
                self.config.max_workers,
            );
        }
        Ok(report)
    }

    fn lock_workers(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<AgentId, HarnessWorker>>, HarnessError> {
        self.workers
            .lock()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))
    }

    fn lock_responders(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeSet<HarnessResponderId>>, HarnessError> {
        self.responders
            .lock()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))
    }

    fn ensure_accepting(&self) -> Result<(), HarnessError> {
        if self.accepting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(HarnessError::new(HarnessErrorClass::IntakeClosed))
        }
    }
}

#[derive(Clone, Copy)]
enum WorkerTerminal {
    Closed,
    Failed(HarnessErrorClass),
}

#[derive(Default)]
struct WorkerPump {
    events_polled: usize,
    snapshots_replaced: usize,
    interactive_requests: usize,
    interactive_requests_failed_closed: usize,
    failures: Vec<HarnessErrorClass>,
    terminal: Option<WorkerTerminal>,
}

fn pump_worker(
    config: &HarnessSupervisorConfig,
    dependencies: &HarnessSupervisorDependencies,
    agent_id: AgentId,
    worker: &mut HarnessWorker,
    responder_available: bool,
) -> WorkerPump {
    let mut report = WorkerPump::default();
    if let Err(error) = renew_worker(config, dependencies, agent_id, worker) {
        report.terminal = Some(WorkerTerminal::Failed(error.class));
        return report;
    }
    if let Err(error) = drain_events(dependencies, agent_id, worker) {
        report.failures.push(error.class);
    }
    if let Some(staged) = worker.staged.take() {
        match admit_polled_event(config, worker, staged, responder_available) {
            Ok(admission) => account_admission(&mut report, admission),
            Err(failure) => {
                account_failed_closed(&mut report, failure.failed_closed);
                worker.staged = Some(*failure.event);
                return report;
            }
        }
        if let Err(error) = drain_events(dependencies, agent_id, worker) {
            report.failures.push(error.class);
        }
    }
    match worker.session.poll_event(Duration::ZERO) {
        Ok(HarnessEventPoll::Event(event)) => {
            report.events_polled = 1;
            match admit_polled_event(config, worker, event, responder_available) {
                Ok(admission) => account_admission(&mut report, admission),
                Err(failure) => {
                    account_failed_closed(&mut report, failure.failed_closed);
                    worker.staged = Some(*failure.event);
                }
            }
            if let Err(error) = drain_events(dependencies, agent_id, worker) {
                report.failures.push(error.class);
            }
        }
        Ok(HarnessEventPoll::TimedOut) => {}
        Ok(HarnessEventPoll::Closed) => report.terminal = Some(WorkerTerminal::Closed),
        Err(error) => report.terminal = Some(WorkerTerminal::Failed(error.class)),
    }
    report
}

#[derive(Clone, Copy)]
enum EventAdmission {
    Value {
        push: HarnessBufferPush,
        failed_closed: usize,
    },
    Interactive,
    DuplicateInteractive,
    InteractiveFailedClosed,
}

struct EventAdmissionFailure {
    event: Box<HarnessEvent>,
    failed_closed: usize,
}

fn admit_polled_event(
    config: &HarnessSupervisorConfig,
    worker: &mut HarnessWorker,
    event: HarnessEvent,
    responder_available: bool,
) -> Result<EventAdmission, EventAdmissionFailure> {
    match event {
        HarnessEvent::Output(output) => {
            let failed_closed = if output.kind == HarnessOutputKind::FinalAnswer {
                fail_closed_operation_requests(worker, output.operation_id).map_err(|_| {
                    EventAdmissionFailure {
                        event: Box::new(HarnessEvent::Output(output.clone())),
                        failed_closed: 0,
                    }
                })?
            } else {
                0
            };
            let buffered = buffered_output(output.clone());
            worker
                .events
                .push(buffered)
                .map(|push| EventAdmission::Value {
                    push,
                    failed_closed,
                })
                .map_err(|_| EventAdmissionFailure {
                    event: Box::new(HarnessEvent::Output(output)),
                    failed_closed,
                })
        }
        HarnessEvent::Activity(activity) => {
            let failed_closed = if activity.kind == ActivityKind::AgentTurn
                && matches!(
                    activity.status,
                    ActivityStatus::Succeeded
                        | ActivityStatus::Failed(_)
                        | ActivityStatus::Interrupted
                ) {
                fail_closed_operation_requests(worker, activity.operation_id).map_err(|_| {
                    EventAdmissionFailure {
                        event: Box::new(HarnessEvent::Activity(activity.clone())),
                        failed_closed: 0,
                    }
                })?
            } else {
                0
            };
            let buffered = buffered_activity(activity.clone());
            worker
                .events
                .push(buffered)
                .map(|push| EventAdmission::Value {
                    push,
                    failed_closed,
                })
                .map_err(|_| EventAdmissionFailure {
                    event: Box::new(HarnessEvent::Activity(activity)),
                    failed_closed,
                })
        }
        HarnessEvent::InteractiveRequest(request) => {
            if !responder_available {
                let answer = HarnessInteractiveAnswer {
                    request_id: request.request_id,
                    response: crate::HarnessInteractiveResponse::Cancelled,
                };
                return match worker.session.answer_interactive(answer) {
                    Ok(()) => Ok(EventAdmission::InteractiveFailedClosed),
                    Err(error) if error.class == HarnessErrorClass::InteractiveAlreadyAnswered => {
                        Ok(EventAdmission::InteractiveFailedClosed)
                    }
                    Err(_) => Err(EventAdmissionFailure {
                        event: Box::new(HarnessEvent::InteractiveRequest(request)),
                        failed_closed: 0,
                    }),
                };
            }
            if let Some(prior) = worker
                .requests
                .iter()
                .find(|prior| prior.request_id == request.request_id)
            {
                return if prior == &request {
                    Ok(EventAdmission::DuplicateInteractive)
                } else {
                    Err(EventAdmissionFailure {
                        event: Box::new(HarnessEvent::InteractiveRequest(request)),
                        failed_closed: 0,
                    })
                };
            }
            if worker.requests.len() == config.event_capacity.get() {
                return Err(EventAdmissionFailure {
                    event: Box::new(HarnessEvent::InteractiveRequest(request)),
                    failed_closed: 0,
                });
            }
            worker.requests.push_back(request);
            Ok(EventAdmission::Interactive)
        }
    }
}

fn account_admission(report: &mut WorkerPump, admission: EventAdmission) {
    match admission {
        EventAdmission::Value {
            push,
            failed_closed,
        } => {
            account_failed_closed(report, failed_closed);
            if push == HarnessBufferPush::Replaced {
                report.snapshots_replaced = report.snapshots_replaced.saturating_add(1);
            }
        }
        EventAdmission::DuplicateInteractive => {}
        EventAdmission::Interactive => {
            report.interactive_requests = report.interactive_requests.saturating_add(1);
        }
        EventAdmission::InteractiveFailedClosed => {
            report.interactive_requests_failed_closed =
                report.interactive_requests_failed_closed.saturating_add(1);
        }
    }
}

fn account_failed_closed(report: &mut WorkerPump, count: usize) {
    report.interactive_requests_failed_closed = report
        .interactive_requests_failed_closed
        .saturating_add(count);
}

fn buffered_output(output: HarnessOutput) -> HarnessBufferedEvent {
    let mut digest = Sha256::new();
    digest.update(b"hq-harness-buffered-output-v1\0");
    digest.update(output.output_id.as_bytes());
    digest.update(output.operation_id.as_bytes());
    digest.update([match output.kind {
        HarnessOutputKind::Update => 1,
        HarnessOutputKind::FinalAnswer => 2,
    }]);
    update_activity_status(&mut digest, &output.status);
    update_digest_text(&mut digest, output.body.as_str());
    HarnessBufferedEvent::Output {
        event_id: output.output_id,
        digest: CommandDigest::from_bytes(digest.finalize().into()),
        output,
    }
}

fn buffered_activity(activity: HarnessActivity) -> HarnessBufferedEvent {
    let mut identity = Sha256::new();
    identity.update(b"hq-harness-buffered-activity-id-v1\0");
    identity.update(activity.operation_id.as_bytes());
    update_optional_digest_text(
        &mut identity,
        activity.item.as_ref().map(hq_domain::BoundedText::as_str),
    );
    identity.update([activity_kind_code(activity.kind)]);
    update_digest_text(&mut identity, activity.logical_key.as_str());
    update_digest_text(&mut identity, activity.runtime.as_str());
    identity.update(activity.sequence.get().to_be_bytes());
    let event_id = MessageId::from_bytes(identity.finalize().into());

    let mut digest = Sha256::new();
    digest.update(b"hq-harness-buffered-activity-value-v1\0");
    digest.update(event_id.as_bytes());
    update_activity_status(&mut digest, &activity.status);
    update_digest_text(&mut digest, activity.content.as_str());
    digest.update([u8::from(activity.truncated)]);
    update_completed_digest(&mut digest, activity.completed.as_deref());
    let digest = CommandDigest::from_bytes(digest.finalize().into());
    if activity.status == ActivityStatus::Snapshot {
        HarnessBufferedEvent::Snapshot {
            event_id,
            digest,
            key: HarnessSnapshotKey {
                operation_id: activity.operation_id,
                logical_key: activity.logical_key.clone(),
            },
            activity,
        }
    } else {
        HarnessBufferedEvent::Activity {
            event_id,
            digest,
            activity,
        }
    }
}

fn update_completed_digest(
    digest: &mut Sha256,
    completed: Option<&hq_domain::CompletedItemPresentation>,
) {
    digest.update([u8::from(completed.is_some())]);
    if let Some(completed) = completed {
        let encoded = completed.canonical_digest_bytes();
        digest.update(encoded.len().to_be_bytes());
        digest.update(encoded);
    }
}

fn update_digest_text(digest: &mut Sha256, value: &str) {
    digest.update(value.len().to_be_bytes());
    digest.update(value.as_bytes());
}

fn update_optional_digest_text(digest: &mut Sha256, value: Option<&str>) {
    digest.update([u8::from(value.is_some())]);
    if let Some(value) = value {
        update_digest_text(digest, value);
    }
}

fn update_activity_status(digest: &mut Sha256, status: &ActivityStatus) {
    match status {
        ActivityStatus::Snapshot => digest.update([1]),
        ActivityStatus::Running => digest.update([2]),
        ActivityStatus::Succeeded => digest.update([3]),
        ActivityStatus::Failed(code) => {
            digest.update([4]);
            update_digest_text(digest, code.as_str());
        }
        ActivityStatus::Interrupted => digest.update([5]),
    }
}

const fn activity_kind_code(kind: ActivityKind) -> u8 {
    match kind {
        ActivityKind::Status => 1,
        ActivityKind::AgentTurn => 6,
        ActivityKind::Progress => 2,
        ActivityKind::Plan => 3,
        ActivityKind::Diff => 4,
        ActivityKind::CompletedItem => 5,
    }
}

fn reconcile_delivery(
    dependencies: &HarnessSupervisorDependencies,
    worker: &mut HarnessWorker,
    delivery: &HarnessDeliveryRecord,
) -> Result<(), HarnessError> {
    if delivery.provider_id != worker.provider_id || delivery.session_id != worker.session_id {
        return Err(HarnessError::new(
            HarnessErrorClass::SessionIdentityMismatch,
        ));
    }
    if delivery.state == HarnessDeliveryState::Uncertain {
        match worker.session.lookup_submission(&delivery.submission)? {
            HarnessSubmissionLookup::Accepted => {
                return set_delivery_state(
                    dependencies,
                    delivery,
                    worker.token,
                    HarnessDeliveryState::Accepted,
                );
            }
            HarnessSubmissionLookup::Missing => {}
        }
    } else if delivery.state == HarnessDeliveryState::Pending {
        set_delivery_state(
            dependencies,
            delivery,
            worker.token,
            HarnessDeliveryState::Uncertain,
        )?;
    } else {
        return Ok(());
    }
    match worker.session.submit(delivery.submission.clone())? {
        HarnessSubmissionOutcome::Accepted => set_delivery_state(
            dependencies,
            delivery,
            worker.token,
            HarnessDeliveryState::Accepted,
        ),
        HarnessSubmissionOutcome::Uncertain(_) => Ok(()),
        HarnessSubmissionOutcome::Rejected(class) => {
            set_delivery_state(
                dependencies,
                delivery,
                worker.token,
                HarnessDeliveryState::Rejected,
            )?;
            Err(HarnessError::new(class))
        }
    }
}

fn validate_session_control(
    operation: &HarnessSessionOperation,
    launch: Option<&HarnessLaunchRequest>,
) -> Result<(), HarnessError> {
    match (&operation.kind, launch) {
        (HarnessSessionOperationKind::Stop, None) => Ok(()),
        (HarnessSessionOperationKind::Start, Some(launch))
            if launch.agent_id == operation.agent_id
                && launch.provider_id == operation.provider_id
                && launch.session == HarnessSessionRequest::Start =>
        {
            Ok(())
        }
        (HarnessSessionOperationKind::Resume(expected), Some(launch))
            if launch.agent_id == operation.agent_id
                && launch.provider_id == operation.provider_id
                && launch.session
                    == (HarnessSessionRequest::Resume {
                        session_id: expected.clone(),
                    }) =>
        {
            Ok(())
        }
        (
            HarnessSessionOperationKind::Start
            | HarnessSessionOperationKind::Resume(_)
            | HarnessSessionOperationKind::Stop,
            _,
        ) => Err(HarnessError::new(HarnessErrorClass::InvalidInput)),
    }
}

fn same_session_operation_identity(
    proposed: &HarnessSessionOperation,
    retained: &HarnessSessionOperation,
) -> bool {
    proposed.operation_id == retained.operation_id
        && proposed.request_digest == retained.request_digest
        && proposed.agent_id == retained.agent_id
        && proposed.provider_id == retained.provider_id
        && proposed.kind == retained.kind
}

fn terminal_session_outcome(
    state: &HarnessSessionOperationState,
) -> Option<HarnessSessionControlOutcome> {
    match state {
        HarnessSessionOperationState::Prepared | HarnessSessionOperationState::Uncertain => None,
        HarnessSessionOperationState::Ready(session) => {
            Some(HarnessSessionControlOutcome::Ready(session.clone()))
        }
        HarnessSessionOperationState::Stopped => Some(HarnessSessionControlOutcome::Stopped),
        HarnessSessionOperationState::Rejected => Some(HarnessSessionControlOutcome::Rejected),
    }
}

const fn definitive_session_rejection(class: HarnessErrorClass) -> bool {
    matches!(
        class,
        HarnessErrorClass::InvalidInput
            | HarnessErrorClass::Unsupported
            | HarnessErrorClass::ProviderNotRegistered
            | HarnessErrorClass::RegistrationConflict
            | HarnessErrorClass::UnsafeRecovery
            | HarnessErrorClass::SessionIdentityMismatch
            | HarnessErrorClass::SessionNotFound
            | HarnessErrorClass::SecretInputRejected
            | HarnessErrorClass::IntakeClosed
            | HarnessErrorClass::OwnershipConflict
    )
}

fn renew_worker(
    config: &HarnessSupervisorConfig,
    dependencies: &HarnessSupervisorDependencies,
    agent_id: AgentId,
    worker: &HarnessWorker,
) -> Result<(), HarnessError> {
    let now = dependencies.clock.now_millis();
    let expires = lease_deadline(now, config.lease_duration)?;
    match dependencies.state.apply(HarnessStateMutation::ClaimLease {
        agent_id,
        owner_token: worker.token,
        now_millis: now,
        expires_at_millis: expires,
    })? {
        HarnessLeaseOutcome::Acquired => Ok(()),
        HarnessLeaseOutcome::Held | HarnessLeaseOutcome::Released => {
            Err(HarnessError::new(HarnessErrorClass::OwnershipConflict))
        }
    }
}

fn set_delivery_state(
    dependencies: &HarnessSupervisorDependencies,
    delivery: &HarnessDeliveryRecord,
    token: HarnessOwnerToken,
    state: HarnessDeliveryState,
) -> Result<(), HarnessError> {
    dependencies
        .state
        .apply(HarnessStateMutation::SetDeliveryState {
            agent_id: delivery.agent_id,
            submission_id: delivery.submission.submission_id,
            owner_token: token,
            state,
        })
        .map(|_| ())
}

fn drain_events(
    dependencies: &HarnessSupervisorDependencies,
    agent_id: AgentId,
    worker: &mut HarnessWorker,
) -> Result<(), HarnessError> {
    while let Some(event) = worker.events.front().cloned() {
        persist_one(dependencies, agent_id, worker, &event)?;
        let _ = worker.events.pop();
    }
    Ok(())
}

fn drain_owned_values(
    config: &HarnessSupervisorConfig,
    dependencies: &HarnessSupervisorDependencies,
    agent_id: AgentId,
    worker: &mut HarnessWorker,
) -> Result<(), HarnessError> {
    drain_events(dependencies, agent_id, worker)?;
    let Some(staged) = worker.staged.take() else {
        return Ok(());
    };
    if let Err(failure) = admit_polled_event(config, worker, staged, true) {
        worker.staged = Some(*failure.event);
        return Err(HarnessError::new(HarnessErrorClass::Backpressure));
    }
    drain_events(dependencies, agent_id, worker)
}

fn persist_one(
    dependencies: &HarnessSupervisorDependencies,
    agent_id: AgentId,
    worker: &HarnessWorker,
    event: &HarnessBufferedEvent,
) -> Result<(), HarnessError> {
    let (event_id, digest, output, activity) = match event {
        HarnessBufferedEvent::Output {
            event_id,
            digest,
            output,
        } => (*event_id, *digest, Some(output), None),
        HarnessBufferedEvent::Activity {
            event_id,
            digest,
            activity,
        }
        | HarnessBufferedEvent::Snapshot {
            event_id,
            digest,
            activity,
            ..
        } => (*event_id, *digest, None, Some(activity)),
        HarnessBufferedEvent::OutputAndActivity {
            event_id,
            digest,
            output,
            activity,
        } => (*event_id, *digest, Some(output), Some(activity)),
    };
    if output
        .zip(activity)
        .is_some_and(|(output, activity)| output.operation_id != activity.operation_id)
    {
        return Err(HarnessError::new(HarnessErrorClass::PersistenceCollision));
    }
    let operation_id = output
        .map(|value| value.operation_id)
        .or_else(|| activity.map(|value| value.operation_id))
        .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
    let delivery = attributed_delivery(dependencies, agent_id, worker, operation_id)?;
    if let Some(output) = output {
        dependencies.persistence.persist_output(
            agent_id,
            &worker.provider_id,
            &worker.session_id,
            delivery.as_ref(),
            output,
        )?;
        checkpoint_event(
            dependencies,
            worker.token,
            HarnessEventCheckpoint {
                agent_id,
                event_id,
                digest,
                output_complete: true,
                activity_complete: activity.is_none(),
            },
        )?;
    }
    if let Some(activity) = activity {
        dependencies.persistence.persist_activity(
            agent_id,
            &worker.provider_id,
            &worker.session_id,
            delivery.as_ref(),
            activity,
        )?;
        checkpoint_event(
            dependencies,
            worker.token,
            HarnessEventCheckpoint {
                agent_id,
                event_id,
                digest,
                output_complete: true,
                activity_complete: true,
            },
        )?;
    }
    Ok(())
}

fn attributed_delivery(
    dependencies: &HarnessSupervisorDependencies,
    agent_id: AgentId,
    worker: &HarnessWorker,
    operation_id: hq_domain::OperationId,
) -> Result<Option<HarnessDeliveryRecord>, HarnessError> {
    let Some(delivery) = dependencies
        .state
        .delivery_for_operation(agent_id, operation_id)?
    else {
        return Ok(None);
    };
    if delivery.agent_id != agent_id
        || delivery.provider_id != worker.provider_id
        || delivery.session_id != worker.session_id
        || delivery.submission.operation_id != operation_id
        || delivery.project.is_none()
    {
        return Err(HarnessError::new(HarnessErrorClass::PersistenceCollision));
    }
    Ok(Some(delivery))
}

fn checkpoint_event(
    dependencies: &HarnessSupervisorDependencies,
    token: HarnessOwnerToken,
    checkpoint: HarnessEventCheckpoint,
) -> Result<(), HarnessError> {
    dependencies
        .state
        .apply(HarnessStateMutation::CheckpointEvent {
            owner_token: token,
            checkpoint,
        })
        .map(|_| ())
}

fn stop_worker(
    config: &HarnessSupervisorConfig,
    dependencies: &HarnessSupervisorDependencies,
    agent_id: AgentId,
    mut worker: HarnessWorker,
) -> HarnessSupervisorReport {
    let mut report = HarnessSupervisorReport::default();
    if let Err(error) = worker.session.stop_intake() {
        retain_failure(&mut report, error.class, config.max_workers);
    }
    if let Err(error) = drain_owned_values(config, dependencies, agent_id, &mut worker) {
        retain_failure(&mut report, error.class, config.max_workers);
    }
    let needs_force = match worker.session.drain(config.drain_wait) {
        Ok(HarnessDrainOutcome::Complete) => false,
        Ok(HarnessDrainOutcome::Pending { .. }) | Err(_) => true,
    };
    if needs_force {
        report.workers_forced = 1;
    }
    if let Err(error) = worker.session.force_stop() {
        retain_failure(&mut report, error.class, config.max_workers);
    }
    match dependencies
        .state
        .apply(HarnessStateMutation::ReleaseLease {
            agent_id,
            owner_token: worker.token,
        }) {
        Ok(HarnessLeaseOutcome::Released) => report.workers_released = 1,
        Ok(HarnessLeaseOutcome::Acquired | HarnessLeaseOutcome::Held) => retain_failure(
            &mut report,
            HarnessErrorClass::OwnershipConflict,
            config.max_workers,
        ),
        Err(error) => retain_failure(&mut report, error.class, config.max_workers),
    }
    report
}

fn merge_report(
    target: &mut HarnessSupervisorReport,
    source: HarnessSupervisorReport,
    limit: usize,
) {
    target.workers_released = target
        .workers_released
        .saturating_add(source.workers_released);
    target.workers_forced = target.workers_forced.saturating_add(source.workers_forced);
    for failure in source.failures {
        retain_failure(target, failure, limit);
    }
}

fn merge_pump_report(
    target: &mut HarnessEventPumpReport,
    source: HarnessEventPumpReport,
    limit: usize,
) {
    target.events_polled = target.events_polled.saturating_add(source.events_polled);
    target.snapshots_replaced = target
        .snapshots_replaced
        .saturating_add(source.snapshots_replaced);
    target.interactive_requests = target
        .interactive_requests
        .saturating_add(source.interactive_requests);
    target.interactive_requests_failed_closed = target
        .interactive_requests_failed_closed
        .saturating_add(source.interactive_requests_failed_closed);
    target.workers_closed = target.workers_closed.saturating_add(source.workers_closed);
    target.workers_failed = target.workers_failed.saturating_add(source.workers_failed);
    target.pending_values = source.pending_values;
    target.live_workers = source.live_workers;
    for failure in source.failures {
        retain_pump_failure(target, failure, limit);
    }
}

fn fail_closed_retained_requests(worker: &mut HarnessWorker) -> Result<usize, HarnessError> {
    let mut terminal = 0usize;
    while let Some(request) = worker.requests.front().cloned() {
        let answer = HarnessInteractiveAnswer {
            request_id: request.request_id,
            response: crate::HarnessInteractiveResponse::Cancelled,
        };
        match worker.session.answer_interactive(answer) {
            Ok(()) => {}
            Err(error) if error.class == HarnessErrorClass::InteractiveAlreadyAnswered => {}
            Err(error) => return Err(error),
        }
        let _ = worker.requests.pop_front();
        terminal = terminal.saturating_add(1);
    }
    Ok(terminal)
}

fn fail_closed_operation_requests(
    worker: &mut HarnessWorker,
    operation_id: OperationId,
) -> Result<usize, HarnessError> {
    let mut terminal = 0usize;
    let mut index = 0usize;
    while let Some(request) = worker.requests.get(index).cloned() {
        if request.operation_id != operation_id {
            index = index.saturating_add(1);
            continue;
        }
        let answer = HarnessInteractiveAnswer {
            request_id: request.request_id,
            response: crate::HarnessInteractiveResponse::Cancelled,
        };
        match worker.session.answer_interactive(answer) {
            Ok(()) => {}
            Err(error) if error.class == HarnessErrorClass::InteractiveAlreadyAnswered => {}
            Err(error) => return Err(error),
        }
        let _ = worker.requests.remove(index);
        terminal = terminal.saturating_add(1);
    }
    Ok(terminal)
}

fn retain_pump_failure(
    report: &mut HarnessEventPumpReport,
    failure: HarnessErrorClass,
    limit: usize,
) {
    if report.failures.len() < limit {
        report.failures.push(failure);
    }
}

fn retain_failure(report: &mut HarnessSupervisorReport, failure: HarnessErrorClass, limit: usize) {
    if report.failures.len() < limit {
        report.failures.push(failure);
    }
}

fn lease_deadline(now: u64, duration: Duration) -> Result<u64, HarnessError> {
    let millis = u64::try_from(duration.as_millis())
        .map_err(|_| HarnessError::new(HarnessErrorClass::InvalidInput))?;
    now.checked_add(millis)
        .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))
}
