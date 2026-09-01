//! Pure reconnecting local client state machine and transport adapter contract.

use std::{
    collections::{BTreeMap, VecDeque},
    error::Error,
    fmt,
    num::{NonZeroU64, NonZeroUsize},
    time::{Duration, Instant},
};

use hq_domain::{CommandDigest, CommandId, OperationId};
use sha2::{Digest, Sha256};

use crate::protocol::v1::{
    AgentRetirementOutcomeDto, AgentRetirementRequestDto, AgentSessionRequestDto,
    AgentSessionResultDto, AuthoritativeConversationViewDto,
    AuthoritativeConversationViewRequestDto, AuthoritativeSnapshotDto, BuildMetadata, ClientHello,
    ConversationPageSelectionDto, DecodeError, EffectOutcomeDto, EffectRequestDto, ErrorResponse,
    Id32, InvalidationTopic, MailboxCommandRequestDto, MutationAttemptDto, MutationRequest,
    ProjectCommandOutcomeDto, ProjectCommandRequestDto, Request, RequestEnvelope, RequestId,
    Response, ResponseResult, SubscriptionRequestDto, V1, VersionRange, WireMessage,
    agent_session_request_digest,
};

/// Maximum simultaneous exact retryable frames retained for response-loss replay.
pub const MAX_IN_FLIGHT_RETRYABLE_COMMANDS: usize = 256;

const SUBSCRIPTION_ID_DOMAIN: &[u8] = b"hq-local-api-client-subscription-v1\0";
const RESPONDER_ID_DOMAIN: &[u8] = b"hq-local-api-client-interaction-responder-v1\0";

/// Initial authoritative-view behavior selected for one reconnecting client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialView {
    /// Load an authoritative snapshot after every successful negotiation.
    Snapshot,
    /// Negotiate without loading state until an explicit request needs it.
    OnDemand,
}

/// Narrow blocking transport operations used by a runner around the pure state machine.
pub trait ClientTransport {
    /// Adapter-owned live connection handle.
    type Connection;
    /// Adapter-owned redacted transport failure.
    type Error: Error;

    /// Opens one local transport connection.
    fn connect(&mut self) -> Result<Self::Connection, Self::Error>;
    /// Writes one complete exact frame.
    fn write(
        &mut self,
        connection: &mut Self::Connection,
        frame: &[u8],
        timeout: Duration,
    ) -> Result<(), Self::Error>;
    /// Reads one complete bounded exact frame.
    fn read_frame(
        &mut self,
        connection: &mut Self::Connection,
        timeout: Duration,
    ) -> Result<Vec<u8>, Self::Error>;
    /// Polls for one complete frame, returning `None` when the bounded wait elapsed normally.
    ///
    /// Blocking command transports may use the default implementation. Interactive transports
    /// override this method so an idle socket is distinct from a disconnected socket.
    fn poll_frame(
        &mut self,
        connection: &mut Self::Connection,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        self.read_frame(connection, timeout).map(Some)
    }
    /// Closes one connection idempotently.
    fn close(&mut self, connection: Self::Connection);
    /// Waits for one deterministic reconnect delay.
    fn wait(&mut self, delay: Duration);
}

/// Passive bounds for one synchronous local command execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockingClientConfig {
    /// Inclusive wall-time bound for one execution call.
    pub deadline: Duration,
    /// Maximum connection attempts across initial connect and retries.
    pub max_connection_attempts: NonZeroUsize,
}

/// Closed synchronous runner failure without transport implementation prose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockingClientError {
    /// The configured wall-time bound was zero.
    InvalidDeadline,
    /// The pure reconnecting client rejected a transition or request.
    Client(ClientError),
    /// The command exceeded its explicit wall-time bound.
    Deadline,
    /// The bounded connection-attempt budget was exhausted.
    ConnectionAttemptsExhausted,
    /// An ordinary request lost its response and was not replayed.
    ResponseLost,
    /// The server supports no compatible local protocol version.
    Incompatible,
}

impl fmt::Display for BlockingClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "blocking local client failed: {self:?}")
    }
}

impl Error for BlockingClientError {}

/// Opaque synchronous owner that drives one pure reconnecting client over an injected transport.
pub struct BlockingClientRunner<T: ClientTransport> {
    config: BlockingClientConfig,
    client: ReconnectingClient,
    transport: T,
    connection: Option<(ConnectionGeneration, T::Connection)>,
    actions: VecDeque<QueuedClientAction>,
    events: VecDeque<ClientEvent>,
    connection_attempts: usize,
    response_pending: bool,
}

struct QueuedClientAction {
    action: ClientAction,
    not_before: Instant,
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

/// Closed observable connection phase for long-lived client shells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientConnectionState {
    /// No connection attempt has started.
    Idle,
    /// A delayed or immediate connection attempt is pending.
    Connecting(ConnectionGeneration),
    /// A transport is open and version negotiation is pending.
    Negotiating(ConnectionGeneration),
    /// The current generation completed negotiation.
    Active(ConnectionGeneration),
    /// The current generation has no compatible local API version.
    Incompatible(ConnectionGeneration),
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
    /// A fresh subscribed snapshot and optional selected conversation page became available.
    AuthoritativeConversationView(AuthoritativeConversationViewDto),
    /// A stable mutation completed or remains explicitly uncertain.
    Mutation(MutationAttemptDto),
    /// A retry-safe project command returned typed durable progress.
    ProjectCommand {
        /// Stable command identity from the submitted request.
        command_id: CommandId,
        /// Typed workflow result or checkpoint.
        outcome: ProjectCommandOutcomeDto,
    },
    /// A retry-safe named-agent retirement returned typed durable progress.
    AgentRetirement {
        /// Stable command identity from the submitted request.
        command_id: CommandId,
        /// Typed workflow result or checkpoint.
        outcome: AgentRetirementOutcomeDto,
    },
    /// A retry-safe managed-session operation returned typed durable progress.
    AgentSession {
        /// Stable external operation identity from the submitted request.
        operation_id: OperationId,
        /// Definite or explicitly uncertain runtime outcome.
        outcome: EffectOutcomeDto<AgentSessionResultDto>,
    },
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
        /// Semantic operation whose correlated response failed.
        operation: ClientOperation,
        /// Stable typed failure.
        error: ErrorResponse,
    },
    /// A non-mutation request lost its response during connection loss.
    RequestLost(RequestId),
    /// The server explicitly supports no compatible local API version.
    IncompatibleVersion,
}

/// Semantic origin of a correlated server error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientOperation {
    /// Automatic or invalidation-triggered authoritative snapshot refresh.
    Snapshot,
    /// Automatic or selection-triggered subscribed materialized-view refresh.
    AuthoritativeConversationView,
    /// Broad invalidation subscription registration.
    Subscription,
    /// Caller-submitted ordinary request.
    Ordinary,
    /// Retry-safe fact mutation.
    Mutation(CommandId),
    /// Retry-safe durable project command.
    Project(CommandId),
    /// Retry-safe node-owned named-agent retirement.
    AgentRetirement(CommandId),
    /// Retry-safe managed named-agent session control.
    AgentSession(OperationId),
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
    /// The bounded in-flight retryable-command capacity was exhausted.
    RetryableCommandCapacity,
    /// A frame failed strict local API v1 encoding or decoding.
    Codec,
    /// A current connection delivered a message outside the client protocol state.
    ProtocolOrder,
    /// The request-correlation or connection-generation space was exhausted.
    IdentityExhausted,
    /// Subscription topics did not satisfy the closed v1 bounds.
    InvalidSubscription,
    /// A managed-session body or its exact request digest is invalid.
    InvalidAgentSessionRequest,
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
struct PendingProjectCommand {
    request_id: RequestId,
    digest: CommandDigest,
    frame: Vec<u8>,
}

#[derive(Clone, Debug)]
struct PendingAgentRetirement {
    request_id: RequestId,
    digest: CommandDigest,
    frame: Vec<u8>,
}

#[derive(Clone, Debug)]
struct PendingAgentSession {
    request_id: RequestId,
    digest: CommandDigest,
    frame: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct RetryableResponseIdentity {
    mutation: Option<CommandId>,
    project: Option<CommandId>,
    retirement: Option<CommandId>,
    session: Option<OperationId>,
}

#[derive(Clone, Debug)]
struct SubscriptionIntent {
    seed: Id32,
    topics: Vec<InvalidationTopic>,
    conversation: Option<ConversationPageSelectionDto>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingRequest {
    Subscription {
        subscription_id: Id32,
        conversation: Option<ConversationPageSelectionDto>,
    },
    InteractionResponder {
        responder_id: Id32,
    },
    Interactions,
    Snapshot,
    AuthoritativeConversationView {
        conversation: Option<ConversationPageSelectionDto>,
    },
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
    pending_project_commands: BTreeMap<CommandId, PendingProjectCommand>,
    pending_agent_retirements: BTreeMap<CommandId, PendingAgentRetirement>,
    pending_agent_sessions: BTreeMap<OperationId, PendingAgentSession>,
    completed_digests: BTreeMap<CommandId, CommandDigest>,
    completed_order: VecDeque<CommandId>,
    completed_capacity: usize,
    pending_requests: BTreeMap<RequestId, PendingRequest>,
    subscription: Option<SubscriptionIntent>,
    responder_seed: Option<Id32>,
    initial_view: InitialView,
    active_subscription_id: Option<Id32>,
    active_responder_id: Option<Id32>,
    current_revision: Option<u64>,
    newest_invalidation: u64,
    view_current: bool,
    refresh_in_flight: bool,
    interactions_refresh_in_flight: bool,
}

impl ReconnectingClient {
    /// Constructs an idle client with explicit build, backoff, and bounded identity history.
    pub fn new(
        build: BuildMetadata,
        reconnect: ReconnectPolicy,
        completed_identity_capacity: usize,
        initial_view: InitialView,
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
            pending_project_commands: BTreeMap::new(),
            pending_agent_retirements: BTreeMap::new(),
            pending_agent_sessions: BTreeMap::new(),
            completed_digests: BTreeMap::new(),
            completed_order: VecDeque::new(),
            completed_capacity: completed_identity_capacity,
            pending_requests: BTreeMap::new(),
            subscription: None,
            responder_seed: None,
            initial_view,
            active_subscription_id: None,
            active_responder_id: None,
            current_revision: None,
            newest_invalidation: 0,
            view_current: false,
            refresh_in_flight: false,
            interactions_refresh_in_flight: false,
        })
    }

    /// Configures one logical broad-topic subscription before the client starts.
    pub fn configure_subscription(
        &mut self,
        seed: Id32,
        topics: Vec<InvalidationTopic>,
    ) -> Result<(), ClientError> {
        self.configure_subscription_view(seed, topics, None)
    }

    /// Configures one logical subscription and its initial selected conversation before start.
    pub fn configure_subscription_view(
        &mut self,
        seed: Id32,
        topics: Vec<InvalidationTopic>,
        conversation: Option<ConversationPageSelectionDto>,
    ) -> Result<(), ClientError> {
        if self.phase != Phase::Idle {
            return Err(ClientError::AlreadyStarted);
        }
        SubscriptionRequestDto::new(seed, topics.clone(), conversation.clone())
            .map_err(|_| ClientError::InvalidSubscription)?;
        self.subscription = Some(SubscriptionIntent {
            seed,
            topics,
            conversation,
        });
        Ok(())
    }

    /// Configures one reconnecting session-scoped interaction responder before startup.
    pub fn configure_interaction_responder(&mut self, seed: Id32) -> Result<(), ClientError> {
        if self.phase != Phase::Idle {
            return Err(ClientError::AlreadyStarted);
        }
        if seed == Id32::new([0; 32]) {
            return Err(ClientError::InvalidSubscription);
        }
        self.responder_seed = Some(seed);
        Ok(())
    }

    /// Replaces the latest selected-conversation interest on one logical subscription.
    pub fn update_subscription_conversation(
        &mut self,
        conversation: Option<ConversationPageSelectionDto>,
    ) -> Result<ClientTransition, ClientError> {
        let Some(current) = self.subscription.as_ref() else {
            return Err(ClientError::InvalidSubscription);
        };
        SubscriptionRequestDto::new(current.seed, current.topics.clone(), conversation.clone())
            .map_err(|_| ClientError::InvalidSubscription)?;
        if current.conversation == conversation {
            return Ok(ClientTransition::default());
        }
        self.subscription
            .as_mut()
            .ok_or(ClientError::InvalidSubscription)?
            .conversation = conversation;
        self.view_current = false;
        let Some(generation) = self.active_generation() else {
            return Ok(ClientTransition::default());
        };
        if self.active_subscription_id.is_none() || self.refresh_in_flight {
            return Ok(ClientTransition::default());
        }
        Ok(ClientTransition {
            actions: vec![self.begin_authoritative_conversation_view_refresh(generation)?],
            events: Vec::new(),
        })
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
        if self.retryable_identity_exists(command_id, digest)? {
            return Ok(ClientTransition::default());
        }
        if self.retryable_command_count() >= MAX_IN_FLIGHT_RETRYABLE_COMMANDS {
            return Err(ClientError::RetryableCommandCapacity);
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

    /// Queues or sends one exact mailbox command and retains it across response loss.
    pub fn submit_mailbox_command(
        &mut self,
        request: MailboxCommandRequestDto,
    ) -> Result<ClientTransition, ClientError> {
        let command_id = request.command_id();
        let digest = request.request_digest();
        if self.retryable_identity_exists(command_id, digest)? {
            return Ok(ClientTransition::default());
        }
        if self.retryable_command_count() >= MAX_IN_FLIGHT_RETRYABLE_COMMANDS {
            return Err(ClientError::RetryableCommandCapacity);
        }
        let request_id = self.allocate_request_id()?;
        let frame = WireMessage::Request(RequestEnvelope::new(
            request_id,
            Request::ControlMailbox(Box::new(request)),
        ))
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

    /// Queues or sends one exact project command and retains its frame across response loss.
    pub fn submit_project_command(
        &mut self,
        request: ProjectCommandRequestDto,
    ) -> Result<ClientTransition, ClientError> {
        let command_id = CommandId::from_bytes(request.command_id.bytes());
        let digest = CommandDigest::from_bytes(request.request_digest.bytes());
        if self.retryable_identity_exists(command_id, digest)? {
            return Ok(ClientTransition::default());
        }
        if self.retryable_command_count() >= MAX_IN_FLIGHT_RETRYABLE_COMMANDS {
            return Err(ClientError::RetryableCommandCapacity);
        }
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(request_id, Request::ControlProject(Box::new(request)))?;
        self.pending_project_commands.insert(
            command_id,
            PendingProjectCommand {
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

    /// Queues or sends one exact named-agent retirement and retains its frame across response loss.
    pub fn submit_agent_retirement(
        &mut self,
        request: AgentRetirementRequestDto,
    ) -> Result<ClientTransition, ClientError> {
        let command_id = CommandId::from_bytes(request.command_id.bytes());
        let digest = CommandDigest::from_bytes(request.request_digest.bytes());
        if self.retryable_identity_exists(command_id, digest)? {
            return Ok(ClientTransition::default());
        }
        if self.retryable_command_count() >= MAX_IN_FLIGHT_RETRYABLE_COMMANDS {
            return Err(ClientError::RetryableCommandCapacity);
        }
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(request_id, Request::RetireAgent(Box::new(request)))?;
        self.pending_agent_retirements.insert(
            command_id,
            PendingAgentRetirement {
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

    /// Queues or sends exact managed-session control and retains its frame across response loss.
    pub fn submit_agent_session(
        &mut self,
        request: EffectRequestDto<AgentSessionRequestDto>,
    ) -> Result<ClientTransition, ClientError> {
        let operation_id = OperationId::from_bytes(request.operation_id.bytes());
        let identity = CommandId::from_bytes(request.operation_id.bytes());
        let digest = CommandDigest::from_bytes(request.request_digest.bytes());
        let computed = agent_session_request_digest(&request)
            .map_err(|_| ClientError::InvalidAgentSessionRequest)?;
        if computed != digest {
            return Err(ClientError::InvalidAgentSessionRequest);
        }
        if self.retryable_identity_exists(identity, digest)? {
            return Ok(ClientTransition::default());
        }
        if self.retryable_command_count() >= MAX_IN_FLIGHT_RETRYABLE_COMMANDS {
            return Err(ClientError::RetryableCommandCapacity);
        }
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(request_id, Request::ControlAgentSession(Box::new(request)))?;
        self.pending_agent_sessions.insert(
            operation_id,
            PendingAgentSession {
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

    fn retryable_identity_exists(
        &self,
        command_id: CommandId,
        digest: CommandDigest,
    ) -> Result<bool, ClientError> {
        let pending_digest = self
            .pending_mutations
            .get(&command_id)
            .map(|pending| pending.digest)
            .or_else(|| {
                self.pending_project_commands
                    .get(&command_id)
                    .map(|pending| pending.digest)
            })
            .or_else(|| {
                self.pending_agent_retirements
                    .get(&command_id)
                    .map(|pending| pending.digest)
            })
            .or_else(|| {
                self.pending_agent_sessions
                    .get(&OperationId::from_bytes(*command_id.as_bytes()))
                    .map(|pending| pending.digest)
            });
        let completed_digest = self.completed_digests.get(&command_id).copied();
        if pending_digest
            .into_iter()
            .chain(completed_digest)
            .any(|existing| existing != digest)
        {
            return Err(ClientError::ChangedCommandIdentity);
        }
        Ok(pending_digest.is_some() || completed_digest.is_some())
    }

    fn retryable_command_count(&self) -> usize {
        self.pending_mutations.len()
            + self.pending_project_commands.len()
            + self.pending_agent_retirements.len()
            + self.pending_agent_sessions.len()
    }

    /// Sends one ordinary typed request through the shared correlation and loss path.
    pub fn submit_request(&mut self, request: Request) -> Result<ClientTransition, ClientError> {
        if matches!(
            request,
            Request::Mutation(_)
                | Request::ControlMailbox(_)
                | Request::ControlProject(_)
                | Request::RetireAgent(_)
                | Request::ControlAgentSession(_)
                | Request::Subscribe(_)
                | Request::AuthoritativeSnapshot
                | Request::AuthoritativeConversationView(_)
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

    /// Requests one complete authoritative refresh on the current generation.
    pub fn refresh_snapshot(&mut self) -> Result<ClientTransition, ClientError> {
        let generation = self.active_generation().ok_or(ClientError::NotConnected)?;
        if self.refresh_in_flight {
            return Ok(ClientTransition::default());
        }
        self.view_current = false;
        Ok(ClientTransition {
            actions: vec![self.begin_refresh(generation)?],
            events: Vec::new(),
        })
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

    /// Returns the current generation-scoped connection phase.
    pub const fn connection_state(&self) -> ClientConnectionState {
        match self.phase {
            Phase::Idle => ClientConnectionState::Idle,
            Phase::Connecting(generation) => ClientConnectionState::Connecting(generation),
            Phase::Negotiating(generation) => ClientConnectionState::Negotiating(generation),
            Phase::Active(generation) => ClientConnectionState::Active(generation),
            Phase::Incompatible(generation) => ClientConnectionState::Incompatible(generation),
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

    /// Reports whether the current generation completed negotiation.
    pub const fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Active(_))
    }

    fn begin_responder_registration(
        &mut self,
        generation: ConnectionGeneration,
        server_session: Id32,
    ) -> Result<Option<ClientAction>, ClientError> {
        let Some(seed) = self.responder_seed else {
            return Ok(None);
        };
        let responder_id = derive_responder_id(seed, server_session);
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(
            request_id,
            Request::RegisterInteractionResponder { responder_id },
        )?;
        self.pending_requests.insert(
            request_id,
            PendingRequest::InteractionResponder { responder_id },
        );
        Ok(Some(ClientAction::Write { generation, frame }))
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
                self.active_responder_id = None;
                self.current_revision = None;
                self.newest_invalidation = 0;
                self.view_current = false;
                self.refresh_in_flight = false;
                self.interactions_refresh_in_flight = false;
                let mut transition = ClientTransition::default();
                if let Some(action) =
                    self.begin_responder_registration(generation, hello.session_id)?
                {
                    transition.actions.push(action);
                }
                if let Some(subscription) = self.subscription.clone() {
                    let subscription_id =
                        derive_subscription_id(subscription.seed, hello.session_id);
                    self.active_subscription_id = Some(subscription_id);
                    let request_id = self.allocate_request_id()?;
                    let conversation = subscription.conversation;
                    let request = SubscriptionRequestDto::new(
                        subscription_id,
                        subscription.topics,
                        conversation.clone(),
                    )
                    .map_err(|_| ClientError::InvalidSubscription)?;
                    let frame = request_frame(request_id, Request::Subscribe(request))?;
                    self.pending_requests.insert(
                        request_id,
                        PendingRequest::Subscription {
                            subscription_id,
                            conversation,
                        },
                    );
                    self.refresh_in_flight = true;
                    transition
                        .actions
                        .push(ClientAction::Write { generation, frame });
                } else if self.initial_view == InitialView::Snapshot {
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
                transition
                    .actions
                    .extend(self.pending_project_commands.values().map(|pending| {
                        ClientAction::Write {
                            generation,
                            frame: pending.frame.clone(),
                        }
                    }));
                transition
                    .actions
                    .extend(self.pending_agent_retirements.values().map(|pending| {
                        ClientAction::Write {
                            generation,
                            frame: pending.frame.clone(),
                        }
                    }));
                transition
                    .actions
                    .extend(self.pending_agent_sessions.values().map(|pending| {
                        ClientAction::Write {
                            generation,
                            frame: pending.frame.clone(),
                        }
                    }));
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
                let mut actions = Vec::new();
                if invalidation.topics.iter().any(|topic| {
                    matches!(
                        topic,
                        InvalidationTopic::All | InvalidationTopic::Operations
                    )
                }) && !self.interactions_refresh_in_flight
                {
                    actions.push(self.begin_interactions_refresh(generation)?);
                }
                if self.current_revision.is_none() {
                    return Ok(ClientTransition {
                        actions,
                        events: Vec::new(),
                    });
                }
                if self
                    .current_revision
                    .is_some_and(|revision| invalidation.revision > revision)
                {
                    self.view_current = false;
                    if !self.refresh_in_flight {
                        actions.push(self.begin_refresh(generation)?);
                    }
                }
                Ok(ClientTransition {
                    actions,
                    events: Vec::new(),
                })
            }
            WireMessage::ClientHello(_)
            | WireMessage::ServerHello(_)
            | WireMessage::VersionRejected(_)
            | WireMessage::Request(_) => Err(ClientError::ProtocolOrder),
        }
    }

    #[allow(clippy::too_many_lines)]
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
        let project_command =
            self.pending_project_commands
                .iter()
                .find_map(|(command_id, pending)| {
                    (pending.request_id == response.id).then_some(*command_id)
                });
        let retirement_command =
            self.pending_agent_retirements
                .iter()
                .find_map(|(command_id, pending)| {
                    (pending.request_id == response.id).then_some(*command_id)
                });
        let session_operation =
            self.pending_agent_sessions
                .iter()
                .find_map(|(operation_id, pending)| {
                    (pending.request_id == response.id).then_some(*operation_id)
                });
        let retryable = RetryableResponseIdentity {
            mutation: mutation_command,
            project: project_command,
            retirement: retirement_command,
            session: session_operation,
        };
        match response.response {
            Response::Error(error) => {
                self.handle_error_response(generation, response.id, error, retryable)
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
                    let Some(PendingRequest::Subscription {
                        subscription_id: expected,
                        conversation,
                    }) = self.pending_requests.remove(&response.id)
                    else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    if acknowledgement.subscription_id != expected
                        || Some(expected) != self.active_subscription_id
                        || !view_matches_conversation(&acknowledgement.view, conversation.as_ref())
                    {
                        return Err(ClientError::ProtocolOrder);
                    }
                    self.refresh_in_flight = false;
                    let desired = self
                        .subscription
                        .as_ref()
                        .and_then(|subscription| subscription.conversation.clone());
                    if conversation != desired {
                        self.current_revision = Some(acknowledgement.view.snapshot.revision);
                        self.view_current = false;
                        return Ok(ClientTransition {
                            actions: vec![
                                self.begin_authoritative_conversation_view_refresh(generation)?,
                            ],
                            events: Vec::new(),
                        });
                    }
                    self.accept_authoritative_conversation_view(generation, acknowledgement.view)
                }
                ResponseResult::InteractionResponder(acknowledgement) => {
                    let Some(PendingRequest::InteractionResponder {
                        responder_id: expected,
                    }) = self.pending_requests.remove(&response.id)
                    else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    if acknowledgement.responder_id != expected {
                        return Err(ClientError::ProtocolOrder);
                    }
                    self.active_responder_id = Some(expected);
                    let actions = if self.interactions_refresh_in_flight {
                        Vec::new()
                    } else {
                        vec![self.begin_interactions_refresh(generation)?]
                    };
                    Ok(ClientTransition {
                        actions,
                        events: vec![ClientEvent::Response {
                            request_id: response.id,
                            result: ResponseResult::InteractionResponder(acknowledgement),
                        }],
                    })
                }
                ResponseResult::AuthoritativeSnapshot(snapshot) => {
                    if self.pending_requests.remove(&response.id) != Some(PendingRequest::Snapshot)
                    {
                        return Err(ClientError::ProtocolOrder);
                    }
                    self.refresh_in_flight = false;
                    self.accept_snapshot(generation, &snapshot)
                }
                ResponseResult::AuthoritativeConversationView(view) => {
                    let Some(PendingRequest::AuthoritativeConversationView { conversation }) =
                        self.pending_requests.remove(&response.id)
                    else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    self.refresh_in_flight = false;
                    if !view_matches_conversation(&view, conversation.as_ref()) {
                        return Err(ClientError::ProtocolOrder);
                    }
                    let desired = self
                        .subscription
                        .as_ref()
                        .and_then(|subscription| subscription.conversation.clone());
                    if conversation != desired {
                        self.current_revision = Some(view.snapshot.revision);
                        self.view_current = false;
                        return Ok(ClientTransition {
                            actions: vec![
                                self.begin_authoritative_conversation_view_refresh(generation)?,
                            ],
                            events: Vec::new(),
                        });
                    }
                    self.accept_authoritative_conversation_view(generation, view)
                }
                ResponseResult::PendingInteractions(interactions) => {
                    if self.pending_requests.remove(&response.id)
                        != Some(PendingRequest::Interactions)
                    {
                        return Err(ClientError::ProtocolOrder);
                    }
                    self.interactions_refresh_in_flight = false;
                    Ok(ClientTransition {
                        actions: Vec::new(),
                        events: vec![ClientEvent::Response {
                            request_id: response.id,
                            result: ResponseResult::PendingInteractions(interactions),
                        }],
                    })
                }
                ResponseResult::ProjectCommand(outcome) => {
                    let Some(command_id) = project_command else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    let pending = self
                        .pending_project_commands
                        .remove(&command_id)
                        .ok_or(ClientError::ProtocolOrder)?;
                    if project_outcome_is_terminal(&outcome) {
                        self.remember_completed(command_id, pending.digest);
                    }
                    Ok(ClientTransition {
                        actions: Vec::new(),
                        events: vec![ClientEvent::ProjectCommand {
                            command_id,
                            outcome,
                        }],
                    })
                }
                ResponseResult::AgentRetirement(outcome) => {
                    let Some(command_id) = retirement_command else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    let pending = self
                        .pending_agent_retirements
                        .remove(&command_id)
                        .ok_or(ClientError::ProtocolOrder)?;
                    if agent_retirement_is_terminal(&outcome) {
                        self.remember_completed(command_id, pending.digest);
                    }
                    Ok(ClientTransition {
                        actions: Vec::new(),
                        events: vec![ClientEvent::AgentRetirement {
                            command_id,
                            outcome,
                        }],
                    })
                }
                ResponseResult::AgentSession(outcome) => {
                    let Some(operation_id) = session_operation else {
                        return Err(ClientError::ProtocolOrder);
                    };
                    let pending = self
                        .pending_agent_sessions
                        .get(&operation_id)
                        .ok_or(ClientError::ProtocolOrder)?;
                    let digest = pending.digest;
                    let terminal = !matches!(outcome, EffectOutcomeDto::Uncertain(_));
                    let mut transition = ClientTransition {
                        actions: Vec::new(),
                        events: vec![ClientEvent::AgentSession {
                            operation_id,
                            outcome,
                        }],
                    };
                    if terminal {
                        self.pending_agent_sessions
                            .remove(&operation_id)
                            .ok_or(ClientError::ProtocolOrder)?;
                        self.remember_completed(
                            CommandId::from_bytes(*operation_id.as_bytes()),
                            digest,
                        );
                    } else {
                        transition.actions.push(ClientAction::Close { generation });
                        let reconnect = self.prepare_reconnect()?;
                        transition.actions.extend(reconnect.actions);
                        transition.events.extend(reconnect.events);
                    }
                    Ok(transition)
                }
                ResponseResult::Lifecycle(_)
                | ResponseResult::ProviderCatalog(_)
                | ResponseResult::ConversationPage(_)
                | ResponseResult::MailboxDrafts(_)
                | ResponseResult::MailboxDraftSave(_)
                | ResponseResult::MailboxDraftDelete(_)
                | ResponseResult::CanonicalEvidence(_)
                | ResponseResult::EvidenceIngest(_)
                | ResponseResult::EmptyEffect(_)
                | ResponseResult::RelayStatus(_)
                | ResponseResult::StateHealth(_)
                | ResponseResult::StateRepair(_)
                | ResponseResult::ResourceInspection(_)
                | ResponseResult::InteractionAnswer(_)
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
        retryable: RetryableResponseIdentity,
    ) -> Result<ClientTransition, ClientError> {
        if let Some(command_id) = retryable.mutation {
            if let Some(pending) = self.pending_mutations.remove(&command_id) {
                self.remember_completed(command_id, pending.digest);
            }
            return Ok(error_transition(
                request_id,
                ClientOperation::Mutation(command_id),
                error,
            ));
        }

        if let Some(command_id) = retryable.project {
            self.pending_project_commands
                .remove(&command_id)
                .ok_or(ClientError::ProtocolOrder)?;
            return Ok(error_transition(
                request_id,
                ClientOperation::Project(command_id),
                error,
            ));
        }

        if let Some(command_id) = retryable.retirement {
            self.pending_agent_retirements
                .remove(&command_id)
                .ok_or(ClientError::ProtocolOrder)?;
            return Ok(error_transition(
                request_id,
                ClientOperation::AgentRetirement(command_id),
                error,
            ));
        }

        if let Some(operation_id) = retryable.session {
            self.pending_agent_sessions
                .remove(&operation_id)
                .ok_or(ClientError::ProtocolOrder)?;
            return Ok(error_transition(
                request_id,
                ClientOperation::AgentSession(operation_id),
                error,
            ));
        }

        let pending = self
            .pending_requests
            .remove(&request_id)
            .ok_or(ClientError::ProtocolOrder)?;
        let operation = match pending {
            PendingRequest::Subscription { .. } => ClientOperation::Subscription,
            PendingRequest::Snapshot => ClientOperation::Snapshot,
            PendingRequest::AuthoritativeConversationView { .. } => {
                ClientOperation::AuthoritativeConversationView
            }
            PendingRequest::InteractionResponder { .. }
            | PendingRequest::Interactions
            | PendingRequest::Ordinary => ClientOperation::Ordinary,
        };
        let mut transition = error_transition(request_id, operation, error);
        if matches!(
            pending,
            PendingRequest::Subscription { .. }
                | PendingRequest::Snapshot
                | PendingRequest::AuthoritativeConversationView { .. }
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
                transition.actions.push(self.begin_refresh(generation)?);
            }
        } else {
            self.view_current = true;
        }
        Ok(transition)
    }

    fn accept_authoritative_conversation_view(
        &mut self,
        generation: ConnectionGeneration,
        view: AuthoritativeConversationViewDto,
    ) -> Result<ClientTransition, ClientError> {
        let revision = view.snapshot.revision;
        self.current_revision = Some(revision);
        self.failures = 0;
        let event = if view.conversation.is_some() {
            ClientEvent::AuthoritativeConversationView(view)
        } else {
            ClientEvent::Snapshot(view.snapshot)
        };
        let mut transition = ClientTransition {
            actions: Vec::new(),
            events: vec![event],
        };
        if self.newest_invalidation > revision {
            self.view_current = false;
            if !self.refresh_in_flight {
                transition
                    .actions
                    .push(self.begin_authoritative_conversation_view_refresh(generation)?);
            }
        } else {
            self.view_current = true;
        }
        Ok(transition)
    }

    fn begin_refresh(
        &mut self,
        generation: ConnectionGeneration,
    ) -> Result<ClientAction, ClientError> {
        if self.subscription.is_some() && self.active_subscription_id.is_some() {
            self.begin_authoritative_conversation_view_refresh(generation)
        } else {
            self.begin_snapshot_refresh(generation)
        }
    }

    fn begin_authoritative_conversation_view_refresh(
        &mut self,
        generation: ConnectionGeneration,
    ) -> Result<ClientAction, ClientError> {
        let conversation = self
            .subscription
            .as_ref()
            .and_then(|subscription| subscription.conversation.clone());
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(
            request_id,
            Request::AuthoritativeConversationView(AuthoritativeConversationViewRequestDto::new(
                conversation.clone(),
            )),
        )?;
        self.pending_requests.insert(
            request_id,
            PendingRequest::AuthoritativeConversationView { conversation },
        );
        self.refresh_in_flight = true;
        Ok(ClientAction::Write { generation, frame })
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

    fn begin_interactions_refresh(
        &mut self,
        generation: ConnectionGeneration,
    ) -> Result<ClientAction, ClientError> {
        let request_id = self.allocate_request_id()?;
        let frame = request_frame(
            request_id,
            Request::PendingInteractions(crate::protocol::v1::PendingInteractionsRequestDto {
                limit: u16::try_from(hq_application::MAX_PENDING_INTERACTIONS)
                    .map_err(|_| ClientError::Codec)?,
            }),
        )?;
        self.pending_requests
            .insert(request_id, PendingRequest::Interactions);
        self.interactions_refresh_in_flight = true;
        Ok(ClientAction::Write { generation, frame })
    }

    fn prepare_reconnect(&mut self) -> Result<ClientTransition, ClientError> {
        let lost = self
            .pending_requests
            .iter()
            .filter_map(|(request_id, pending)| {
                matches!(pending, PendingRequest::Ordinary)
                    .then_some(ClientEvent::RequestLost(*request_id))
            })
            .collect::<Vec<_>>();
        self.pending_requests.clear();
        self.active_subscription_id = None;
        self.active_responder_id = None;
        self.current_revision = None;
        self.newest_invalidation = 0;
        self.view_current = false;
        self.refresh_in_flight = false;
        self.interactions_refresh_in_flight = false;
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

impl<T: ClientTransport> BlockingClientRunner<T> {
    /// Owns one validated synchronous execution policy, pure client, and transport.
    pub fn new(
        config: BlockingClientConfig,
        client: ReconnectingClient,
        transport: T,
    ) -> Result<Self, BlockingClientError> {
        if config.deadline.is_zero() {
            return Err(BlockingClientError::InvalidDeadline);
        }
        Ok(Self {
            config,
            client,
            transport,
            connection: None,
            actions: VecDeque::new(),
            events: VecDeque::new(),
            connection_attempts: 0,
            response_pending: false,
        })
    }

    /// Executes one non-retryable ordinary request after negotiated readiness.
    pub fn request(&mut self, request: Request) -> Result<ClientEvent, BlockingClientError> {
        let deadline = self.execution_deadline();
        self.begin_execution();
        self.ensure_connected(deadline)?;
        let transition = self
            .client
            .submit_request(request)
            .map_err(BlockingClientError::Client)?;
        let request_id = submitted_request_id(&transition)?;
        self.enqueue(transition)?;
        loop {
            match self.step(deadline)? {
                Some(
                    event @ ClientEvent::Response {
                        request_id: completed,
                        ..
                    },
                ) if completed == request_id => {
                    return Ok(event);
                }
                Some(
                    event @ ClientEvent::Error {
                        request_id: failed,
                        operation: ClientOperation::Ordinary,
                        ..
                    },
                ) if failed == request_id => return Ok(event),
                Some(ClientEvent::RequestLost(lost)) if lost == request_id => {
                    return Err(BlockingClientError::ResponseLost);
                }
                Some(ClientEvent::IncompatibleVersion) => {
                    return Err(BlockingClientError::Incompatible);
                }
                Some(
                    ClientEvent::Snapshot(_)
                    | ClientEvent::AuthoritativeConversationView(_)
                    | ClientEvent::Mutation(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_)
                    | ClientEvent::Error { .. },
                )
                | None => {}
            }
        }
    }

    /// Loads one fresh complete authoritative snapshot.
    pub fn snapshot(&mut self) -> Result<AuthoritativeSnapshotDto, BlockingClientError> {
        let deadline = self.execution_deadline();
        self.begin_execution();
        self.ensure_connected(deadline)?;
        let transition = self
            .client
            .refresh_snapshot()
            .map_err(BlockingClientError::Client)?;
        self.enqueue(transition)?;
        loop {
            match self.step(deadline)? {
                Some(ClientEvent::Snapshot(snapshot)) => return Ok(snapshot),
                Some(ClientEvent::IncompatibleVersion) => {
                    return Err(BlockingClientError::Incompatible);
                }
                Some(
                    ClientEvent::AuthoritativeConversationView(_)
                    | ClientEvent::Mutation(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_)
                    | ClientEvent::Error { .. },
                )
                | None => {}
            }
        }
    }

    /// Executes or reconciles one retry-safe mutation until its result is definite.
    pub fn mutation(
        &mut self,
        request: MutationRequest,
    ) -> Result<ClientEvent, BlockingClientError> {
        let deadline = self.execution_deadline();
        let command_id = request.command_id();
        self.begin_execution();
        let transition = self
            .client
            .submit_mutation(request)
            .map_err(BlockingClientError::Client)?;
        self.enqueue(transition)?;
        self.ensure_started()?;
        loop {
            match self.step(deadline)? {
                Some(
                    event @ ClientEvent::Mutation(MutationAttemptDto::Completed {
                        command_id: completed,
                        ..
                    }),
                ) if CommandId::from_bytes(completed.bytes()) == command_id => {
                    return Ok(event);
                }
                Some(
                    event @ ClientEvent::Error {
                        operation: ClientOperation::Mutation(failed),
                        ..
                    },
                ) if failed == command_id => return Ok(event),
                Some(ClientEvent::IncompatibleVersion) => {
                    return Err(BlockingClientError::Incompatible);
                }
                Some(
                    ClientEvent::Mutation(
                        MutationAttemptDto::Completed { .. } | MutationAttemptDto::Uncertain { .. },
                    )
                    | ClientEvent::Snapshot(_)
                    | ClientEvent::AuthoritativeConversationView(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_)
                    | ClientEvent::Error { .. },
                )
                | None => {}
            }
        }
    }

    /// Executes or reconciles one retry-safe authoritative mailbox command.
    pub fn mailbox_command(
        &mut self,
        request: MailboxCommandRequestDto,
    ) -> Result<ClientEvent, BlockingClientError> {
        let deadline = self.execution_deadline();
        let command_id = request.command_id();
        self.begin_execution();
        let transition = self
            .client
            .submit_mailbox_command(request)
            .map_err(BlockingClientError::Client)?;
        self.enqueue(transition)?;
        self.ensure_started()?;
        loop {
            match self.step(deadline)? {
                Some(
                    event @ ClientEvent::Mutation(MutationAttemptDto::Completed {
                        command_id: completed,
                        ..
                    }),
                ) if CommandId::from_bytes(completed.bytes()) == command_id => {
                    return Ok(event);
                }
                Some(
                    event @ ClientEvent::Error {
                        operation: ClientOperation::Mutation(failed),
                        ..
                    },
                ) if failed == command_id => return Ok(event),
                Some(ClientEvent::IncompatibleVersion) => {
                    return Err(BlockingClientError::Incompatible);
                }
                Some(
                    ClientEvent::Mutation(
                        MutationAttemptDto::Completed { .. } | MutationAttemptDto::Uncertain { .. },
                    )
                    | ClientEvent::Snapshot(_)
                    | ClientEvent::AuthoritativeConversationView(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_)
                    | ClientEvent::Error { .. },
                )
                | None => {}
            }
        }
    }

    /// Executes or replays one project command through its next definite durable outcome.
    pub fn project(
        &mut self,
        request: ProjectCommandRequestDto,
    ) -> Result<ClientEvent, BlockingClientError> {
        let deadline = self.execution_deadline();
        let command_id = CommandId::from_bytes(request.command_id.bytes());
        self.begin_execution();
        let transition = self
            .client
            .submit_project_command(request)
            .map_err(BlockingClientError::Client)?;
        self.enqueue(transition)?;
        self.ensure_started()?;
        loop {
            match self.step(deadline)? {
                Some(
                    event @ ClientEvent::ProjectCommand {
                        command_id: completed,
                        ..
                    },
                ) if completed == command_id => return Ok(event),
                Some(
                    event @ ClientEvent::Error {
                        operation: ClientOperation::Project(failed),
                        ..
                    },
                ) if failed == command_id => return Ok(event),
                Some(ClientEvent::IncompatibleVersion) => {
                    return Err(BlockingClientError::Incompatible);
                }
                Some(
                    ClientEvent::Snapshot(_)
                    | ClientEvent::AuthoritativeConversationView(_)
                    | ClientEvent::Mutation(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_)
                    | ClientEvent::Error { .. },
                )
                | None => {}
            }
        }
    }

    /// Executes or replays one named-agent retirement through its next durable outcome.
    pub fn agent_retirement(
        &mut self,
        request: AgentRetirementRequestDto,
    ) -> Result<ClientEvent, BlockingClientError> {
        let deadline = self.execution_deadline();
        let command_id = CommandId::from_bytes(request.command_id.bytes());
        self.begin_execution();
        let transition = self
            .client
            .submit_agent_retirement(request)
            .map_err(BlockingClientError::Client)?;
        self.enqueue(transition)?;
        self.ensure_started()?;
        loop {
            match self.step(deadline)? {
                Some(
                    event @ ClientEvent::AgentRetirement {
                        command_id: completed,
                        ..
                    },
                ) if completed == command_id => return Ok(event),
                Some(
                    event @ ClientEvent::Error {
                        operation: ClientOperation::AgentRetirement(failed),
                        ..
                    },
                ) if failed == command_id => return Ok(event),
                Some(ClientEvent::IncompatibleVersion) => {
                    return Err(BlockingClientError::Incompatible);
                }
                Some(
                    ClientEvent::Snapshot(_)
                    | ClientEvent::AuthoritativeConversationView(_)
                    | ClientEvent::Mutation(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_)
                    | ClientEvent::Error { .. },
                )
                | None => {}
            }
        }
    }

    /// Executes or replays one managed-session operation through its next definite outcome.
    pub fn agent_session(
        &mut self,
        request: EffectRequestDto<AgentSessionRequestDto>,
    ) -> Result<ClientEvent, BlockingClientError> {
        let deadline = self.execution_deadline();
        let operation_id = OperationId::from_bytes(request.operation_id.bytes());
        self.begin_execution();
        let transition = self
            .client
            .submit_agent_session(request)
            .map_err(BlockingClientError::Client)?;
        self.enqueue(transition)?;
        self.ensure_started()?;
        loop {
            match self.step(deadline)? {
                Some(
                    event @ ClientEvent::AgentSession {
                        operation_id: completed,
                        ..
                    },
                ) if completed == operation_id => return Ok(event),
                Some(
                    event @ ClientEvent::Error {
                        operation: ClientOperation::AgentSession(failed),
                        ..
                    },
                ) if failed == operation_id => return Ok(event),
                Some(ClientEvent::IncompatibleVersion) => {
                    return Err(BlockingClientError::Incompatible);
                }
                Some(
                    ClientEvent::Snapshot(_)
                    | ClientEvent::AuthoritativeConversationView(_)
                    | ClientEvent::Mutation(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_)
                    | ClientEvent::Error { .. },
                )
                | None => {}
            }
        }
    }

    /// Returns the transport after closing any live generation.
    pub fn into_transport(mut self) -> T {
        if let Some((_, connection)) = self.connection.take() {
            self.transport.close(connection);
        }
        self.transport
    }

    /// Replaces the subscribed materialized-view selection without blocking for its response.
    pub fn update_subscription_conversation(
        &mut self,
        conversation: Option<ConversationPageSelectionDto>,
    ) -> Result<(), BlockingClientError> {
        let transition = self
            .client
            .update_subscription_conversation(conversation)
            .map_err(BlockingClientError::Client)?;
        self.enqueue(transition)
    }

    /// Drives connection, subscription, and refresh work for at most the supplied wait.
    ///
    /// A normal idle timeout returns `Ok(None)` without closing the active connection. Semantic
    /// client events remain ordered and are returned one at a time.
    pub fn poll_event(
        &mut self,
        wait: Duration,
    ) -> Result<Option<ClientEvent>, BlockingClientError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        if wait.is_zero() {
            return Ok(None);
        }
        let deadline = Instant::now()
            .checked_add(wait)
            .ok_or(BlockingClientError::Deadline)?;
        self.connection_attempts = 0;
        self.ensure_started()?;
        loop {
            match self.step(deadline) {
                Ok(Some(event)) => return Ok(Some(event)),
                Ok(None) => {}
                Err(BlockingClientError::Deadline) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }

    /// Drives connection, subscription, and refresh work until an event, state change, or timeout.
    ///
    /// Unlike [`Self::poll_event`], this returns `Ok(None)` as soon as the generation-scoped
    /// connection state changes. Interactive observers can therefore publish reconnect boundaries
    /// without waiting for the rest of a reconnect attempt or for the supplied deadline.
    pub fn poll_event_or_state_change(
        &mut self,
        wait: Duration,
    ) -> Result<Option<ClientEvent>, BlockingClientError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        if wait.is_zero() {
            return Ok(None);
        }
        let observed_state = self.connection_state();
        let deadline = Instant::now()
            .checked_add(wait)
            .ok_or(BlockingClientError::Deadline)?;
        self.connection_attempts = 0;
        self.ensure_started()?;
        if self.connection_state() != observed_state {
            return Ok(None);
        }
        loop {
            match self.step(deadline) {
                Ok(Some(event)) => return Ok(Some(event)),
                Ok(None) if self.connection_state() != observed_state => return Ok(None),
                Ok(None) => {}
                Err(BlockingClientError::Deadline) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }

    /// Returns the generation-scoped state of the owned reconnecting client.
    pub const fn connection_state(&self) -> ClientConnectionState {
        self.client.connection_state()
    }

    fn begin_execution(&mut self) {
        self.connection_attempts = 0;
        self.events.clear();
    }

    fn ensure_started(&mut self) -> Result<(), BlockingClientError> {
        if self.client.current_generation().is_none() {
            let transition = self.client.start().map_err(BlockingClientError::Client)?;
            self.enqueue(transition)?;
        }
        Ok(())
    }

    fn ensure_connected(&mut self, deadline: Instant) -> Result<(), BlockingClientError> {
        self.ensure_started()?;
        while !self.client.is_active() {
            if let Some(ClientEvent::IncompatibleVersion) = self.step(deadline)? {
                return Err(BlockingClientError::Incompatible);
            }
        }
        Ok(())
    }

    fn step(&mut self, deadline: Instant) -> Result<Option<ClientEvent>, BlockingClientError> {
        if Instant::now() >= deadline {
            return Err(BlockingClientError::Deadline);
        }
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        if !self.response_pending
            && let Some(queued) = self.actions.front()
        {
            let now = Instant::now();
            if queued.not_before > now {
                let delay = queued.not_before.duration_since(now);
                self.transport.wait(delay.min(remaining(deadline)?));
                return Ok(None);
            }
            let action = self
                .actions
                .pop_front()
                .ok_or(BlockingClientError::Client(ClientError::ProtocolOrder))?
                .action;
            self.apply_action(action, deadline)?;
            return Ok(None);
        }
        let Some((generation, connection)) = self.connection.as_mut() else {
            return Err(BlockingClientError::ConnectionAttemptsExhausted);
        };
        let timeout = remaining(deadline)?;
        let polled = self.transport.poll_frame(connection, timeout);
        let transition = match polled {
            Ok(Some(frame)) => {
                let completes_response = matches!(
                    WireMessage::decode_frame(&frame),
                    Ok(WireMessage::ServerHello(_)
                        | WireMessage::VersionRejected(_)
                        | WireMessage::Response(_))
                );
                let transition = self
                    .client
                    .receive_frame(*generation, &frame)
                    .map_err(BlockingClientError::Client)?;
                if completes_response {
                    self.response_pending = false;
                }
                transition
            }
            Ok(None) => ClientTransition::default(),
            Err(_) => {
                let (generation, connection) = self
                    .connection
                    .take()
                    .ok_or(BlockingClientError::ConnectionAttemptsExhausted)?;
                self.transport.close(connection);
                self.response_pending = false;
                self.client
                    .disconnected(generation)
                    .map_err(BlockingClientError::Client)?
            }
        };
        self.enqueue(transition)?;
        Ok(None)
    }

    fn apply_action(
        &mut self,
        action: ClientAction,
        deadline: Instant,
    ) -> Result<(), BlockingClientError> {
        let transition = match action {
            ClientAction::ConnectAfter { generation, .. } => {
                self.connection_attempts = self.connection_attempts.saturating_add(1);
                if self.connection_attempts > self.config.max_connection_attempts.get() {
                    return Err(BlockingClientError::ConnectionAttemptsExhausted);
                }
                let connected = self.transport.connect();
                if Instant::now() >= deadline {
                    if let Ok(connection) = connected {
                        self.transport.close(connection);
                    }
                    return Err(BlockingClientError::Deadline);
                }
                match connected {
                    Ok(connection) => {
                        self.connection = Some((generation, connection));
                        self.response_pending = false;
                        self.client
                            .connected(generation)
                            .map_err(BlockingClientError::Client)?
                    }
                    Err(_) => self
                        .client
                        .connection_failed(generation)
                        .map_err(BlockingClientError::Client)?,
                }
            }
            ClientAction::Write { generation, frame } => {
                let Some((current, connection)) = self.connection.as_mut() else {
                    return Err(BlockingClientError::Client(ClientError::ProtocolOrder));
                };
                if *current != generation {
                    return Err(BlockingClientError::Client(ClientError::ProtocolOrder));
                }
                let timeout = remaining(deadline)?;
                if self.transport.write(connection, &frame, timeout).is_ok() {
                    self.response_pending = true;
                    ClientTransition::default()
                } else {
                    let (_, connection) = self
                        .connection
                        .take()
                        .ok_or(BlockingClientError::ConnectionAttemptsExhausted)?;
                    self.transport.close(connection);
                    self.response_pending = false;
                    self.client
                        .disconnected(generation)
                        .map_err(BlockingClientError::Client)?
                }
            }
            ClientAction::Close { generation } => {
                if self
                    .connection
                    .as_ref()
                    .is_some_and(|(current, _)| *current == generation)
                    && let Some((_, connection)) = self.connection.take()
                {
                    self.transport.close(connection);
                }
                self.response_pending = false;
                ClientTransition::default()
            }
        };
        self.enqueue(transition)?;
        Ok(())
    }

    fn enqueue(&mut self, transition: ClientTransition) -> Result<(), BlockingClientError> {
        let now = Instant::now();
        for action in transition.actions {
            let delay = match action {
                ClientAction::ConnectAfter { delay, .. } => delay,
                ClientAction::Write { .. } | ClientAction::Close { .. } => Duration::ZERO,
            };
            let not_before = now
                .checked_add(delay)
                .ok_or(BlockingClientError::Deadline)?;
            self.actions
                .push_back(QueuedClientAction { action, not_before });
        }
        self.events.extend(transition.events);
        Ok(())
    }

    fn execution_deadline(&self) -> Instant {
        Instant::now()
            .checked_add(self.config.deadline)
            .unwrap_or_else(Instant::now)
    }
}

fn request_frame(request_id: RequestId, request: Request) -> Result<Vec<u8>, ClientError> {
    WireMessage::Request(RequestEnvelope::new(request_id, request))
        .encode_frame()
        .map_err(|_| ClientError::Codec)
}

fn view_matches_conversation(
    view: &AuthoritativeConversationViewDto,
    conversation: Option<&ConversationPageSelectionDto>,
) -> bool {
    match (conversation, view.conversation.as_ref()) {
        (None, None) => true,
        (Some(expected), Some(actual)) => actual.key == expected.key,
        (None, Some(_)) | (Some(_), None) => false,
    }
}

fn submitted_request_id(transition: &ClientTransition) -> Result<RequestId, BlockingClientError> {
    transition
        .actions
        .iter()
        .find_map(|action| match action {
            ClientAction::Write { frame, .. } => match WireMessage::decode_frame(frame) {
                Ok(WireMessage::Request(envelope)) => Some(envelope.id),
                Ok(
                    WireMessage::ClientHello(_)
                    | WireMessage::ServerHello(_)
                    | WireMessage::VersionRejected(_)
                    | WireMessage::Response(_)
                    | WireMessage::Invalidation(_),
                )
                | Err(_) => None,
            },
            ClientAction::ConnectAfter { .. } | ClientAction::Close { .. } => None,
        })
        .ok_or(BlockingClientError::Client(ClientError::ProtocolOrder))
}

fn remaining(deadline: Instant) -> Result<Duration, BlockingClientError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(BlockingClientError::Deadline)
}

fn write_transition(generation: ConnectionGeneration, frame: Vec<u8>) -> ClientTransition {
    ClientTransition {
        actions: vec![ClientAction::Write { generation, frame }],
        events: Vec::new(),
    }
}

fn error_transition(
    request_id: RequestId,
    operation: ClientOperation,
    error: ErrorResponse,
) -> ClientTransition {
    ClientTransition {
        actions: Vec::new(),
        events: vec![ClientEvent::Error {
            request_id,
            operation,
            error,
        }],
    }
}

fn derive_subscription_id(seed: Id32, server_session: Id32) -> Id32 {
    let mut digest = Sha256::new();
    digest.update(SUBSCRIPTION_ID_DOMAIN);
    digest.update(seed.bytes());
    digest.update(server_session.bytes());
    Id32::new(digest.finalize().into())
}

fn derive_responder_id(seed: Id32, server_session: Id32) -> Id32 {
    let mut digest = Sha256::new();
    digest.update(RESPONDER_ID_DOMAIN);
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

const fn project_outcome_is_terminal(outcome: &ProjectCommandOutcomeDto) -> bool {
    matches!(
        outcome,
        ProjectCommandOutcomeDto::Completed { .. } | ProjectCommandOutcomeDto::Rejected { .. }
    )
}

const fn agent_retirement_is_terminal(outcome: &AgentRetirementOutcomeDto) -> bool {
    matches!(
        outcome,
        AgentRetirementOutcomeDto::Completed { .. } | AgentRetirementOutcomeDto::Rejected { .. }
    )
}

const fn map_decode_error(_error: DecodeError) -> ClientError {
    ClientError::Codec
}
