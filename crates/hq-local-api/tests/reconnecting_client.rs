//! Reconnecting client replay, freshness, and stale-session contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{collections::VecDeque, fmt, num::NonZeroUsize, time::Duration};

use hq_domain::{
    BoundedSet, CausalReferences, CommandId, EncryptionPublicKey, FactScope, InstallationId,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, OperationId, SemanticPayload, ShortText,
    SigningPublicKey, Timestamp,
};
use hq_local_api::protocol::v1::{
    AgentLaunchContextDto, AgentRetirementOutcomeDto, AgentRetirementRequestDto,
    AgentSessionRequestDto, AgentSessionResultDto, AuthoritativeSnapshotDto, BuildMetadata,
    ClientHello, EffectOutcomeDto, EffectRequestDto, ErrorClass, ErrorResponse, Id32,
    InvalidationTopic, LaunchEnvironmentDto, LifecycleRequest, LifecycleState, LifecycleStatus,
    MutationAttemptDto, MutationRequest, ProjectCommandActionDto, ProjectCommandOutcomeDto,
    ProjectCommandRequestDto, ProjectCreationRequestDto, Request, RequestId, ResourceLocatorDto,
    ResourceSchemeDto, ResponseEnvelope, ResponseResult, RevisionInvalidation, ServerHello,
    SessionControlDto, SubscriptionAcknowledgement, V1, VersionRange, VersionRejected, WireMessage,
    agent_session_request_digest,
};
use hq_local_api::{
    BlockingClientConfig, BlockingClientError, BlockingClientRunner, ClientAction, ClientError,
    ClientEvent, ClientTransport, ConnectionGeneration, InitialView, ReconnectPolicy,
    ReconnectingClient,
};

#[derive(Clone, Copy, Debug)]
struct ScriptedTransportError;

impl fmt::Display for ScriptedTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("scripted transport failure")
    }
}

impl std::error::Error for ScriptedTransportError {}

struct ScriptedTransport {
    reads: VecDeque<Result<Vec<u8>, ScriptedTransportError>>,
    writes: Vec<Vec<u8>>,
    connects: usize,
    failed_connects_remaining: usize,
    closes: usize,
}

impl ClientTransport for ScriptedTransport {
    type Connection = ();
    type Error = ScriptedTransportError;

    fn connect(&mut self) -> Result<Self::Connection, Self::Error> {
        self.connects += 1;
        if self.failed_connects_remaining > 0 {
            self.failed_connects_remaining -= 1;
            return Err(ScriptedTransportError);
        }
        Ok(())
    }

    fn write(
        &mut self,
        _connection: &mut Self::Connection,
        frame: &[u8],
        _timeout: Duration,
    ) -> Result<(), Self::Error> {
        self.writes.push(frame.to_vec());
        Ok(())
    }

    fn read_frame(
        &mut self,
        _connection: &mut Self::Connection,
        _timeout: Duration,
    ) -> Result<Vec<u8>, Self::Error> {
        self.reads
            .pop_front()
            .unwrap_or(Err(ScriptedTransportError))
    }

    fn close(&mut self, _connection: Self::Connection) {
        self.closes += 1;
    }

    fn wait(&mut self, _delay: Duration) {}
}

fn build() -> BuildMetadata {
    BuildMetadata::new("hq-test", "0.1.0", Some("0123456789ab")).expect("bounded build")
}

fn policy() -> ReconnectPolicy {
    ReconnectPolicy::new(Duration::from_millis(10), Duration::from_millis(40))
        .expect("valid backoff")
}

fn client() -> ReconnectingClient {
    ReconnectingClient::new(build(), policy(), 2, InitialView::Snapshot)
        .expect("positive identity history")
}

fn plan(at: i64) -> hq_application::FactPlan {
    let installation = InstallationId::from_bytes([7; 32]);
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([]).expect("empty set"),
        [],
    )
    .expect("empty references");
    hq_application::FactPlan::new(
        installation,
        Timestamp::from_unix_millis(at),
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

fn mutation(command: u8, at: i64) -> MutationRequest {
    MutationRequest::from_plan(CommandId::from_bytes([command; 32]), plan(at))
        .expect("mutation request")
}

fn project_command(command: u8, digest: u8) -> ProjectCommandRequestDto {
    ProjectCommandRequestDto {
        command_id: Id32::new([command; 32]),
        operation_id: Id32::new([command.wrapping_add(1); 32]),
        request_digest: Id32::new([digest; 32]),
        account_id: Id32::new([3; 32]),
        project_id: Id32::new([4; 32]),
        home: Id32::new([5; 32]),
        expected_head: None,
        issued_at_unix_millis: 1_700_000_000_000,
        action: ProjectCommandActionDto::Create(ProjectCreationRequestDto {
            mailbox_id: Id32::new([6; 32]),
            project_name: "existing".to_owned(),
            brief: None,
            resource_id: Id32::new([7; 32]),
            resource: ResourceLocatorDto::new(
                ResourceSchemeDto::WorkingTree,
                "/work/existing".to_owned(),
            )
            .expect("resource locator"),
        }),
    }
}

fn agent_retirement(command: u8, digest: u8) -> AgentRetirementRequestDto {
    AgentRetirementRequestDto {
        command_id: Id32::new([command; 32]),
        operation_id: Id32::new([command.wrapping_add(1); 32]),
        request_digest: Id32::new([digest; 32]),
        account_id: Id32::new([3; 32]),
        agent_id: Id32::new([4; 32]),
        expected_claim: Id32::new([5; 32]),
        home: Id32::new([6; 32]),
        issued_at_unix_millis: 1_700_000_000_000,
        force: false,
    }
}

fn agent_session(operation: u8) -> EffectRequestDto<AgentSessionRequestDto> {
    let body = AgentSessionRequestDto::new(
        Id32::new([4; 32]),
        "fake".to_owned(),
        SessionControlDto::Start,
        Some(AgentLaunchContextDto {
            directory: ResourceLocatorDto::new(
                ResourceSchemeDto::WorkingTree,
                "/work/hq".to_owned(),
            )
            .expect("launch directory"),
            environment: LaunchEnvironmentDto::copy_from([("HQ_TOKEN", b"do-not-log".as_slice())])
                .expect("launch environment"),
        }),
    )
    .expect("session body");
    let mut request = EffectRequestDto::new(
        Id32::new([operation; 32]),
        Id32::new([0; 32]),
        1_700_000_000_000,
        body,
    );
    request.request_digest = Id32::new(
        *agent_session_request_digest(&request)
            .expect("session digest")
            .as_bytes(),
    );
    request
}

fn snapshot_response(request_id: u64, revision: u64) -> Vec<u8> {
    WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(request_id).expect("request id"),
        ResponseResult::AuthoritativeSnapshot(
            AuthoritativeSnapshotDto::new(revision, Vec::new()).expect("snapshot"),
        ),
    ))
    .encode_frame()
    .expect("snapshot response")
}

fn only_connect(actions: &[ClientAction]) -> (ConnectionGeneration, Duration) {
    let [ClientAction::ConnectAfter { generation, delay }] = actions else {
        panic!("expected one connect action: {actions:?}");
    };
    (*generation, *delay)
}

fn only_write(actions: &[ClientAction]) -> (ConnectionGeneration, Vec<u8>) {
    let [ClientAction::Write { generation, frame }] = actions else {
        panic!("expected one write action: {actions:?}");
    };
    (*generation, frame.clone())
}

fn hello(client: &mut ReconnectingClient, generation: ConnectionGeneration, session: u8) {
    let transition = client
        .connected(generation)
        .expect("current connect succeeds");
    let (_, frame) = only_write(&transition.actions);
    assert!(matches!(
        WireMessage::decode_frame(&frame).expect("hello decodes"),
        WireMessage::ClientHello(ClientHello { .. })
    ));
    let server = WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([session; 32])))
        .encode_frame()
        .expect("server hello");
    client
        .receive_frame(generation, &server)
        .expect("server hello accepted");
}

#[test]
fn lost_mutation_response_replays_the_byte_identical_original_frame() {
    let mut client = client();
    let request = mutation(1, 1_700_000_000_000);
    assert!(
        client
            .submit_mutation(request.clone())
            .expect("queue")
            .actions
            .is_empty()
    );
    let (first, delay) = only_connect(&client.start().expect("start").actions);
    assert_eq!(delay, Duration::ZERO);

    let hello_write = client.connected(first).expect("connect");
    assert!(matches!(
        WireMessage::decode_frame(&only_write(&hello_write.actions).1).expect("client hello"),
        WireMessage::ClientHello(_)
    ));
    let negotiated = client
        .receive_frame(
            first,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([21; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("negotiates");
    let mutation_frame = negotiated
        .actions
        .iter()
        .find_map(|action| match action {
            ClientAction::Write { frame, .. }
                if matches!(
                    WireMessage::decode_frame(frame),
                    Ok(WireMessage::Request(envelope))
                        if matches!(envelope.request, Request::Mutation(_))
                ) =>
            {
                Some(frame.clone())
            }
            _ => None,
        })
        .expect("queued mutation writes after negotiation");

    let reconnect = client.disconnected(first).expect("response lost");
    let (second, _) = only_connect(&reconnect.actions);
    let _ = client.connected(second).expect("reconnect starts hello");
    let replay = client
        .receive_frame(
            second,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([22; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("renegotiates");
    assert!(replay.actions.iter().any(|action| {
        matches!(action, ClientAction::Write { frame, .. } if frame == &mutation_frame)
    }));

    assert_eq!(
        client.submit_mutation(mutation(1, 1_700_000_000_001)),
        Err(ClientError::ChangedCommandIdentity)
    );
    assert_eq!(request.command_id(), CommandId::from_bytes([1; 32]));
}

#[test]
fn lost_project_response_replays_exact_frame_and_terminal_identity_is_retained() {
    let mut client = client();
    let request = project_command(81, 82);
    assert!(
        client
            .submit_project_command(request.clone())
            .expect("queue project command")
            .actions
            .is_empty()
    );
    let (first, _) = only_connect(&client.start().expect("start").actions);
    let _ = client.connected(first).expect("connect");
    let negotiated = client
        .receive_frame(
            first,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([83; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("negotiates");
    let original = negotiated
        .actions
        .iter()
        .find_map(|action| match action {
            ClientAction::Write { frame, .. }
                if matches!(
                    WireMessage::decode_frame(frame),
                    Ok(WireMessage::Request(envelope))
                        if matches!(envelope.request, Request::ControlProject(_))
                ) =>
            {
                Some(frame.clone())
            }
            _ => None,
        })
        .expect("project frame");

    let (second, _) = only_connect(&client.disconnected(first).expect("lost response").actions);
    let _ = client.connected(second).expect("reconnect");
    let replay = client
        .receive_frame(
            second,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([84; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("renegotiates");
    assert!(
        replay.actions.iter().any(
            |action| matches!(action, ClientAction::Write { frame, .. } if frame == &original)
        )
    );

    let WireMessage::Request(envelope) = WireMessage::decode_frame(&original).expect("request")
    else {
        panic!("expected project request")
    };
    let completed = WireMessage::Response(ResponseEnvelope::success(
        envelope.id,
        ResponseResult::ProjectCommand(ProjectCommandOutcomeDto::Completed {
            operation_id: request.operation_id,
            project_head: Id32::new([85; 32]),
            runtime: None,
        }),
    ))
    .encode_frame()
    .expect("completion frame");
    let transition = client
        .receive_frame(second, &completed)
        .expect("completion accepted");
    assert!(matches!(
        transition.events.as_slice(),
        [ClientEvent::ProjectCommand { command_id, outcome: ProjectCommandOutcomeDto::Completed { .. } }]
            if *command_id == CommandId::from_bytes([81; 32])
    ));
    assert_eq!(client.completed_identity_count(), 1);
    assert!(
        client
            .submit_project_command(request.clone())
            .expect("terminal replay is local")
            .actions
            .is_empty()
    );
    let mut changed = request;
    changed.request_digest = Id32::new([86; 32]);
    assert_eq!(
        client.submit_project_command(changed),
        Err(ClientError::ChangedCommandIdentity)
    );
}

#[test]
fn lost_agent_retirement_response_replays_exact_frame_and_retains_terminal_identity() {
    let mut client = client();
    let request = agent_retirement(91, 92);
    assert!(
        client
            .submit_agent_retirement(request)
            .expect("queue agent retirement")
            .actions
            .is_empty()
    );
    let (first, _) = only_connect(&client.start().expect("start").actions);
    let _ = client.connected(first).expect("connect");
    let negotiated = client
        .receive_frame(
            first,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([93; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("negotiates");
    let original = negotiated
        .actions
        .iter()
        .find_map(|action| match action {
            ClientAction::Write { frame, .. }
                if matches!(
                    WireMessage::decode_frame(frame),
                    Ok(WireMessage::Request(envelope))
                        if matches!(envelope.request, Request::RetireAgent(_))
                ) =>
            {
                Some(frame.clone())
            }
            _ => None,
        })
        .expect("agent retirement frame");

    let (second, _) = only_connect(&client.disconnected(first).expect("lost response").actions);
    let _ = client.connected(second).expect("reconnect");
    let replay = client
        .receive_frame(
            second,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([94; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("renegotiates");
    assert!(
        replay.actions.iter().any(
            |action| matches!(action, ClientAction::Write { frame, .. } if frame == &original)
        )
    );

    let WireMessage::Request(envelope) = WireMessage::decode_frame(&original).expect("request")
    else {
        panic!("expected agent retirement request")
    };
    let completed = WireMessage::Response(ResponseEnvelope::success(
        envelope.id,
        ResponseResult::AgentRetirement(AgentRetirementOutcomeDto::Completed {
            operation_id: request.operation_id,
            project_id: None,
            runtime: None,
        }),
    ))
    .encode_frame()
    .expect("completion frame");
    let transition = client
        .receive_frame(second, &completed)
        .expect("completion accepted");
    assert!(matches!(
        transition.events.as_slice(),
        [ClientEvent::AgentRetirement { command_id, outcome: AgentRetirementOutcomeDto::Completed { .. } }]
            if *command_id == CommandId::from_bytes([91; 32])
    ));
    assert_eq!(client.completed_identity_count(), 1);
    assert!(
        client
            .submit_agent_retirement(request)
            .expect("terminal replay is local")
            .actions
            .is_empty()
    );
    let mut changed = request;
    changed.request_digest = Id32::new([95; 32]);
    assert_eq!(
        client.submit_agent_retirement(changed),
        Err(ClientError::ChangedCommandIdentity)
    );
}

#[test]
fn lost_agent_session_response_replays_exact_secret_bearing_frame() {
    let mut client = client();
    let request = agent_session(96);
    assert!(
        client
            .submit_agent_session(request.clone())
            .expect("queue agent session")
            .actions
            .is_empty()
    );
    let (first, _) = only_connect(&client.start().expect("start").actions);
    let _ = client.connected(first).expect("connect");
    let negotiated = client
        .receive_frame(
            first,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([97; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("negotiates");
    let original = negotiated
        .actions
        .iter()
        .find_map(|action| match action {
            ClientAction::Write { frame, .. }
                if matches!(
                    WireMessage::decode_frame(frame),
                    Ok(WireMessage::Request(envelope))
                        if matches!(envelope.request, Request::ControlAgentSession(_))
                ) =>
            {
                Some(frame.clone())
            }
            _ => None,
        })
        .expect("managed session frame");

    let (second, _) = only_connect(&client.disconnected(first).expect("lost response").actions);
    let _ = client.connected(second).expect("reconnect");
    let replay = client
        .receive_frame(
            second,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([98; 32])))
                .encode_frame()
                .expect("hello frame"),
        )
        .expect("renegotiates");
    assert!(
        replay.actions.iter().any(
            |action| matches!(action, ClientAction::Write { frame, .. } if frame == &original)
        )
    );

    let WireMessage::Request(envelope) = WireMessage::decode_frame(&original).expect("request")
    else {
        panic!("expected managed session request")
    };
    let completed = WireMessage::Response(ResponseEnvelope::success(
        envelope.id,
        ResponseResult::AgentSession(EffectOutcomeDto::Accepted(AgentSessionResultDto::Ready(
            "session-1".to_owned(),
        ))),
    ))
    .encode_frame()
    .expect("completion frame");
    let transition = client
        .receive_frame(second, &completed)
        .expect("completion accepted");
    assert!(matches!(
        transition.events.as_slice(),
        [ClientEvent::AgentSession { operation_id, outcome: EffectOutcomeDto::Accepted(AgentSessionResultDto::Ready(session)) }]
            if *operation_id == OperationId::from_bytes([96; 32]) && session == "session-1"
    ));
    assert!(
        client
            .submit_agent_session(request.clone())
            .expect("terminal replay is local")
            .actions
            .is_empty()
    );

    let mut changed = request;
    changed.body.provider = "other".to_owned();
    changed.request_digest = Id32::new(
        *agent_session_request_digest(&changed)
            .expect("changed digest")
            .as_bytes(),
    );
    assert_eq!(
        client.submit_agent_session(changed),
        Err(ClientError::ChangedCommandIdentity)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn subscription_acknowledgement_is_the_fresh_base_and_gaps_force_full_refresh() {
    let mut client = client();
    client
        .configure_subscription(Id32::new([31; 32]), vec![InvalidationTopic::Conversation])
        .expect("subscription intent");
    let (generation, _) = only_connect(&client.start().expect("start").actions);
    let _ = client.connected(generation).expect("connect");
    let negotiated = client
        .receive_frame(
            generation,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([41; 32])))
                .encode_frame()
                .expect("hello"),
        )
        .expect("negotiates");
    let (request_id, subscription_id) = negotiated
        .actions
        .iter()
        .find_map(|action| match action {
            ClientAction::Write { frame, .. } => match WireMessage::decode_frame(frame).ok()? {
                WireMessage::Request(envelope) => match envelope.request {
                    Request::Subscribe(request) => Some((envelope.id, request.subscription_id)),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .expect("subscribe action");

    let early = WireMessage::Invalidation(
        RevisionInvalidation::new(
            subscription_id,
            8,
            vec![InvalidationTopic::Conversation],
            false,
        )
        .expect("invalidation"),
    )
    .encode_frame()
    .expect("frame");
    let transition = client
        .receive_frame(generation, &early)
        .expect("early notice retained");
    assert!(transition.events.is_empty());
    assert!(transition.actions.is_empty());

    let acknowledgement = WireMessage::Response(ResponseEnvelope::success(
        request_id,
        ResponseResult::Subscription(SubscriptionAcknowledgement::new(
            subscription_id,
            AuthoritativeSnapshotDto::new(7, Vec::new()).expect("snapshot"),
        )),
    ))
    .encode_frame()
    .expect("ack frame");
    let transition = client
        .receive_frame(generation, &acknowledgement)
        .expect("ack accepted");
    assert!(
        matches!(transition.events.as_slice(), [ClientEvent::Snapshot(snapshot)] if snapshot.revision == 7)
    );
    assert!(transition.actions.iter().any(|action| matches!(
        action,
        ClientAction::Write { frame, .. }
            if matches!(WireMessage::decode_frame(frame), Ok(WireMessage::Request(envelope)) if matches!(envelope.request, Request::AuthoritativeSnapshot))
    )));
    assert!(!client.view_is_current());

    let (_, refresh_frame) = only_write(&transition.actions);
    let WireMessage::Request(refresh) =
        WireMessage::decode_frame(&refresh_frame).expect("refresh request")
    else {
        panic!("expected refresh request")
    };
    let newer = WireMessage::Invalidation(
        RevisionInvalidation::new(
            subscription_id,
            9,
            vec![InvalidationTopic::Conversation],
            false,
        )
        .expect("newer invalidation"),
    )
    .encode_frame()
    .expect("newer frame");
    assert!(
        client
            .receive_frame(generation, &newer)
            .expect("coalesces while refreshing")
            .actions
            .is_empty()
    );
    let stale_refresh = WireMessage::Response(ResponseEnvelope::success(
        refresh.id,
        ResponseResult::AuthoritativeSnapshot(
            AuthoritativeSnapshotDto::new(8, Vec::new()).expect("stale snapshot"),
        ),
    ))
    .encode_frame()
    .expect("stale refresh response");
    let follow_up = client
        .receive_frame(generation, &stale_refresh)
        .expect("stale refresh accepted as an intermediate base");
    assert_eq!(follow_up.actions.len(), 1);
    assert!(!client.view_is_current());

    let first_subscription = client.active_subscription_id().expect("first registration");
    let reconnect = client.disconnected(generation).expect("disconnect");
    let (next_generation, _) = only_connect(&reconnect.actions);
    let _ = client.connected(next_generation).expect("reconnect hello");
    let resubscribed = client
        .receive_frame(
            next_generation,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([42; 32])))
                .encode_frame()
                .expect("next hello"),
        )
        .expect("resubscribes");
    let next_subscription = resubscribed
        .actions
        .iter()
        .find_map(|action| match action {
            ClientAction::Write { frame, .. } => match WireMessage::decode_frame(frame).ok()? {
                WireMessage::Request(envelope) => match envelope.request {
                    Request::Subscribe(request) => Some(request.subscription_id),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .expect("fresh registration");
    assert_ne!(first_subscription, next_subscription);
}

#[test]
fn lost_subscription_acknowledgement_is_discarded_and_registered_fresh() {
    let mut client = client();
    client
        .configure_subscription(Id32::new([43; 32]), vec![InvalidationTopic::All])
        .expect("subscription intent");
    let (first_generation, _) = only_connect(&client.start().expect("start").actions);
    let _ = client.connected(first_generation).expect("connect");
    let first = client
        .receive_frame(
            first_generation,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([44; 32])))
                .encode_frame()
                .expect("first hello"),
        )
        .expect("first registration");
    let first_id = client.active_subscription_id().expect("first id");
    assert_eq!(first.actions.len(), 1);

    let reconnect = client
        .disconnected(first_generation)
        .expect("acknowledgement lost");
    let (second_generation, _) = only_connect(&reconnect.actions);
    let _ = client.connected(second_generation).expect("reconnect");
    let second = client
        .receive_frame(
            second_generation,
            &WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([45; 32])))
                .encode_frame()
                .expect("second hello"),
        )
        .expect("fresh registration");
    let second_id = client.active_subscription_id().expect("second id");
    assert_ne!(first_id, second_id);
    assert_eq!(second.actions.len(), 1);

    let (_, second_frame) = only_write(&second.actions);
    let WireMessage::Request(second_request) =
        WireMessage::decode_frame(&second_frame).expect("second request")
    else {
        panic!("expected subscription request")
    };
    let base_failure = WireMessage::Response(ResponseEnvelope::error(
        second_request.id,
        ErrorResponse::new(
            ErrorClass::Unavailable,
            "snapshot_unavailable".to_owned(),
            None,
        )
        .expect("bounded error"),
    ))
    .encode_frame()
    .expect("failure response");
    let retry = client
        .receive_frame(second_generation, &base_failure)
        .expect("base failure reconnects");
    assert!(matches!(
        retry.actions.as_slice(),
        [
            ClientAction::Close { .. },
            ClientAction::ConnectAfter { .. }
        ]
    ));
    assert!(matches!(
        retry.events.as_slice(),
        [ClientEvent::Error { request_id, .. }] if *request_id == second_request.id
    ));
    assert!(!client.view_is_current());

    let stale_ack = first
        .actions
        .into_iter()
        .next()
        .expect("first subscribe action");
    let ClientAction::Write { frame, .. } = stale_ack else {
        panic!("expected subscription write")
    };
    let WireMessage::Request(first_request) = WireMessage::decode_frame(&frame).expect("request")
    else {
        panic!("expected subscription request")
    };
    let response = WireMessage::Response(ResponseEnvelope::success(
        first_request.id,
        ResponseResult::Subscription(SubscriptionAcknowledgement::new(
            first_id,
            AuthoritativeSnapshotDto::new(1, Vec::new()).expect("snapshot"),
        )),
    ))
    .encode_frame()
    .expect("stale acknowledgement");
    assert!(
        client
            .receive_frame(first_generation, &response)
            .expect("stale generation ignored")
            .events
            .is_empty()
    );
}

#[test]
fn ordinary_requests_are_correlated_and_report_response_loss_without_replay() {
    let mut client = client();
    let (generation, _) = only_connect(&client.start().expect("start").actions);
    hello(&mut client, generation, 71);

    let submitted = client
        .submit_request(Request::Lifecycle(LifecycleRequest::Status))
        .expect("active ordinary request");
    let (_, frame) = only_write(&submitted.actions);
    let WireMessage::Request(envelope) = WireMessage::decode_frame(&frame).expect("request") else {
        panic!("expected request")
    };
    let status = LifecycleStatus::new(LifecycleState::Ready, build(), Some(11), None)
        .expect("lifecycle status");
    let response = WireMessage::Response(ResponseEnvelope::success(
        envelope.id,
        ResponseResult::Lifecycle(status.clone()),
    ))
    .encode_frame()
    .expect("response");
    let completed = client
        .receive_frame(generation, &response)
        .expect("correlated response");
    assert_eq!(
        completed.events,
        vec![ClientEvent::Response {
            request_id: envelope.id,
            result: ResponseResult::Lifecycle(status),
        }]
    );

    let in_flight = client
        .submit_request(Request::Lifecycle(LifecycleRequest::Restart))
        .expect("restart request");
    let (_, frame) = only_write(&in_flight.actions);
    let WireMessage::Request(lost) = WireMessage::decode_frame(&frame).expect("lost request")
    else {
        panic!("expected request")
    };
    let disconnected = client.disconnected(generation).expect("response loss");
    assert_eq!(disconnected.events, vec![ClientEvent::RequestLost(lost.id)]);
    assert_eq!(disconnected.actions.len(), 1);
}

#[test]
fn reconnect_backoff_is_capped_and_stale_or_incompatible_sessions_do_not_resume() {
    let mut client = client();
    let (first, _) = only_connect(&client.start().expect("start").actions);
    let (second, first_delay) = only_connect(
        &client
            .connection_failed(first)
            .expect("first failure")
            .actions,
    );
    let (third, second_delay) = only_connect(
        &client
            .connection_failed(second)
            .expect("second failure")
            .actions,
    );
    let (fourth, third_delay) = only_connect(
        &client
            .connection_failed(third)
            .expect("third failure")
            .actions,
    );
    let (_, capped_delay) = only_connect(
        &client
            .connection_failed(fourth)
            .expect("fourth failure")
            .actions,
    );
    assert_eq!(
        [first_delay, second_delay, third_delay, capped_delay],
        [
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(40),
            Duration::from_millis(40),
        ]
    );
    assert!(
        client
            .connected(first)
            .expect("stale event ignored")
            .actions
            .is_empty()
    );

    let current = client.current_generation().expect("current generation");
    let _ = client.connected(current).expect("current connects");
    let rejected = WireMessage::VersionRejected(VersionRejected::new(
        VersionRange::new(2, 3).expect("range"),
        build(),
    ))
    .encode_frame()
    .expect("rejection");
    let transition = client
        .receive_frame(current, &rejected)
        .expect("incompatibility handled");
    assert!(matches!(
        transition.events.as_slice(),
        [ClientEvent::IncompatibleVersion]
    ));
    assert!(matches!(
        transition.actions.as_slice(),
        [ClientAction::Close { .. }]
    ));
    assert!(
        client
            .disconnected(current)
            .expect("closed")
            .actions
            .is_empty()
    );
}

#[test]
fn two_clients_derive_distinct_registrations_and_keep_bounded_identity_history() {
    let mut left = client();
    let mut right = client();
    left.configure_subscription(Id32::new([51; 32]), vec![InvalidationTopic::All])
        .expect("left subscription");
    right
        .configure_subscription(Id32::new([52; 32]), vec![InvalidationTopic::All])
        .expect("right subscription");
    let (left_generation, _) = only_connect(&left.start().expect("left start").actions);
    let (right_generation, _) = only_connect(&right.start().expect("right start").actions);
    hello(&mut left, left_generation, 61);
    hello(&mut right, right_generation, 61);
    assert_ne!(
        left.active_subscription_id(),
        right.active_subscription_id()
    );

    for command in 1..=3 {
        let request = mutation(command, i64::from(command));
        let submitted = left.submit_mutation(request.clone()).expect("submit");
        let (_, frame) = only_write(&submitted.actions);
        let WireMessage::Request(envelope) = WireMessage::decode_frame(&frame).expect("request")
        else {
            panic!("expected request")
        };
        let response = WireMessage::Response(ResponseEnvelope::success(
            envelope.id,
            ResponseResult::Mutation(MutationAttemptDto::Completed {
                command_id: Id32::new([command; 32]),
                request_digest: Id32::new(*request.request_digest().as_bytes()),
                revision: u64::from(command),
                outcome: hq_local_api::protocol::v1::MutationOutcomeDto::Committed,
            }),
        ))
        .encode_frame()
        .expect("response");
        left.receive_frame(left_generation, &response)
            .expect("completion accepted");
    }
    assert_eq!(left.completed_identity_count(), 2);
}

#[test]
fn blocking_runner_replays_mutation_bytes_after_response_loss() {
    let request = mutation(91, 1_700_000_000_091);
    let command_id = request.command_id();
    let request_digest = request.request_digest();
    let hello = |session| {
        WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([session; 32])))
            .encode_frame()
            .expect("hello frame")
    };
    let completed = WireMessage::Response(ResponseEnvelope::success(
        hq_local_api::protocol::v1::RequestId::new(1).expect("request id"),
        ResponseResult::Mutation(MutationAttemptDto::Completed {
            command_id: Id32::new(*command_id.as_bytes()),
            request_digest: Id32::new(*request_digest.as_bytes()),
            revision: 92,
            outcome: hq_local_api::protocol::v1::MutationOutcomeDto::Committed,
        }),
    ))
    .encode_frame()
    .expect("completion frame");
    let transport = ScriptedTransport {
        reads: VecDeque::from([
            Ok(hello(1)),
            Ok(snapshot_response(2, 1)),
            Err(ScriptedTransportError),
            Ok(hello(2)),
            Ok(snapshot_response(3, 1)),
            Ok(completed),
        ]),
        writes: Vec::new(),
        connects: 0,
        failed_connects_remaining: 0,
        closes: 0,
    };
    let mut runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: Duration::from_secs(1),
            max_connection_attempts: NonZeroUsize::new(3).expect("nonzero"),
        },
        client(),
        transport,
    )
    .expect("runner config");

    assert!(matches!(
        runner.mutation(request).expect("mutation reconciles"),
        ClientEvent::Mutation(MutationAttemptDto::Completed { revision: 92, .. })
    ));
    let transport = runner.into_transport();
    assert_eq!(transport.connects, 2);
    assert_eq!(transport.writes.len(), 6);
    assert_eq!(transport.writes[2], transport.writes[5]);
}

#[test]
fn blocking_runner_returns_explicitly_uncertain_agent_session_outcome() {
    let request = agent_session(92);
    let hello = WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([92; 32])))
        .encode_frame()
        .expect("hello frame");
    let uncertain = WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(1).expect("request id"),
        ResponseResult::AgentSession(EffectOutcomeDto::Uncertain(Id32::new([93; 32]))),
    ))
    .encode_frame()
    .expect("uncertain response");
    let transport = ScriptedTransport {
        reads: VecDeque::from([Ok(hello), Ok(snapshot_response(2, 1)), Ok(uncertain)]),
        writes: Vec::new(),
        connects: 0,
        failed_connects_remaining: 0,
        closes: 0,
    };
    let mut runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: Duration::from_secs(1),
            max_connection_attempts: NonZeroUsize::new(2).expect("nonzero"),
        },
        client(),
        transport,
    )
    .expect("runner config");

    assert!(matches!(
        runner.agent_session(request).expect("uncertain outcome"),
        ClientEvent::AgentSession {
            operation_id,
            outcome: EffectOutcomeDto::Uncertain(reconciliation),
        } if operation_id == OperationId::from_bytes([92; 32])
            && reconciliation == Id32::new([93; 32])
    ));
}

#[test]
fn blocking_runner_returns_the_in_flight_initial_snapshot_then_refreshes_again() {
    let hello = WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([90; 32])))
        .encode_frame()
        .expect("hello frame");
    let transport = ScriptedTransport {
        reads: VecDeque::from([
            Ok(hello),
            Ok(snapshot_response(1, 17)),
            Ok(snapshot_response(2, 18)),
        ]),
        writes: Vec::new(),
        connects: 0,
        failed_connects_remaining: 0,
        closes: 0,
    };
    let mut runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: Duration::from_secs(1),
            max_connection_attempts: NonZeroUsize::new(2).expect("nonzero"),
        },
        client(),
        transport,
    )
    .expect("runner config");

    assert_eq!(runner.snapshot().expect("initial snapshot").revision, 17);
    assert_eq!(runner.snapshot().expect("explicit refresh").revision, 18);
    let transport = runner.into_transport();
    assert_eq!(transport.connects, 1);
    assert_eq!(transport.writes.len(), 3);
}

#[test]
fn blocking_runner_never_replays_an_ordinary_request_after_response_loss() {
    let hello = WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([93; 32])))
        .encode_frame()
        .expect("hello frame");
    let transport = ScriptedTransport {
        reads: VecDeque::from([
            Ok(hello),
            Ok(snapshot_response(1, 1)),
            Err(ScriptedTransportError),
        ]),
        writes: Vec::new(),
        connects: 0,
        failed_connects_remaining: 0,
        closes: 0,
    };
    let mut runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: Duration::from_secs(1),
            max_connection_attempts: NonZeroUsize::new(2).expect("nonzero"),
        },
        client(),
        transport,
    )
    .expect("runner config");

    assert_eq!(
        runner.request(Request::Lifecycle(LifecycleRequest::Status)),
        Err(BlockingClientError::ResponseLost)
    );
    let transport = runner.into_transport();
    assert_eq!(transport.connects, 1);
    assert_eq!(transport.writes.len(), 3);
}

#[test]
fn blocking_runner_reports_bounded_connection_exhaustion() {
    let transport = ScriptedTransport {
        reads: VecDeque::new(),
        writes: Vec::new(),
        connects: 0,
        failed_connects_remaining: 3,
        closes: 0,
    };
    let mut runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: Duration::from_secs(1),
            max_connection_attempts: NonZeroUsize::new(2).expect("nonzero"),
        },
        client(),
        transport,
    )
    .expect("runner config");

    assert_eq!(
        runner.mutation(mutation(94, 1_700_000_000_094)),
        Err(BlockingClientError::ConnectionAttemptsExhausted)
    );
    assert_eq!(runner.into_transport().connects, 2);
}

#[test]
fn blocking_runner_rejects_a_zero_workflow_deadline() {
    let transport = ScriptedTransport {
        reads: VecDeque::new(),
        writes: Vec::new(),
        connects: 0,
        failed_connects_remaining: 0,
        closes: 0,
    };
    assert!(matches!(
        BlockingClientRunner::new(
            BlockingClientConfig {
                deadline: Duration::ZERO,
                max_connection_attempts: NonZeroUsize::new(1).expect("nonzero"),
            },
            client(),
            transport,
        ),
        Err(BlockingClientError::InvalidDeadline)
    ));
}

#[test]
fn blocking_runner_reports_incompatible_negotiation() {
    let rejected = WireMessage::VersionRejected(VersionRejected::new(
        VersionRange::new(2, 3).expect("versions"),
        build(),
    ))
    .encode_frame()
    .expect("rejection frame");
    let transport = ScriptedTransport {
        reads: VecDeque::from([Ok(rejected)]),
        writes: Vec::new(),
        connects: 0,
        failed_connects_remaining: 0,
        closes: 0,
    };
    let mut runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: Duration::from_secs(1),
            max_connection_attempts: NonZeroUsize::new(2).expect("nonzero"),
        },
        client(),
        transport,
    )
    .expect("runner config");

    assert_eq!(
        runner.request(Request::Lifecycle(LifecycleRequest::Status)),
        Err(BlockingClientError::Incompatible)
    );
    let transport = runner.into_transport();
    assert_eq!(transport.connects, 1);
    assert_eq!(transport.closes, 1);
}
