//! Pure reconnecting local client state machine and transport adapter contract.

use std::{
    collections::{BTreeMap, VecDeque},
    error::Error,
    fmt,
    num::NonZeroU64,
    time::Duration,
};

use hq_domain::{CommandDigest, CommandId};
use sha2::{Digest, Sha256};

use crate::protocol::v1::{
    AuthoritativeSnapshotDto, BuildMetadata, ClientHello, DecodeError, ErrorResponse, Id32,
    InvalidationTopic, MutationAttemptDto, MutationRequest, Request, RequestEnvelope, RequestId,
    Response, ResponseResult, SubscriptionRequestDto, V1, VersionRange, WireMessage,
};

/// Maximum simultaneous exact mutation frames retained for response-loss replay.
pub const MAX_IN_FLIGHT_MUTATIONS: usize = 256;

const SUBSCRIPTION_ID_DOMAIN: &[u8] = b"hq-local-api-client-subscription-v1\0";

/// Narrow blocking transport operations used by a runner around the pure state machine.
pub trait ClientTransport {
    /// Adapter-owned live connection handle.
    type Connection;
    /// Adapter-owned redacted transport failure.
    type Error: Error;

    /// Opens one local transport connection.
    fn connect(&mut self) -> Result<Self::Connection, Self::Error>;
    /// Writes one complete exact frame.
    fn write(&mut self, connection: &mut Self::Connection, frame: &[u8])
    -> Result<(), Self::Error>;
    /// Closes one connection idempotently.
    fn close(&mut self, connection: Self::Connection);
}

/// Nonzero client-local connection attempt identity used to discard stale events.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConnectionGeneration(NonZeroU64);

impl ConnectionGeneration {
    /// Returns the diagnostic client-local generation number.
    pub const fn value(self) -> u64 {
        self.0.get()
    }
}

/// Deterministic exponential reconnect schedule with an inclusive maximum delay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    initial: Duration,
    maximum: Duration,
}

impl ReconnectPolicy {
    /// Constructs a positive ordered reconnect policy.
    pub fn new(initial: Duration, maximum: Duration) -> Result<Self, ClientError> {
        if initial.is_zero() || maximum.is_zero() || initial > maximum {
            return Err(ClientError::InvalidReconnectPolicy);
        }
        Ok(Self { initial, maximum })
    }

    /// Returns the capped delay for a zero-based consecutive failure number.
    pub fn delay(self, failure: u32) -> Duration {
        self.initial
            .saturating_mul(1_u32.checked_shl(failure.min(31)).unwrap_or(u32::MAX))
            .min(self.maximum)
    }
}

/// Pure side effect requested from a client runner or transport adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientAction {
    /// Open a new connection after the explicit deterministic delay.
    ConnectAfter {
        /// Generation to report with every event from the attempted connection.
        generation: ConnectionGeneration,
        /// Delay before the adapter attempts the connection.
        delay: Duration,
    },
    /// Write one complete exact local API frame on the named current generation.
    Write {
        /// Current connection generation.
        generation: ConnectionGeneration,
        /// Exact bytes to write; mutation replay reuses this vector byte-for-byte.
        frame: Vec<u8>,
    },
    /// Close the named connection idempotently.
    Close {
        /// Current connection generation.
        generation: ConnectionGeneration,
    },
}

/// Semantic result delivered to a CLI, TUI, harness launcher, or test runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientEvent {
    /// A fresh complete authoritative snapshot became available.
    Snapshot(AuthoritativeSnapshotDto),
    /// A stable mutation completed or remains explicitly uncertain.
    Mutation(MutationAttemptDto),
    /// A correlated ordinary request completed successfully.
    Response {
        /// Original request correlation identity.
        request_id: RequestId,
        /// Typed successful result.
        result: ResponseResult,
    },
    /// A correlated server operation failed with a stable wire error.
    Error {
        /// Original request correlation identity.
        request_id: RequestId,
        /// Stable typed failure.
        error: ErrorResponse,
    },
    /// A non-mutation request lost its response during connection loss.
    RequestLost(RequestId),
    /// The server explicitly supports no compatible local API version.
    IncompatibleVersion,
}

/// Pure actions and semantic events produced by one state transition.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClientTransition {
    /// Ordered adapter actions.
    pub actions: Vec<ClientAction>,
    /// Ordered consumer-visible semantic events.
    pub events: Vec<ClientEvent>,
}

/// Closed client state or input failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientError {
    /// Reconnect delays were zero or reversed.
    InvalidReconnectPolicy,
    /// Completed-command identity retention must have positive capacity.
    ZeroIdentityHistory,
    /// The client was started more than once without a terminal reset.
    AlreadyStarted,
    /// A stable command ID was reused with a changed exact request digest.
    ChangedCommandIdentity,
    /// The bounded in-flight mutation capacity was exhausted.
    MutationCapacity,
    /// A frame failed strict local API v1 encoding or decoding.
    Codec,
    /// A current connection delivered a message outside the client protocol state.
    ProtocolOrder,
    /// The request-correlation or connection-generation space was exhausted.
    IdentityExhausted,
    /// Subscription topics did not satisfy the closed v1 bounds.
    InvalidSubscription,
    /// An ordinary request was submitted without an active negotiated connection.
    NotConnected,
    /// Mutation, subscription, and refresh requests require their dedicated client methods.
    ReservedRequest,
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "reconnecting local client failed: {self:?}")
    }
}

impl Error for ClientError {}

#[derive(Clone, Debug)]
struct PendingMutation {
    request_id: RequestId,
    digest: CommandDigest,
    frame: Vec<u8>,
}

#[derive(Clone, Debug)]
struct SubscriptionIntent {
    seed: Id32,
    topics: Vec<InvalidationTopic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingRequest {
    Subscription(Id32),
    Snapshot,
    Ordinary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Connecting(ConnectionGeneration),
    Negotiating(ConnectionGeneration),
    Active(ConnectionGeneration),
    Incompatible(ConnectionGeneration),
}

/// Shared reconnecting client state used by every local frontend.
#[derive(Debug)]
pub struct ReconnectingClient {
    build: BuildMetadata,
    reconnect: ReconnectPolicy,
    phase: Phase,
    next_generation: u64,
    failures: u32,
    next_request_id: u64,
    pending_mutations: BTreeMap<CommandId, PendingMutation>,
    completed_digests: BTreeMap<CommandId, CommandDigest>,
    completed_order: VecDeque<CommandId>,
    completed_capacity: usize,
    pending_requests: BTreeMap<RequestId, PendingRequest>,
    subscription: Option<SubscriptionIntent>,
    active_subscription_id: Option<Id32>,
    current_revision: Option<u64>,
    newest_invalidation: u64,
    view_current: bool,
    refresh_in_flight: bool,
}

impl ReconnectingClient {
    /// Constructs an idle client with explicit build, backoff, and bounded identity history.
    pub fn new(
        build: BuildMetadata,
        reconnect: ReconnectPolicy,
        completed_identity_capacity: usize,
    ) -> Result<Self, ClientError> {
        if completed_identity_capacity == 0 {
            return Err(ClientError::ZeroIdentityHistory);
        }
        Ok(Self {
            build,
            reconnect,
            phase: Phase::Idle,
            next_generation: 1,
            failures: 0,
            next_request_id: 1,
            pending_mutations: BTreeMap::new(),
            completed_digests: BTreeMap::new(),
            completed_order: VecDeque::new(),
            completed_capacity: completed_identity_capacity,
            pending_requests: BTreeMap::new(),
            subscription: None,
            active_subscription_id: None,
            current_revision: None,
            newest_invalidation: 0,
            view_current: false,
            refresh_in_flight: false,
        })
    }

    /// Configures one logical broad-topic subscription before the client starts.
    pub fn configure_subscription(
        &mut self,
        seed: Id32,
        topics: Vec<InvalidationTopic>,
    ) -> Result<(), ClientError> {
        if self.phase != Phase::Idle {
            return Err(ClientError::AlreadyStarted);
        }
        SubscriptionRequestDto::new(seed, topics.clone())
            .map_err(|_| ClientError::InvalidSubscription)?;
        self.subscription = Some(SubscriptionIntent { seed, topics });
        Ok(())
    }

    /// Starts the initial immediate connection attempt.
    pub fn start(&mut self) -> Result<ClientTransition, ClientError> {
        if self.phase != Phase::Idle {
            return Err(ClientError::AlreadyStarted);
        }
        self.schedule_connection(Duration::ZERO)
    }

    /// Reports that the adapter opened the named generation and requests a fresh hello write.
    pub fn connected(
        &mut self,
        generation: ConnectionGeneration,
    ) -> Result<ClientTransition, ClientError> {
        if self.phase != Phase::Connecting(generation) {
            return Ok(ClientTransition::default());
        }
        self.phase = Phase::Negotiating(generation);
        let versions = VersionRange::new(V1, V1).map_err(|_| ClientError::Codec)?;
        let frame = WireMessage::ClientHello(ClientHello::new(versions, self.build.clone()))
            .encode_frame()
            .map_err(|_| ClientError::Codec)?;
        Ok(write_transition(generation, frame))
    }

    /// Reports a failed connection attempt and schedules the next capped generation.
    pub fn connection_failed(
        &mut self,
        generation: ConnectionGeneration,
    ) -> Result<ClientTransition, ClientError> {
        if self.current_generation() != Some(generation)
            || matches!(self.phase, Phase::Incompatible(_))
        {
            return Ok(ClientTransition::default());
        }
        self.prepare_reconnect()
    }

    /// Reports connection loss; in-flight mutations remain byte-identical for replay.
    pub fn disconnected(
        &mut self,
        generation: ConnectionGeneration,
    ) -> Result<ClientTransition, ClientError> {
        if self.current_generation() != Some(generation) {
            return Ok(ClientTransition::default());
        }
        if matches!(self.phase, Phase::Incompatible(_)) {
            return Ok(ClientTransition::default());
        }
        self.prepare_reconnect()
    }

    /// Strictly decodes and applies one frame from the named connection generation.
    pub fn receive_frame(
        &mut self,
        generation: ConnectionGeneration,
        frame: &[u8],
    ) -> Result<ClientTransition, ClientError> {
        if self.current_generation() != Some(generation) {
            return Ok(ClientTransition::default());
        }
        let message = WireMessage::decode_frame(frame).map_err(map_decode_error)?;
        match self.phase {
            Phase::Negotiating(current) if current == generation => {
                self.handle_negotiation(generation, message)
            }
            Phase::Active(current) if current == generation => {
                self.handle_active(generation, message)
            }
            Phase::Idle
            | Phase::Connecting(_)
            | Phase::Negotiating(_)
            | Phase::Active(_)
            | Phase::Incompatible(_) => Err(ClientError::ProtocolOrder),
        }
    }

    /// Queues or sends one exact mutation, rejecting changed reuse before transport work.
    pub fn submit_mutation(
        &mut self,
        request: MutationRequest,
    ) -> Result<ClientTransition, ClientError> {
        let command_id = request.command_id();
        let digest = request.request_digest();
        if self
            .pending_mutations
            .get(&command_id)
            .is_some_and(|pending| pending.digest != digest)
            || self
                .completed_digests
                .get(&command_id)
                .is_some_and(|completed| *completed != digest)
        {
            return Err(ClientError::ChangedCommandIdentity);
        }
        if self.pending_mutations.contains_key(&command_id)
            || self.completed_digests.contains_key(&command_id)
        {
            return Ok(ClientTransition::default());
        }
        if self.pending_mutations.len() >= MAX_IN_FLIGHT_MUTATIONS {
            return Err(ClientError::MutationCapacity);
        }
        let request_id = self.allocate_request_id()?;
        let frame =
            WireMessage::Request(RequestEnvelope::new(request_id, Request::Mutation(request)))
                .encode_frame()
                .map_err(|_| ClientError::Codec)?;
        self.pending_mutations.insert(
            command_id,
            PendingMutation {
                request_id,
                digest,
                frame: frame.clone(),
            },
        );
        let actions = self
            .active_generation()
            .map_or_else(Vec::new, |generation| {
                vec![ClientAction::Write { generation, frame }]
            });
        Ok(ClientTransition {
            actions,
            events: Vec::new(),
        })
    }

    /// Sends one ordinary typed request through the shared correlation and loss path.
    pub fn submit_request(&mut self, request: Request) -> Result<ClientTransition, ClientError> {
        if matches!(
            request,
            Request::Mutation(_) | Request::Subscribe(_) | Request::AuthoritativeSnapshot
        ) {
            return Err(ClientError::ReservedRequest);
        }
        let generation = self.active_generation().ok_or(ClientError::NotConnected)?;
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(request_id, request)?;
        self.pending_requests
            .insert(request_id, PendingRequest::Ordinary);
        Ok(write_transition(generation, frame))
    }

    /// Returns the currently scheduled or connected generation.
    pub const fn current_generation(&self) -> Option<ConnectionGeneration> {
        match self.phase {
            Phase::Idle => None,
            Phase::Connecting(generation)
            | Phase::Negotiating(generation)
            | Phase::Active(generation)
            | Phase::Incompatible(generation) => Some(generation),
        }
    }

    /// Returns the connection-specific subscription identity after negotiation.
    pub const fn active_subscription_id(&self) -> Option<Id32> {
        self.active_subscription_id
    }

    /// Reports whether an acknowledged snapshot covers every observed invalidation.
    pub const fn view_is_current(&self) -> bool {
        self.view_current
    }

    /// Returns the bounded number of retained completed command identities.
    pub fn completed_identity_count(&self) -> usize {
        self.completed_digests.len()
    }

    fn handle_negotiation(
        &mut self,
        generation: ConnectionGeneration,
        message: WireMessage,
    ) -> Result<ClientTransition, ClientError> {
        match message {
            WireMessage::ServerHello(hello) if hello.selected_version == V1 => {
                self.phase = Phase::Active(generation);
                self.pending_requests.clear();
                self.active_subscription_id = None;
                self.current_revision = None;
                self.newest_invalidation = 0;
                self.view_current = false;
                self.refresh_in_flight = false;
                let mut transition = ClientTransition::default();
                if let Some(subscription) = self.subscription.clone() {
                    let subscription_id =
                        derive_subscription_id(subscription.seed, hello.session_id);
                    self.active_subscription_id = Some(subscription_id);
                    let request_id = self.allocate_request_id()?;
                    let request = SubscriptionRequestDto::new(subscription_id, subscription.topics)
                        .map_err(|_| ClientError::InvalidSubscription)?;
                    let frame = request_frame(request_id, Request::Subscribe(request))?;
                    self.pending_requests
                        .insert(request_id, PendingRequest::Subscription(subscription_id));
                    transition
                        .actions
                        .push(ClientAction::Write { generation, frame });
                } else {
                    transition
                        .actions
                        .push(self.begin_snapshot_refresh(generation)?);
                }
                transition
                    .actions
                    .extend(
                        self.pending_mutations
                            .values()
                            .map(|pending| ClientAction::Write {
                                generation,
                                frame: pending.frame.clone(),
                            }),
                    );
                Ok(transition)
            }
            WireMessage::VersionRejected(_) => {
                self.phase = Phase::Incompatible(generation);
                Ok(ClientTransition {
                    actions: vec![ClientAction::Close { generation }],
                    events: vec![ClientEvent::IncompatibleVersion],
                })
            }
            WireMessage::ServerHello(_)
            | WireMessage::ClientHello(_)
            | WireMessage::Request(_)
            | WireMessage::Response(_)
            | WireMessage::Invalidation(_) => Err(ClientError::ProtocolOrder),
        }
    }

    fn handle_active(
        &mut self,
        generation: ConnectionGeneration,
        message: WireMessage,
    ) -> Result<ClientTransition, ClientError> {
        match message {
            WireMessage::Response(response) => self.handle_response(generation, response),
            WireMessage::Invalidation(invalidation) => {
                if Some(invalidation.subscription_id) != self.active_subscription_id {
                    return Ok(ClientTransition::default());
                }
                self.newest_invalidation = self.newest_invalidation.max(invalidation.revision);
                if self.current_revision.is_none() {
                    return Ok(ClientTransition::default());
                }
                if self
                    .current_revision
                    .is_some_and(|revision| invalidation.revision > revision)
                {
                    self.view_current = false;
                    if !self.refresh_in_flight {
                        return Ok(ClientTransition {
                            actions: vec![self.begin_snapshot_refresh(generation)?],
                            events: Vec::new(),
                        });
                    }
                }
                Ok(ClientTransition::default())
            }
            WireMessage::ClientHello(_)
            | WireMessage::ServerHello(_)
            | WireMessage::VersionRejected(_)
            | WireMessage::Request(_) => Err(ClientError::ProtocolOrder),
        }
    }

    fn handle_response(
        &mut self,
        generation: ConnectionGeneration,
        response: crate::protocol::v1::ResponseEnvelope,
    ) -> Result<ClientTransition, ClientError> {
        let mutation_command = self
            .pending_mutations
            .iter()
            .find_map(|(command_id, pending)| {
                (pending.request_id == response.id).then_some(*command_id)
            });
        match response.response {
            Response::Error(error) => {
                self.handle_error_response(generation, response.id, error, mutation_command)
            }
            Response::Success(result) => match result {
                ResponseResult::Mutation(attempt) => {
                    let Some(command_id) = mutation_command else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    if !mutation_matches(
                        &attempt,
                        command_id,
                        self.pending_mutations[&command_id].digest,
                    ) {
                        return Err(ClientError::ProtocolOrder);
                    }
                    let mut transition = ClientTransition {
                        actions: Vec::new(),
                        events: vec![ClientEvent::Mutation(attempt.clone())],
                    };
                    match attempt {
                        MutationAttemptDto::Completed { .. } => {
                            let pending = self
                                .pending_mutations
                                .remove(&command_id)
                                .ok_or(ClientError::ProtocolOrder)?;
                            self.remember_completed(command_id, pending.digest);
                        }
                        MutationAttemptDto::Uncertain { .. } => {
                            transition.actions.push(ClientAction::Close { generation });
                            let reconnect = self.prepare_reconnect()?;
                            transition.actions.extend(reconnect.actions);
                            transition.events.extend(reconnect.events);
                        }
                    }
                    Ok(transition)
                }
                ResponseResult::Subscription(acknowledgement) => {
                    let Some(PendingRequest::Subscription(expected)) =
                        self.pending_requests.remove(&response.id)
                    else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    if acknowledgement.subscription_id != expected
                        || Some(expected) != self.active_subscription_id
                    {
                        return Err(ClientError::ProtocolOrder);
                    }
                    self.accept_snapshot(generation, &acknowledgement.snapshot)
                }
                ResponseResult::AuthoritativeSnapshot(snapshot) => {
                    if self.pending_requests.remove(&response.id) != Some(PendingRequest::Snapshot)
                    {
                        return Err(ClientError::ProtocolOrder);
                    }
                    self.refresh_in_flight = false;
                    self.accept_snapshot(generation, &snapshot)
                }
                ResponseResult::Lifecycle(_)
                | ResponseResult::ConversationPage(_)
                | ResponseResult::EmptyEffect(_)
                | ResponseResult::AgentSession(_)
                | ResponseResult::ResourceInspection(_)
                | ResponseResult::Empty => {
                    if self.pending_requests.remove(&response.id) != Some(PendingRequest::Ordinary)
                    {
                        return Err(ClientError::ProtocolOrder);
                    }
                    Ok(ClientTransition {
                        actions: Vec::new(),
                        events: vec![ClientEvent::Response {
                            request_id: response.id,
                            result,
                        }],
                    })
                }
            },
        }
    }

    fn handle_error_response(
        &mut self,
        generation: ConnectionGeneration,
        request_id: RequestId,
        error: ErrorResponse,
        mutation_command: Option<CommandId>,
    ) -> Result<ClientTransition, ClientError> {
        if let Some(command_id) = mutation_command {
            if let Some(pending) = self.pending_mutations.remove(&command_id) {
                self.remember_completed(command_id, pending.digest);
            }
            return Ok(error_transition(request_id, error));
        }

        let pending = self
            .pending_requests
            .remove(&request_id)
            .ok_or(ClientError::ProtocolOrder)?;
        let mut transition = error_transition(request_id, error);
        if matches!(
            pending,
            PendingRequest::Subscription(_) | PendingRequest::Snapshot
        ) {
            transition.actions.push(ClientAction::Close { generation });
            let reconnect = self.prepare_reconnect()?;
            transition.actions.extend(reconnect.actions);
            transition.events.extend(reconnect.events);
        }
        Ok(transition)
    }

    fn accept_snapshot(
        &mut self,
        generation: ConnectionGeneration,
        snapshot: &AuthoritativeSnapshotDto,
    ) -> Result<ClientTransition, ClientError> {
        self.current_revision = Some(snapshot.revision);
        self.failures = 0;
        let mut transition = ClientTransition {
            actions: Vec::new(),
            events: vec![ClientEvent::Snapshot(snapshot.clone())],
        };
        if self.newest_invalidation > snapshot.revision {
            self.view_current = false;
            if !self.refresh_in_flight {
                transition
                    .actions
                    .push(self.begin_snapshot_refresh(generation)?);
            }
        } else {
            self.view_current = true;
        }
        Ok(transition)
    }

    fn begin_snapshot_refresh(
        &mut self,
        generation: ConnectionGeneration,
    ) -> Result<ClientAction, ClientError> {
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(request_id, Request::AuthoritativeSnapshot)?;
        self.pending_requests
            .insert(request_id, PendingRequest::Snapshot);
        self.refresh_in_flight = true;
        Ok(ClientAction::Write { generation, frame })
    }

    fn prepare_reconnect(&mut self) -> Result<ClientTransition, ClientError> {
        let lost = self
            .pending_requests
            .iter()
            .filter_map(|(request_id, pending)| {
                (*pending == PendingRequest::Ordinary)
                    .then_some(ClientEvent::RequestLost(*request_id))
            })
            .collect::<Vec<_>>();
        self.pending_requests.clear();
        self.active_subscription_id = None;
        self.current_revision = None;
        self.newest_invalidation = 0;
        self.view_current = false;
        self.refresh_in_flight = false;
        let delay = self.reconnect.delay(self.failures);
        self.failures = self.failures.saturating_add(1);
        let mut transition = self.schedule_connection(delay)?;
        transition.events = lost;
        Ok(transition)
    }

    fn schedule_connection(&mut self, delay: Duration) -> Result<ClientTransition, ClientError> {
        let generation = NonZeroU64::new(self.next_generation)
            .map(ConnectionGeneration)
            .ok_or(ClientError::IdentityExhausted)?;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(ClientError::IdentityExhausted)?;
        self.phase = Phase::Connecting(generation);
        Ok(ClientTransition {
            actions: vec![ClientAction::ConnectAfter { generation, delay }],
            events: Vec::new(),
        })
    }

    fn allocate_request_id(&mut self) -> Result<RequestId, ClientError> {
        let id =
            RequestId::new(self.next_request_id).map_err(|_| ClientError::IdentityExhausted)?;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(ClientError::IdentityExhausted)?;
        Ok(id)
    }

    fn remember_completed(&mut self, command_id: CommandId, digest: CommandDigest) {
        if self.completed_digests.insert(command_id, digest).is_none() {
            self.completed_order.push_back(command_id);
        }
        while self.completed_order.len() > self.completed_capacity {
            if let Some(oldest) = self.completed_order.pop_front() {
                self.completed_digests.remove(&oldest);
            }
        }
    }

    const fn active_generation(&self) -> Option<ConnectionGeneration> {
        match self.phase {
            Phase::Active(generation) => Some(generation),
            Phase::Idle | Phase::Connecting(_) | Phase::Negotiating(_) | Phase::Incompatible(_) => {
                None
            }
        }
    }
}

fn request_frame(request_id: RequestId, request: Request) -> Result<Vec<u8>, ClientError> {
    WireMessage::Request(RequestEnvelope::new(request_id, request))
        .encode_frame()
        .map_err(|_| ClientError::Codec)
}

fn write_transition(generation: ConnectionGeneration, frame: Vec<u8>) -> ClientTransition {
    ClientTransition {
        actions: vec![ClientAction::Write { generation, frame }],
        events: Vec::new(),
    }
}

fn error_transition(request_id: RequestId, error: ErrorResponse) -> ClientTransition {
    ClientTransition {
        actions: Vec::new(),
        events: vec![ClientEvent::Error { request_id, error }],
    }
}

fn derive_subscription_id(seed: Id32, server_session: Id32) -> Id32 {
    let mut digest = Sha256::new();
    digest.update(SUBSCRIPTION_ID_DOMAIN);
    digest.update(seed.bytes());
    digest.update(server_session.bytes());
    Id32::new(digest.finalize().into())
}

fn mutation_matches(
    attempt: &MutationAttemptDto,
    command_id: CommandId,
    digest: CommandDigest,
) -> bool {
    match attempt {
        MutationAttemptDto::Completed {
            command_id: actual_command,
            request_digest,
            ..
        }
        | MutationAttemptDto::Uncertain {
            command_id: actual_command,
            request_digest,
        } => {
            actual_command.bytes() == *command_id.as_bytes()
                && request_digest.bytes() == *digest.as_bytes()
        }
    }
}

const fn map_decode_error(_error: DecodeError) -> ClientError {
    ClientError::Codec
}
