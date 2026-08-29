//! Bounded blocking Unix transport for the pure reconnecting local API client.

use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    io::Read as _,
    num::NonZeroUsize,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use hq_domain::{AgentId, InstallationId, ProviderId, ProviderSessionId, ShortText};
use hq_local_api::{
    BlockingClientConfig, BlockingClientError, BlockingClientRunner, ClientConnectionState,
    ClientEvent, ClientTransport, InitialView, ReconnectPolicy, ReconnectingClient,
    protocol::v1::{
        AgentRetirementRequestDto, AgentSessionRequestDto, AuthoritativeSnapshotDto, BuildMetadata,
        EffectRequestDto, FrameDecoder, Id32, InvalidationTopic, MailboxCommandRequestDto,
        MutationRequest, ProjectCommandRequestDto, Request,
    },
};

use crate::{
    LifecycleClient, LifecycleClientConfig, NodeClientCoordinator, NodeCoordinatorConfig,
    NodeCoordinatorError, NodeLaunchError, NodeLauncher, ProcessNodeLauncher, RuntimePathError,
    RuntimePaths, StatePaths,
    cli::{
        CliError, HarnessCommand, NamedAgentCommand, NamedAgentSelector, NamedAgentView,
        named_agent_catalog_view, run_harness_for_tui, run_named_agent_for_tui,
    },
    unix_frame,
};

/// Passive named-agent command accepted by the ordinary local client composition.
pub(crate) enum LocalNamedAgentCommand {
    Create {
        name: String,
    },
    RenameSession {
        agent_id: [u8; 32],
        provider: String,
        session: String,
        display_name: Option<String>,
    },
    Retire {
        agent_id: [u8; 32],
        force: bool,
    },
}

pub(crate) fn execute_named_agent_command(
    state: &StatePaths,
    command: LocalNamedAgentCommand,
) -> Result<u64, CliError> {
    let command = match command {
        LocalNamedAgentCommand::Create { name } => NamedAgentCommand::Create {
            name: ShortText::new(name).map_err(|_| CliError::Arguments)?,
            mailbox_id: None,
        },
        LocalNamedAgentCommand::RenameSession {
            agent_id,
            provider,
            session,
            display_name,
        } => NamedAgentCommand::Rename {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(agent_id)),
            provider: Some(ProviderId::new(provider).map_err(|_| CliError::Arguments)?),
            session: Some(ProviderSessionId::new(session).map_err(|_| CliError::Arguments)?),
            display_name: display_name
                .map(ShortText::new)
                .transpose()
                .map_err(|_| CliError::Arguments)?,
        },
        LocalNamedAgentCommand::Retire { agent_id, force } => NamedAgentCommand::Retire {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(agent_id)),
            force,
        },
    };
    run_named_agent_for_tui(&command, state)
}

pub(crate) fn tui_named_agent_catalog(snapshot: &AuthoritativeSnapshotDto) -> Vec<NamedAgentView> {
    named_agent_catalog_view(snapshot, "tui_snapshot", None, None).agents
}

/// Passive managed-session command accepted by the ordinary local client composition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalManagedSessionCommand {
    Start {
        agent_id: [u8; 32],
        provider: String,
    },
    Resume {
        agent_id: [u8; 32],
        provider: String,
        session: String,
    },
    Stop {
        agent_id: [u8; 32],
        provider: String,
    },
}

/// Typed managed-session outcome returned across the passive local-client seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalManagedSessionOutcome {
    Ready { session: String },
    Stopped,
    Rejected { category: String, code: String },
    Uncertain { reconciliation_id: [u8; 32] },
}

/// Passive completion evidence for one stable managed-session operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalManagedSessionResult {
    pub command: LocalManagedSessionCommand,
    pub operation_id: [u8; 32],
    pub outcome: LocalManagedSessionOutcome,
}

pub(crate) fn execute_managed_session_command(
    state: &StatePaths,
    command: LocalManagedSessionCommand,
) -> Result<LocalManagedSessionResult, CliError> {
    let action = match &command {
        LocalManagedSessionCommand::Start { agent_id, provider } => HarnessCommand::Start {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
            directory: None,
        },
        LocalManagedSessionCommand::Resume {
            agent_id,
            provider,
            session,
        } => HarnessCommand::Resume {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
            session: ProviderSessionId::new(session.clone()).map_err(|_| CliError::Arguments)?,
            directory: None,
        },
        LocalManagedSessionCommand::Stop { agent_id, provider } => HarnessCommand::Stop {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
        },
    };
    let view = run_harness_for_tui(&action, state)?;
    let outcome = match view.status {
        "ready" => LocalManagedSessionOutcome::Ready {
            session: view.ready_session.ok_or(CliError::HarnessState)?,
        },
        "stopped" => LocalManagedSessionOutcome::Stopped,
        "rejected" => LocalManagedSessionOutcome::Rejected {
            category: view.error_category.ok_or(CliError::HarnessState)?,
            code: view.error_code.ok_or(CliError::HarnessState)?,
        },
        "uncertain" => LocalManagedSessionOutcome::Uncertain {
            reconciliation_id: view.reconciliation_id.ok_or(CliError::HarnessState)?,
        },
        _ => return Err(CliError::HarnessState),
    };
    Ok(LocalManagedSessionResult {
        command,
        operation_id: *view.operation_id.as_bytes(),
        outcome,
    })
}

/// Passive local Unix transport configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnixClientTransportConfig {
    /// Exact private runtime namespace containing the node socket.
    pub runtime: RuntimePaths,
    /// Positive bound independently applied to every socket read and write.
    pub io_timeout: Duration,
}

/// Closed local transport failure without operating-system prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnixClientTransportError {
    /// The configured I/O timeout was zero.
    InvalidTimeout,
    /// No local node listener accepted the connection.
    Absent,
    /// Socket setup, read, or write failed.
    Transport,
    /// A frame length exceeded the protocol bound.
    Protocol,
}

impl fmt::Display for UnixClientTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local client transport failed: {self:?}")
    }
}

impl Error for UnixClientTransportError {}

/// Plain bounds and identities for one reusable command client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalNodeClientConfig {
    /// Validated installation state layout used for readiness and runtime derivation.
    pub state: StatePaths,
    /// Safe build metadata sent during protocol negotiation.
    pub build: BuildMetadata,
    /// Whether this client needs an initial authoritative state view.
    pub initial_view: InitialView,
    /// Positive timeout independently applied to every socket read and write.
    pub io_timeout: Duration,
    /// Inclusive wall-time bound for each typed command execution.
    pub command_deadline: Duration,
    /// Maximum connection attempts for each typed command execution.
    pub max_connection_attempts: NonZeroUsize,
    /// Maximum time allowed for absent-node autostart convergence.
    pub readiness_timeout: Duration,
    /// Positive coordinator polling interval.
    pub readiness_retry_interval: Duration,
    /// Initial positive reconnect delay after connection loss.
    pub reconnect_initial: Duration,
    /// Inclusive maximum reconnect delay.
    pub reconnect_maximum: Duration,
    /// Maximum retained completed retry-safe command identities.
    pub completed_identity_capacity: NonZeroUsize,
}

pub(crate) const fn installed_local_client_config(
    state: StatePaths,
    build: BuildMetadata,
    initial_view: InitialView,
) -> LocalNodeClientConfig {
    LocalNodeClientConfig {
        state,
        build,
        initial_view,
        io_timeout: Duration::from_secs(2),
        command_deadline: Duration::from_secs(10),
        max_connection_attempts: positive_usize(8),
        readiness_timeout: Duration::from_secs(10),
        readiness_retry_interval: Duration::from_millis(25),
        reconnect_initial: Duration::from_millis(25),
        reconnect_maximum: Duration::from_millis(250),
        completed_identity_capacity: positive_usize(64),
    }
}

const fn positive_usize(value: usize) -> NonZeroUsize {
    match NonZeroUsize::new(value) {
        Some(value) => value,
        None => unreachable!(),
    }
}

/// Closed command-client setup or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalNodeClientError {
    /// The private runtime path could not be derived.
    RuntimePath,
    /// Node readiness or autostart did not converge.
    Coordinator(NodeCoordinatorError),
    /// The current executable could not be resolved for autostart.
    Launcher(NodeLaunchError),
    /// The Unix transport configuration was invalid.
    Transport(UnixClientTransportError),
    /// Reconnect policy or client state construction failed.
    Client,
    /// A bounded request execution failed.
    Execution(BlockingClientError),
}

impl fmt::Display for LocalNodeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local node client failed: {self:?}")
    }
}

impl Error for LocalNodeClientError {}

/// Reusable typed local command seam with no direct signer, storage, relay, or provider access.
pub struct LocalNodeClient {
    installation_id: InstallationId,
    runner: BlockingClientRunner<UnixClientTransport>,
}

/// Long-lived subscribed local client for interactive event-driven frontends.
pub struct LocalNodeEventClient {
    installation_id: InstallationId,
    runner: BlockingClientRunner<UnixClientTransport>,
}

#[derive(Clone, Copy)]
enum SubscriptionMode {
    None,
    All,
}

impl LocalNodeClient {
    /// Converges readiness through the installed executable and opens a bounded command client.
    pub fn connect(config: LocalNodeClientConfig) -> Result<Self, LocalNodeClientError> {
        let launcher =
            ProcessNodeLauncher::current_executable().map_err(LocalNodeClientError::Launcher)?;
        Self::connect_with_launcher(config, launcher)
    }

    /// Converges readiness through an injected launcher before opening the local API transport.
    pub fn connect_with_launcher<L: NodeLauncher>(
        config: LocalNodeClientConfig,
        launcher: L,
    ) -> Result<Self, LocalNodeClientError> {
        let (installation_id, runner) = connect_runner(config, launcher, SubscriptionMode::None)?;
        Ok(Self {
            installation_id,
            runner,
        })
    }

    /// Returns the installation authenticated by coordinator readiness.
    pub const fn installation_id(&self) -> InstallationId {
        self.installation_id
    }

    /// Executes one non-retryable typed request.
    pub fn request(&mut self, request: Request) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .request(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Loads one fresh complete authoritative snapshot.
    pub fn snapshot(&mut self) -> Result<AuthoritativeSnapshotDto, LocalNodeClientError> {
        self.runner
            .snapshot()
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe fact mutation.
    pub fn mutation(
        &mut self,
        request: MutationRequest,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .mutation(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe authoritative mailbox command.
    pub fn mailbox_command(
        &mut self,
        request: MailboxCommandRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .mailbox_command(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe durable project command.
    pub fn project(
        &mut self,
        request: ProjectCommandRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .project(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe node-owned named-agent retirement.
    pub fn agent_retirement(
        &mut self,
        request: AgentRetirementRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .agent_retirement(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe managed named-agent session operation.
    pub fn agent_session(
        &mut self,
        request: EffectRequestDto<AgentSessionRequestDto>,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .agent_session(request)
            .map_err(LocalNodeClientError::Execution)
    }
}

impl LocalNodeEventClient {
    /// Converges readiness and opens a broad-invalidation subscribed local client.
    pub fn connect(config: LocalNodeClientConfig) -> Result<Self, LocalNodeClientError> {
        let launcher =
            ProcessNodeLauncher::current_executable().map_err(LocalNodeClientError::Launcher)?;
        Self::connect_with_launcher(config, launcher)
    }

    /// Converges readiness through an injected launcher before subscribing to all revisions.
    pub fn connect_with_launcher<L: NodeLauncher>(
        config: LocalNodeClientConfig,
        launcher: L,
    ) -> Result<Self, LocalNodeClientError> {
        let (installation_id, runner) = connect_runner(config, launcher, SubscriptionMode::All)?;
        Ok(Self {
            installation_id,
            runner,
        })
    }

    /// Returns the installation authenticated by coordinator readiness.
    pub const fn installation_id(&self) -> InstallationId {
        self.installation_id
    }

    /// Drives connection, subscription, and invalidation refresh work for a bounded interval.
    pub fn poll_event(
        &mut self,
        wait: Duration,
    ) -> Result<Option<ClientEvent>, LocalNodeClientError> {
        self.runner
            .poll_event(wait)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Loads one explicit complete authoritative snapshot on the same subscribed connection.
    pub fn snapshot(&mut self) -> Result<AuthoritativeSnapshotDto, LocalNodeClientError> {
        self.runner
            .snapshot()
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes one non-retryable typed request on the subscribed connection.
    pub fn request(&mut self, request: Request) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .request(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe mailbox command on the subscribed connection.
    pub fn mailbox_command(
        &mut self,
        request: MailboxCommandRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .mailbox_command(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Returns the generation-scoped reconnecting-client state.
    pub const fn connection_state(&self) -> ClientConnectionState {
        self.runner.connection_state()
    }
}

fn connect_runner<L: NodeLauncher>(
    config: LocalNodeClientConfig,
    launcher: L,
    subscription: SubscriptionMode,
) -> Result<(InstallationId, BlockingClientRunner<UnixClientTransport>), LocalNodeClientError> {
    let runtime = RuntimePaths::new(config.state.root().join("runtime"))
        .map_err(|_error: RuntimePathError| LocalNodeClientError::RuntimePath)?;
    let probe = LifecycleClient::new(LifecycleClientConfig {
        runtime: runtime.clone(),
        build: config.build.clone(),
        io_timeout: config.io_timeout,
    })
    .map_err(|_| LocalNodeClientError::Client)?;
    let mut coordinator = NodeClientCoordinator::new(
        probe,
        launcher,
        NodeCoordinatorConfig {
            state_root: config.state.root().to_path_buf(),
            readiness_timeout: config.readiness_timeout,
            retry_interval: config.readiness_retry_interval,
        },
    )
    .map_err(LocalNodeClientError::Coordinator)?;
    let ready = coordinator
        .ensure_ready()
        .map_err(LocalNodeClientError::Coordinator)?;
    let installation_id = ready
        .observation
        .readiness
        .as_ref()
        .map(|readiness| InstallationId::from_bytes(readiness.installation_id.bytes()))
        .ok_or(LocalNodeClientError::Client)?;
    let transport = UnixClientTransport::new(UnixClientTransportConfig {
        runtime,
        io_timeout: config.io_timeout,
    })
    .map_err(LocalNodeClientError::Transport)?;
    let reconnect = ReconnectPolicy::new(config.reconnect_initial, config.reconnect_maximum)
        .map_err(|_| LocalNodeClientError::Client)?;
    let mut client = ReconnectingClient::new(
        config.build,
        reconnect,
        config.completed_identity_capacity.get(),
        config.initial_view,
    )
    .map_err(|_| LocalNodeClientError::Client)?;
    if matches!(subscription, SubscriptionMode::All) {
        client
            .configure_subscription(
                Id32::new(*installation_id.as_bytes()),
                vec![InvalidationTopic::All],
            )
            .map_err(|_| LocalNodeClientError::Client)?;
    }
    let runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: config.command_deadline,
            max_connection_attempts: config.max_connection_attempts,
        },
        client,
        transport,
    )
    .map_err(LocalNodeClientError::Execution)?;
    Ok((installation_id, runner))
}

/// Standard blocking transport that owns no state beyond validated configuration.
#[derive(Clone, Debug)]
pub struct UnixClientTransport {
    config: UnixClientTransportConfig,
}

/// One Unix client connection with an incremental frame decoder retained across idle polls.
#[derive(Debug)]
pub struct UnixClientConnection {
    stream: UnixStream,
    decoder: FrameDecoder,
    ready_frames: VecDeque<Vec<u8>>,
}

impl UnixClientConnection {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
            ready_frames: VecDeque::new(),
        }
    }
}

impl UnixClientTransport {
    /// Validates and retains one local Unix transport configuration.
    pub fn new(config: UnixClientTransportConfig) -> Result<Self, UnixClientTransportError> {
        if config.io_timeout.is_zero() {
            return Err(UnixClientTransportError::InvalidTimeout);
        }
        Ok(Self { config })
    }
}

impl ClientTransport for UnixClientTransport {
    type Connection = UnixClientConnection;
    type Error = UnixClientTransportError;

    fn connect(&mut self) -> Result<Self::Connection, Self::Error> {
        let stream = UnixStream::connect(self.config.runtime.socket_file()).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                UnixClientTransportError::Absent
            } else {
                UnixClientTransportError::Transport
            }
        })?;
        stream
            .set_read_timeout(Some(self.config.io_timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        Ok(UnixClientConnection::new(stream))
    }

    fn write(
        &mut self,
        connection: &mut Self::Connection,
        frame: &[u8],
        timeout: Duration,
    ) -> Result<(), Self::Error> {
        connection
            .stream
            .set_write_timeout(Some(timeout.min(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        unix_frame::write_frame(&mut connection.stream, frame)
            .map_err(|_| UnixClientTransportError::Transport)
    }

    fn read_frame(
        &mut self,
        connection: &mut Self::Connection,
        timeout: Duration,
    ) -> Result<Vec<u8>, Self::Error> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(UnixClientTransportError::Transport)?;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(UnixClientTransportError::Transport)?;
            if let Some(frame) = self.poll_frame(connection, remaining)? {
                return Ok(frame);
            }
        }
    }

    fn poll_frame(
        &mut self,
        connection: &mut Self::Connection,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        if let Some(frame) = connection.ready_frames.pop_front() {
            return Ok(Some(frame));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        connection
            .stream
            .set_read_timeout(Some(timeout.min(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        let mut bytes = [0_u8; 8_192];
        let count = match connection.stream.read(&mut bytes) {
            Ok(0) if connection.decoder.buffered_len() == 0 => {
                return Err(UnixClientTransportError::Transport);
            }
            Ok(0) => return Err(UnixClientTransportError::Protocol),
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(_) => return Err(UnixClientTransportError::Transport),
        };
        let mut next = connection.decoder.push(&bytes[..count]);
        loop {
            let message = match next {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(_) => return Err(UnixClientTransportError::Protocol),
            };
            connection.ready_frames.push_back(
                message
                    .encode_frame()
                    .map_err(|_| UnixClientTransportError::Protocol)?,
            );
            next = connection.decoder.push(&[]);
        }
        Ok(connection.ready_frames.pop_front())
    }

    fn close(&mut self, connection: Self::Connection) {
        let _ = connection.stream.shutdown(std::net::Shutdown::Both);
    }

    fn wait(&mut self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{io::Write as _, os::unix::net::UnixStream, time::Duration};

    use hq_local_api::{
        ClientTransport,
        protocol::v1::{BuildMetadata, Id32, ServerHello, V1, WireMessage},
    };

    use super::{UnixClientConnection, UnixClientTransport, UnixClientTransportConfig};
    use crate::RuntimePaths;

    #[test]
    fn unix_poll_preserves_a_partial_frame_across_an_idle_timeout() {
        let (mut writer, reader) = UnixStream::pair().expect("socket pair");
        let mut connection = UnixClientConnection::new(reader);
        let mut transport = UnixClientTransport::new(UnixClientTransportConfig {
            runtime: RuntimePaths::new(
                std::env::temp_dir().join("hq-local-client-partial-frame-test"),
            )
            .expect("absolute runtime path"),
            io_timeout: Duration::from_millis(10),
        })
        .expect("transport");
        let frame = WireMessage::ServerHello(ServerHello::new(
            V1,
            BuildMetadata::new("hq-test", "0.1.0", None::<String>).expect("build"),
            Id32::new([7; 32]),
        ))
        .encode_frame()
        .expect("frame");

        writer.write_all(&frame[..2]).expect("partial prefix");
        assert_eq!(
            transport
                .poll_frame(&mut connection, Duration::from_millis(1))
                .expect("idle partial poll"),
            None
        );
        writer.write_all(&frame[2..]).expect("remaining frame");
        assert_eq!(
            transport
                .poll_frame(&mut connection, Duration::from_millis(10))
                .expect("completed poll"),
            Some(frame)
        );
    }
}
