//! Transport-independent server routing and write-confirmation race contracts.

#![allow(clippy::expect_used)]

use std::{
    cell::RefCell,
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use hq_application::{
    AgentSessionRequest, AgentSessionResult, Application, ApplicationError, ApplicationErrorCode,
    ApplicationPorts, AuthoritativeConversationView, AuthoritativeSnapshot, CanonicalEvidence,
    CommitFacts, ConfigureRelays, ControlInteractions, ControlMailbox, ConversationKey,
    ConversationPageSelection, DomainSnapshot, EffectOutcome, EffectRequest, EvidenceIngestOutcome,
    FactMutation, InspectResource, InteractionResponderLease, MailboxCommandRequest, MailboxDraft,
    MailboxDraftDeleteOutcome, MailboxDraftDeleteRequest, MailboxDraftSaveOutcome,
    MailboxDraftSaveRequest, MutationAttempt, MutationOutcome, MutationReceipt, ObserveRevisions,
    ProjectCommandOutcome, ProjectCommandRequest, PublishWake, QueryDomain, RelayConfiguration,
    ResourceInspectionRequest, ResourceInspectionResult, ResourceReleaseState,
    SelectedConversationPage, SubscriptionRequest, SubscriptionTopic, SynchronizationRequest,
    WakeDisposition,
};
use hq_domain::{
    BoundedSet, CausalReferences, CommandId, EncryptionPublicKey, FactId, FactScope,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, OperationId, Page, PageCursor, ResourceHealth,
    Revision, SemanticPayload, ShortText, SigningPublicKey, Timestamp,
};
use hq_local_api::protocol::v1::{
    AgentSessionRequestDto, AuthoritativeConversationViewRequestDto, AuthoritativeSnapshotDto,
    BuildMetadata, CanonicalEvidenceDto, CanonicalEvidenceRequestDto, ClientHello,
    ConversationKeyDto, ConversationPageRequest, ConversationPageSelectionDto, EffectRequestDto,
    Id32, InstallationConfigurationDto, InstallationConfigurationPatchDto, InvalidationTopic,
    LifecycleRequest, LifecycleState, LifecycleStatus, MailboxCommandActionDto,
    MailboxCommandRequestDto, MailboxDraftDeleteRequestDto, MailboxDraftSaveRequestDto,
    MailboxDraftTargetDto, MutationRequest, ProjectCommandActionDto, ProjectCommandRequestDto,
    RelayAccessDto, RelayAuthenticationDto, RelayConfigurationDto, Request, RequestEnvelope,
    RequestId, ResourceInspectionRequestDto, ResourceLocatorDto, ResourceSchemeDto, Response,
    ResponseResult, SessionControlDto, SubscriptionRequestDto, SynchronizationRequestDto, V1,
    VersionRange, WireMessage, agent_session_request_digest, resource_inspection_request_digest,
};
use hq_local_api::{
    LifecycleControl, RevisionHub, ServerSession, ServerSessionError, ServerWriteDisposition,
};
#[derive(Clone)]
struct Ports {
    hub: RevisionHub,
    trace: std::rc::Rc<RefCell<Vec<&'static str>>>,
    fail_view: bool,
    responder_activations: Arc<AtomicUsize>,
    responder_drops: Arc<AtomicUsize>,
}

impl Ports {
    fn new(hub: RevisionHub) -> Self {
        Self {
            hub,
            trace: std::rc::Rc::new(RefCell::new(Vec::new())),
            fail_view: false,
            responder_activations: Arc::new(AtomicUsize::new(0)),
            responder_drops: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn failing_view(hub: RevisionHub) -> Self {
        Self {
            hub,
            trace: std::rc::Rc::new(RefCell::new(Vec::new())),
            fail_view: true,
            responder_activations: Arc::new(AtomicUsize::new(0)),
            responder_drops: Arc::new(AtomicUsize::new(0)),
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

    fn authoritative_conversation_view(
        &self,
        selection: Option<&ConversationPageSelection>,
    ) -> Result<AuthoritativeConversationView, ApplicationError> {
        self.trace.borrow_mut().push("view");
        if self.fail_view {
            return Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ));
        }
        let snapshot = AuthoritativeSnapshot::new(Revision::new(7), DomainSnapshot::empty());
        let conversation = selection.map(|selection| {
            SelectedConversationPage::new(selection.key().clone(), Page::new(Vec::new(), None))
        });
        Ok(AuthoritativeConversationView::new(snapshot, conversation))
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

    fn canonical_evidence(
        &self,
        roots: &BTreeSet<FactId>,
        _maximum_facts: usize,
        _maximum_bytes: usize,
    ) -> Result<Vec<CanonicalEvidence>, ApplicationError> {
        self.trace.borrow_mut().push("evidence");
        Ok(roots
            .iter()
            .map(|fact_id| CanonicalEvidence {
                fact_id: *fact_id,
                exact_event: b"{}".to_vec(),
            })
            .collect())
    }

    fn state_health(&self) -> Result<hq_application::StateHealth, ApplicationError> {
        self.trace.borrow_mut().push("state_health");
        Ok(hq_application::StateHealth {
            revision: Revision::new(7),
            domains: health_domains(),
        })
    }

    fn repair_state(
        &self,
        operation_id: OperationId,
    ) -> Result<hq_application::StateRepairReport, ApplicationError> {
        self.trace.borrow_mut().push("repair_state");
        Ok(hq_application::StateRepairReport {
            operation_id,
            revision: Revision::new(7),
            domains: health_domains(),
        })
    }
}

fn health_domains() -> Vec<hq_application::DomainHealth> {
    [
        hq_application::HealthDomain::Authority,
        hq_application::HealthDomain::Conversation,
        hq_application::HealthDomain::Agent,
        hq_application::HealthDomain::Project,
    ]
    .into_iter()
    .map(|domain| hq_application::DomainHealth {
        domain,
        projected: 1,
        unresolved: 0,
        unauthorized: 0,
        conflicted: 0,
        invalid: 0,
        unsupported: 0,
        conflicts: 0,
    })
    .collect()
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

    fn ingest_canonical_evidence(
        &self,
        evidence: &[CanonicalEvidence],
    ) -> Result<Vec<EvidenceIngestOutcome>, ApplicationError> {
        self.trace.borrow_mut().push("ingest_evidence");
        Ok(evidence
            .iter()
            .map(|item| EvidenceIngestOutcome {
                fact_id: item.fact_id,
                revision: Revision::new(8),
                inserted: true,
            })
            .collect())
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

    fn relay_status(&self) -> Result<hq_application::RelayStatus, ApplicationError> {
        self.trace.borrow_mut().push("relay_status");
        Ok(hq_application::RelayStatus {
            policies: Vec::new(),
            queued: 0,
            prepared: 0,
            uncertain: 0,
            rejected: 0,
            accepted: 0,
            staged: 0,
            quarantined: 0,
            truncated: false,
        })
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

impl hq_application::QueryProviders for Ports {
    fn provider_catalog(&self) -> Result<hq_application::ProviderCatalog, ApplicationError> {
        self.trace.borrow_mut().push("provider_catalog");
        Ok(hq_application::ProviderCatalog {
            providers: Vec::new(),
            default_provider: None,
        })
    }
}

impl InspectResource for Ports {
    fn inspect_resource(
        &self,
        _request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        self.trace.borrow_mut().push("resource");
        Ok(EffectOutcome::Accepted(ResourceInspectionResult {
            condition: hq_application::ResourceCondition::Healthy,
            health: ResourceHealth::Healthy,
            observed_canonical: None,
            release: ResourceReleaseState::Clean,
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

impl hq_application::RetireAgents for Ports {
    fn retire_agent(
        &self,
        request: hq_application::AgentRetirementRequest,
    ) -> Result<hq_application::AgentRetirementOutcome, ApplicationError> {
        self.trace.borrow_mut().push("retire_agent");
        Ok(hq_application::AgentRetirementOutcome::Completed {
            operation_id: request.operation_id,
            project_id: None,
            runtime: None,
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
impl hq_application::QueryInteractions for Ports {}

impl ControlInteractions for Ports {
    fn prepare_interaction_responder(
        &self,
        _responder_id: OperationId,
    ) -> Result<Box<dyn InteractionResponderLease>, ApplicationError> {
        Ok(Box::new(TestResponderLease {
            activations: Arc::clone(&self.responder_activations),
            drops: Arc::clone(&self.responder_drops),
        }))
    }
}

struct TestResponderLease {
    activations: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

impl InteractionResponderLease for TestResponderLease {
    fn activate(&mut self) -> Result<(), ApplicationError> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl Drop for TestResponderLease {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl ControlMailbox for Ports {
    fn mailbox_drafts(&self) -> Result<Vec<MailboxDraft>, ApplicationError> {
        self.trace.borrow_mut().push("mailbox_drafts");
        Ok(Vec::new())
    }

    fn save_mailbox_draft(
        &self,
        request: MailboxDraftSaveRequest,
    ) -> Result<MailboxDraftSaveOutcome, ApplicationError> {
        self.trace.borrow_mut().push("save_mailbox_draft");
        Ok(MailboxDraftSaveOutcome::Saved(MailboxDraft {
            draft_id: request.draft_id,
            target: request.target,
            content: request.content,
            version: 1,
        }))
    }

    fn delete_mailbox_draft(
        &self,
        _request: MailboxDraftDeleteRequest,
    ) -> Result<MailboxDraftDeleteOutcome, ApplicationError> {
        self.trace.borrow_mut().push("delete_mailbox_draft");
        Ok(MailboxDraftDeleteOutcome::Deleted)
    }

    fn control_mailbox(
        &self,
        request: MailboxCommandRequest,
    ) -> Result<MutationAttempt, ApplicationError> {
        self.trace.borrow_mut().push("control_mailbox");
        Ok(MutationAttempt::Completed(MutationReceipt::new(
            request.command_id,
            request.request_digest,
            Revision::new(8),
            MutationOutcome::Committed,
        )))
    }
}

#[derive(Clone, Copy)]
struct Lifecycle;

impl LifecycleControl for Lifecycle {
    fn lifecycle(&self, _request: LifecycleRequest) -> Result<LifecycleStatus, ApplicationError> {
        LifecycleStatus::new(LifecycleState::Ready, build(), Some(7), None).map_err(|_| {
            ApplicationError::new(hq_application::ApplicationErrorCode::InvariantViolation)
        })
    }
}

struct ConfigurationLifecycle(RefCell<InstallationConfigurationDto>);

impl LifecycleControl for ConfigurationLifecycle {
    fn lifecycle(&self, _request: LifecycleRequest) -> Result<LifecycleStatus, ApplicationError> {
        Lifecycle.lifecycle(LifecycleRequest::Status)
    }

    fn installation_configuration(&self) -> Result<InstallationConfigurationDto, ApplicationError> {
        Ok(self.0.borrow().clone())
    }

    fn update_installation_configuration(
        &self,
        patch: InstallationConfigurationPatchDto,
    ) -> Result<InstallationConfigurationDto, ApplicationError> {
        let mut current = self.0.borrow_mut();
        match patch {
            InstallationConfigurationPatchDto::DefaultProvider(value) => {
                current.default_provider = value;
            }
            InstallationConfigurationPatchDto::Theme(value) => current.theme = value,
            InstallationConfigurationPatchDto::CodexModel(value) => current.codex_model = value,
            InstallationConfigurationPatchDto::CodexYolo(value) => current.codex_yolo = value,
        }
        Ok(current.clone())
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
    negotiate_with(session, application, &Lifecycle);
}

fn negotiate_with<L: LifecycleControl>(
    session: &mut ServerSession,
    application: &Application<Ports>,
    lifecycle: &L,
) {
    let hello = WireMessage::ClientHello(ClientHello::new(
        VersionRange::new(V1, V1).expect("v1 range"),
        build(),
    ));
    let outbound = session
        .receive(hello, application, lifecycle)
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

fn agent_effect(body: AgentSessionRequestDto) -> EffectRequestDto<AgentSessionRequestDto> {
    let mut request = EffectRequestDto::new(
        Id32::new([31; 32]),
        Id32::new([0; 32]),
        1_700_000_000_000,
        body,
    );
    request.request_digest = Id32::new(
        *agent_session_request_digest(&request)
            .expect("valid session request")
            .as_bytes(),
    );
    request
}

fn resource_effect(
    body: ResourceInspectionRequestDto,
) -> EffectRequestDto<ResourceInspectionRequestDto> {
    let mut request = EffectRequestDto::new(
        Id32::new([31; 32]),
        Id32::new([0; 32]),
        1_700_000_000_000,
        body,
    );
    request.request_digest = Id32::new(
        *resource_inspection_request_digest(&request)
            .expect("valid resource inspection request")
            .as_bytes(),
    );
    request
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
fn configuration_queries_and_field_updates_route_through_node_control() {
    let hub = RevisionHub::new(4).expect("capacity");
    let (mut server, application) = session(hub);
    let lifecycle = ConfigurationLifecycle(RefCell::new(InstallationConfigurationDto {
        default_provider: None,
        theme: None,
        codex_model: None,
        codex_yolo: false,
    }));
    negotiate_with(&mut server, &application, &lifecycle);

    let update = server
        .receive(
            request(
                1,
                Request::UpdateInstallationConfiguration(
                    InstallationConfigurationPatchDto::CodexYolo(true),
                ),
            ),
            &application,
            &lifecycle,
        )
        .expect("field update routes");
    assert!(matches!(
        update.message(),
        WireMessage::Response(response)
            if matches!(
                &response.response,
                Response::Success(ResponseResult::InstallationConfiguration(configuration))
                    if configuration.codex_yolo && configuration.default_provider.is_none()
            )
    ));
    server
        .confirm_written(update.ticket())
        .expect("update written");

    let query = server
        .receive(
            request(2, Request::InstallationConfiguration),
            &application,
            &lifecycle,
        )
        .expect("configuration query routes");
    assert!(matches!(
        query.message(),
        WireMessage::Response(response)
            if matches!(
                &response.response,
                Response::Success(ResponseResult::InstallationConfiguration(configuration))
                    if configuration.codex_yolo
            )
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
    let selected_key = ConversationKeyDto::ProjectThread {
        project: Id32::new([41; 32]),
        thread: Id32::new([42; 32]),
    };
    let outbound = server
        .receive(
            request(
                1,
                Request::Subscribe(
                    SubscriptionRequestDto::new(
                        Id32::new(*operation_id.as_bytes()),
                        vec![InvalidationTopic::Conversation],
                        Some(
                            ConversationPageSelectionDto::new(selected_key.clone(), 100)
                                .expect("selection"),
                        ),
                    )
                    .expect("subscription"),
                ),
            ),
            &application,
            &Lifecycle,
        )
        .expect("ack prepared");
    assert!(matches!(outbound.message(), WireMessage::Response(_)));
    let WireMessage::Response(response) = outbound.message() else {
        return;
    };
    assert!(matches!(
        response.response,
        Response::Success(ResponseResult::Subscription(_))
    ));
    let Response::Success(ResponseResult::Subscription(acknowledgement)) = &response.response
    else {
        return;
    };
    assert_eq!(acknowledgement.view.snapshot.revision, 7);
    assert!(matches!(
        &acknowledgement.view.conversation,
        Some(conversation) if conversation.key == selected_key && conversation.page.items.is_empty()
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
fn failed_materialized_subscription_read_releases_pending_registration() {
    let hub = RevisionHub::new(1).expect("capacity");
    let application = Application::new(Ports::failing_view(hub.clone()));
    let mut server = ServerSession::new(hub.clone(), build(), Id32::new([100; 32]));
    negotiate(&mut server, &application);

    let outbound = server
        .receive(
            request(
                1,
                Request::Subscribe(
                    SubscriptionRequestDto::new(
                        Id32::new([101; 32]),
                        vec![InvalidationTopic::Conversation],
                        None,
                    )
                    .expect("subscription"),
                ),
            ),
            &application,
            &Lifecycle,
        )
        .expect("query failure is returned as a typed response");

    assert!(matches!(
        outbound.message(),
        WireMessage::Response(response) if matches!(response.response, Response::Error(_))
    ));
    assert_eq!(hub.len(), 0);
}

#[test]
fn responder_is_inactive_until_acknowledged_and_drops_on_disconnect() {
    let hub = RevisionHub::new(2).expect("capacity");
    let (mut server, application) = session(hub);
    negotiate(&mut server, &application);
    let outbound = server
        .receive(
            request(
                1,
                Request::RegisterInteractionResponder {
                    responder_id: Id32::new([0x61; 32]),
                },
            ),
            &application,
            &Lifecycle,
        )
        .expect("responder acknowledgement prepares");
    assert_eq!(
        application
            .ports()
            .responder_activations
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        application.ports().responder_drops.load(Ordering::SeqCst),
        0
    );

    server
        .confirm_written(outbound.ticket())
        .expect("written acknowledgement activates responder");
    assert_eq!(
        application
            .ports()
            .responder_activations
            .load(Ordering::SeqCst),
        1
    );
    server.disconnect();
    assert_eq!(
        application.ports().responder_drops.load(Ordering::SeqCst),
        1
    );
}

#[test]
fn lost_responder_acknowledgement_drops_the_pending_lease_without_activation() {
    let hub = RevisionHub::new(2).expect("capacity");
    let (mut server, application) = session(hub);
    negotiate(&mut server, &application);
    let outbound = server
        .receive(
            request(
                1,
                Request::RegisterInteractionResponder {
                    responder_id: Id32::new([0x62; 32]),
                },
            ),
            &application,
            &Lifecycle,
        )
        .expect("responder acknowledgement prepares");

    server.disconnect();

    assert_eq!(
        application
            .ports()
            .responder_activations
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        application.ports().responder_drops.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        server.confirm_written(outbound.ticket()),
        Err(ServerSessionError::UnknownWriteTicket)
    );
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
                    SubscriptionRequestDto::new(
                        Id32::new([45; 32]),
                        vec![InvalidationTopic::All],
                        None,
                    )
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
#[allow(clippy::too_many_lines)]
fn every_typed_request_family_routes_without_storage_types() {
    let hub = RevisionHub::new(8).expect("capacity");
    let (mut server, application) = session(hub);
    negotiate(&mut server, &application);
    let page = ConversationPageRequest::new(
        ConversationKeyDto::ProjectThread {
            project: Id32::new([1; 32]),
            thread: Id32::new([3; 32]),
        },
        32,
        None,
    )
    .expect("page");
    let view_selection = ConversationPageSelectionDto::new(page.key.clone(), 32)
        .expect("materialized view selection");
    let requests = vec![
        Request::Lifecycle(LifecycleRequest::Status),
        Request::AuthoritativeSnapshot,
        Request::AuthoritativeConversationView(AuthoritativeConversationViewRequestDto::new(Some(
            view_selection,
        ))),
        Request::ConversationPage(page),
        Request::MailboxDrafts,
        Request::SaveMailboxDraft(MailboxDraftSaveRequestDto {
            draft_id: Id32::new([8; 32]),
            target: MailboxDraftTargetDto::SelfNote,
            content: String::new(),
            expected_version: None,
        }),
        Request::DeleteMailboxDraft(MailboxDraftDeleteRequestDto {
            draft_id: Id32::new([8; 32]),
            expected_version: 1,
        }),
        Request::ControlMailbox(Box::new(MailboxCommandRequestDto::new(
            Id32::new([9; 32]),
            None,
            MailboxCommandActionDto::SelfNote {
                message_id: Id32::new([10; 32]),
            },
            Some("note".to_owned()),
            1,
            [11; 32],
        ))),
        Request::Mutation(
            MutationRequest::from_plan(CommandId::from_bytes([11; 32]), plan()).expect("mutation"),
        ),
        Request::CanonicalEvidence(CanonicalEvidenceRequestDto {
            roots: vec![Id32::new([12; 32])],
        }),
        Request::IngestCanonicalEvidence(vec![CanonicalEvidenceDto {
            fact_id: Id32::new([12; 32]),
            exact_event: "{}".to_owned(),
        }]),
        Request::ConfigureRelay(effect(RelayConfigurationDto::new(
            locator(),
            RelayAccessDto::ReadWrite,
            RelayAuthenticationDto::OnChallenge,
            true,
        ))),
        Request::Synchronize(effect(SynchronizationRequestDto::All)),
        Request::RelayStatus,
        Request::StateHealth,
        Request::RepairState {
            operation_id: Id32::new([24; 32]),
        },
        Request::ControlAgentSession(Box::new(agent_effect(
            AgentSessionRequestDto::new(
                Id32::new([13; 32]),
                "codex".to_owned(),
                SessionControlDto::Stop,
                None,
            )
            .expect("agent request"),
        ))),
        Request::InspectResource(resource_effect(ResourceInspectionRequestDto {
            project_id: Id32::new([14; 32]),
            resource_id: Id32::new([15; 32]),
            display_locator: locator(),
            canonical_locator: Some(locator()),
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
fn provider_catalog_request_routes_without_storage_types() {
    let hub = RevisionHub::new(4).expect("capacity");
    let (mut server, application) = session(hub);
    negotiate(&mut server, &application);

    let outbound = server
        .receive(
            request(1, Request::ProviderCatalog),
            &application,
            &Lifecycle,
        )
        .expect("route succeeds");
    assert_success(&outbound);
}

#[test]
fn resource_inspection_rejects_a_digest_that_does_not_bind_its_body() {
    let hub = RevisionHub::new(4).expect("capacity");
    let (mut server, application) = session(hub);
    negotiate(&mut server, &application);
    let invalid = effect(ResourceInspectionRequestDto {
        project_id: Id32::new([14; 32]),
        resource_id: Id32::new([15; 32]),
        display_locator: locator(),
        canonical_locator: Some(locator()),
    });
    let outbound = server
        .receive(
            request(1, Request::InspectResource(invalid)),
            &application,
            &Lifecycle,
        )
        .expect("invalid request receives a response");
    assert!(matches!(
        outbound.message(),
        WireMessage::Response(response) if matches!(response.response, Response::Error(_))
    ));
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
                        None,
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
                        None,
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
