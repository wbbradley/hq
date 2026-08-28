//! Component lifecycle ownership and normative node startup/shutdown order.

use std::{error::Error, fmt, num::NonZeroUsize};

use hq_application::{
    AgentSessionRequest, AgentSessionResult, ApplicationError, ApplicationPorts, CommitFacts,
    ConfigureRelays, ControlHarness, ControlProjects, EffectOutcome, EffectRequest, FactMutation,
    InspectResource, MutationAttempt, ObserveRevisions, ProjectCommandOutcome,
    ProjectCommandRequest, PublishWake, QueryDomain, RelayConfiguration, ResourceInspectionRequest,
    ResourceInspectionResult, SubscriptionRequest, SynchronizationRequest, WakeDisposition,
};
use hq_domain::{Page, PageCursor, Revision};
use hq_local_api::RevisionHub;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use hq_local_api::protocol::v1::{BuildMetadata, Id32, LifecycleState, LifecycleStatus};
use hq_reducer::{AuthorityPolicy, ConversationKey};
use hq_store::StoreGateway;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::{
    AcceptedLocalStream, LocalSessionPump, LocalSessionPumpConfig, LocalSessionPumpOpenError,
    NodePhase, ReadinessRecord, RuntimeArtifactError, local_transport::ready_record,
};
use crate::{
    CancellationToken, NodeAdmission, NodeFoundation, NodeLifecycleError, TaskJoinReport,
    TaskTracker,
};

/// Closed long-lived component catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentKind {
    /// Local client sessions and subscription ownership.
    LocalSessions,
    /// Relay ingress/outbound durable handoff owner.
    RelayManager,
    /// Provider-neutral managed-runtime owner.
    HarnessSupervisor,
    /// Project saga and resource-observation owner.
    ProjectWorkflows,
}

/// Stable component lifecycle failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentError;

impl ComponentError {
    /// Constructs a redacted unavailable/failure result.
    pub const fn unavailable() -> Self {
        Self
    }
}

impl fmt::Display for ComponentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("node component operation failed")
    }
}

impl Error for ComponentError {}

/// Component drain acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentDrain {
    /// Every accepted item was drained and owned execution stopped.
    Complete,
    /// The component checkpointed uncertainty but requires forced escalation.
    Escalate,
}

/// Real ownership seam shared by deterministic fakes and later concrete adapters.
pub trait NodeComponent {
    /// Starts owned execution and returns only after readiness acknowledgement.
    fn start(&mut self, cancellation: CancellationToken) -> Result<(), ComponentError>;
    /// Stops accepting new work without discarding accepted items.
    fn stop_intake(&mut self) -> Result<(), ComponentError>;
    /// Drains accepted work or requests explicit escalation.
    fn drain(&mut self) -> Result<ComponentDrain, ComponentError>;
    /// Performs last-resort idempotent forced stop.
    fn force_stop(&mut self) -> Result<(), ComponentError>;
}

/// Four concrete owners retained without erasing their application capabilities.
#[derive(Debug)]
pub struct NodeComponents<L, R, H, P> {
    local: L,
    relay: R,
    harness: H,
    project: P,
}

impl<L, R, H, P> NodeComponents<L, R, H, P> {
    /// Constructs the closed component set.
    pub const fn new(local: L, relay: R, harness: H, project: P) -> Self {
        Self {
            local,
            relay,
            harness,
            project,
        }
    }
}

/// Startup failure after deterministic partial rollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeOwnerStartError {
    /// A required component did not acknowledge readiness.
    Component(ComponentKind),
    /// Foundation readiness or revision fanout initialization failed.
    Foundation,
}

impl fmt::Display for NodeOwnerStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Component(component) => {
                write!(formatter, "node component {component:?} failed startup")
            }
            Self::Foundation => formatter.write_str("node foundation failed readiness"),
        }
    }
}

impl Error for NodeOwnerStartError {}

/// Cleanup stage that reported a typed issue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownStage {
    /// Intake closure failed.
    StopIntake,
    /// Graceful drain failed.
    Drain,
    /// Forced escalation failed.
    ForceStop,
    /// A tracked task failed or panicked.
    Tasks,
    /// Store/foundation shutdown did not acknowledge cleanly.
    Foundation,
}

/// One shutdown issue that never prevents later cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownIssue {
    /// The component, or `None` for node task/foundation ownership.
    pub component: Option<ComponentKind>,
    /// Cleanup stage that reported the issue.
    pub stage: ShutdownStage,
}

/// Complete node shutdown report.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeShutdownReport {
    /// Every stable issue accumulated while continuing cleanup.
    pub issues: Vec<ShutdownIssue>,
    /// Components that required forced escalation.
    pub escalated: Vec<ComponentKind>,
    /// Complete accepted-task join report.
    pub tasks: TaskJoinReport,
}

/// Transient complete application capability bundle borrowing the concrete component owners.
pub struct NodeApplicationPorts<'a, R, H, P> {
    store: StoreGateway,
    revisions: &'a RevisionHub,
    relay: &'a R,
    harness: &'a H,
    project: &'a P,
}

impl<R, H, P> QueryDomain for NodeApplicationPorts<'_, R, H, P> {
    fn authoritative_snapshot(
        &self,
    ) -> Result<hq_application::AuthoritativeSnapshot, ApplicationError> {
        self.store.authoritative_snapshot()
    }

    fn conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<hq_application::ConversationEntry>, ApplicationError> {
        self.store.conversation_entries(key, limit, cursor)
    }
}

impl<R, H, P> CommitFacts for NodeApplicationPorts<'_, R, H, P> {
    fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
        self.store.commit_facts(request)
    }
}

impl<R: PublishWake, H, P> PublishWake for NodeApplicationPorts<'_, R, H, P> {
    fn publish_wake(&self, revision: Revision) -> Result<WakeDisposition, ApplicationError> {
        self.relay.publish_wake(revision)
    }
}

impl<R: ConfigureRelays, H, P> ConfigureRelays for NodeApplicationPorts<'_, R, H, P> {
    fn configure_relay(
        &self,
        request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.relay.configure_relay(request)
    }

    fn synchronize(
        &self,
        request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.relay.synchronize(request)
    }
}

impl<R, H: ControlHarness, P> ControlHarness for NodeApplicationPorts<'_, R, H, P> {
    fn control_harness(
        &self,
        request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        self.harness.control_harness(request)
    }
}

impl<R, H, P: InspectResource> InspectResource for NodeApplicationPorts<'_, R, H, P> {
    fn inspect_resource(
        &self,
        request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        self.project.inspect_resource(request)
    }
}

impl<R, H, P: ControlProjects> ControlProjects for NodeApplicationPorts<'_, R, H, P> {
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.project.control_project(request)
    }
}

impl<R, H, P> ObserveRevisions for NodeApplicationPorts<'_, R, H, P> {
    fn register_subscription(&self, request: &SubscriptionRequest) -> Result<(), ApplicationError> {
        self.revisions.register_subscription(request)
    }

    fn activate_subscription(
        &self,
        operation_id: hq_domain::OperationId,
    ) -> Result<(), ApplicationError> {
        self.revisions.activate_subscription(operation_id)
    }

    fn cancel_subscription(
        &self,
        operation_id: hq_domain::OperationId,
    ) -> Result<(), ApplicationError> {
        self.revisions.cancel_subscription(operation_id)
    }
}

impl<R, H, P> ApplicationPorts for NodeApplicationPorts<'_, R, H, P>
where
    R: PublishWake + ConfigureRelays,
    H: ControlHarness,
    P: InspectResource + ControlProjects,
{
}

/// Sole runtime owner over foundations, components, cancellation, tasks, and revision fanout.
pub struct NodeOwner<L: NodeComponent, R: NodeComponent, H: NodeComponent, P: NodeComponent> {
    foundation: Option<NodeFoundation>,
    components: Option<NodeComponents<L, R, H, P>>,
    cancellation: CancellationToken,
    tasks: TaskTracker,
    revisions: RevisionHub,
}

impl<L: NodeComponent, R: NodeComponent, H: NodeComponent, P: NodeComponent> NodeOwner<L, R, H, P> {
    /// Starts every required component in dependency order and then acknowledges node readiness.
    pub fn start(
        mut foundation: NodeFoundation,
        mut components: NodeComponents<L, R, H, P>,
        task_capacity: NonZeroUsize,
        subscription_capacity: NonZeroUsize,
    ) -> Result<Self, NodeOwnerStartError> {
        let cancellation = CancellationToken::new();
        if start_one(&mut components.local, &cancellation).is_err() {
            return Err(NodeOwnerStartError::Component(ComponentKind::LocalSessions));
        }
        if start_one(&mut components.relay, &cancellation).is_err() {
            rollback(&mut components, 1);
            return Err(NodeOwnerStartError::Component(ComponentKind::RelayManager));
        }
        if start_one(&mut components.harness, &cancellation).is_err() {
            rollback(&mut components, 2);
            return Err(NodeOwnerStartError::Component(
                ComponentKind::HarnessSupervisor,
            ));
        }
        if start_one(&mut components.project, &cancellation).is_err() {
            rollback(&mut components, 3);
            return Err(NodeOwnerStartError::Component(
                ComponentKind::ProjectWorkflows,
            ));
        }
        let started = 4;
        if foundation.mark_ready().is_err() {
            rollback(&mut components, started);
            return Err(NodeOwnerStartError::Foundation);
        }
        let revisions = RevisionHub::new(subscription_capacity.get()).map_err(|_| {
            rollback(&mut components, started);
            NodeOwnerStartError::Foundation
        })?;
        Ok(Self {
            foundation: Some(foundation),
            components: Some(components),
            cancellation,
            tasks: TaskTracker::new(task_capacity),
            revisions,
        })
    }

    /// Returns current pure admission policy.
    pub fn admits(&self, admission: NodeAdmission) -> bool {
        self.foundation
            .as_ref()
            .is_some_and(|foundation| foundation.admits(admission))
    }

    /// Returns the root cancellation observer; only this owner invokes root cancel.
    pub const fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Mutably borrows the fixed-capacity accepted task tracker.
    pub const fn tasks_mut(&mut self) -> &mut TaskTracker {
        &mut self.tasks
    }

    /// Returns the shared fixed-capacity revision hub.
    pub const fn revisions(&self) -> &RevisionHub {
        &self.revisions
    }

    /// Borrows one complete application port bundle without exposing storage or signer ownership.
    pub fn application_ports(
        &self,
        policy: AuthorityPolicy,
    ) -> Option<NodeApplicationPorts<'_, R, H, P>>
    where
        R: PublishWake + ConfigureRelays,
        H: ControlHarness,
        P: InspectResource + ControlProjects,
    {
        let foundation = self.foundation.as_ref()?;
        let components = self.components.as_ref()?;
        let store = foundation.store()?;
        Some(NodeApplicationPorts {
            store: StoreGateway::new(store, policy, foundation.signer_handle()),
            revisions: &self.revisions,
            relay: &components.relay,
            harness: &components.harness,
            project: &components.project,
        })
    }

    /// Binds the private local listener through the retained foundation owner.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn bind_local_listener(&mut self) -> Result<(), RuntimeArtifactError> {
        self.foundation
            .as_mut()
            .ok_or_else(RuntimeArtifactError::from_shutdown_state)?
            .bind_local_listener()
    }

    /// Accepts one waiting same-user local connection.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn accept_local(&self) -> Result<AcceptedLocalStream, RuntimeArtifactError> {
        self.foundation
            .as_ref()
            .ok_or_else(RuntimeArtifactError::from_shutdown_state)?
            .accept_local()
    }

    /// Atomically publishes diagnostic readiness for this acknowledged node generation.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn publish_readiness(
        &mut self,
        build: BuildMetadata,
        boot_nonce: Id32,
    ) -> Result<ReadinessRecord, RuntimeArtifactError> {
        let foundation = self
            .foundation
            .as_mut()
            .ok_or_else(RuntimeArtifactError::from_shutdown_state)?;
        if foundation.lifecycle().phase() != NodePhase::Ready {
            return Err(RuntimeArtifactError::from_nonready_state());
        }
        let revision = foundation
            .lifecycle()
            .revision()
            .ok_or_else(RuntimeArtifactError::from_shutdown_state)?;
        let record = ready_record(
            build,
            foundation.public_identity().installation_id(),
            revision,
            boot_nonce,
        )?;
        foundation.publish_readiness(&record)?;
        Ok(record)
    }

    /// Binds, publishes, and transfers the private listener into the sole session pump.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn open_local_session_pump(
        &mut self,
        config: LocalSessionPumpConfig,
        build: BuildMetadata,
    ) -> Result<(LocalSessionPump, ReadinessRecord), LocalSessionPumpOpenError> {
        self.bind_local_listener()
            .map_err(|error| LocalSessionPumpOpenError::Bind(error.class()))?;
        let record = self
            .publish_readiness(build.clone(), config.boot_nonce)
            .map_err(|error| LocalSessionPumpOpenError::Publish(error.class()))?;
        let foundation = self
            .foundation
            .as_mut()
            .ok_or(LocalSessionPumpOpenError::Start(
                crate::LocalSessionPumpStartError::Listener(
                    crate::RuntimeArtifactErrorClass::NotBound,
                ),
            ))?;
        let pump = LocalSessionPump::start(foundation, config, self.revisions.clone(), build)
            .map_err(LocalSessionPumpOpenError::Start)?;
        Ok((pump, record))
    }

    /// Projects the current node-owned lifecycle into the local protocol DTO.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn lifecycle_status(
        &self,
        build: BuildMetadata,
    ) -> Result<LifecycleStatus, NodeLifecycleError> {
        let lifecycle = self
            .foundation
            .as_ref()
            .ok_or(NodeLifecycleError)?
            .lifecycle();
        let state = match lifecycle.phase() {
            NodePhase::Starting => LifecycleState::Starting,
            NodePhase::Ready => LifecycleState::Ready,
            NodePhase::Draining => LifecycleState::Draining,
            NodePhase::Failed => LifecycleState::Failed,
            NodePhase::Stopped => LifecycleState::Stopped,
        };
        Ok(LifecycleStatus {
            state,
            build,
            revision: lifecycle.revision(),
            detail: None,
        })
    }

    /// Enters an orderly stop drain without replacing an existing restart intent.
    pub fn request_stop(&mut self) -> Result<(), NodeLifecycleError> {
        self.foundation
            .as_mut()
            .ok_or(NodeLifecycleError)?
            .begin_drain()
            .map(|_| ())
    }

    /// Enters drain while retaining clean-restart intent.
    pub fn request_restart(&mut self) -> Result<(), NodeLifecycleError> {
        self.foundation
            .as_mut()
            .ok_or(NodeLifecycleError)?
            .begin_restart()
            .map(|_| ())
    }

    /// Completes every cleanup stage and returns its accumulated report.
    pub fn shutdown(mut self) -> NodeShutdownReport {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> NodeShutdownReport {
        let mut report = NodeShutdownReport::default();
        if let Some(foundation) = self.foundation.as_mut() {
            let _ = foundation.begin_drain();
        }
        if let Some(components) = self.components.as_mut() {
            for (kind, component) in [
                (
                    ComponentKind::LocalSessions,
                    &mut components.local as &mut dyn NodeComponent,
                ),
                (
                    ComponentKind::RelayManager,
                    &mut components.relay as &mut dyn NodeComponent,
                ),
                (
                    ComponentKind::HarnessSupervisor,
                    &mut components.harness as &mut dyn NodeComponent,
                ),
                (
                    ComponentKind::ProjectWorkflows,
                    &mut components.project as &mut dyn NodeComponent,
                ),
            ] {
                if component.stop_intake().is_err() {
                    report.issues.push(ShutdownIssue {
                        component: Some(kind),
                        stage: ShutdownStage::StopIntake,
                    });
                }
            }
            self.cancellation.cancel();
            for (kind, component) in [
                (
                    ComponentKind::ProjectWorkflows,
                    &mut components.project as &mut dyn NodeComponent,
                ),
                (
                    ComponentKind::HarnessSupervisor,
                    &mut components.harness as &mut dyn NodeComponent,
                ),
                (
                    ComponentKind::RelayManager,
                    &mut components.relay as &mut dyn NodeComponent,
                ),
                (
                    ComponentKind::LocalSessions,
                    &mut components.local as &mut dyn NodeComponent,
                ),
            ] {
                let drain = component.drain();
                match drain {
                    Ok(ComponentDrain::Complete) => {}
                    Ok(ComponentDrain::Escalate) | Err(_) => {
                        if drain.is_err() {
                            report.issues.push(ShutdownIssue {
                                component: Some(kind),
                                stage: ShutdownStage::Drain,
                            });
                        }
                        report.escalated.push(kind);
                        if component.force_stop().is_err() {
                            report.issues.push(ShutdownIssue {
                                component: Some(kind),
                                stage: ShutdownStage::ForceStop,
                            });
                        }
                    }
                }
            }
        }
        self.tasks.close_intake();
        report.tasks = self.tasks.join_all();
        if !report.tasks.failures.is_empty() {
            report.issues.push(ShutdownIssue {
                component: None,
                stage: ShutdownStage::Tasks,
            });
        }
        self.components.take();
        if self
            .foundation
            .take()
            .is_some_and(|foundation| foundation.shutdown().is_err())
        {
            report.issues.push(ShutdownIssue {
                component: None,
                stage: ShutdownStage::Foundation,
            });
        }
        report
    }
}

impl<L: NodeComponent, R: NodeComponent, H: NodeComponent, P: NodeComponent> Drop
    for NodeOwner<L, R, H, P>
{
    fn drop(&mut self) {
        if self.foundation.is_some() || self.components.is_some() {
            let _ = self.shutdown_inner();
        }
    }
}

impl<
    L: NodeComponent + fmt::Debug,
    R: NodeComponent + fmt::Debug,
    H: NodeComponent + fmt::Debug,
    P: NodeComponent + fmt::Debug,
> fmt::Debug for NodeOwner<L, R, H, P>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeOwner")
            .field("foundation", &self.foundation)
            .field("task_count", &self.tasks.live_count())
            .finish_non_exhaustive()
    }
}

fn rollback<L: NodeComponent, R: NodeComponent, H: NodeComponent, P: NodeComponent>(
    components: &mut NodeComponents<L, R, H, P>,
    started: u8,
) {
    if started >= 4 {
        rollback_one(&mut components.project);
    }
    if started >= 3 {
        rollback_one(&mut components.harness);
    }
    if started >= 2 {
        rollback_one(&mut components.relay);
    }
    if started >= 1 {
        rollback_one(&mut components.local);
    }
}

fn start_one(
    component: &mut dyn NodeComponent,
    cancellation: &CancellationToken,
) -> Result<(), ComponentError> {
    match component.start(cancellation.child()) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = component.force_stop();
            Err(error)
        }
    }
}

fn rollback_one(component: &mut dyn NodeComponent) {
    let _ = component.stop_intake();
    match component.drain() {
        Ok(ComponentDrain::Complete) => {}
        Ok(ComponentDrain::Escalate) | Err(_) => {
            let _ = component.force_stop();
        }
    }
}
