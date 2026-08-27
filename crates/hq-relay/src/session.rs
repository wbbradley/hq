//! Deterministic ownership of one relay policy generation.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use hq_application::{RelayAccess, RelayAuthentication};
use sha2::{Digest, Sha256};

use crate::{
    AttemptDisposition, CanonicalIngest, CatchupCursor, FailureClass, GIFT_WRAP_KIND, InboundClaim,
    MAX_GIFT_WRAP_BYTES, MAX_QUARANTINE_SAMPLE_BYTES, MAX_RELAY_STATUS_BYTES, MAX_STAGING_BYTES,
    MAX_STATE_QUERY_ITEMS, OutboundIntent, PreparedOutbound, QuarantineEvidence, RelayAttempt,
    RelayAttemptFailure, RelayClock, RelayConnection, RelayConnector, RelayEnvelopePort,
    RelayFrame, RelayOpenOutcome, RelayPagePosition, RelayPolicy, RelayPortError, RelayReceive,
    RelayStateMutation, RelayStatePort, RelayStateQuery, RelayUrl, RouteResolver, StagedInput,
};

const LIVE_OVERLAP_SECONDS: u64 = 300;

/// Explicit bounds and retry policy for one relay session owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelaySessionConfig {
    /// Rows read from each active durable collection per work tick.
    pub state_page_items: usize,
    /// Maximum retained events requested in one backward page.
    pub retained_page_items: usize,
    /// Maximum live-edge wrappers buffered during retained catch-up.
    pub live_buffer_items: usize,
    /// Maximum total live-edge wrapper bytes buffered in memory.
    pub live_buffer_bytes: usize,
    /// Maximum received frames handled by one deterministic tick.
    pub max_frames_per_tick: usize,
    /// Whether this owner performs global staged-input recovery.
    pub recover_staging: bool,
    /// Maximum time one connection receive may block.
    pub receive_wait: Duration,
    /// Base outbound and staging retry delay.
    pub retry_initial: Duration,
    /// Inclusive retry-delay cap after deterministic jitter.
    pub retry_max: Duration,
}

impl Default for RelaySessionConfig {
    fn default() -> Self {
        Self {
            state_page_items: 64,
            retained_page_items: 256,
            live_buffer_items: 256,
            live_buffer_bytes: 8 * 1024 * 1024,
            max_frames_per_tick: 64,
            recover_staging: true,
            receive_wait: Duration::from_millis(100),
            retry_initial: Duration::from_millis(500),
            retry_max: Duration::from_secs(60),
        }
    }
}

impl RelaySessionConfig {
    fn validate(&self) -> Result<(), RelayPortError> {
        if self.state_page_items == 0
            || self.state_page_items > MAX_STATE_QUERY_ITEMS
            || self.retained_page_items == 0
            || self.retained_page_items > 1_000
            || self.live_buffer_items == 0
            || self.live_buffer_bytes == 0
            || self.live_buffer_bytes > MAX_STAGING_BYTES
            || self.max_frames_per_tick == 0
            || self.receive_wait.is_zero()
            || self.retry_initial.is_zero()
            || self.retry_initial > self.retry_max
        {
            return Err(RelayPortError::InvalidInput);
        }
        Ok(())
    }
}

/// Injected deterministic retry jitter.
pub trait RelayJitter: Send + Sync {
    /// Returns a value no greater than `inclusive_max_millis`.
    fn jitter_millis(
        &self,
        url: &RelayUrl,
        identity: [u8; 32],
        attempt: u32,
        inclusive_max_millis: u64,
    ) -> u64;
}

/// Stable hash-derived jitter suitable for reproducible retry scheduling.
#[derive(Clone, Copy, Debug, Default)]
pub struct StableRelayJitter;

impl RelayJitter for StableRelayJitter {
    fn jitter_millis(
        &self,
        url: &RelayUrl,
        identity: [u8; 32],
        attempt: u32,
        inclusive_max_millis: u64,
    ) -> u64 {
        if inclusive_max_millis == 0 {
            return 0;
        }
        let mut digest = Sha256::new();
        digest.update(url.as_str().as_bytes());
        digest.update(identity);
        digest.update(attempt.to_be_bytes());
        let bytes: [u8; 32] = digest.finalize().into();
        let sample = u64::from_be_bytes(bytes[..8].try_into().unwrap_or([0; 8]));
        sample % inclusive_max_millis.saturating_add(1)
    }
}

/// Shared capabilities used by one or more exclusively owned relay sessions.
#[derive(Clone)]
pub struct RelaySessionDependencies {
    /// Durable relay state.
    pub state: Arc<dyn RelayStatePort>,
    /// Verified route resolution independent of relay observations.
    pub routes: Arc<dyn RouteResolver>,
    /// Common canonical verification and ingest path.
    pub ingest: Arc<dyn CanonicalIngest>,
    /// Installation-root envelope operations.
    pub envelopes: Arc<dyn RelayEnvelopePort>,
    /// Injected wall and monotonic clock.
    pub clock: Arc<dyn RelayClock>,
    /// Connection factory.
    pub connector: Arc<dyn RelayConnector>,
    /// Deterministic retry jitter.
    pub jitter: Arc<dyn RelayJitter>,
}

/// Bounded progress report from one deterministic session tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RelaySessionProgress {
    /// Complete frames handled.
    pub frames: usize,
    /// Exact outbound wrappers sent.
    pub published: usize,
    /// Inbound wrappers committed or deduplicated.
    pub ingested: usize,
    /// Inputs retained for transient retry.
    pub staged: usize,
    /// Inputs permanently quarantined.
    pub quarantined: usize,
}

/// Exclusive state-machine owner for one relay policy generation.
pub struct RelaySession {
    policy: RelayPolicy,
    config: RelaySessionConfig,
    dependencies: RelaySessionDependencies,
    connection: Option<Box<dyn RelayConnection>>,
    ordinary_started: bool,
    authenticated: bool,
    latest_challenge: Option<String>,
    authentication_event_id: Option<[u8; 32]>,
    live_subscription: String,
    retained_subscription: String,
    retained_active: bool,
    retained_count: usize,
    retained_oldest: Option<(u64, [u8; 32])>,
    retained_retry_at: Option<u64>,
    retained_stalls: u32,
    live_buffer: VecDeque<Vec<u8>>,
    live_buffer_bytes: usize,
    inflight: BTreeMap<[u8; 32], RelayAttempt>,
    outbound_query: RelayStateQuery,
    staging_query: RelayStateQuery,
}

impl RelaySession {
    /// Creates one disconnected owner for an enabled durable policy generation.
    pub fn new(
        policy: RelayPolicy,
        config: RelaySessionConfig,
        dependencies: RelaySessionDependencies,
    ) -> Result<Self, RelayPortError> {
        config.validate()?;
        if !policy.enabled {
            return Err(RelayPortError::InvalidInput);
        }
        let outbound_query = outbound_query(config.state_page_items);
        let staging_query = staging_query(config.state_page_items);
        Ok(Self {
            live_subscription: format!("hq-live-{}", policy.generation),
            retained_subscription: format!("hq-retained-{}", policy.generation),
            authenticated: policy.authentication != RelayAuthentication::Required,
            policy,
            config,
            dependencies,
            connection: None,
            ordinary_started: false,
            latest_challenge: None,
            authentication_event_id: None,
            retained_active: false,
            retained_count: 0,
            retained_oldest: None,
            retained_retry_at: None,
            retained_stalls: 0,
            live_buffer: VecDeque::new(),
            live_buffer_bytes: 0,
            inflight: BTreeMap::new(),
            outbound_query,
            staging_query,
        })
    }

    /// Connects if needed, performs bounded durable work, and handles bounded input.
    pub fn tick(&mut self) -> Result<RelaySessionProgress, RelayPortError> {
        self.ensure_connected()?;
        let mut progress = RelaySessionProgress::default();
        if self.authenticated {
            self.start_ordinary_work()?;
            self.resume_retained_if_due()?;
            if self.config.recover_staging {
                tolerate_local_retry(self.recover_staging(&mut progress))?;
            }
            tolerate_local_retry(self.publish_outbound(&mut progress))?;
        }
        let receive_wait = self.config.receive_wait;
        for _ in 0..self.config.max_frames_per_tick {
            let receive = self
                .connection_mut()?
                .receive(receive_wait)
                .map_err(|_| RelayPortError::Connection)?;
            match receive {
                RelayReceive::Frame(frame) => {
                    progress.frames += 1;
                    self.handle_frame(frame, &mut progress)?;
                }
                RelayReceive::TimedOut => break,
                RelayReceive::Closed => return Err(RelayPortError::Connection),
            }
        }
        Ok(progress)
    }

    /// Closes named subscriptions and the owned connection idempotently.
    pub fn close(&mut self) -> Result<(), RelayPortError> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let mut outcome = Ok(());
        if self.ordinary_started && readable(self.policy.access) {
            if let Err(error) = connection.send(RelayFrame::Close(self.live_subscription.clone())) {
                outcome = Err(connection_error(error));
            }
            if self.retained_active
                && let Err(error) =
                    connection.send(RelayFrame::Close(self.retained_subscription.clone()))
                && outcome.is_ok()
            {
                outcome = Err(connection_error(error));
            }
        }
        if let Err(error) = connection.close()
            && outcome.is_ok()
        {
            outcome = Err(connection_error(error));
        }
        self.reset_connection_state();
        outcome
    }

    fn ensure_connected(&mut self) -> Result<(), RelayPortError> {
        if self.connection.is_none() {
            self.connection = Some(
                self.dependencies
                    .connector
                    .connect(&self.policy.url)
                    .map_err(connection_error)?,
            );
            self.reset_connection_state();
            self.authenticated = self.policy.authentication != RelayAuthentication::Required;
        }
        Ok(())
    }

    fn reset_connection_state(&mut self) {
        self.ordinary_started = false;
        self.latest_challenge = None;
        self.authentication_event_id = None;
        self.retained_active = false;
        self.retained_count = 0;
        self.retained_oldest = None;
        self.retained_retry_at = None;
        self.retained_stalls = 0;
        self.inflight.clear();
    }

    fn start_ordinary_work(&mut self) -> Result<(), RelayPortError> {
        if self.ordinary_started {
            return Ok(());
        }
        if readable(self.policy.access) {
            let now_seconds = self.dependencies.clock.unix_millis() / 1_000;
            let filter = live_filter(
                self.dependencies.envelopes.local_public_key(),
                now_seconds.saturating_sub(LIVE_OVERLAP_SECONDS),
            );
            let subscription = self.live_subscription.clone();
            self.send(RelayFrame::Request {
                subscription,
                filter,
            })?;
            let cursor = self.dependencies.state.cursor(&self.policy.url)?;
            if !cursor.as_ref().is_some_and(|cursor| {
                cursor.generation == self.policy.generation && cursor.exhausted
            }) {
                self.open_retained_page(cursor.as_ref())?;
            }
        }
        self.ordinary_started = true;
        Ok(())
    }

    fn open_retained_page(&mut self, cursor: Option<&CatchupCursor>) -> Result<(), RelayPortError> {
        let until = cursor
            .filter(|cursor| cursor.generation == self.policy.generation)
            .and_then(|cursor| cursor.oldest_created_at)
            .unwrap_or_else(|| self.dependencies.clock.unix_millis() / 1_000);
        let filter = retained_filter(
            self.dependencies.envelopes.local_public_key(),
            until,
            self.config.retained_page_items,
        );
        let subscription = self.retained_subscription.clone();
        self.send(RelayFrame::Request {
            subscription,
            filter,
        })?;
        self.retained_active = true;
        self.retained_count = 0;
        self.retained_oldest = None;
        self.retained_retry_at = None;
        Ok(())
    }

    fn resume_retained_if_due(&mut self) -> Result<(), RelayPortError> {
        if !readable(self.policy.access) || self.retained_active {
            return Ok(());
        }
        let cursor = self.dependencies.state.cursor(&self.policy.url)?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.generation == self.policy.generation && cursor.exhausted)
        {
            return Ok(());
        }
        let now = self.dependencies.clock.unix_millis();
        if self.retained_retry_at.is_none_or(|retry| retry <= now) {
            self.open_retained_page(cursor.as_ref())?;
        }
        Ok(())
    }

    fn recover_staging(
        &mut self,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        let page = self
            .dependencies
            .state
            .load_page(self.staging_query.clone())?;
        self.staging_query = page
            .next
            .unwrap_or_else(|| staging_query(self.config.state_page_items));
        let now = self.dependencies.clock.unix_millis();
        for input in page.state.staged {
            if input.attempts == u32::MAX || input.retry_at_millis > now {
                continue;
            }
            self.process_input(
                input.exact_outer.clone(),
                Some(input.wrapper_sha256),
                Some(&input),
                progress,
            )?;
        }
        Ok(())
    }

    fn publish_outbound(
        &mut self,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        if !writable(self.policy.access) {
            return Ok(());
        }
        let page = self
            .dependencies
            .state
            .load_page(self.outbound_query.clone())?;
        self.outbound_query = page
            .next
            .unwrap_or_else(|| outbound_query(self.config.state_page_items));
        for intent in page.state.outbound {
            let route = self.dependencies.routes.resolve(intent.key)?;
            if !route.relays.iter().any(|url| url == &self.policy.url) {
                continue;
            }
            let prepared = self.ensure_prepared(&intent, route.recipient_public_key)?;
            let wrapper_id = prepared.envelope.metadata.wrapper_id;
            let prior = self
                .dependencies
                .state
                .attempt(&self.policy.url, wrapper_id)?;
            if !attempt_due(prior.as_ref(), self.dependencies.clock.unix_millis()) {
                continue;
            }
            let attempts = prior
                .as_ref()
                .map_or(1, |attempt| attempt.attempts.saturating_add(1));
            if attempts == u32::MAX
                && prior
                    .as_ref()
                    .is_some_and(|value| value.attempts == u32::MAX)
            {
                continue;
            }
            let now = self.dependencies.clock.unix_millis();
            let retry_at = now.saturating_add(self.retry_delay(wrapper_id, attempts));
            let attempt = RelayAttempt {
                url: self.policy.url.clone(),
                wrapper_id,
                attempts,
                disposition: AttemptDisposition::Uncertain,
                failure: None,
                last_attempt_millis: now,
                retry_at_millis: Some(retry_at),
            };
            self.dependencies
                .state
                .apply(RelayStateMutation::Attempt(attempt.clone()))?;
            self.send(RelayFrame::Event(prepared.envelope.exact_wire.clone()))?;
            self.inflight.insert(wrapper_id, attempt);
            progress.published += 1;
        }
        Ok(())
    }

    fn ensure_prepared(
        &self,
        intent: &OutboundIntent,
        recipient_public_key: [u8; 32],
    ) -> Result<PreparedOutbound, RelayPortError> {
        if let Some(prepared) = self.dependencies.state.prepared(intent.key)? {
            return Ok(prepared);
        }
        let prepared = self.dependencies.envelopes.prepare(
            intent,
            recipient_public_key,
            self.dependencies.clock.unix_millis() / 1_000,
        )?;
        match self
            .dependencies
            .state
            .apply(RelayStateMutation::Prepare(prepared.clone()))
        {
            Ok(()) => Ok(prepared),
            Err(RelayPortError::Conflict) => self
                .dependencies
                .state
                .prepared(intent.key)?
                .ok_or(RelayPortError::Corrupt),
            Err(error) => Err(error),
        }
    }

    fn handle_frame(
        &mut self,
        frame: RelayFrame,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        match frame {
            RelayFrame::Auth(challenge) => self.handle_challenge(&challenge),
            RelayFrame::Ok {
                event_id,
                accepted,
                message,
            } => self.handle_ok(event_id, accepted, &message),
            RelayFrame::SubscriptionEvent {
                subscription,
                exact_event,
            } => self.handle_subscription_event(&subscription, exact_event, progress),
            RelayFrame::EndOfStoredEvents(subscription) => {
                self.handle_eose(&subscription, progress)
            }
            RelayFrame::Closed { subscription, .. }
                if subscription == self.live_subscription
                    || subscription == self.retained_subscription =>
            {
                Err(RelayPortError::Connection)
            }
            RelayFrame::Notice(_)
            | RelayFrame::Closed { .. }
            | RelayFrame::Event(_)
            | RelayFrame::Request { .. }
            | RelayFrame::Close(_) => Ok(()),
        }
    }

    fn handle_challenge(&mut self, challenge: &str) -> Result<(), RelayPortError> {
        if challenge.is_empty() || challenge.len() > MAX_RELAY_STATUS_BYTES {
            return Err(RelayPortError::Connection);
        }
        self.latest_challenge = Some(challenge.to_owned());
        if self.policy.authentication == RelayAuthentication::Disabled {
            return Ok(());
        }
        let authentication = self.dependencies.envelopes.authenticate(
            &self.policy.url,
            challenge,
            self.dependencies.clock.unix_millis() / 1_000,
        )?;
        self.authentication_event_id = Some(authentication.event_id);
        self.send(RelayFrame::Auth(string_from_utf8(
            authentication.exact_event,
        )?))
    }

    fn handle_ok(
        &mut self,
        event_id: [u8; 32],
        accepted: bool,
        message: &str,
    ) -> Result<(), RelayPortError> {
        let accepted = accepted || ack_prefix(message, "duplicate:");
        if self.authentication_event_id == Some(event_id) {
            self.authentication_event_id = None;
            if !accepted {
                return Err(RelayPortError::Unavailable);
            }
            self.authenticated = true;
            return self.start_ordinary_work();
        }
        let prior = self.inflight.remove(&event_id).or(self
            .dependencies
            .state
            .attempt(&self.policy.url, event_id)?);
        let Some(prior) = prior else {
            return Ok(());
        };
        let (disposition, failure, retry_at_millis) = if accepted {
            (AttemptDisposition::Accepted, None, None)
        } else {
            let failure = classify_rejection(message);
            let retry = if failure == RelayAttemptFailure::Permanent {
                None
            } else {
                Some(
                    self.dependencies
                        .clock
                        .unix_millis()
                        .saturating_add(self.retry_delay(event_id, prior.attempts)),
                )
            };
            (AttemptDisposition::Rejected, Some(failure), retry)
        };
        self.dependencies
            .state
            .apply(RelayStateMutation::Attempt(RelayAttempt {
                disposition,
                failure,
                retry_at_millis,
                ..prior
            }))?;
        if !accepted
            && failure == Some(RelayAttemptFailure::AuthenticationRequired)
            && self.policy.authentication != RelayAuthentication::Disabled
            && let Some(challenge) = self.latest_challenge.clone()
        {
            self.handle_challenge(&challenge)?;
        }
        Ok(())
    }

    fn handle_subscription_event(
        &mut self,
        subscription: &str,
        exact_event: Vec<u8>,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        if subscription == self.live_subscription && self.retained_active {
            if self.live_buffer.len() < self.config.live_buffer_items
                && self
                    .live_buffer_bytes
                    .checked_add(exact_event.len())
                    .is_some_and(|total| total <= self.config.live_buffer_bytes)
            {
                self.live_buffer_bytes += exact_event.len();
                self.live_buffer.push_back(exact_event);
                return Ok(());
            }
            return self.stage_new(exact_event, progress);
        }
        if subscription != self.live_subscription && subscription != self.retained_subscription {
            return Ok(());
        }
        if subscription == self.retained_subscription {
            self.retained_count = self.retained_count.saturating_add(1);
        }
        let observed = self.process_input(exact_event, None, None, progress)?;
        if subscription == self.retained_subscription
            && let Some(boundary) = observed
        {
            self.retained_oldest = Some(
                self.retained_oldest
                    .map_or(boundary, |oldest| oldest.min(boundary)),
            );
        }
        Ok(())
    }

    fn handle_eose(
        &mut self,
        subscription: &str,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        if subscription != self.retained_subscription || !self.retained_active {
            return Ok(());
        }
        let prior = self.dependencies.state.cursor(&self.policy.url)?;
        let short_page = self.retained_count < self.config.retained_page_items;
        let advances = self.retained_oldest.is_some_and(|candidate| {
            prior
                .as_ref()
                .and_then(|cursor| cursor.oldest_created_at.zip(cursor.oldest_wrapper_id))
                .is_none_or(|oldest| candidate < oldest)
        });
        if short_page || advances {
            let boundary = self.retained_oldest.or_else(|| {
                prior
                    .as_ref()
                    .and_then(|cursor| cursor.oldest_created_at.zip(cursor.oldest_wrapper_id))
            });
            self.dependencies
                .state
                .apply(RelayStateMutation::Cursor(CatchupCursor {
                    url: self.policy.url.clone(),
                    generation: self.policy.generation,
                    oldest_created_at: boundary.map(|value| value.0),
                    oldest_wrapper_id: boundary.map(|value| value.1),
                    exhausted: short_page,
                }))?;
        }
        let subscription = self.retained_subscription.clone();
        self.send(RelayFrame::Close(subscription))?;
        self.retained_active = false;
        if short_page {
            self.retained_stalls = 0;
            self.retained_retry_at = None;
            while let Some(exact) = self.live_buffer.pop_front() {
                self.live_buffer_bytes = self.live_buffer_bytes.saturating_sub(exact.len());
                self.process_input(exact, None, None, progress)?;
            }
        } else if advances {
            self.retained_stalls = 0;
            let cursor = self.dependencies.state.cursor(&self.policy.url)?;
            self.open_retained_page(cursor.as_ref())?;
        } else {
            self.retained_stalls = self.retained_stalls.saturating_add(1);
            let identity: [u8; 32] = Sha256::digest(self.policy.url.as_str().as_bytes()).into();
            self.retained_retry_at = Some(
                self.dependencies
                    .clock
                    .unix_millis()
                    .saturating_add(self.retry_delay(identity, self.retained_stalls)),
            );
        }
        Ok(())
    }

    fn process_input(
        &self,
        exact_outer: Vec<u8>,
        remove_staged: Option<[u8; 32]>,
        staged: Option<&StagedInput>,
        progress: &mut RelaySessionProgress,
    ) -> Result<Option<(u64, [u8; 32])>, RelayPortError> {
        let digest: [u8; 32] = Sha256::digest(&exact_outer).into();
        match self.dependencies.envelopes.open(&exact_outer)? {
            RelayOpenOutcome::Opened(opened) => {
                match self
                    .dependencies
                    .ingest
                    .ingest(opened.exact_canonical_bytes)
                {
                    Ok(()) => {
                        self.dependencies
                            .state
                            .apply(RelayStateMutation::ClaimInbound {
                                claim: InboundClaim {
                                    wrapper_id: opened.wrapper_id,
                                    logical_id: opened.logical_id,
                                    canonical_sha256: opened.canonical_sha256,
                                    received_at_millis: self.dependencies.clock.unix_millis(),
                                },
                                remove_staged,
                            })?;
                        progress.ingested += 1;
                    }
                    Err(
                        RelayPortError::Unavailable
                        | RelayPortError::Backpressure
                        | RelayPortError::Connection,
                    ) => {
                        self.stage_retry(exact_outer, digest, staged, progress)?;
                    }
                    Err(
                        RelayPortError::InvalidInput
                        | RelayPortError::Conflict
                        | RelayPortError::Corrupt,
                    ) => {
                        self.quarantine(
                            exact_outer,
                            digest,
                            Some(opened.wrapper_id),
                            FailureClass::Canonical,
                            remove_staged,
                            progress,
                        )?;
                    }
                }
                Ok(Some((opened.wrapper_created_at, opened.wrapper_id)))
            }
            RelayOpenOutcome::Rejected(rejected) => {
                self.quarantine(
                    exact_outer,
                    digest,
                    rejected.wrapper_id,
                    rejected.failure,
                    remove_staged,
                    progress,
                )?;
                Ok(None)
            }
        }
    }

    fn stage_new(
        &self,
        exact_outer: Vec<u8>,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        let digest: [u8; 32] = Sha256::digest(&exact_outer).into();
        if exact_outer.is_empty() || exact_outer.len() > MAX_GIFT_WRAP_BYTES {
            return self.quarantine(
                exact_outer,
                digest,
                None,
                FailureClass::Size,
                None,
                progress,
            );
        }
        self.stage_retry(exact_outer, digest, None, progress)
    }

    fn stage_retry(
        &self,
        exact_outer: Vec<u8>,
        digest: [u8; 32],
        prior: Option<&StagedInput>,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        let now = self.dependencies.clock.unix_millis();
        let attempts = prior.map_or(0, |input| input.attempts.saturating_add(1));
        let first_received_millis = prior.map_or(now, |input| input.first_received_millis);
        self.dependencies
            .state
            .apply(RelayStateMutation::Stage(StagedInput {
                wrapper_sha256: digest,
                exact_outer,
                first_received_millis,
                attempts,
                retry_at_millis: now.saturating_add(self.retry_delay(digest, attempts.max(1))),
            }))?;
        progress.staged += 1;
        Ok(())
    }

    fn quarantine(
        &self,
        exact_outer: Vec<u8>,
        digest: [u8; 32],
        wrapper_id: Option<[u8; 32]>,
        failure: FailureClass,
        remove_staged: Option<[u8; 32]>,
        progress: &mut RelaySessionProgress,
    ) -> Result<(), RelayPortError> {
        let byte_len = exact_outer.len();
        self.dependencies
            .state
            .apply(RelayStateMutation::Quarantine {
                evidence: QuarantineEvidence {
                    wrapper_sha256: digest,
                    wrapper_id,
                    failure,
                    received_at_millis: self.dependencies.clock.unix_millis(),
                    byte_len,
                    raw_sample: exact_outer
                        .into_iter()
                        .take(MAX_QUARANTINE_SAMPLE_BYTES)
                        .collect(),
                },
                remove_staged,
            })?;
        progress.quarantined += 1;
        Ok(())
    }

    fn retry_delay(&self, identity: [u8; 32], attempt: u32) -> u64 {
        let initial = duration_millis(self.config.retry_initial);
        let maximum = duration_millis(self.config.retry_max);
        let shift = attempt.saturating_sub(1).min(63);
        let base = initial.checked_shl(shift).unwrap_or(u64::MAX).min(maximum);
        let jitter_max = base / 4;
        let jitter =
            self.dependencies
                .jitter
                .jitter_millis(&self.policy.url, identity, attempt, jitter_max);
        base.saturating_add(jitter).min(maximum)
    }

    fn connection_mut(&mut self) -> Result<&mut (dyn RelayConnection + '_), RelayPortError> {
        if let Some(connection) = self.connection.as_deref_mut() {
            Ok(connection)
        } else {
            Err(RelayPortError::Connection)
        }
    }

    fn send(&mut self, frame: RelayFrame) -> Result<(), RelayPortError> {
        self.connection_mut()?.send(frame).map_err(connection_error)
    }
}

impl Drop for RelaySession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn outbound_query(limit: usize) -> RelayStateQuery {
    let mut query = done_query(limit);
    query.outbound = RelayPagePosition::Start;
    query
}

fn staging_query(limit: usize) -> RelayStateQuery {
    let mut query = done_query(limit);
    query.staged = RelayPagePosition::Start;
    query
}

fn done_query(limit: usize) -> RelayStateQuery {
    RelayStateQuery {
        limit,
        policies: RelayPagePosition::Done,
        outbound: RelayPagePosition::Done,
        prepared: RelayPagePosition::Done,
        attempts: RelayPagePosition::Done,
        cursors: RelayPagePosition::Done,
        staged: RelayPagePosition::Done,
        quarantine: RelayPagePosition::Done,
    }
}

fn attempt_due(attempt: Option<&RelayAttempt>, now: u64) -> bool {
    match attempt {
        None => true,
        Some(attempt) if attempt.disposition == AttemptDisposition::Accepted => false,
        Some(attempt)
            if attempt.disposition == AttemptDisposition::Rejected
                && attempt.failure == Some(RelayAttemptFailure::Permanent) =>
        {
            false
        }
        Some(attempt) => attempt.retry_at_millis.is_none_or(|retry| retry <= now),
    }
}

fn tolerate_local_retry(result: Result<(), RelayPortError>) -> Result<(), RelayPortError> {
    match result {
        Ok(()) | Err(RelayPortError::Unavailable | RelayPortError::Backpressure) => Ok(()),
        Err(error) => Err(error),
    }
}

const fn connection_error(_error: RelayPortError) -> RelayPortError {
    RelayPortError::Connection
}

fn readable(access: RelayAccess) -> bool {
    matches!(access, RelayAccess::Read | RelayAccess::ReadWrite)
}

fn writable(access: RelayAccess) -> bool {
    matches!(access, RelayAccess::Write | RelayAccess::ReadWrite)
}

fn classify_rejection(message: &str) -> RelayAttemptFailure {
    if ack_prefix(message, "auth-required:") {
        RelayAttemptFailure::AuthenticationRequired
    } else if ack_prefix(message, "rate-limited:") {
        RelayAttemptFailure::RateLimited
    } else {
        RelayAttemptFailure::Permanent
    }
}

fn ack_prefix(message: &str, prefix: &str) -> bool {
    if message.len() > MAX_RELAY_STATUS_BYTES {
        return false;
    }
    message
        .trim_start()
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn live_filter(public_key: [u8; 32], since: u64) -> String {
    format!(
        "{{\"kinds\":[{GIFT_WRAP_KIND}],\"#p\":[\"{}\"],\"since\":{since}}}",
        hex(public_key)
    )
}

fn retained_filter(public_key: [u8; 32], until: u64, limit: usize) -> String {
    format!(
        "{{\"kinds\":[{GIFT_WRAP_KIND}],\"#p\":[\"{}\"],\"until\":{until},\"limit\":{limit}}}",
        hex(public_key)
    )
}

fn hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn string_from_utf8(bytes: Vec<u8>) -> Result<String, RelayPortError> {
    String::from_utf8(bytes).map_err(|_| RelayPortError::Corrupt)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
