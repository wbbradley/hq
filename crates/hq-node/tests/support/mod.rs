#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use hq_application::{
    AgentSessionRequest, AgentSessionResult, Application, ApplicationError, ApplicationErrorCode,
    ApplicationPorts, AuthoritativeSnapshot, CommitFacts, ConfigureRelays, ConversationEntry,
    ConversationKey, EffectOutcome, EffectRequest, FactMutation, InspectResource, MutationAttempt,
    ObserveRevisions, PublishWake, RelayConfiguration, ResourceInspectionRequest,
    ResourceInspectionResult, SubscriptionRequest, SynchronizationRequest, WakeDisposition,
};
use hq_domain::{OperationId, Page, PageCursor, Revision};
use hq_local_api::{
    LifecycleControl, RevisionHub,
    protocol::v1::{LifecycleRequest, LifecycleStatus},
};

pub struct TestDirectory(PathBuf);

#[derive(Clone)]
pub struct UnavailableApplicationPorts {
    hub: RevisionHub,
    snapshot: Option<AuthoritativeSnapshot>,
}

impl UnavailableApplicationPorts {
    pub const fn new(hub: RevisionHub) -> Self {
        Self {
            hub,
            snapshot: None,
        }
    }

    pub fn with_snapshot(hub: RevisionHub, snapshot: AuthoritativeSnapshot) -> Self {
        Self {
            hub,
            snapshot: Some(snapshot),
        }
    }
}

fn unavailable<T>() -> Result<T, ApplicationError> {
    Err(ApplicationError::new(
        ApplicationErrorCode::AdapterUnavailable,
    ))
}

impl hq_application::QueryDomain for UnavailableApplicationPorts {
    fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError> {
        self.snapshot.clone().map_or_else(unavailable, Ok)
    }

    fn conversation_entries(
        &self,
        _key: &ConversationKey,
        _limit: usize,
        _cursor: Option<&PageCursor>,
    ) -> Result<Page<ConversationEntry>, ApplicationError> {
        unavailable()
    }
}

impl CommitFacts for UnavailableApplicationPorts {
    fn commit_facts(&self, _request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
        unavailable()
    }
}

impl PublishWake for UnavailableApplicationPorts {
    fn publish_wake(&self, _revision: Revision) -> Result<WakeDisposition, ApplicationError> {
        unavailable()
    }
}

impl ConfigureRelays for UnavailableApplicationPorts {
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

impl hq_application::ControlHarness for UnavailableApplicationPorts {
    fn control_harness(
        &self,
        _request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        unavailable()
    }
}

impl InspectResource for UnavailableApplicationPorts {
    fn inspect_resource(
        &self,
        _request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        unavailable()
    }
}

impl ObserveRevisions for UnavailableApplicationPorts {
    fn register_subscription(&self, request: &SubscriptionRequest) -> Result<(), ApplicationError> {
        self.hub.register_subscription(request)
    }

    fn activate_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError> {
        self.hub.activate_subscription(operation_id)
    }

    fn cancel_subscription(&self, operation_id: OperationId) -> Result<(), ApplicationError> {
        self.hub.cancel_subscription(operation_id)
    }
}

impl ApplicationPorts for UnavailableApplicationPorts {}

pub fn unavailable_application(hub: RevisionHub) -> Application<UnavailableApplicationPorts> {
    Application::new(UnavailableApplicationPorts::new(hub))
}

pub fn snapshot_application(hub: RevisionHub) -> Application<UnavailableApplicationPorts> {
    Application::new(UnavailableApplicationPorts::with_snapshot(
        hub,
        AuthoritativeSnapshot::new(Revision::new(7), hq_application::DomainSnapshot::empty()),
    ))
}

pub struct UnavailableLifecycle;

impl LifecycleControl for UnavailableLifecycle {
    fn lifecycle(&self, _request: LifecycleRequest) -> Result<LifecycleStatus, ApplicationError> {
        unavailable()
    }
}

impl TestDirectory {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("hq-{}-{unique}", std::process::id()));
        fs::create_dir(&path).expect("test directory creates");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("fixture writes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("fixture mode sets");
    }
}

#[cfg(unix)]
pub fn assert_private_mode(path: &Path, expected: u32) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .expect("metadata exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, expected, "unexpected mode for {}", path.display());
}

#[cfg(not(unix))]
pub fn assert_private_mode(_: &Path, _: u32) {}
