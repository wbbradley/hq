//! Exact-owner managed-runtime supervision over consumer-owned durable and persistence ports.

use std::{
    collections::BTreeMap,
    fmt,
    num::NonZeroUsize,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};

use hq_domain::{AgentId, CommandDigest, MessageId, ProjectId, ProviderId, ProviderSessionId};

use crate::{
    HarnessActivity, HarnessBufferedEvent, HarnessCancellationOutcome, HarnessDrainOutcome,
    HarnessEnvironment, HarnessError, HarnessErrorClass, HarnessEventBuffer,
    HarnessInstanceRequest, HarnessInteractiveAnswer, HarnessOutput, HarnessRegistry,
    HarnessSession, HarnessSessionRequest, HarnessSubmission, HarnessSubmissionLookup,
    HarnessSubmissionOutcome,
};

/// Maximum state rows inspected by one supervisor repair pass.
pub const MAX_HARNESS_SUPERVISOR_STATE_ITEMS: usize = 1_024;

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

    /// Loads one exact durable delivery for idempotent client replay.
    fn delivery(
        &self,
        agent_id: AgentId,
        submission_id: MessageId,
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
        output: &HarnessOutput,
    ) -> Result<(), HarnessError>;

    /// Idempotently persists one exact activity or rejects an unequal stable identity.
    fn persist_activity(
        &self,
        agent_id: AgentId,
        provider_id: &ProviderId,
        session_id: &ProviderSessionId,
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
}

impl Default for HarnessSupervisorConfig {
    fn default() -> Self {
        Self {
            max_workers: 64,
            state_query_items: 256,
            lease_duration: Duration::from_secs(30),
            event_capacity: NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
            drain_wait: Duration::from_secs(2),
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
    /// Registered neutral provider namespace.
    pub provider_id: ProviderId,
    /// Exact start or resume request.
    pub session: HarnessSessionRequest,
    /// Copied memory-only launch environment.
    pub environment: HarnessEnvironment,
}

impl fmt::Debug for HarnessLaunchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HarnessLaunchRequest")
            .field("agent_id", &self.agent_id)
            .field("project_id", &self.project_id)
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

struct HarnessWorker {
    token: HarnessOwnerToken,
    provider_id: ProviderId,
    session_id: ProviderSessionId,
    session: Box<dyn HarnessSession>,
    events: HarnessEventBuffer,
}

/// Sole synchronous owner of named-agent provider workers and recovery checkpoints.
pub struct HarnessSupervisor {
    config: HarnessSupervisorConfig,
    dependencies: HarnessSupervisorDependencies,
    workers: Mutex<BTreeMap<AgentId, HarnessWorker>>,
    accepting: AtomicBool,
}

impl HarnessSupervisor {
    /// Constructs an empty supervisor after validating every explicit bound.
    pub fn new(
        config: HarnessSupervisorConfig,
        dependencies: HarnessSupervisorDependencies,
    ) -> Result<Self, HarnessError> {
        if config.max_workers == 0
            || config.state_query_items == 0
            || config.state_query_items > MAX_HARNESS_SUPERVISOR_STATE_ITEMS
            || config.lease_duration.is_zero()
            || config.drain_wait.is_zero()
        {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        Ok(Self {
            config,
            dependencies,
            workers: Mutex::new(BTreeMap::new()),
            accepting: AtomicBool::new(true),
        })
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
                provider_id: request.provider_id,
                session_id: opened.session_id,
                session: opened.session,
                events: HarnessEventBuffer::new(self.config.event_capacity),
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
        worker.session.answer_interactive(answer)
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

    fn ensure_accepting(&self) -> Result<(), HarnessError> {
        if self.accepting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(HarnessError::new(HarnessErrorClass::IntakeClosed))
        }
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
    if let Some(output) = output {
        dependencies.persistence.persist_output(
            agent_id,
            &worker.provider_id,
            &worker.session_id,
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
    if let Err(error) = drain_events(dependencies, agent_id, &mut worker) {
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
