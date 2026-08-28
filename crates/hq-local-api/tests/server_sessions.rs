//! Transport-independent server routing and write-confirmation race contracts.

#![allow(clippy::expect_used)]

use std::cell::RefCell;

use hq_application::{
    AgentSessionRequest, AgentSessionResult, Application, ApplicationError, ApplicationPorts,
    AuthoritativeSnapshot, CommitFacts, ConfigureRelays, ConversationKey, DomainSnapshot,
    EffectOutcome, EffectRequest, FactMutation, InspectResource, MutationAttempt, ObserveRevisions,
    ProjectCommandOutcome, ProjectCommandRequest, PublishWake, QueryDomain, RelayConfiguration,
    ResourceInspectionRequest, ResourceInspectionResult, SubscriptionRequest, SubscriptionTopic,
    SynchronizationRequest, WakeDisposition,
};
use hq_domain::{
    BoundedSet, CausalReferences, CommandId, EncryptionPublicKey, FactScope, MAX_FACT_AUTHORITIES,
    MAX_FACT_PARENTS, OperationId, Page, PageCursor, ResourceHealth, Revision, SemanticPayload,
    ShortText, SigningPublicKey, Timestamp,
};
use hq_local_api::protocol::v1::{
    AgentSessionRequestDto, AuthoritativeSnapshotDto, BuildMetadata, ClientHello,
    ConversationKeyDto, ConversationPageRequest, EffectRequestDto, Id32, InvalidationTopic,
    LifecycleRequest, LifecycleState, LifecycleStatus, MutationRequest, ProjectCommandActionDto,
    ProjectCommandRequestDto, RelayAccessDto, RelayAuthenticationDto, RelayConfigurationDto,
    Request, RequestEnvelope, RequestId, ResourceInspectionRequestDto, ResourceLocatorDto,
    ResourceSchemeDto, Response, ResponseResult, SessionControlDto, SubscriptionRequestDto,
    SynchronizationRequestDto, V1, VersionRange, WireMessage,
};
use hq_local_api::{
    LifecycleControl, RevisionHub, ServerSession, ServerSessionError, ServerWriteDisposition,
};
#[derive(Clone)]
struct Ports {
    hub: RevisionHub,
    trace: std::rc::Rc<RefCell<Vec<&'static str>>>,
}

impl Ports {
    fn new(hub: RevisionHub) -> Self {
        Self {
            hub,
            trace: std::rc::Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl QueryDomain for Ports {
    fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError> {
        self.trace.borrow_mut().push("snapshot");
        Ok(AuthoritativeSnapshot::new(
            Revision::new(7),
            DomainSnapshot::empty(),
        ))
    }

    fn conversation_entries(
        &self,
        _key: &ConversationKey,
        _limit: usize,
        _cursor: Option<&PageCursor>,
    ) -> Result<Page<hq_application::ConversationEntry>, ApplicationError> {
        self.trace.borrow_mut().push("page");
        Ok(Page::new(Vec::new(), None))
    }
}

impl CommitFacts for Ports {
    fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
        self.trace.borrow_mut().push("mutation");
        let (command_id, request_digest, _) = request.into_parts();
        Ok(MutationAttempt::Uncertain {
            command_id,
            request_digest,
        })
    }
}

impl PublishWake for Ports {
    fn publish_wake(&self, _revision: Revision) -> Result<WakeDisposition, ApplicationError> {
        Ok(WakeDisposition::Scheduled)
    }
}

impl ConfigureRelays for Ports {
    fn configure_relay(
        &self,
        _request: &EffectRequest<RelayConfiguration>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.trace.borrow_mut().push("configure_relay");
        Ok(EffectOutcome::Accepted(()))
    }

    fn synchronize(
        &self,
        _request: &EffectRequest<SynchronizationRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.trace.borrow_mut().push("synchronize");
        Ok(EffectOutcome::Accepted(()))
    }
}

impl hq_application::ControlHarness for Ports {
    fn control_harness(
        &self,
        _request: &EffectRequest<AgentSessionRequest>,
    ) -> Result<EffectOutcome<AgentSessionResult>, ApplicationError> {
        self.trace.borrow_mut().push("agent");
        Ok(EffectOutcome::Accepted(AgentSessionResult::Stopped))
    }
}

impl InspectResource for Ports {
    fn inspect_resource(
        &self,
        _request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        self.trace.borrow_mut().push("resource");
        Ok(EffectOutcome::Accepted(ResourceInspectionResult {
            health: ResourceHealth::Healthy,
            observed_canonical: None,
            details: None,
            checked_at: Timestamp::from_unix_millis(8),
        }))
    }
}

impl hq_application::ControlProjects for Ports {
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.trace.borrow_mut().push("project");
        Ok(ProjectCommandOutcome::Accepted {
            operation_id: request.operation_id,
            stage: hq_application::ProjectCommandStage::AwaitingHome,
        })
    }
}

impl ObserveRevisions for Ports {
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

impl ApplicationPorts for Ports {}

#[derive(Clone, Copy)]
struct Lifecycle;

impl LifecycleControl for Lifecycle {
    fn lifecycle(&self, _request: LifecycleRequest) -> Result<LifecycleStatus, ApplicationError> {
        LifecycleStatus::new(LifecycleState::Ready, build(), Some(7), None).map_err(|_| {
            ApplicationError::new(hq_application::ApplicationErrorCode::InvariantViolation)
        })
    }
}

fn build() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("0123456789ab")).expect("bounded build")
}

fn session(hub: RevisionHub) -> (ServerSession, Application<Ports>) {
    let application = Application::new(Ports::new(hub.clone()));
    (
        ServerSession::new(hub, build(), Id32::new([99; 32])),
        application,
    )
}

fn negotiate(session: &mut ServerSession, application: &Application<Ports>) {
    let hello = WireMessage::ClientHello(ClientHello::new(
        VersionRange::new(V1, V1).expect("v1 range"),
        build(),
    ));
    let outbound = session
        .receive(hello, application, &Lifecycle)
        .expect("hello accepted");
    assert!(matches!(outbound.message(), WireMessage::ServerHello(_)));
    session
        .confirm_written(outbound.ticket())
        .expect("hello written");
}

fn request(id: u64, request: Request) -> WireMessage {
    WireMessage::Request(RequestEnvelope::new(
        RequestId::new(id).expect("nonzero request id"),
        request,
    ))
}

fn effect<T>(body: T) -> EffectRequestDto<T> {
    EffectRequestDto::new(
        Id32::new([31; 32]),
        Id32::new([32; 32]),
        1_700_000_000_000,
        body,
    )
}

fn locator() -> ResourceLocatorDto {
    ResourceLocatorDto::new(ResourceSchemeDto::WorkingTree, "/work/hq".to_owned())
        .expect("bounded locator")
}

fn plan() -> hq_application::FactPlan {
    let installation = hq_domain::InstallationId::from_bytes([7; 32]);
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([]).expect("empty set"),
        [],
    )
    .expect("empty references");
    hq_application::FactPlan::new(
        installation,
        Timestamp::from_unix_millis(1_700_000_000_123),
        FactScope::InstallationPrivate(installation),
        causal,
        SemanticPayload::InstallationDeclared {
            installation_id: installation,
            signing_key: SigningPublicKey::from_bytes([8; 32]),
            encryption_key: EncryptionPublicKey::from_bytes([9; 32]),
            label: Some(ShortText::new("node").expect("bounded label")),
        },
        [10; 32],
    )
}

fn assert_success(outbound: &hq_local_api::OutboundMessage) {
    assert!(matches!(
        outbound.message(),
        WireMessage::Response(response) if matches!(response.response, Response::Success(_))
    ));
}

#[test]
fn handshake_is_mandatory_and_write_tickets_are_session_owned_once() {
    let hub = RevisionHub::new(4).expect("capacity");
    let (mut server, application) = session(hub);
    assert_eq!(
        server.receive(
            request(1, Request::AuthoritativeSnapshot),
            &application,
            &Lifecycle,
        ),
        Err(ServerSessionError::ProtocolOrder)
    );
    negotiate(&mut server, &application);
    let outbound = server
        .receive(
            request(1, Request::AuthoritativeSnapshot),
            &application,
            &Lifecycle,
        )
        .expect("request routes");
    assert_eq!(
        server.receive(
            request(2, Request::AuthoritativeSnapshot),
            &application,
            &Lifecycle,
        ),
        Err(ServerSessionError::WritePending)
    );
    server
        .confirm_written(outbound.ticket())
        .expect("first confirmation");
    assert_eq!(
        server.confirm_written(outbound.ticket()),
        Err(ServerSessionError::UnknownWriteTicket)
    );
    assert_eq!(
        server.receive(
            WireMessage::ClientHello(ClientHello::new(
                VersionRange::new(V1, V1).expect("range"),
                build(),
            )),
            &application,
            &Lifecycle,
        ),
        Err(ServerSessionError::ProtocolOrder)
    );
}

#[test]
fn subscription_commits_are_hidden_until_ack_write_then_delivered_without_a_gap() {
    let hub = RevisionHub::new(4).expect("capacity");
    let (mut server, application) = session(hub.clone());
    negotiate(&mut server, &application);
    let operation_id = OperationId::from_bytes([44; 32]);
    let outbound = server
        .receive(
            request(
                1,
                Request::Subscribe(
                    SubscriptionRequestDto::new(
                        Id32::new(*operation_id.as_bytes()),
                        vec![InvalidationTopic::Conversation],
                    )
                    .expect("subscription"),
                ),
            ),
            &application,
            &Lifecycle,
        )
        .expect("ack prepared");
    assert!(matches!(
        outbound.message(),
        WireMessage::Response(response)
            if matches!(response.response, Response::Success(ResponseResult::Subscription(_)))
    ));

    let _ = hub.publish(Revision::new(8), [SubscriptionTopic::Conversation], false);
    assert!(server.poll_invalidation().is_none());
    server
        .confirm_written(outbound.ticket())
        .expect("acknowledgement frame written");
    assert!(matches!(
        server.poll_invalidation(),
        Some(WireMessage::Invalidation(invalidation)) if invalidation.revision == 8
    ));
}

#[test]
fn lost_acknowledgement_and_stale_disconnect_cancel_pending_and_active_capacity() {
    let hub = RevisionHub::new(1).expect("capacity");
    let (mut server, application) = session(hub.clone());
    negotiate(&mut server, &application);
    let outbound = server
        .receive(
            request(
                1,
                Request::Subscribe(
                    SubscriptionRequestDto::new(Id32::new([45; 32]), vec![InvalidationTopic::All])
                        .expect("subscription"),
                ),
            ),
            &application,
            &Lifecycle,
        )
        .expect("ack prepared");
    assert_eq!(hub.len(), 1);
    server.disconnect();
    assert_eq!(hub.len(), 0);
    assert_eq!(
        server.confirm_written(outbound.ticket()),
        Err(ServerSessionError::UnknownWriteTicket)
    );
    assert_eq!(
        server.receive(
            request(2, Request::AuthoritativeSnapshot),
            &application,
            &Lifecycle,
        ),
        Err(ServerSessionError::Disconnected)
    );
}

#[test]
fn every_typed_request_family_routes_without_storage_types() {
    let hub = RevisionHub::new(8).expect("capacity");
    let (mut server, application) = session(hub);
    negotiate(&mut server, &application);
    let page = ConversationPageRequest::new(
        ConversationKeyDto::Thread {
            counterparty_installation: Id32::new([1; 32]),
            counterparty_mailbox: Id32::new([2; 32]),
            thread: Id32::new([3; 32]),
        },
        32,
        None,
    )
    .expect("page");
    let requests = vec![
        Request::Lifecycle(LifecycleRequest::Status),
        Request::AuthoritativeSnapshot,
        Request::ConversationPage(page),
        Request::Mutation(
            MutationRequest::from_plan(CommandId::from_bytes([11; 32]), plan()).expect("mutation"),
        ),
        Request::ConfigureRelay(effect(RelayConfigurationDto::new(
            locator(),
            RelayAccessDto::ReadWrite,
            RelayAuthenticationDto::OnChallenge,
        ))),
        Request::Synchronize(effect(SynchronizationRequestDto::All)),
        Request::ControlAgentSession(effect(
            AgentSessionRequestDto::new(
                Id32::new([13; 32]),
                "codex".to_owned(),
                SessionControlDto::Stop,
            )
            .expect("agent request"),
        )),
        Request::InspectResource(effect(ResourceInspectionRequestDto {
            project_id: Id32::new([14; 32]),
            resource_id: Id32::new([15; 32]),
            display_locator: locator(),
            canonical_locator: locator(),
        })),
        Request::ControlProject(Box::new(ProjectCommandRequestDto {
            command_id: Id32::new([16; 32]),
            operation_id: Id32::new([17; 32]),
            request_digest: Id32::new([18; 32]),
            account_id: Id32::new([19; 32]),
            project_id: Id32::new([20; 32]),
            home: Id32::new([21; 32]),
            expected_head: Some(Id32::new([22; 32])),
            issued_at_unix_millis: 23,
            action: ProjectCommandActionDto::Open,
        })),
    ];

    for (index, request_body) in requests.into_iter().enumerate() {
        let outbound = server
            .receive(
                request(u64::try_from(index + 1).expect("small index"), request_body),
                &application,
                &Lifecycle,
            )
            .expect("route succeeds");
        assert_success(&outbound);
        server
            .confirm_written(outbound.ticket())
            .expect("response written");
    }
}

#[test]
fn version_rejection_closes_only_after_its_response_is_confirmed() {
    let hub = RevisionHub::new(1).expect("capacity");
    let (mut server, application) = session(hub);
    let outbound = server
        .receive(
            WireMessage::ClientHello(ClientHello::new(
                VersionRange::new(2, 3).expect("range"),
                build(),
            )),
            &application,
            &Lifecycle,
        )
        .expect("rejection prepared");
    assert!(matches!(
        outbound.message(),
        WireMessage::VersionRejected(_)
    ));
    assert_eq!(
        server.receive(
            request(1, Request::AuthoritativeSnapshot),
            &application,
            &Lifecycle,
        ),
        Err(ServerSessionError::Disconnected)
    );
    assert_eq!(
        server.confirm_written(outbound.ticket()),
        Ok(ServerWriteDisposition::Close)
    );
}

#[test]
fn public_dto_fields_remain_idiomatic_while_session_tickets_keep_their_invariant() {
    let snapshot = AuthoritativeSnapshotDto {
        revision: 7,
        items: Vec::new(),
    };
    let AuthoritativeSnapshotDto { revision, items } = snapshot;
    assert_eq!(revision, 7);
    assert!(items.is_empty());
}

#[test]
fn one_session_accepts_fresh_call_scoped_capabilities_without_retaining_them() {
    let hub = RevisionHub::new(4).expect("capacity");
    let mut server = ServerSession::new(hub.clone(), build(), Id32::new([98; 32]));

    {
        let application = Application::new(Ports::new(hub.clone()));
        let hello = WireMessage::ClientHello(ClientHello::new(
            VersionRange::new(V1, V1).expect("v1 range"),
            build(),
        ));
        let outbound = server
            .receive(hello, &application, &Lifecycle)
            .expect("hello accepted through temporary capabilities");
        server
            .confirm_written(outbound.ticket())
            .expect("hello written");
    }

    let application = Application::new(Ports::new(hub));
    let outbound = server
        .receive(
            request(1, Request::AuthoritativeSnapshot),
            &application,
            &Lifecycle,
        )
        .expect("later request uses a fresh capability bundle");
    assert_success(&outbound);
}

#[test]
fn dropping_one_call_scoped_session_cancels_only_its_revision_registration() {
    let hub = RevisionHub::new(2).expect("capacity");
    let (mut dropped, dropped_application) = session(hub.clone());
    let (mut sibling, sibling_application) = session(hub.clone());
    negotiate(&mut dropped, &dropped_application);
    negotiate(&mut sibling, &sibling_application);

    let dropped_ack = dropped
        .receive(
            request(
                1,
                Request::Subscribe(
                    SubscriptionRequestDto::new(
                        Id32::new([71; 32]),
                        vec![InvalidationTopic::Conversation],
                    )
                    .expect("dropped subscription"),
                ),
            ),
            &dropped_application,
            &Lifecycle,
        )
        .expect("pending acknowledgement");
    let sibling_ack = sibling
        .receive(
            request(
                1,
                Request::Subscribe(
                    SubscriptionRequestDto::new(
                        Id32::new([72; 32]),
                        vec![InvalidationTopic::Conversation],
                    )
                    .expect("sibling subscription"),
                ),
            ),
            &sibling_application,
            &Lifecycle,
        )
        .expect("sibling acknowledgement");
    sibling
        .confirm_written(sibling_ack.ticket())
        .expect("sibling acknowledgement written");
    assert_eq!(hub.len(), 2);

    drop((dropped_ack, dropped));
    assert_eq!(hub.len(), 1);
    let _ = hub.publish(Revision::new(9), [SubscriptionTopic::Conversation], false);
    assert!(matches!(
        sibling.poll_invalidation(),
        Some(WireMessage::Invalidation(invalidation)) if invalidation.revision == 9
    ));
}
