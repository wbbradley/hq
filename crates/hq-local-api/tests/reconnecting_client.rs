//! Reconnecting client replay, freshness, and stale-session contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use hq_domain::{
    BoundedSet, CausalReferences, CommandId, EncryptionPublicKey, FactScope, InstallationId,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, SemanticPayload, ShortText, SigningPublicKey,
    Timestamp,
};
use hq_local_api::protocol::v1::{
    AuthoritativeSnapshotDto, BuildMetadata, ClientHello, ErrorClass, ErrorResponse, Id32,
    InvalidationTopic, LifecycleRequest, LifecycleState, LifecycleStatus, MutationAttemptDto,
    MutationRequest, Request, ResponseEnvelope, ResponseResult, RevisionInvalidation, ServerHello,
    SubscriptionAcknowledgement, V1, VersionRange, VersionRejected, WireMessage,
};
use hq_local_api::{
    ClientAction, ClientError, ClientEvent, ConnectionGeneration, ReconnectPolicy,
    ReconnectingClient,
};

fn build() -> BuildMetadata {
    BuildMetadata::new("hq-test", "0.1.0", Some("0123456789ab")).expect("bounded build")
}

fn policy() -> ReconnectPolicy {
    ReconnectPolicy::new(Duration::from_millis(10), Duration::from_millis(40))
        .expect("valid backoff")
}

fn client() -> ReconnectingClient {
    ReconnectingClient::new(build(), policy(), 2).expect("positive identity history")
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
