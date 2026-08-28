//! Bounded blocking Unix transport for the pure reconnecting local API client.

use std::{error::Error, fmt, num::NonZeroUsize, os::unix::net::UnixStream, time::Duration};

use hq_local_api::{
    BlockingClientConfig, BlockingClientError, BlockingClientRunner, ClientEvent, ClientTransport,
    InitialView, ReconnectPolicy, ReconnectingClient,
    protocol::v1::{BuildMetadata, MutationRequest, ProjectCommandRequestDto, Request},
};

use crate::{
    LifecycleClient, LifecycleClientConfig, NodeClientCoordinator, NodeCoordinatorConfig,
    NodeCoordinatorError, NodeLaunchError, NodeLauncher, ProcessNodeLauncher, RuntimePathError,
    RuntimePaths, StatePaths, unix_frame,
};

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
    runner: BlockingClientRunner<UnixClientTransport>,
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
        let _ready = coordinator
            .ensure_ready()
            .map_err(LocalNodeClientError::Coordinator)?;
        let transport = UnixClientTransport::new(UnixClientTransportConfig {
            runtime,
            io_timeout: config.io_timeout,
        })
        .map_err(LocalNodeClientError::Transport)?;
        let reconnect = ReconnectPolicy::new(config.reconnect_initial, config.reconnect_maximum)
            .map_err(|_| LocalNodeClientError::Client)?;
        let client = ReconnectingClient::new(
            config.build,
            reconnect,
            config.completed_identity_capacity.get(),
            config.initial_view,
        )
        .map_err(|_| LocalNodeClientError::Client)?;
        let runner = BlockingClientRunner::new(
            BlockingClientConfig {
                deadline: config.command_deadline,
                max_connection_attempts: config.max_connection_attempts,
            },
            client,
            transport,
        )
        .map_err(LocalNodeClientError::Execution)?;
        Ok(Self { runner })
    }

    /// Executes one non-retryable typed request.
    pub fn request(&mut self, request: Request) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .request(request)
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

    /// Executes or reconciles one retry-safe durable project command.
    pub fn project(
        &mut self,
        request: ProjectCommandRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .project(request)
            .map_err(LocalNodeClientError::Execution)
    }
}

/// Standard blocking transport that owns no state beyond validated configuration.
#[derive(Clone, Debug)]
pub struct UnixClientTransport {
    config: UnixClientTransportConfig,
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
    type Connection = UnixStream;
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
        Ok(stream)
    }

    fn write(
        &mut self,
        connection: &mut Self::Connection,
        frame: &[u8],
        timeout: Duration,
    ) -> Result<(), Self::Error> {
        connection
            .set_write_timeout(Some(timeout.min(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        unix_frame::write_frame(connection, frame).map_err(|_| UnixClientTransportError::Transport)
    }

    fn read_frame(
        &mut self,
        connection: &mut Self::Connection,
        timeout: Duration,
    ) -> Result<Vec<u8>, Self::Error> {
        connection
            .set_read_timeout(Some(timeout.min(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        unix_frame::read_frame(connection).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                UnixClientTransportError::Protocol
            } else {
                UnixClientTransportError::Transport
            }
        })
    }

    fn close(&mut self, connection: Self::Connection) {
        let _ = connection.shutdown(std::net::Shutdown::Both);
    }

    fn wait(&mut self, delay: Duration) {
        std::thread::sleep(delay);
    }
}
