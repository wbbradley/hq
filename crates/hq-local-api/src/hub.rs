//! Bounded nonblocking revision invalidation fanout.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    future::poll_fn,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll, Waker},
};

use hq_application::{
    ApplicationError, ApplicationErrorCode, ObserveRevisions, SubscriptionRequest,
    SubscriptionTopic,
};
use hq_domain::{OperationId, Revision};

/// Default maximum concurrent local subscriptions owned by one node.
pub const DEFAULT_MAX_SUBSCRIPTIONS: usize = 256;

/// Failure to construct a revision hub.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HubConfigError {
    /// A bounded registry must have room for at least one subscription.
    ZeroCapacity,
}

/// Failure to acquire the hub's sole event-loop wake listener.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevisionWakeListenerUnavailable;

impl fmt::Display for RevisionWakeListenerUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("revision wake listener is already owned")
    }
}

impl Error for RevisionWakeListenerUnavailable {}

impl fmt::Display for HubConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("revision hub capacity must be positive")
    }
}

impl Error for HubConfigError {}

/// One bounded coalesced invalidation owned by a subscription.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionNotice {
    revision: Revision,
    topics: BTreeSet<SubscriptionTopic>,
    full_snapshot: bool,
}

impl RevisionNotice {
    /// Returns the newest committed revision represented by this notice.
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// Returns the union of broad changed topics represented by this notice.
    pub const fn topics(&self) -> &BTreeSet<SubscriptionTopic> {
        &self.topics
    }

    /// Reports whether an incremental refresh is insufficient.
    pub const fn full_snapshot(&self) -> bool {
        self.full_snapshot
    }

    fn merge(
        &mut self,
        revision: Revision,
        topics: &BTreeSet<SubscriptionTopic>,
        full_snapshot: bool,
    ) {
        self.revision = self.revision.max(revision);
        self.topics.extend(topics.iter().copied());
        self.full_snapshot |= full_snapshot;
    }
}

/// Aggregate result of one nonblocking publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FanoutDisposition {
    /// No registered subscription matched the change.
    Ignored,
    /// At least one matching subscriber gained its first pending wake.
    Scheduled {
        /// Number of matching subscribers touched by this publication.
        subscribers: usize,
    },
    /// Every matching subscriber already had pending work and was updated in place.
    Coalesced {
        /// Number of matching subscribers touched by this publication.
        subscribers: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Pending,
    Active,
}

#[derive(Debug)]
struct Registration {
    topics: BTreeSet<SubscriptionTopic>,
    phase: Phase,
    notice: Option<RevisionNotice>,
}

#[derive(Debug, Default)]
struct HubState {
    registrations: BTreeMap<OperationId, Registration>,
    wake_generation: u64,
    wake_listener_owned: bool,
    waiter: Option<Waker>,
}

/// Shared bounded subscription registry with one coalesced wake per subscriber.
#[derive(Clone, Debug)]
pub struct RevisionHub {
    capacity: usize,
    state: Arc<Mutex<HubState>>,
}

impl Default for RevisionHub {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_MAX_SUBSCRIPTIONS,
            state: Arc::new(Mutex::new(HubState::default())),
        }
    }
}

impl RevisionHub {
    /// Constructs a hub with an explicit inclusive subscription capacity.
    pub fn new(capacity: usize) -> Result<Self, HubConfigError> {
        if capacity == 0 {
            return Err(HubConfigError::ZeroCapacity);
        }
        Ok(Self {
            capacity,
            state: Arc::new(Mutex::new(HubState::default())),
        })
    }

    /// Transfers the sole runtime-neutral readiness listener for newly actionable notices.
    pub fn take_wake_listener(
        &self,
    ) -> Result<RevisionWakeListener, RevisionWakeListenerUnavailable> {
        let mut state = self.lock();
        if state.wake_listener_owned {
            return Err(RevisionWakeListenerUnavailable);
        }
        state.wake_listener_owned = true;
        Ok(RevisionWakeListener {
            state: Arc::clone(&self.state),
            observed_generation: 0,
        })
    }

    /// Publishes one revision change without waiting for or allocating work per client read.
    pub fn publish(
        &self,
        revision: Revision,
        topics: impl IntoIterator<Item = SubscriptionTopic>,
        full_snapshot: bool,
    ) -> FanoutDisposition {
        let topics = topics.into_iter().collect::<BTreeSet<_>>();
        if revision.value() == 0 || (topics.is_empty() && !full_snapshot) {
            return FanoutDisposition::Ignored;
        }

        let (disposition, waiter) = {
            let mut state = self.lock();
            let mut matched = 0;
            let mut scheduled = false;
            let mut active_scheduled = false;
            for registration in state.registrations.values_mut() {
                if !full_snapshot && !topics_match(&registration.topics, &topics) {
                    continue;
                }
                matched += 1;
                match &mut registration.notice {
                    Some(notice) => notice.merge(revision, &topics, full_snapshot),
                    slot @ None => {
                        *slot = Some(RevisionNotice {
                            revision,
                            topics: topics.clone(),
                            full_snapshot,
                        });
                        scheduled = true;
                        active_scheduled |= registration.phase == Phase::Active;
                    }
                }
            }
            let disposition = match (matched, scheduled) {
                (0, _) => FanoutDisposition::Ignored,
                (subscribers, true) => FanoutDisposition::Scheduled { subscribers },
                (subscribers, false) => FanoutDisposition::Coalesced { subscribers },
            };
            let waiter = active_scheduled.then(|| advance_wake(&mut state)).flatten();
            (disposition, waiter)
        };
        if let Some(waiter) = waiter {
            waiter.wake();
        }
        disposition
    }

    /// Takes the sole pending wake for an active subscription.
    pub fn take(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<RevisionNotice>, ApplicationError> {
        let mut state = self.lock();
        let registration = state
            .registrations
            .get_mut(&operation_id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::ItemNotFound))?;
        if registration.phase == Phase::Pending {
            return Ok(None);
        }
        Ok(registration.notice.take())
    }

    /// Returns the current number of pending and active registrations.
    pub fn len(&self) -> usize {
        self.lock().registrations.len()
    }

    /// Reports whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the configured inclusive registration capacity.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ObserveRevisions for RevisionHub {
    fn register_subscription(&self, request: &SubscriptionRequest) -> Result<(), ApplicationError> {
        let mut state = self.lock();
        if state.registrations.contains_key(&request.operation_id()) {
            return Err(ApplicationError::new(
                ApplicationErrorCode::StateIdentityConflict,
            ));
        }
        if state.registrations.len() >= self.capacity {
            return Err(ApplicationError::new(ApplicationErrorCode::IntakeFull));
        }
        state.registrations.insert(
            request.operation_id(),
            Registration {
                topics: request.topics().clone(),
                phase: Phase::Pending,
                notice: None,
            },
        );
        Ok(())
    }

    fn activate_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError> {
        let waiter = {
            let mut state = self.lock();
            let registration = state
                .registrations
                .get_mut(&operation_id)
                .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::ItemNotFound))?;
            if registration.phase != Phase::Pending {
                return Err(ApplicationError::new(
                    ApplicationErrorCode::StateIdentityConflict,
                ));
            }
            registration.phase = Phase::Active;
            registration
                .notice
                .is_some()
                .then(|| advance_wake(&mut state))
                .flatten()
        };
        if let Some(waiter) = waiter {
            waiter.wake();
        }
        Ok(())
    }

    fn cancel_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError> {
        self.lock().registrations.remove(&operation_id);
        Ok(())
    }
}

/// Sole runtime-neutral observer of newly actionable revision notices.
pub struct RevisionWakeListener {
    state: Arc<Mutex<HubState>>,
    observed_generation: u64,
}

impl fmt::Debug for RevisionWakeListener {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RevisionWakeListener")
            .field("observed_generation", &self.observed_generation)
            .finish_non_exhaustive()
    }
}

impl RevisionWakeListener {
    /// Consumes an already-published generation without blocking.
    pub fn has_changed(&mut self) -> bool {
        let generation = lock_state(&self.state).wake_generation;
        if generation == self.observed_generation {
            return false;
        }
        self.observed_generation = generation;
        true
    }

    /// Marks every generation published before this call as observed.
    pub fn observe_current(&mut self) {
        let mut state = lock_state(&self.state);
        self.observed_generation = state.wake_generation;
        state.waiter = None;
    }

    /// Waits until at least one newly actionable notice is published.
    pub async fn changed(&mut self) {
        poll_fn(|context| self.poll_changed(context)).await;
    }

    fn poll_changed(&mut self, context: &Context<'_>) -> Poll<()> {
        let mut state = lock_state(&self.state);
        if state.wake_generation != self.observed_generation {
            self.observed_generation = state.wake_generation;
            return Poll::Ready(());
        }
        if state
            .waiter
            .as_ref()
            .is_none_or(|waiter| !waiter.will_wake(context.waker()))
        {
            state.waiter = Some(context.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for RevisionWakeListener {
    fn drop(&mut self) {
        let mut state = lock_state(&self.state);
        state.waiter = None;
        state.wake_listener_owned = false;
    }
}

fn advance_wake(state: &mut HubState) -> Option<Waker> {
    state.wake_generation = state.wake_generation.wrapping_add(1);
    state.waiter.take()
}

fn lock_state(state: &Mutex<HubState>) -> MutexGuard<'_, HubState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn topics_match(
    subscribed: &BTreeSet<SubscriptionTopic>,
    changed: &BTreeSet<SubscriptionTopic>,
) -> bool {
    subscribed.contains(&SubscriptionTopic::All)
        || changed.contains(&SubscriptionTopic::All)
        || !subscribed.is_disjoint(changed)
}
