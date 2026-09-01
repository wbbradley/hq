//! Bounded node component ownership, rollback, and drain contracts.

#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    num::NonZeroUsize,
    sync::{Arc, Barrier, Mutex},
};

use hq_application::{
    AgentSessionRequest, AgentSessionResult, ApplicationError, ApplicationErrorCode,
    ApplicationPorts, ConfigureRelays, ControlHarness, EffectOutcome, EffectRequest,
    InspectResource, ObserveRevisions, ProjectCommandOutcome, ProjectCommandRequest, PublishWake,
    QueryDomain, RelayConfiguration, ResourceInspectionRequest, ResourceInspectionResult,
    SubscriptionRequest, SubscriptionTopic, SynchronizationRequest, WakeDisposition,
};
use hq_domain::{MailboxId, OperationId, Revision};
use hq_node::{
    CancellationToken, ComponentDrain, ComponentError, ComponentKind, MailboxSendError,
    NodeAdmission, NodeComponent, NodeComponents, NodeFoundation, NodeFoundationConfig, NodeOwner,
    ProjectMessageReconciliation, ReconcileProjectMessages, RuntimePaths,
    ScheduleProjectReconciliation, ShutdownIssue, ShutdownStage, StateDirectoryOwner, StatePaths,
    TaskError, TaskFailureKind, TaskTracker, TaskTrackerError, bounded_mailbox,
};
use hq_projects::{ProjectInputReconciliation, ReconcileProjectInputs};
use hq_reducer::AuthorityPolicy;

use support::TestDirectory;

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
struct FakeComponent {
    kind: ComponentKind,
    trace: Arc<Mutex<Vec<String>>>,
    fail_start: bool,
    fail_stop: bool,
    fail_drain: bool,
    escalate: bool,
}

impl ReconcileProjectInputs for FakeComponent {
    fn reconcile_project_inputs(
        &self,
        _limit: usize,
    ) -> Result<ProjectInputReconciliation, ApplicationError> {
        Ok(ProjectInputReconciliation {
            accepted: 0,
            truncated: false,
        })
    }
}

impl ReconcileProjectMessages for FakeComponent {
    fn reconcile_project_messages(
        &self,
        _limit: usize,
    ) -> Result<ProjectMessageReconciliation, ApplicationError> {
        self.record("messages");
        Ok(ProjectMessageReconciliation {
            inputs: ProjectInputReconciliation {
                accepted: 0,
                truncated: false,
            },
            automatic_commands: 0,
            truncated: false,
        })
    }
}

impl ScheduleProjectReconciliation for FakeComponent {
    fn schedule_project_reconciliation(&self) {
        self.record("schedule_messages");
    }
}

impl FakeComponent {
    fn new(kind: ComponentKind, trace: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            kind,
            trace,
            fail_start: false,
            fail_stop: false,
            fail_drain: false,
            escalate: false,
        }
    }

    fn record(&self, action: &str) {
        self.trace
            .lock()
            .expect("trace lock")
            .push(format!("{:?}:{action}", self.kind));
    }

    const fn with_start_failure(mut self) -> Self {
        self.fail_start = true;
        self
    }

    const fn with_stop_failure(mut self) -> Self {
        self.fail_stop = true;
        self
    }

    const fn with_drain_failure(mut self) -> Self {
        self.fail_drain = true;
        self
    }

    const fn with_escalation(mut self) -> Self {
        self.escalate = true;
        self
    }
}

impl NodeComponent for FakeComponent {
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        self.record("start");
        if self.fail_start {
            Err(ComponentError::unavailable())
        } else {
            Ok(())
        }
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.record("stop-intake");
        if self.fail_stop {
            Err(ComponentError::unavailable())
        } else {
            Ok(())
        }
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        self.record("drain");
        if self.fail_drain {
            return Err(ComponentError::unavailable());
        }
        Ok(if self.escalate {
            ComponentDrain::Escalate
        } else {
            ComponentDrain::Complete
        })
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        self.record("force-stop");
        Ok(())
    }
}

impl PublishWake for FakeComponent {
    fn publish_wake(&self, _revision: Revision) -> Result<WakeDisposition, ApplicationError> {
        self.record("publish-wake");
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl ConfigureRelays for FakeComponent {
    fn configure_relay(
        &self,
        _request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.record("configure-relay");
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }

    fn synchronize(
        &self,
        _request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.record("synchronize");
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl ControlHarness for FakeComponent {
    fn control_harness(
        &self,
        _request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        self.record("control-harness");
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl hq_application::QueryProviders for FakeComponent {
    fn provider_catalog(&self) -> Result<hq_application::ProviderCatalog, ApplicationError> {
        self.record("provider-catalog");
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl InspectResource for FakeComponent {
    fn inspect_resource(
        &self,
        _request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        self.record("inspect-resource");
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl hq_application::ControlProjects for FakeComponent {
    fn control_project(
        &self,
        _request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.record("control-project");
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

impl hq_application::RetireAgents for FakeComponent {
    fn retire_agent(
        &self,
        _request: hq_application::AgentRetirementRequest,
    ) -> Result<hq_application::AgentRetirementOutcome, ApplicationError> {
        Err(ApplicationError::new(
            hq_application::ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}

fn foundation(directory: &TestDirectory) -> (NodeFoundation, StatePaths) {
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity");
    drop(owner);
    let runtime = RuntimePaths::new(directory.path().join("runtime")).expect("runtime paths");
    let foundation = NodeFoundation::open(NodeFoundationConfig::new(
        state.clone(),
        runtime,
        NonZeroUsize::new(8).expect("store capacity"),
    ))
    .expect("foundation");
    (foundation, state)
}

fn components_failing_start(
    trace: &Arc<Mutex<Vec<String>>>,
    failed: ComponentKind,
) -> NodeComponents<FakeComponent, FakeComponent, FakeComponent, FakeComponent> {
    let component = |kind| {
        let component = FakeComponent::new(kind, Arc::clone(trace));
        if kind == failed {
            component.with_start_failure()
        } else {
            component
        }
    };
    NodeComponents::new(
        component(ComponentKind::LocalSessions),
        component(ComponentKind::RelayManager),
        component(ComponentKind::HarnessSupervisor),
        component(ComponentKind::ProjectWorkflows),
    )
}

#[test]
fn cancellation_mailboxes_and_tasks_are_hierarchical_bounded_and_joined() {
    let root = CancellationToken::new();
    let left = root.child();
    let left_child = left.child();
    let right = root.child();
    left.cancel();
    assert!(left.is_cancelled());
    assert!(left_child.is_cancelled());
    assert!(!right.is_cancelled());
    root.cancel();
    assert!(right.is_cancelled());

    let retained_root = CancellationToken::new();
    let retained_grandchild = retained_root.child().child();
    retained_root.cancel();
    assert!(retained_grandchild.is_cancelled());

    let (sender, receiver) = bounded_mailbox(NonZeroUsize::new(1).expect("capacity"));
    sender.try_send(1).expect("first item");
    assert_eq!(sender.try_send(2), Err(MailboxSendError::Full(2)));
    assert_eq!(receiver.try_receive(), Ok(1));
    assert_eq!(
        receiver.try_receive(),
        Err(hq_node::MailboxReceiveError::Empty)
    );
    receiver.close();
    assert_eq!(sender.try_send(3), Err(MailboxSendError::Closed(3)));
    assert_eq!(
        receiver.try_receive(),
        Err(hq_node::MailboxReceiveError::Closed)
    );

    let (orphaned_sender, orphaned_receiver) =
        bounded_mailbox(NonZeroUsize::new(1).expect("capacity"));
    drop(orphaned_receiver);
    assert_eq!(
        orphaned_sender.try_send(4),
        Err(MailboxSendError::Closed(4))
    );

    let mut tracker = TaskTracker::new(NonZeroUsize::new(3).expect("task capacity"));
    assert_eq!(
        tracker.spawn("", || Ok(())),
        Err(TaskTrackerError::InvalidName)
    );
    tracker
        .spawn("clean", || Ok(()))
        .expect("clean task accepted");
    tracker
        .spawn("failed", || Err(TaskError::failed()))
        .expect("failed task retained");
    tracker
        .spawn("panicked", || panic!("scripted task panic"))
        .expect("panicked task retained");
    assert_eq!(
        tracker.spawn("excess", || Ok(())),
        Err(TaskTrackerError::Full)
    );
    tracker.close_intake();
    assert_eq!(
        tracker.spawn("closed", || Ok(())),
        Err(TaskTrackerError::Closed)
    );
    let report = tracker.join_all();
    assert_eq!(report.joined, 3);
    assert_eq!(report.failures.len(), 2);
    assert_eq!(tracker.live_count(), 0);
}

#[test]
fn every_component_startup_position_rolls_back_and_releases_foundation() {
    for failed in [
        ComponentKind::LocalSessions,
        ComponentKind::RelayManager,
        ComponentKind::HarnessSupervisor,
        ComponentKind::ProjectWorkflows,
    ] {
        let directory = TestDirectory::new();
        let (foundation, state) = foundation(&directory);
        let trace = Arc::new(Mutex::new(Vec::new()));
        let components = components_failing_start(&trace, failed);
        let error = NodeOwner::start(
            foundation,
            components,
            NonZeroUsize::new(1).expect("tasks"),
            NonZeroUsize::new(1).expect("subscriptions"),
        )
        .expect_err("scripted component fails");
        assert_eq!(error, hq_node::NodeOwnerStartError::Component(failed));
        let expected = match failed {
            ComponentKind::LocalSessions => vec!["LocalSessions:start", "LocalSessions:force-stop"],
            ComponentKind::RelayManager => vec![
                "LocalSessions:start",
                "RelayManager:start",
                "RelayManager:force-stop",
                "LocalSessions:stop-intake",
                "LocalSessions:drain",
            ],
            ComponentKind::HarnessSupervisor => vec![
                "LocalSessions:start",
                "RelayManager:start",
                "HarnessSupervisor:start",
                "HarnessSupervisor:force-stop",
                "RelayManager:stop-intake",
                "RelayManager:drain",
                "LocalSessions:stop-intake",
                "LocalSessions:drain",
            ],
            ComponentKind::ProjectWorkflows => vec![
                "LocalSessions:start",
                "RelayManager:start",
                "HarnessSupervisor:start",
                "ProjectWorkflows:start",
                "ProjectWorkflows:force-stop",
                "HarnessSupervisor:stop-intake",
                "HarnessSupervisor:drain",
                "RelayManager:stop-intake",
                "RelayManager:drain",
                "LocalSessions:stop-intake",
                "LocalSessions:drain",
            ],
        };
        assert_eq!(*trace.lock().expect("trace"), expected);
        let owner = StateDirectoryOwner::acquire(state).expect("failed start releases foundation");
        drop(owner);
    }
}

#[test]
fn startup_failure_force_stops_partial_component_and_rolls_back_in_reverse() {
    let directory = TestDirectory::new();
    let (foundation, state) = foundation(&directory);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let components = components_failing_start(&trace, ComponentKind::HarnessSupervisor);
    let error = NodeOwner::start(
        foundation,
        components,
        NonZeroUsize::new(4).expect("tasks"),
        NonZeroUsize::new(4).expect("subscriptions"),
    )
    .expect_err("harness startup fails");
    assert_eq!(
        error,
        hq_node::NodeOwnerStartError::Component(ComponentKind::HarnessSupervisor)
    );
    assert_eq!(
        *trace.lock().expect("trace"),
        [
            "LocalSessions:start",
            "RelayManager:start",
            "HarnessSupervisor:start",
            "HarnessSupervisor:force-stop",
            "RelayManager:stop-intake",
            "RelayManager:drain",
            "LocalSessions:stop-intake",
            "LocalSessions:drain",
        ]
    );
    let owner = StateDirectoryOwner::acquire(state).expect("foundation ownership rolled back");
    drop(owner);
}

#[test]
fn transient_application_ports_delegate_store_revision_and_owned_effect_capabilities() {
    fn assert_complete_ports(_ports: &impl ApplicationPorts) {}

    let directory = TestDirectory::new();
    let (foundation, _state) = foundation(&directory);
    let installation = foundation.public_identity().installation_id;
    let trace = Arc::new(Mutex::new(Vec::new()));
    let components = NodeComponents::new(
        FakeComponent::new(ComponentKind::LocalSessions, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::RelayManager, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::HarnessSupervisor, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::ProjectWorkflows, Arc::clone(&trace)),
    );
    let node = NodeOwner::start(
        foundation,
        components,
        NonZeroUsize::new(1).expect("tasks"),
        NonZeroUsize::new(2).expect("subscriptions"),
    )
    .expect("node starts");
    let operation = OperationId::from_bytes([0x51; 32]);
    let request = SubscriptionRequest::new(operation, [SubscriptionTopic::All])
        .expect("subscription request");
    let ports = node
        .application_ports(AuthorityPolicy::new(
            installation,
            MailboxId::from_bytes([0x52; 32]),
        ))
        .expect("foundation retains store");
    assert_complete_ports(&ports);
    assert_eq!(
        ports.authoritative_snapshot(),
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable
        ))
    );
    ports
        .register_subscription(&request)
        .expect("revision registration delegates");
    ports
        .activate_subscription(operation)
        .expect("revision activation delegates");
    assert_eq!(
        ports.publish_wake(Revision::new(1)),
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable
        ))
    );
    drop(ports);
    assert_eq!(node.revisions().len(), 1);
    assert_eq!(
        trace.lock().expect("trace").last().map(String::as_str),
        Some("RelayManager:publish-wake")
    );
    let _ = node.shutdown();
}

#[test]
fn shutdown_closes_admission_drains_in_order_escalates_and_releases_every_owner() {
    let directory = TestDirectory::new();
    let (foundation, state) = foundation(&directory);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let components = NodeComponents::new(
        FakeComponent::new(ComponentKind::LocalSessions, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::RelayManager, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::HarnessSupervisor, Arc::clone(&trace)).with_escalation(),
        FakeComponent::new(ComponentKind::ProjectWorkflows, Arc::clone(&trace)),
    );
    let mut node = NodeOwner::start(
        foundation,
        components,
        NonZeroUsize::new(4).expect("tasks"),
        NonZeroUsize::new(4).expect("subscriptions"),
    )
    .expect("node starts");
    assert!(node.admits(NodeAdmission::Mutation));
    node.request_restart().expect("restart enters drain");
    assert!(!node.admits(NodeAdmission::Mutation));
    let token = node.cancellation().child();
    node.tasks_mut()
        .spawn("observes-cancel", {
            move || {
                while !token.is_cancelled() {
                    std::thread::yield_now();
                }
                Ok(())
            }
        })
        .expect("task starts");
    let report = node.shutdown();
    assert!(report.issues.is_empty());
    assert_eq!(report.tasks.joined, 1);
    assert_eq!(report.escalated, [ComponentKind::HarnessSupervisor]);
    assert_eq!(
        *trace.lock().expect("trace"),
        [
            "LocalSessions:start",
            "RelayManager:start",
            "HarnessSupervisor:start",
            "ProjectWorkflows:start",
            "LocalSessions:stop-intake",
            "RelayManager:stop-intake",
            "HarnessSupervisor:stop-intake",
            "ProjectWorkflows:stop-intake",
            "ProjectWorkflows:drain",
            "HarnessSupervisor:drain",
            "HarnessSupervisor:force-stop",
            "RelayManager:drain",
            "LocalSessions:drain",
        ]
    );
    let owner = StateDirectoryOwner::acquire(state).expect("all ownership released");
    drop(owner);
}

#[test]
fn mutation_admission_racing_drain_serializes_to_ready_or_rejected_then_closes() {
    let directory = TestDirectory::new();
    let (foundation, state) = foundation(&directory);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let components = NodeComponents::new(
        FakeComponent::new(ComponentKind::LocalSessions, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::RelayManager, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::HarnessSupervisor, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::ProjectWorkflows, trace),
    );
    let node = NodeOwner::start(
        foundation,
        components,
        NonZeroUsize::new(1).expect("tasks"),
        NonZeroUsize::new(1).expect("subscriptions"),
    )
    .expect("node starts");
    let node = Arc::new(Mutex::new(Some(node)));
    let start = Arc::new(Barrier::new(3));

    let mutation_node = Arc::clone(&node);
    let mutation_start = Arc::clone(&start);
    let mutation = std::thread::spawn(move || {
        mutation_start.wait();
        mutation_node
            .lock()
            .expect("node lock")
            .as_ref()
            .expect("node remains owned")
            .admits(NodeAdmission::Mutation)
    });
    let drain_node = Arc::clone(&node);
    let drain_start = Arc::clone(&start);
    let drain = std::thread::spawn(move || {
        drain_start.wait();
        drain_node
            .lock()
            .expect("node lock")
            .as_mut()
            .expect("node remains owned")
            .request_restart()
    });
    start.wait();

    let _admitted_before_drain = mutation.join().expect("mutation admission joins");
    drain
        .join()
        .expect("drain transition joins")
        .expect("restart enters drain");
    let mut wrapped_node = Arc::try_unwrap(node)
        .expect("race participants released owner")
        .into_inner()
        .expect("node lock");
    let node = wrapped_node.take().expect("node remains owned");
    assert!(!node.admits(NodeAdmission::Mutation));
    let _ = node.shutdown();
    let state_owner =
        StateDirectoryOwner::acquire(state).expect("race shutdown releases ownership");
    drop(state_owner);
}

#[test]
fn shutdown_issues_never_skip_later_components_tasks_or_foundation_release() {
    let directory = TestDirectory::new();
    let (foundation, state) = foundation(&directory);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let components = NodeComponents::new(
        FakeComponent::new(ComponentKind::LocalSessions, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::RelayManager, Arc::clone(&trace)).with_stop_failure(),
        FakeComponent::new(ComponentKind::HarnessSupervisor, Arc::clone(&trace)),
        FakeComponent::new(ComponentKind::ProjectWorkflows, trace).with_drain_failure(),
    );
    let mut node = NodeOwner::start(
        foundation,
        components,
        NonZeroUsize::new(1).expect("tasks"),
        NonZeroUsize::new(1).expect("subscriptions"),
    )
    .expect("node starts");
    node.tasks_mut()
        .spawn("failed-cleanup-task", || Err(TaskError::failed()))
        .expect("task is retained before shutdown");
    let report = node.shutdown();
    assert_eq!(report.tasks.joined, 1);
    assert_eq!(report.tasks.failures.len(), 1);
    assert_eq!(report.tasks.failures[0].kind, TaskFailureKind::Failed);
    assert_eq!(
        report.issues,
        [
            ShutdownIssue {
                component: Some(ComponentKind::RelayManager),
                stage: ShutdownStage::StopIntake,
            },
            ShutdownIssue {
                component: Some(ComponentKind::ProjectWorkflows),
                stage: ShutdownStage::Drain,
            },
            ShutdownIssue {
                component: None,
                stage: ShutdownStage::Tasks,
            },
        ]
    );
    assert_eq!(report.escalated, [ComponentKind::ProjectWorkflows]);
    let owner = StateDirectoryOwner::acquire(state).expect("issues do not leak foundation");
    drop(owner);
}
