//! Foreground node generation construction over currently available component owners.

use std::{error::Error, fmt, future::Future, num::NonZeroUsize, sync::Arc, time::Duration};

use hq_application::{
    AgentSessionRequest, AgentSessionResult, ApplicationError, ApplicationErrorCode,
    ConfigureRelays, ControlHarness, EffectOutcome, EffectRequest, InspectResource, PublishWake,
    RelayConfiguration, ResourceInspectionRequest, ResourceInspectionResult,
    SynchronizationRequest, WakeDisposition,
};
use hq_domain::{MailboxId, Revision, Timestamp};
use hq_local_api::protocol::v1::{BuildMetadata, Id32};
use hq_reducer::AuthorityPolicy;

use crate::{
    CancellationToken, ComponentDrain, ComponentError, HarnessNodeComponent, LocalNodeRuntime,
    LocalNodeRuntimeConfig, LocalNodeRuntimeError, LocalNodeRuntimeReport,
    LocalNodeRuntimeStartError, LocalSessionPumpConfig, LocalSessionRegistryConfig, NodeComponent,
    NodeComponents, NodeFoundation, NodeFoundationConfig, NodeOwner, NodeOwnerStartError,
    NodeStartupError, ProjectNodeConfig, RelayNodeComponent, RelayNodeConfig, RuntimePaths,
    ShutdownIntent, StandardProjectNodeComponent, StatePaths, compose_standard_project_component,
};

/// Explicit capacities and paths for one foreground node process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForegroundNodeConfig {
    /// Exact initialized installation state layout.
    pub state: StatePaths,
    /// Exact private state-qualified runtime layout.
    pub runtime: RuntimePaths,
    /// Safe executable build identity.
    pub build: BuildMetadata,
    /// Synchronous store mailbox capacity.
    pub store_capacity: NonZeroUsize,
    /// Accepted non-session node task capacity.
    pub task_capacity: NonZeroUsize,
    /// Revision subscription capacity.
    pub subscription_capacity: NonZeroUsize,
    /// Concurrent local session capacity.
    pub session_capacity: NonZeroUsize,
    /// Shared decoded/completion event capacity.
    pub event_capacity: NonZeroUsize,
    /// Per-session encoded write capacity.
    pub write_capacity: NonZeroUsize,
    /// Maximum accepted response drain duration.
    pub response_drain_timeout: Duration,
}

/// Stable foreground generation startup or runtime failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForegroundNodeError {
    /// State, identity, store, or runtime foundation opening failed.
    Foundation(NodeStartupError),
    /// Required component or revision-hub startup failed.
    Components(NodeOwnerStartError),
    /// Runtime artifacts or local pump startup failed.
    RuntimeStart(LocalNodeRuntimeStartError),
    /// Runtime lifecycle or signal coordination failed.
    Runtime(LocalNodeRuntimeError),
    /// A fresh nonzero boot nonce could not be generated.
    Entropy,
    /// A required concrete component capability was absent after foundation startup.
    Composition,
}

impl fmt::Display for ForegroundNodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Foundation(error) => error.fmt(formatter),
            Self::Components(error) => error.fmt(formatter),
            Self::RuntimeStart(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Entropy => formatter.write_str("foreground node entropy is unavailable"),
            Self::Composition => formatter.write_str("foreground node composition is unavailable"),
        }
    }
}

impl Error for ForegroundNodeError {}

impl From<NodeStartupError> for ForegroundNodeError {
    fn from(error: NodeStartupError) -> Self {
        Self::Foundation(error)
    }
}

impl From<NodeOwnerStartError> for ForegroundNodeError {
    fn from(error: NodeOwnerStartError) -> Self {
        Self::Components(error)
    }
}

impl From<LocalNodeRuntimeStartError> for ForegroundNodeError {
    fn from(error: LocalNodeRuntimeStartError) -> Self {
        Self::RuntimeStart(error)
    }
}

impl From<LocalNodeRuntimeError> for ForegroundNodeError {
    fn from(error: LocalNodeRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

/// Runs process generations until a complete stop report is returned.
pub async fn run_foreground(
    config: ForegroundNodeConfig,
) -> Result<LocalNodeRuntimeReport, ForegroundNodeError> {
    loop {
        let runtime = open_generation(&config)?;
        let report = runtime.run_with_unix_signals().await?;
        match report.intent {
            ShutdownIntent::Stop => return Ok(report),
            ShutdownIntent::Restart => {}
        }
    }
}

/// Runs one generation against an injected shutdown future for deterministic composition tests.
pub async fn run_foreground_generation_until<F>(
    config: &ForegroundNodeConfig,
    shutdown: F,
) -> Result<LocalNodeRuntimeReport, ForegroundNodeError>
where
    F: Future<Output = ShutdownIntent>,
{
    open_generation(config)?
        .run_until(shutdown)
        .await
        .map_err(Into::into)
}

fn open_generation(
    config: &ForegroundNodeConfig,
) -> Result<
    LocalNodeRuntime<
        DormantNodeComponent,
        RelayNodeComponent,
        HarnessNodeComponent,
        StandardProjectNodeComponent<HarnessNodeComponent>,
    >,
    ForegroundNodeError,
> {
    let foundation = NodeFoundation::open(NodeFoundationConfig::new(
        config.state.clone(),
        config.runtime.clone(),
        config.store_capacity,
    ))?;
    let policy = AuthorityPolicy::new(
        foundation.public_identity().installation_id,
        reserved_human_mailbox(),
    );
    foundation.bootstrap_installation(policy)?;
    let relay = foundation.compose_relay(
        RelayNodeConfig::default(),
        policy,
        Arc::new(hq_relay::WebSocketRelayConnector::default()),
    )?;
    let store = foundation.store().ok_or(ForegroundNodeError::Composition)?;
    let harness = HarnessNodeComponent::without_providers(store);
    let project = compose_standard_project_component(
        ProjectNodeConfig {
            recovery_limit: NonZeroUsize::new(256).unwrap_or(NonZeroUsize::MIN),
            recovery_time: current_timestamp(),
        },
        store,
        policy,
        foundation.signer_handle(),
        foundation.public_identity().installation_id,
        harness.clone(),
        relay.clone(),
    );
    let owner = NodeOwner::start(
        foundation,
        NodeComponents::new(DormantNodeComponent, relay, harness, project),
        config.task_capacity,
        config.subscription_capacity,
    )?;
    let runtime = LocalNodeRuntime::start(
        owner,
        LocalNodeRuntimeConfig {
            pump: LocalSessionPumpConfig {
                registry: LocalSessionRegistryConfig {
                    session_capacity: config.session_capacity,
                    event_capacity: config.event_capacity,
                    write_capacity: config.write_capacity,
                },
                boot_nonce: boot_nonce()?,
            },
            build: config.build.clone(),
            authority_policy: policy,
            response_drain_timeout: config.response_drain_timeout,
        },
    )?
    .0;
    Ok(runtime)
}

fn current_timestamp() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX);
    Timestamp::from_unix_millis(millis)
}

fn boot_nonce() -> Result<Id32, ForegroundNodeError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| ForegroundNodeError::Entropy)?;
    if bytes == [0; 32] {
        return Err(ForegroundNodeError::Entropy);
    }
    Ok(Id32::new(bytes))
}

pub(crate) const fn reserved_human_mailbox() -> MailboxId {
    let mut bytes = [0_u8; 32];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    MailboxId::from_bytes(bytes)
}

#[derive(Debug)]
struct DormantNodeComponent;

impl NodeComponent for DormantNodeComponent {
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        Ok(())
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        Ok(ComponentDrain::Complete)
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        Ok(())
    }
}

fn unavailable<T>() -> Result<T, ApplicationError> {
    Err(ApplicationError::new(
        ApplicationErrorCode::AdapterUnavailable,
    ))
}

impl PublishWake for DormantNodeComponent {
    fn publish_wake(&self, _revision: Revision) -> Result<WakeDisposition, ApplicationError> {
        unavailable()
    }
}

impl ConfigureRelays for DormantNodeComponent {
    fn configure_relay(
        &self,
        _request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        unavailable()
    }

    fn synchronize(
        &self,
        _request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        unavailable()
    }
}

impl ControlHarness for DormantNodeComponent {
    fn control_harness(
        &self,
        _request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        unavailable()
    }
}

impl InspectResource for DormantNodeComponent {
    fn inspect_resource(
        &self,
        _request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        unavailable()
    }
}

impl hq_application::ControlProjects for DormantNodeComponent {
    fn control_project(
        &self,
        _request: hq_application::ProjectCommandRequest,
    ) -> Result<hq_application::ProjectCommandOutcome, ApplicationError> {
        unavailable()
    }
}
