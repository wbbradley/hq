//! Local API v1 semantic interoperability and adversarial framing contracts.

#![allow(clippy::expect_used)]

use hq_domain::{
    BoundedSet, CausalReferences, CommandId, EncryptionPublicKey, FactScope, InstallationId,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, SemanticPayload, ShortText, SigningPublicKey,
    Timestamp,
};
use hq_local_api::project_command_from_v1;
use hq_local_api::protocol::v1::{
    AgentSessionRequestDto, AgentSessionResultDto, AuthoritativeSnapshotDto, BuildMetadata,
    CanonicalEvidenceDto, CanonicalEvidenceRequestDto, ClientHello, ConversationKeyDto,
    ConversationPageDto, ConversationPageRequest, DecodeError, DeviceGrantDto, DomainErrorDto,
    EffectOutcomeDto, EffectRequestDto, EncodeError, ErrorClass, ErrorResponse,
    EvidenceIngestOutcomeDto, FrameDecoder, Id32, InvalidationTopic, LifecycleRequest,
    LifecycleState, LifecycleStatus, MAX_FRAME_BYTES, MutationAttemptDto, MutationOutcomeDto,
    MutationRequest, PeerRouteBlockDto, PeerRouteCandidateDto, ProjectCommandActionDto,
    ProjectCommandOutcomeDto, ProjectCommandRequestDto, RelayAccessDto, RelayAuthenticationDto,
    RelayConfigurationDto, RemoteCommandProgressDto, Request, RequestEnvelope, RequestId,
    ResourceHealthDto, ResourceInspectionRequestDto, ResourceInspectionResultDto,
    ResourceLocatorDto, ResourceSchemeDto, ResponseEnvelope, ResponseResult, RevisionInvalidation,
    ServerHello, SessionControlDto, SnapshotItem, SubscriptionAcknowledgement,
    SubscriptionRequestDto, SynchronizationRequestDto, V1, ValueError, VersionRange,
    VersionRejected, WireMessage, WorktreeProvisioningRequestDto, negotiate,
};

fn build() -> BuildMetadata {
    BuildMetadata::new("hq", "0.1.0", Some("0123456789ab")).expect("bounded build metadata")
}

fn hello() -> WireMessage {
    WireMessage::ClientHello(ClientHello::new(
        VersionRange::new(V1, V1).expect("ordered range"),
        build(),
    ))
}

fn plan() -> hq_application::FactPlan {
    let bytes = [7; 32];
    let installation = InstallationId::from_bytes(bytes);
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([]).expect("empty bounded set"),
        [],
    )
    .expect("empty causal references");
    hq_application::FactPlan::new(
        installation,
        Timestamp::from_unix_millis(1_700_000_000_123),
        FactScope::InstallationPrivate(installation),
        causal,
        SemanticPayload::InstallationDeclared {
            installation_id: installation,
            signing_key: SigningPublicKey::from_bytes(bytes),
            encryption_key: EncryptionPublicKey::from_bytes([9; 32]),
            label: Some(ShortText::new("laptop").expect("bounded label")),
        },
        [11; 32],
    )
}

#[test]
fn canonical_frame_round_trips_incrementally() {
    let frame = hello().encode_frame().expect("encodes");
    let split = frame.len() / 2;
    let mut decoder = FrameDecoder::new();
    assert_eq!(
        decoder.push(&frame[..split]).expect("prefix accepted"),
        None
    );
    assert_eq!(
        decoder.push(&frame[split..]).expect("suffix accepted"),
        Some(hello())
    );
    assert_eq!(decoder.buffered_len(), 0);
}

#[test]
fn framing_rejects_oversize_truncation_trailing_and_noncanonical_json() {
    let oversized = u32::try_from(MAX_FRAME_BYTES + 1)
        .expect("frame limit fits u32")
        .to_be_bytes();
    assert_eq!(
        WireMessage::decode_frame(&oversized),
        Err(DecodeError::FrameTooLarge)
    );

    let frame = hello().encode_frame().expect("encodes");
    assert_eq!(
        WireMessage::decode_frame(&frame[..frame.len() - 1]),
        Err(DecodeError::Truncated)
    );

    let mut trailing = frame.clone();
    trailing.push(0);
    assert_eq!(
        WireMessage::decode_frame(&trailing),
        Err(DecodeError::TrailingData)
    );

    let body = &frame[4..];
    let mut spaced_body = Vec::with_capacity(body.len() + 1);
    spaced_body.push(b' ');
    spaced_body.extend_from_slice(body);
    let mut spaced = u32::try_from(spaced_body.len())
        .expect("bounded test body")
        .to_be_bytes()
        .to_vec();
    spaced.extend_from_slice(&spaced_body);
    assert_eq!(
        WireMessage::decode_frame(&spaced),
        Err(DecodeError::NonCanonical)
    );
}

#[test]
fn strict_decode_rejects_unknown_fields() {
    let body = br#"{"type":"client_hello","value":{"versions":{"minimum":1,"maximum":1},"build":{"name":"hq","version":"0.1.0","commit":"0123456789ab"},"surprise":true}}"#;
    let mut frame = u32::try_from(body.len())
        .expect("bounded fixture")
        .to_be_bytes()
        .to_vec();
    frame.extend_from_slice(body);
    assert_eq!(
        WireMessage::decode_frame(&frame),
        Err(DecodeError::Malformed)
    );
}

#[test]
fn negotiation_selects_highest_common_version_and_rejects_disjoint_ranges() {
    assert_eq!(
        negotiate(
            VersionRange::new(1, 3).expect("ordered"),
            VersionRange::new(2, 4).expect("ordered")
        )
        .expect("overlap"),
        3
    );
    assert!(
        negotiate(
            VersionRange::new(1, 1).expect("ordered"),
            VersionRange::new(2, 2).expect("ordered")
        )
        .is_err()
    );
}

#[test]
fn exact_mutation_plan_round_trips_and_changed_input_changes_digest() {
    let command_id = CommandId::from_bytes([3; 32]);
    let request = MutationRequest::from_plan(command_id, plan()).expect("plan encodes");
    assert!(request.validate_digest());
    assert_eq!(request.clone().into_plan().expect("plan decodes"), plan());

    let changed = hq_application::FactPlan::new(
        plan().author(),
        Timestamp::from_unix_millis(1_700_000_000_124),
        plan().scope().clone(),
        plan().causal().clone(),
        plan().payload().clone(),
        [11; 32],
    );
    let changed = MutationRequest::from_plan(command_id, changed).expect("changed plan encodes");
    assert_ne!(request.request_digest(), changed.request_digest());
}

#[test]
fn typed_lifecycle_request_round_trips_through_the_one_envelope() {
    let message = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(41).expect("nonzero request id"),
        Request::Lifecycle(LifecycleRequest::Restart),
    ));
    let frame = message.encode_frame().expect("encodes");
    assert_eq!(WireMessage::decode_frame(&frame).expect("decodes"), message);
}

fn round_trip(message: &WireMessage) {
    let frame = message.encode_frame().expect("message encodes");
    assert_eq!(
        &WireMessage::decode_frame(&frame).expect("message decodes"),
        message
    );
}

#[test]
fn project_head_presence_matches_creation_semantics() {
    let id = |byte| Id32::new([byte; 32]);
    let request = |expected_head, action| {
        WireMessage::Request(RequestEnvelope::new(
            RequestId::new(1).expect("nonzero"),
            Request::ControlProject(Box::new(ProjectCommandRequestDto {
                command_id: id(1),
                operation_id: id(2),
                request_digest: id(3),
                account_id: id(4),
                project_id: id(5),
                home: id(6),
                expected_head,
                issued_at_unix_millis: 7,
                action,
            })),
        ))
    };
    let provisioning = || {
        ProjectCommandActionDto::ProvisionWorktree(WorktreeProvisioningRequestDto {
            mailbox_id: id(7),
            project_name: "project".to_owned(),
            brief: None,
            source: locator(),
            destination: locator(),
            branch: "feature".to_owned(),
            create_branch: true,
        })
    };

    request(None, provisioning())
        .encode_frame()
        .expect("creation has no prior project head");
    request(Some(id(8)), ProjectCommandActionDto::Open)
        .encode_frame()
        .expect("existing-project command has a head");
    assert!(matches!(
        request(Some(id(8)), provisioning()).encode_frame(),
        Err(EncodeError::InvalidValue(
            ValueError::InvalidValueCombination
        ))
    ));
    assert!(matches!(
        request(None, ProjectCommandActionDto::Open).encode_frame(),
        Err(EncodeError::InvalidValue(
            ValueError::InvalidValueCombination
        ))
    ));
    let WireMessage::Request(envelope) = request(None, ProjectCommandActionDto::Open) else {
        unreachable!("helper always returns a request")
    };
    let Request::ControlProject(request) = envelope.request else {
        unreachable!("helper always returns project control")
    };
    assert_eq!(
        project_command_from_v1(*request),
        Err(ValueError::InvalidValueCombination)
    );
}

fn effect<T>(body: T) -> EffectRequestDto<T> {
    EffectRequestDto::new(
        Id32::new([21; 32]),
        Id32::new([22; 32]),
        1_700_000_000_000,
        body,
    )
}

fn locator() -> ResourceLocatorDto {
    ResourceLocatorDto::new(ResourceSchemeDto::WorkingTree, "/work/hq".to_owned())
        .expect("bounded locator")
}

#[test]
fn every_request_notification_and_negotiation_family_interoperates() {
    let mut messages = vec![
        hello(),
        WireMessage::ServerHello(ServerHello::new(V1, build(), Id32::new([31; 32]))),
        WireMessage::VersionRejected(VersionRejected::new(
            VersionRange::new(V1, V1).expect("range"),
            build(),
        )),
    ];
    let requests = vec![
        Request::Lifecycle(LifecycleRequest::Status),
        Request::Lifecycle(LifecycleRequest::Readiness),
        Request::Lifecycle(LifecycleRequest::Stop),
        Request::AuthoritativeSnapshot,
        Request::ConversationPage(
            ConversationPageRequest::new(
                ConversationKeyDto::Thread {
                    counterparty_installation: Id32::new([1; 32]),
                    counterparty_mailbox: Id32::new([2; 32]),
                    thread: Id32::new([3; 32]),
                },
                64,
                Some("cursor-v1".to_owned()),
            )
            .expect("page request"),
        ),
        Request::Mutation(
            MutationRequest::from_plan(CommandId::from_bytes([3; 32]), plan()).expect("mutation"),
        ),
        Request::CanonicalEvidence(CanonicalEvidenceRequestDto {
            roots: vec![Id32::new([8; 32])],
        }),
        Request::IngestCanonicalEvidence(vec![CanonicalEvidenceDto {
            fact_id: Id32::new([8; 32]),
            exact_event: "{}".to_owned(),
        }]),
        Request::ConfigureRelay(effect(RelayConfigurationDto::new(
            locator(),
            RelayAccessDto::ReadWrite,
            RelayAuthenticationDto::OnChallenge,
        ))),
        Request::Synchronize(effect(SynchronizationRequestDto::All)),
        Request::ControlAgentSession(effect(
            AgentSessionRequestDto::new(
                Id32::new([4; 32]),
                "fake".to_owned(),
                SessionControlDto::Resume("session-1".to_owned()),
            )
            .expect("agent request"),
        )),
        Request::InspectResource(effect(ResourceInspectionRequestDto {
            project_id: Id32::new([5; 32]),
            resource_id: Id32::new([6; 32]),
            display_locator: locator(),
            canonical_locator: locator(),
        })),
        Request::ControlProject(Box::new(ProjectCommandRequestDto {
            command_id: Id32::new([40; 32]),
            operation_id: Id32::new([41; 32]),
            request_digest: Id32::new([42; 32]),
            account_id: Id32::new([43; 32]),
            project_id: Id32::new([44; 32]),
            home: Id32::new([45; 32]),
            expected_head: Some(Id32::new([46; 32])),
            issued_at_unix_millis: 1_700_000_000_000,
            action: ProjectCommandActionDto::Open,
        })),
        Request::Subscribe(
            SubscriptionRequestDto::new(
                Id32::new([7; 32]),
                vec![InvalidationTopic::Project, InvalidationTopic::Authority],
            )
            .expect("subscription"),
        ),
        Request::CancelSubscription {
            subscription_id: Id32::new([7; 32]),
        },
    ];
    for (index, request) in requests.into_iter().enumerate() {
        messages.push(WireMessage::Request(RequestEnvelope::new(
            RequestId::new(u64::try_from(index + 1).expect("small index")).expect("nonzero"),
            request,
        )));
    }
    messages.push(WireMessage::Invalidation(
        RevisionInvalidation::new(
            Id32::new([7; 32]),
            42,
            vec![InvalidationTopic::Project, InvalidationTopic::Conversation],
            true,
        )
        .expect("invalidation"),
    ));
    for message in messages {
        round_trip(&message);
    }
}

#[test]
fn every_success_and_error_response_family_interoperates() {
    let id = RequestId::new(9).expect("nonzero");
    let snapshot = AuthoritativeSnapshotDto::new(4, Vec::new()).expect("empty snapshot");
    let lifecycle = LifecycleStatus::new(
        LifecycleState::Ready,
        build(),
        Some(4),
        Some("ready".to_owned()),
    )
    .expect("status");
    let results = vec![
        ResponseResult::Lifecycle(lifecycle),
        ResponseResult::AuthoritativeSnapshot(snapshot.clone()),
        ResponseResult::ConversationPage(
            ConversationPageDto::new(Vec::new(), None).expect("empty page"),
        ),
        ResponseResult::Mutation(MutationAttemptDto::Completed {
            command_id: Id32::new([3; 32]),
            request_digest: Id32::new([8; 32]),
            revision: 4,
            outcome: MutationOutcomeDto::Committed,
        }),
        ResponseResult::CanonicalEvidence(vec![CanonicalEvidenceDto {
            fact_id: Id32::new([8; 32]),
            exact_event: "{}".to_owned(),
        }]),
        ResponseResult::EvidenceIngest(vec![EvidenceIngestOutcomeDto {
            fact_id: Id32::new([8; 32]),
            revision: 5,
            inserted: true,
        }]),
        ResponseResult::EmptyEffect(EffectOutcomeDto::Accepted(())),
        ResponseResult::AgentSession(EffectOutcomeDto::Accepted(AgentSessionResultDto::Ready(
            "session-1".to_owned(),
        ))),
        ResponseResult::ResourceInspection(EffectOutcomeDto::Accepted(
            ResourceInspectionResultDto::new(
                ResourceHealthDto::Healthy,
                Some(locator()),
                Some("clean".to_owned()),
                1_700_000_000_000,
            )
            .expect("inspection"),
        )),
        ResponseResult::ProjectCommand(ProjectCommandOutcomeDto::Completed {
            operation_id: Id32::new([41; 32]),
            project_head: Id32::new([47; 32]),
            runtime: None,
        }),
        ResponseResult::Subscription(SubscriptionAcknowledgement::new(
            Id32::new([7; 32]),
            snapshot,
        )),
        ResponseResult::Empty,
    ];
    for result in results {
        round_trip(&WireMessage::Response(ResponseEnvelope::success(
            id, result,
        )));
    }
    round_trip(&WireMessage::Response(ResponseEnvelope::error(
        id,
        ErrorResponse::new(
            ErrorClass::Conflict,
            "digest-conflict".to_owned(),
            Some("command input changed".to_owned()),
        )
        .expect("bounded error"),
    )));
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        id,
        ResponseResult::EmptyEffect(EffectOutcomeDto::Rejected(
            DomainErrorDto::new("conflict".to_owned(), "relay-conflict".to_owned())
                .expect("domain error"),
        )),
    )));
}

#[test]
fn response_outcomes_use_language_independent_lowercase_tags() {
    let id = RequestId::new(1).expect("nonzero");
    let success = WireMessage::Response(ResponseEnvelope::success(id, ResponseResult::Empty))
        .encode_frame()
        .expect("success encodes");
    let success = std::str::from_utf8(&success[4..]).expect("JSON is UTF-8");
    assert!(success.contains(r#""status":"success""#));
    assert!(!success.contains("Ok"));

    let error = WireMessage::Response(ResponseEnvelope::error(
        id,
        ErrorResponse::new(ErrorClass::Unavailable, "draining".to_owned(), None)
            .expect("bounded error"),
    ))
    .encode_frame()
    .expect("error encodes");
    let error = std::str::from_utf8(&error[4..]).expect("JSON is UTF-8");
    assert!(error.contains(r#""status":"error""#));
    assert!(!error.contains("Err"));
}

#[test]
fn declared_frame_limit_is_enforced_before_body_decode_or_allocation() {
    let maximum = u32::try_from(MAX_FRAME_BYTES)
        .expect("limit fits u32")
        .to_be_bytes();
    assert_eq!(
        WireMessage::decode_frame(&maximum),
        Err(DecodeError::Truncated)
    );

    let oversized = u32::try_from(MAX_FRAME_BYTES + 1)
        .expect("limit fits u32")
        .to_be_bytes();
    let mut decoder = FrameDecoder::new();
    assert_eq!(decoder.push(&oversized), Err(DecodeError::FrameTooLarge));
    assert!(decoder.buffered_len() <= 4);
}

#[test]
fn semantic_bounds_are_inclusive_and_decode_rechecks_constructor_invariants() {
    assert!(BuildMetadata::new("x".repeat(128), "1", None::<String>).is_ok());
    assert_eq!(
        BuildMetadata::new("x".repeat(129), "1", None::<String>),
        Err(ValueError::InvalidBuildMetadata)
    );
    assert_eq!(
        ConversationPageRequest::new(
            ConversationKeyDto::Thread {
                counterparty_installation: Id32::new([1; 32]),
                counterparty_mailbox: Id32::new([2; 32]),
                thread: Id32::new([3; 32]),
            },
            0,
            None,
        ),
        Err(ValueError::InvalidPageLimit)
    );

    let request = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(1).expect("nonzero"),
        Request::Mutation(
            MutationRequest::from_plan(CommandId::from_bytes([3; 32]), plan()).expect("mutation"),
        ),
    ));
    let mut value = serde_json::to_value(request).expect("serializes");
    value["value"]["request"]["params"]["auxiliary_randomness"][0] = 12.into();
    let body = serde_json::to_vec(&value).expect("canonical json value");
    let mut frame = u32::try_from(body.len())
        .expect("bounded fixture")
        .to_be_bytes()
        .to_vec();
    frame.extend_from_slice(&body);
    assert_eq!(
        WireMessage::decode_frame(&frame),
        Err(DecodeError::InvalidValue(
            ValueError::MutationDigestMismatch
        ))
    );
}

#[test]
fn incremental_decoder_retains_a_later_complete_frame() {
    let first = hello().encode_frame().expect("first frame");
    let second_message = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(2).expect("nonzero"),
        Request::Lifecycle(LifecycleRequest::Status),
    ));
    let second = second_message.encode_frame().expect("second frame");
    let mut bytes = first.clone();
    bytes.extend_from_slice(&second);
    let mut decoder = FrameDecoder::new();
    assert_eq!(
        decoder.push(&bytes).expect("two bounded frames"),
        Some(hello())
    );
    assert_eq!(
        decoder.push(&[]).expect("retained second frame"),
        Some(second_message)
    );
    assert_eq!(decoder.buffered_len(), 0);
}

#[test]
#[allow(clippy::too_many_lines)]
fn every_snapshot_projection_variant_round_trips_as_an_owned_client_dto() {
    let id = |byte| Id32::new([byte; 32]);
    let conversation = ConversationKeyDto::Thread {
        counterparty_installation: id(1),
        counterparty_mailbox: id(2),
        thread: id(3),
    };
    let items = vec![
        SnapshotItem::Installation {
            installation_id: id(1),
            root_fact: id(31),
            signing_key: id(2),
            encryption_key: id(3),
            label: Some("home".to_owned()),
        },
        SnapshotItem::Mailbox {
            installation_id: id(1),
            mailbox_id: id(4),
            create_fact: id(32),
            mailbox_kind: "human".to_owned(),
            label: Some("inbox".to_owned()),
        },
        SnapshotItem::Account {
            account_id: id(5),
            root_fact: id(33),
            creator_installation: id(1),
            label: None,
            selected: true,
        },
        SnapshotItem::PeerRoute {
            owner: id(1),
            peer: id(6),
            state: "routable".to_owned(),
            frontier: vec![id(7)],
            routes: vec![PeerRouteCandidateDto {
                fact_id: id(7),
                signing_key: id(35),
                encryption_key: id(36),
                label: Some("peer".to_owned()),
                relay_hints: vec![],
                frontier_member: true,
            }],
            blocks: vec![PeerRouteBlockDto {
                fact_id: id(8),
                reason: "prior-block".to_owned(),
                frontier_member: false,
            }],
        },
        SnapshotItem::MailboxCapability {
            grant_id: id(9),
            grant_fact: id(37),
            mailbox_installation: id(1),
            mailbox_id: id(4),
            grantee_installation: id(6),
            grantee_signing_key: id(38),
            active: true,
            revoke_frontier: vec![],
            observed_actions: vec![],
            support: vec![id(37)],
        },
        SnapshotItem::Membership {
            account_id: id(5),
            device: id(6),
            state: "active".to_owned(),
            frontier: vec![id(9), id(10)],
            grants: vec![DeviceGrantDto {
                grant_id: id(9),
                grant_fact: id(33),
                device: id(6),
                signing_key: id(34),
                label: Some("laptop".to_owned()),
                relay_hints: vec![],
                frontier_member: false,
                active: true,
            }],
            acceptances: vec![id(10)],
            revokes: vec![],
            active_acceptances: vec![id(10)],
        },
        SnapshotItem::AccountSelection {
            installation_id: id(1),
            candidates: vec![id(5)],
            active: Some(id(5)),
            frontier: vec![id(34)],
        },
        SnapshotItem::Conversation {
            key: conversation,
            latest_fact: Some(id(11)),
            open_messages: 1,
        },
        SnapshotItem::Agent {
            agent_id: id(12),
            names: vec!["helper".to_owned()],
            lifecycle: "active".to_owned(),
            runnable: true,
        },
        SnapshotItem::AgentSession {
            provider: "fake".to_owned(),
            session: "session-1".to_owned(),
            mailbox_installation: Some(id(1)),
            mailbox_id: Some(id(13)),
            conflicted: false,
        },
        SnapshotItem::AgentSelection {
            agent_id: id(12),
            provider: Some("fake".to_owned()),
            session: Some("session-1".to_owned()),
            conflicted: false,
        },
        SnapshotItem::AgentSessionName {
            agent_id: id(12),
            provider: "fake".to_owned(),
            session: "session-1".to_owned(),
            resolved: true,
            display_name: Some("work".to_owned()),
        },
        SnapshotItem::Project {
            project_id: id(14),
            home: id(1),
            name: "rewrite".to_owned(),
            lifecycle: "open".to_owned(),
            archived: false,
            claimable: true,
            head: id(15),
            input_sequence: 1,
        },
        SnapshotItem::ProjectResource {
            project_id: id(14),
            resource_id: id(23),
            display_locator: ResourceLocatorDto::new(
                ResourceSchemeDto::WorkingTree,
                "/selected/rewrite".to_owned(),
            )
            .expect("display locator validates"),
            canonical_locator: ResourceLocatorDto::new(
                ResourceSchemeDto::WorkingTree,
                "/workspace/rewrite".to_owned(),
            )
            .expect("canonical locator validates"),
            health: ResourceHealthDto::Healthy,
            primary: true,
            active_claim: true,
            conflicting_projects: vec![id(24)],
        },
        SnapshotItem::ProjectInput {
            project_id: id(14),
            message_id: id(16),
            sequence: 1,
            accepted_fact: id(17),
        },
        SnapshotItem::ProjectDispatch {
            dispatch_id: id(18),
            message_id: id(16),
            sequence: 1,
            fact_id: id(19),
            conflicted: false,
        },
        SnapshotItem::ProjectOutput {
            output_id: id(20),
            dispatch_id: id(18),
            status: "current".to_owned(),
            content: "done".to_owned(),
        },
        SnapshotItem::RemoteCommand {
            command_id: id(21),
            request_digest: id(22),
            account_id: id(23),
            project_id: id(14),
            target_home: id(24),
            expected_head: id(25),
            operation_provider: "hq".to_owned(),
            operation_session: "project-control-v1".to_owned(),
            operation_id: id(26),
            body: "hq-project-action-v1:open".to_owned(),
            issued_at_unix_millis: 1_700_000_000_123,
            request_fact: id(27),
            progress: Box::new(RemoteCommandProgressDto::Received {
                receipt_fact: id(28),
                received_head: id(25),
                received_at_unix_millis: 1_700_000_000_456,
            }),
        },
    ];
    let snapshot = AuthoritativeSnapshotDto::new(9, items).expect("snapshot is bounded");
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(3).expect("nonzero"),
        ResponseResult::AuthoritativeSnapshot(snapshot),
    )));
}

#[test]
fn peer_route_snapshot_rejects_inconsistent_frontier_and_state() {
    let id = |byte| Id32::new([byte; 32]);
    let route = |frontier_member| PeerRouteCandidateDto {
        fact_id: id(3),
        signing_key: id(4),
        encryption_key: id(5),
        label: None,
        relay_hints: vec![],
        frontier_member,
    };
    let item = |state: &str, frontier_member: bool| SnapshotItem::PeerRoute {
        owner: id(1),
        peer: id(2),
        state: state.to_owned(),
        frontier: vec![id(3)],
        routes: vec![route(frontier_member)],
        blocks: vec![],
    };

    assert_eq!(
        AuthoritativeSnapshotDto::new(1, vec![item("routable", false)]),
        Err(ValueError::InvalidValueCombination)
    );
    assert_eq!(
        AuthoritativeSnapshotDto::new(1, vec![item("blocked", true)]),
        Err(ValueError::InvalidValueCombination)
    );
}
