//! Local API v1 semantic interoperability and adversarial framing contracts.

#![allow(clippy::expect_used)]

use hq_application::{
    AuthoritativeSnapshot, ConversationContext, ConversationKey, ConversationSummary,
    DomainSnapshot, ProjectCommandAction, ProjectCommandRequest,
};
use hq_domain::{
    AccountId, BoundedSet, BoundedText, CausalReferences, CommandDigest, CommandId,
    EncryptionPublicKey, FactId, FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS,
    MailboxAddress, MailboxId, MessageId, OperationId, ProjectId, ResourceId, ResourceLocator,
    ResourceScheme, Revision, SemanticPayload, ShortText, SigningPublicKey, ThreadId, Timestamp,
};
use hq_local_api::protocol::v1::{
    ActivityStatusDto, AgentLaunchContextDto, AgentRetirementOutcomeDto, AgentRetirementRequestDto,
    AgentSelectionCandidateDto, AgentSessionBindingDto, AgentSessionNameCandidateDto,
    AgentSessionRequestDto, AgentSessionResultDto, AuthoritativeConversationViewDto,
    AuthoritativeConversationViewRequestDto, AuthoritativeSnapshotDto, BuildMetadata,
    CanonicalEvidenceDto, CanonicalEvidenceRequestDto, ClientHello, CompletedItemPresentationDto,
    ConversationActivityDto, ConversationActivityKindDto, ConversationContextDto,
    ConversationEntryDto, ConversationKeyDto, ConversationMessageDto, ConversationPageDto,
    ConversationPageRequest, ConversationPageSelectionDto, ConversationParticipantDto, DecodeError,
    DeviceGrantDto, DomainErrorDto, DomainHealthDto, EffectOutcomeDto, EffectRequestDto,
    EncodeError, ErrorClass, ErrorResponse, EvidenceIngestOutcomeDto, FrameDecoder,
    HealthDomainDto, Id32, InstallationConfigurationDto, InstallationConfigurationPatchDto,
    InteractionAnswerOutcomeDto, InteractionAnswerRequestDto, InteractionChoiceDto,
    InteractionKindDto, InteractionResponderAcknowledgement, InteractionResponseDto,
    InvalidationTopic, LaunchEnvironmentDto, LifecycleRequest, LifecycleState, LifecycleStatus,
    MAX_FRAME_BYTES, MAX_PROVIDER_CATALOG_ITEMS, MailboxAddressDto, MailboxCommandActionDto,
    MailboxCommandRequestDto, MailboxDraftDeleteOutcomeDto, MailboxDraftDeleteRequestDto,
    MailboxDraftDto, MailboxDraftSaveOutcomeDto, MailboxDraftSaveRequestDto, MailboxDraftTargetDto,
    MessagePurposeDto, MutationAttemptDto, MutationOutcomeDto, MutationRequest, PeerRouteBlockDto,
    PeerRouteCandidateDto, PendingInteractionDto, PendingInteractionsRequestDto,
    PresentationKindDto, ProjectCommandActionDto, ProjectCommandOutcomeDto,
    ProjectCommandRequestDto, ProjectCreationRequestDto, ProjectExternalStateWarningDto,
    ProviderAvailabilityDto, ProviderCatalogDto, RelayAccessDto, RelayAuthenticationDto,
    RelayConfigurationDto, RelayPolicyStatusDto, RelayStatusDto, RemoteCommandProgressDto, Request,
    RequestEnvelope, RequestId, ResourceHealthDto, ResourceInspectionRequestDto,
    ResourceInspectionResultDto, ResourceLocatorDto, ResourceReleaseStateDto, ResourceSchemeDto,
    ResponseEnvelope, ResponseResult, RevisionInvalidation, SelectedConversationPageDto,
    ServerHello, SessionControlDto, SnapshotItem, StateHealthDto, StateRepairReportDto,
    SubscriptionAcknowledgement, SubscriptionRequestDto, SynchronizationRequestDto, V1, ValueError,
    VersionRange, VersionRejected, WireMessage, WorktreeProvisioningRequestDto,
    agent_session_request_digest, negotiate, resource_inspection_request_digest,
};
use hq_local_api::{project_command_from_v1, project_command_request_to_v1, snapshot_to_v1};

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
fn project_thread_conversation_key_round_trips_in_local_api_v1() {
    let key = ConversationKeyDto::ProjectThread {
        project: Id32::new([0x31; 32]),
        thread: Id32::new([0x32; 32]),
    };
    let message = WireMessage::Request(RequestEnvelope::new(
        RequestId::new(1).expect("nonzero request"),
        Request::ConversationPage(
            ConversationPageRequest::new(key.clone(), 32, None).expect("page request"),
        ),
    ));
    let value = serde_json::to_value(&message).expect("request serializes");
    assert_eq!(
        value["value"]["request"]["params"]["key"],
        serde_json::json!({
            "kind": "project_thread",
            "project": vec![0x31; 32],
            "thread": vec![0x32; 32]
        })
    );
    assert_eq!(
        WireMessage::decode_frame(&message.encode_frame().expect("request encodes")),
        Ok(message)
    );
}

#[test]
fn materialized_conversation_view_round_trips_with_one_bounded_typed_selection() {
    let key = ConversationKeyDto::Thread {
        counterparty_installation: Id32::new([1; 32]),
        counterparty_mailbox: Id32::new([2; 32]),
        thread: Id32::new([5; 32]),
    };
    let selection = ConversationPageSelectionDto::new(key.clone(), 100).expect("selection");
    let snapshot = AuthoritativeSnapshotDto::new(7, Vec::new()).expect("snapshot");
    let page = ConversationPageDto::new(Vec::new(), None).expect("page");
    let view = AuthoritativeConversationViewDto::new(
        snapshot,
        Some(SelectedConversationPageDto::new(key.clone(), page)),
    )
    .expect("coherent view");

    round_trip(&WireMessage::Request(RequestEnvelope::new(
        RequestId::new(1).expect("request id"),
        Request::AuthoritativeConversationView(AuthoritativeConversationViewRequestDto::new(Some(
            selection.clone(),
        ))),
    )));
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(1).expect("request id"),
        ResponseResult::AuthoritativeConversationView(view.clone()),
    )));
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(2).expect("request id"),
        ResponseResult::Subscription(SubscriptionAcknowledgement::new(Id32::new([6; 32]), view)),
    )));
    round_trip(&WireMessage::Request(RequestEnvelope::new(
        RequestId::new(3).expect("request id"),
        Request::Subscribe(
            SubscriptionRequestDto::new(
                Id32::new([6; 32]),
                vec![InvalidationTopic::All],
                Some(selection),
            )
            .expect("subscription"),
        ),
    )));

    assert!(ConversationPageSelectionDto::new(key.clone(), 0).is_err());
    assert!(ConversationPageSelectionDto::new(key, 201).is_err());
}

#[test]
fn project_thread_snapshot_summary_converts_to_the_same_typed_v1_key() {
    let project_id = ProjectId::from_bytes([0x41; 32]);
    let thread = ThreadId::from_bytes([0x42; 32]);
    let snapshot = AuthoritativeSnapshot::with_conversations(
        Revision::new(7),
        DomainSnapshot::empty(),
        vec![ConversationSummary {
            key: ConversationKey::ProjectThread { project_id, thread },
            context: ConversationContext::Project {
                project_id,
                name: Some(ShortText::new("release").expect("project name")),
                participant: None,
            },
            local_human: MailboxAddress::new(
                InstallationId::from_bytes([0x45; 32]),
                MailboxId::from_bytes([0x46; 32]),
            ),
            root_message: Some(MessageId::from_bytes([0x44; 32])),
            preview: Some(ShortText::new("Ship it").expect("preview")),
            latest_fact: Some(FactId::from_bytes([0x43; 32])),
            open_messages: 1,
            archived_messages: 0,
            sent_messages: 1,
        }],
    );
    let converted = snapshot_to_v1(&snapshot).expect("snapshot converts");
    assert_eq!(converted.items.len(), 1);
    let (project, converted_thread, context_project, name, participant, preview) = converted
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Conversation {
                key:
                    ConversationKeyDto::ProjectThread {
                        project,
                        thread: converted_thread,
                    },
                context:
                    ConversationContextDto::Project {
                        project: context_project,
                        name,
                        participant,
                    },
                preview,
                ..
            } => Some((
                project,
                converted_thread,
                context_project,
                name,
                participant,
                preview,
            )),
            _ => None,
        })
        .expect("one project conversation summary converts");
    assert_eq!(project.bytes(), *project_id.as_bytes());
    assert_eq!(converted_thread.bytes(), *thread.as_bytes());
    assert_eq!(context_project, project);
    assert_eq!(name.as_deref(), Some("release"));
    assert_eq!(participant, &None);
    assert_eq!(preview.as_deref(), Some("Ship it"));
}

#[test]
fn conversation_summary_validation_rejects_incoherent_v1_context_without_a_version_bump() {
    let id = |byte| Id32::new([byte; 32]);
    let project = id(0x51);
    let thread = id(0x52);
    let summary = |context, preview| SnapshotItem::Conversation {
        key: ConversationKeyDto::ProjectThread { project, thread },
        context,
        local_human: MailboxAddressDto {
            installation_id: id(0x55),
            mailbox_id: id(0x56),
        },
        root_message: Some(id(0x54)),
        preview,
        latest_fact: None,
        open_messages: 0,
        archived_messages: 0,
        sent_messages: 0,
    };

    assert_eq!(V1, 1);
    assert_eq!(
        AuthoritativeSnapshotDto::new(
            1,
            vec![summary(
                ConversationContextDto::Project {
                    project: id(0x53),
                    name: Some("release".to_owned()),
                    participant: None,
                },
                None,
            )],
        ),
        Err(ValueError::InvalidValueCombination)
    );
    assert_eq!(
        AuthoritativeSnapshotDto::new(
            1,
            vec![summary(
                ConversationContextDto::Direct {
                    participant: ConversationParticipantDto {
                        agent: None,
                        installation: Some(id(0x54)),
                        mailbox: Some(id(0x55)),
                        name: Some("unverified name".to_owned()),
                    },
                },
                None,
            )],
        ),
        Err(ValueError::InvalidValueCombination)
    );
    assert!(
        AuthoritativeSnapshotDto::new(
            1,
            vec![summary(
                ConversationContextDto::Project {
                    project,
                    name: None,
                    participant: None,
                },
                Some("x".repeat(hq_domain::SHORT_TEXT_MAX_BYTES + 1)),
            )],
        )
        .is_err()
    );
}

#[test]
fn provider_catalog_bounds_zero_one_many_unavailable_and_stale_defaults() {
    assert!(ProviderCatalogDto::new(Vec::new(), None).is_ok());
    assert!(
        ProviderCatalogDto::new(
            vec![ProviderAvailabilityDto::new("codex", "Codex", true).expect("provider")],
            Some("codex".to_owned()),
        )
        .is_ok()
    );
    assert!(
        ProviderCatalogDto::new(
            vec![ProviderAvailabilityDto::new("removed", "Removed", false).expect("provider")],
            Some("missing".to_owned()),
        )
        .is_ok()
    );
    let duplicate = ProviderAvailabilityDto::new("same", "Same", true).expect("provider");
    assert_eq!(
        ProviderCatalogDto::new(vec![duplicate.clone(), duplicate], None),
        Err(ValueError::InvalidValueCombination)
    );
    let too_many = (0..=MAX_PROVIDER_CATALOG_ITEMS)
        .map(|index| {
            ProviderAvailabilityDto::new(format!("p{index:02}"), "Provider", true)
                .expect("provider")
        })
        .collect();
    assert_eq!(
        ProviderCatalogDto::new(too_many, None),
        Err(ValueError::InvalidValueCombination)
    );
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
fn mailbox_command_digest_binds_content_target_and_draft_source() {
    let request = MailboxCommandRequestDto::new(
        Id32::new([0x21; 32]),
        None,
        MailboxCommandActionDto::SelfNote {
            message_id: Id32::new([0x22; 32]),
        },
        Some("note".to_owned()),
        7,
        [0x23; 32],
    );
    let changed = MailboxCommandRequestDto::new(
        Id32::new([0x21; 32]),
        None,
        MailboxCommandActionDto::SelfNote {
            message_id: Id32::new([0x22; 32]),
        },
        Some("changed".to_owned()),
        7,
        [0x23; 32],
    );
    assert_ne!(request.request_digest, changed.request_digest);
    let mut tampered = request.clone();
    tampered.content = Some("changed".to_owned());
    assert!(
        WireMessage::Request(RequestEnvelope::new(
            RequestId::new(1).expect("id"),
            Request::ControlMailbox(Box::new(tampered)),
        ))
        .encode_frame()
        .is_err()
    );

    let draft_backed = MailboxCommandRequestDto::new(
        Id32::new([0x21; 32]),
        Some(Id32::new([0x24; 32])),
        MailboxCommandActionDto::SelfNote {
            message_id: Id32::new([0x22; 32]),
        },
        None,
        7,
        [0x23; 32],
    );
    assert_ne!(request.request_digest, draft_backed.request_digest);

    let project_root = MailboxCommandRequestDto::new(
        Id32::new([0x21; 32]),
        None,
        MailboxCommandActionDto::Project {
            project_id: Id32::new([0x25; 32]),
            thread_id: None,
            message_id: Id32::new([0x22; 32]),
        },
        Some("project message".to_owned()),
        7,
        [0x23; 32],
    );
    let project_continuation = MailboxCommandRequestDto::new(
        Id32::new([0x21; 32]),
        None,
        MailboxCommandActionDto::Project {
            project_id: Id32::new([0x25; 32]),
            thread_id: Some(Id32::new([0x26; 32])),
            message_id: Id32::new([0x22; 32]),
        },
        Some("project message".to_owned()),
        7,
        [0x23; 32],
    );
    assert_ne!(
        project_root.request_digest,
        project_continuation.request_digest
    );
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

#[test]
fn installation_configuration_patches_are_field_specific_and_bounded() {
    for patch in [
        InstallationConfigurationPatchDto::DefaultProvider(Some("codex".to_owned())),
        InstallationConfigurationPatchDto::Theme(None),
        InstallationConfigurationPatchDto::CodexModel(Some("gpt-test".to_owned())),
        InstallationConfigurationPatchDto::CodexYolo(true),
    ] {
        round_trip(&WireMessage::Request(RequestEnvelope::new(
            RequestId::new(43).expect("request id"),
            Request::UpdateInstallationConfiguration(patch),
        )));
    }
    assert!(matches!(
        WireMessage::Request(RequestEnvelope::new(
            RequestId::new(44).expect("request id"),
            Request::UpdateInstallationConfiguration(
                InstallationConfigurationPatchDto::CodexModel(Some(String::new())),
            ),
        ))
        .encode_frame(),
        Err(EncodeError::InvalidValue(ValueError::InvalidText))
    ));
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
            base: Some("main".to_owned()),
            create_branch: true,
        })
    };
    let creation = || {
        ProjectCommandActionDto::Create(ProjectCreationRequestDto {
            mailbox_id: id(7),
            project_name: "project".to_owned(),
            brief: Some("existing resource".to_owned()),
            resource_id: id(9),
            resource: locator(),
        })
    };

    request(None, provisioning())
        .encode_frame()
        .expect("creation has no prior project head");
    request(None, creation())
        .encode_frame()
        .expect("existing-resource creation has no prior project head");
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
        request(Some(id(8)), creation()).encode_frame(),
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

#[test]
fn lifecycle_project_commands_round_trip_through_the_application_boundary() {
    for action in [
        ProjectCommandAction::Open,
        ProjectCommandAction::Close { force: false },
        ProjectCommandAction::Close { force: true },
        ProjectCommandAction::SetArchived { archived: true },
        ProjectCommandAction::SetArchived { archived: false },
        ProjectCommandAction::AddResource {
            resource_id: ResourceId::from_bytes([8; 32]),
            resource: ResourceLocator::new(
                ResourceScheme::WorkingTree,
                BoundedText::new("/work/added".to_owned()).expect("locator"),
            ),
            make_primary: true,
        },
        ProjectCommandAction::ReplaceResource {
            old_resource_id: ResourceId::from_bytes([9; 32]),
            new_resource_id: ResourceId::from_bytes([10; 32]),
            resource: ResourceLocator::new(
                ResourceScheme::WorkingTree,
                BoundedText::new("/work/replaced".to_owned()).expect("locator"),
            ),
        },
        ProjectCommandAction::SetPrimaryResource {
            resource_id: ResourceId::from_bytes([11; 32]),
        },
    ] {
        let request = ProjectCommandRequest {
            command_id: CommandId::from_bytes([1; 32]),
            operation_id: OperationId::from_bytes([2; 32]),
            request_digest: CommandDigest::from_bytes([3; 32]),
            account_id: AccountId::from_bytes([4; 32]),
            project_id: ProjectId::from_bytes([5; 32]),
            home: InstallationId::from_bytes([6; 32]),
            expected_head: Some(FactId::from_bytes([7; 32])),
            issued_at: Timestamp::from_unix_millis(1_700_000_000_000),
            action,
        };
        let wire = project_command_request_to_v1(&request);
        assert_eq!(
            project_command_from_v1(wire).expect("valid project request"),
            request
        );
    }
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
fn launch_environment_is_binary_safe_redacted_and_bound_into_session_identity() {
    let environment = LaunchEnvironmentDto::copy_from([
        ("HQ_TOKEN", &[0xff, 0x80, b'x'][..]),
        ("PATH", b"/usr/bin".as_slice()),
    ])
    .expect("valid environment");
    let diagnostic = format!("{environment:?}");
    assert_eq!(diagnostic, "LaunchEnvironmentDto { entry_count: 2, .. }");
    assert!(!diagnostic.contains("HQ_TOKEN"));
    assert!(!diagnostic.contains("/usr/bin"));

    let body = AgentSessionRequestDto::new(
        Id32::new([41; 32]),
        "fake".to_owned(),
        SessionControlDto::Start,
        Some(AgentLaunchContextDto {
            directory: locator(),
            environment,
        }),
    )
    .expect("session request");
    let request = EffectRequestDto::new(
        Id32::new([42; 32]),
        Id32::new([0; 32]),
        1_700_000_000_000,
        body,
    );
    let digest = agent_session_request_digest(&request).expect("request digest");
    let encoded = serde_json::to_vec(&request).expect("wire JSON");
    let decoded: EffectRequestDto<AgentSessionRequestDto> =
        serde_json::from_slice(&encoded).expect("binary environment round trips");
    assert_eq!(
        agent_session_request_digest(&decoded).expect("decoded digest"),
        digest
    );

    let changed_environment =
        LaunchEnvironmentDto::copy_from([("HQ_TOKEN", b"different".as_slice())])
            .expect("changed environment");
    let changed = EffectRequestDto::new(
        request.operation_id,
        Id32::new([0; 32]),
        request.issued_at_unix_millis,
        AgentSessionRequestDto::new(
            request.body.agent_id,
            request.body.provider,
            SessionControlDto::Start,
            Some(AgentLaunchContextDto {
                directory: locator(),
                environment: changed_environment,
            }),
        )
        .expect("changed request"),
    );
    assert_ne!(
        agent_session_request_digest(&changed).expect("changed digest"),
        digest
    );
}

#[test]
fn resource_inspection_digest_binds_stable_identity_and_both_locators() {
    let body = ResourceInspectionRequestDto {
        project_id: Id32::new([5; 32]),
        resource_id: Id32::new([6; 32]),
        display_locator: locator(),
        canonical_locator: locator(),
    };
    let request = EffectRequestDto::new(
        Id32::new([7; 32]),
        Id32::new([0; 32]),
        1_700_000_000_000,
        body,
    );
    let digest = resource_inspection_request_digest(&request).expect("inspection digest");
    let decoded: EffectRequestDto<ResourceInspectionRequestDto> =
        serde_json::from_slice(&serde_json::to_vec(&request).expect("inspection JSON"))
            .expect("inspection round trip");
    assert_eq!(
        resource_inspection_request_digest(&decoded).expect("decoded digest"),
        digest
    );

    let mut changed = request;
    changed.body.canonical_locator =
        ResourceLocatorDto::new(ResourceSchemeDto::WorkingTree, "/work/other".to_owned())
            .expect("changed locator");
    assert_ne!(
        resource_inspection_request_digest(&changed).expect("changed digest"),
        digest
    );
}

fn repair_state_request() -> Request {
    Request::RepairState {
        operation_id: Id32::new([39; 32]),
    }
}

fn health_domains() -> Vec<DomainHealthDto> {
    [
        HealthDomainDto::Authority,
        HealthDomainDto::Conversation,
        HealthDomainDto::Agent,
        HealthDomainDto::Project,
    ]
    .into_iter()
    .map(|domain| DomainHealthDto {
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

fn relay_status_result() -> ResponseResult {
    ResponseResult::RelayStatus(RelayStatusDto {
        policies: vec![RelayPolicyStatusDto {
            endpoint: locator(),
            access: RelayAccessDto::ReadWrite,
            authentication: RelayAuthenticationDto::Required,
            enabled: true,
            generation: 1,
        }],
        queued: 1,
        prepared: 1,
        uncertain: 0,
        rejected: 0,
        accepted: 1,
        staged: 0,
        quarantined: 0,
        truncated: false,
    })
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "closed protocol family interoperability matrix"
)]
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
        Request::ProviderCatalog,
        Request::InstallationConfiguration,
        Request::UpdateInstallationConfiguration(InstallationConfigurationPatchDto::CodexYolo(
            true,
        )),
        Request::MailboxDrafts,
        Request::SaveMailboxDraft(MailboxDraftSaveRequestDto {
            draft_id: Id32::new([0x11; 32]),
            target: MailboxDraftTargetDto::ProjectSetup {
                project_id: Id32::new([0x21; 32]),
                agent_id: Id32::new([0x22; 32]),
                provider: "codex".to_owned(),
            },
            content: String::new(),
            expected_version: None,
        }),
        Request::DeleteMailboxDraft(MailboxDraftDeleteRequestDto {
            draft_id: Id32::new([0x11; 32]),
            expected_version: 1,
        }),
        Request::ControlMailbox(Box::new(MailboxCommandRequestDto::new(
            Id32::new([0x12; 32]),
            None,
            MailboxCommandActionDto::SelfNote {
                message_id: Id32::new([0x13; 32]),
            },
            Some("note".to_owned()),
            1,
            [0x14; 32],
        ))),
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
            true,
        ))),
        Request::Synchronize(effect(SynchronizationRequestDto::All)),
        Request::RelayStatus,
        Request::StateHealth,
        repair_state_request(),
        Request::ControlAgentSession(Box::new(effect(
            AgentSessionRequestDto::new(
                Id32::new([4; 32]),
                "fake".to_owned(),
                SessionControlDto::Resume("session-1".to_owned()),
                Some(AgentLaunchContextDto {
                    directory: locator(),
                    environment: LaunchEnvironmentDto::default(),
                }),
            )
            .expect("agent request"),
        ))),
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
        Request::RetireAgent(Box::new(AgentRetirementRequestDto {
            command_id: Id32::new([48; 32]),
            operation_id: Id32::new([49; 32]),
            request_digest: Id32::new([50; 32]),
            account_id: Id32::new([43; 32]),
            agent_id: Id32::new([12; 32]),
            expected_claim: Id32::new([35; 32]),
            home: Id32::new([45; 32]),
            issued_at_unix_millis: 1_700_000_000_000,
            force: true,
        })),
        Request::PendingInteractions(PendingInteractionsRequestDto { limit: 64 }),
        Request::AnswerInteraction(InteractionAnswerRequestDto {
            command_id: Id32::new([51; 32]),
            agent_id: Id32::new([52; 32]),
            request_id: Id32::new([53; 32]),
            response: InteractionResponseDto::Choice("accept".to_owned()),
        }),
        Request::RegisterInteractionResponder {
            responder_id: Id32::new([54; 32]),
        },
        Request::CancelInteractionResponder {
            responder_id: Id32::new([54; 32]),
        },
        Request::Subscribe(
            SubscriptionRequestDto::new(
                Id32::new([7; 32]),
                vec![InvalidationTopic::Project, InvalidationTopic::Authority],
                None,
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
#[allow(clippy::too_many_lines)]
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
    let domains = health_domains();
    let results = vec![
        ResponseResult::Lifecycle(lifecycle),
        ResponseResult::AuthoritativeSnapshot(snapshot.clone()),
        ResponseResult::ProviderCatalog(
            ProviderCatalogDto::new(
                vec![
                    ProviderAvailabilityDto::new("codex", "Codex", true)
                        .expect("available provider"),
                    ProviderAvailabilityDto::new("retired", "Retired service", false)
                        .expect("unavailable provider"),
                ],
                Some("missing".to_owned()),
            )
            .expect("stale defaults remain typed evidence"),
        ),
        ResponseResult::InstallationConfiguration(InstallationConfigurationDto {
            default_provider: Some("codex".to_owned()),
            theme: Some("gruvbox-dark-hard".to_owned()),
            codex_model: Some("gpt-test".to_owned()),
            codex_yolo: true,
        }),
        ResponseResult::ConversationPage(
            ConversationPageDto::new(Vec::new(), None).expect("empty page"),
        ),
        ResponseResult::MailboxDrafts(vec![MailboxDraftDto {
            draft_id: Id32::new([0x11; 32]),
            target: MailboxDraftTargetDto::SelfNote,
            content: String::new(),
            version: 1,
        }]),
        ResponseResult::MailboxDraftSave(MailboxDraftSaveOutcomeDto::Saved(MailboxDraftDto {
            draft_id: Id32::new([0x11; 32]),
            target: MailboxDraftTargetDto::SelfNote,
            content: "note".to_owned(),
            version: 2,
        })),
        ResponseResult::MailboxDraftDelete(MailboxDraftDeleteOutcomeDto::Deleted),
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
        relay_status_result(),
        ResponseResult::StateHealth(StateHealthDto {
            revision: 4,
            domains: domains.clone(),
        }),
        ResponseResult::StateRepair(StateRepairReportDto {
            operation_id: Id32::new([39; 32]),
            revision: 4,
            domains,
        }),
        ResponseResult::AgentSession(EffectOutcomeDto::Accepted(AgentSessionResultDto::Ready(
            "session-1".to_owned(),
        ))),
        ResponseResult::ResourceInspection(EffectOutcomeDto::Accepted(
            ResourceInspectionResultDto::new(
                ResourceHealthDto::Healthy,
                Some(locator()),
                ResourceReleaseStateDto::Clean,
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
        ResponseResult::AgentRetirement(AgentRetirementOutcomeDto::Completed {
            operation_id: Id32::new([49; 32]),
            project_id: Some(Id32::new([44; 32])),
            runtime: Some(
                hq_local_api::protocol::v1::RuntimeObservationDto::Uncertain(
                    "project_runtime_stop_unknown".to_owned(),
                ),
            ),
        }),
        ResponseResult::PendingInteractions(vec![PendingInteractionDto {
            agent_id: Id32::new([52; 32]),
            project_id: Some(Id32::new([44; 32])),
            provider: "codex".to_owned(),
            session: "session-1".to_owned(),
            request_id: Id32::new([53; 32]),
            operation_id: Id32::new([55; 32]),
            kind: InteractionKindDto::CommandApproval,
            prompt: "Run tests?".to_owned(),
            choices: vec![InteractionChoiceDto {
                value: "accept".to_owned(),
                label: "Allow once".to_owned(),
            }],
            allow_text: false,
        }]),
        ResponseResult::InteractionAnswer(InteractionAnswerOutcomeDto::Answered),
        ResponseResult::InteractionResponder(InteractionResponderAcknowledgement {
            responder_id: Id32::new([54; 32]),
        }),
        ResponseResult::Subscription(SubscriptionAcknowledgement::new(
            Id32::new([7; 32]),
            AuthoritativeConversationViewDto::new(snapshot, None).expect("snapshot view"),
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
fn project_external_state_warning_interoperates_as_typed_data() {
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(9).expect("nonzero"),
        ResponseResult::ProjectCommand(ProjectCommandOutcomeDto::Reconcilable {
            operation_id: Id32::new([42; 32]),
            stage: hq_local_api::protocol::v1::ProjectCommandStageDto::CreatingWorktree,
            error: DomainErrorDto::new("effect".to_owned(), "git_unknown".to_owned())
                .expect("domain error"),
            external_state_warning: Some(ProjectExternalStateWarningDto::WorktreeMayExist {
                destination: locator(),
                branch: "feature/exact".to_owned(),
            }),
        }),
    )));
}

#[test]
fn conversation_message_page_preserves_addressing_state_and_ready_thread_semantics() {
    let entry = ConversationEntryDto::Message(Box::new(ConversationMessageDto {
        fact_id: Id32::new([1; 32]),
        message_id: Id32::new([2; 32]),
        thread_id: Id32::new([3; 32]),
        content: "answer".to_owned(),
        sender_installation: Id32::new([4; 32]),
        sender_mailbox: Id32::new([5; 32]),
        recipient_installation: Some(Id32::new([6; 32])),
        recipient_mailbox: Some(Id32::new([7; 32])),
        purpose: MessagePurposeDto::Question,
        presentation: PresentationKindDto::FinalAnswer,
        correlation_provider: None,
        correlation_session: None,
        correlation_operation: None,
        project_id: None,
        open: true,
        rejected: false,
        state_frontier: vec![Id32::new([8; 32])],
        peer_received_by: vec![Id32::new([9; 32])],
        root_fact: Some(Id32::new([10; 32])),
        root_message: Some(Id32::new([11; 32])),
        ready_answer: true,
        thread_cancelled: false,
    }));
    let page = ConversationPageDto::new(vec![entry], None).expect("typed page validates");
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(1).expect("nonzero"),
        ResponseResult::ConversationPage(page),
    )));

    let invalid = ConversationEntryDto::Message(Box::new(ConversationMessageDto {
        fact_id: Id32::new([1; 32]),
        message_id: Id32::new([2; 32]),
        thread_id: Id32::new([3; 32]),
        content: "invalid".to_owned(),
        sender_installation: Id32::new([4; 32]),
        sender_mailbox: Id32::new([5; 32]),
        recipient_installation: Some(Id32::new([6; 32])),
        recipient_mailbox: None,
        purpose: MessagePurposeDto::Question,
        presentation: PresentationKindDto::Message,
        correlation_provider: None,
        correlation_session: None,
        correlation_operation: None,
        project_id: None,
        open: true,
        rejected: false,
        state_frontier: Vec::new(),
        peer_received_by: Vec::new(),
        root_fact: None,
        root_message: None,
        ready_answer: false,
        thread_cancelled: false,
    }));
    assert_eq!(
        ConversationPageDto::new(vec![invalid], None),
        Err(ValueError::InvalidValueCombination)
    );
}

#[test]
fn conversation_activity_status_preserves_typed_failure_reason() {
    let entry = ConversationEntryDto::Activity(Box::new(ConversationActivityDto {
        fact_id: Id32::new([1; 32]),
        activity_kind: ConversationActivityKindDto::AgentTurn,
        sequence: 1,
        source_installation: Id32::new([3; 32]),
        source_mailbox: Id32::new([4; 32]),
        provider: "provider".to_owned(),
        session: "session".to_owned(),
        operation: Id32::new([2; 32]),
        item: None,
        logical_key: "operation".to_owned(),
        runtime: "runtime".to_owned(),
        occurred_at_unix_ms: 1,
        status: ActivityStatusDto::Failed {
            reason: "provider_unavailable".to_owned(),
        },
        content: "operation failed".to_owned(),
        truncated: false,
        completed: None,
    }));
    let page = ConversationPageDto::new(vec![entry], None).expect("typed activity validates");
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(1).expect("nonzero"),
        ResponseResult::ConversationPage(page),
    )));

    let invalid = ConversationEntryDto::Activity(Box::new(ConversationActivityDto {
        fact_id: Id32::new([1; 32]),
        activity_kind: ConversationActivityKindDto::AgentTurn,
        sequence: 1,
        source_installation: Id32::new([3; 32]),
        source_mailbox: Id32::new([4; 32]),
        provider: "provider".to_owned(),
        session: "session".to_owned(),
        operation: Id32::new([2; 32]),
        item: None,
        logical_key: "operation".to_owned(),
        runtime: "runtime".to_owned(),
        occurred_at_unix_ms: 1,
        status: ActivityStatusDto::Failed {
            reason: "x".repeat(1_000),
        },
        content: "operation failed".to_owned(),
        truncated: false,
        completed: None,
    }));
    assert!(ConversationPageDto::new(vec![invalid], None).is_err());
}

#[test]
fn conversation_completed_command_round_trips_separate_multiline_fields() {
    let entry = ConversationEntryDto::Activity(Box::new(ConversationActivityDto {
        fact_id: Id32::new([1; 32]),
        activity_kind: ConversationActivityKindDto::CompletedItem,
        sequence: 2,
        source_installation: Id32::new([3; 32]),
        source_mailbox: Id32::new([4; 32]),
        provider: "provider".to_owned(),
        session: "session".to_owned(),
        operation: Id32::new([2; 32]),
        item: Some("command".to_owned()),
        logical_key: "command".to_owned(),
        runtime: "codex".to_owned(),
        occurred_at_unix_ms: 42,
        status: ActivityStatusDto::Failed {
            reason: "command_failed".to_owned(),
        },
        content: "full bounded detail".to_owned(),
        truncated: false,
        completed: Some(CompletedItemPresentationDto::Command {
            command: "printf one\nprintf two".to_owned(),
            output: Some("one\ntwo\nthree\nfour".to_owned()),
            exit_code: Some(17),
            command_truncated: false,
            output_truncated: true,
        }),
    }));
    let page = ConversationPageDto::new(vec![entry], None).expect("completed command validates");
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(1).expect("nonzero"),
        ResponseResult::ConversationPage(page),
    )));
}

#[test]
fn incomplete_addressed_message_round_trips_as_inert_snapshot_diagnostic() {
    let snapshot = AuthoritativeSnapshotDto::new(
        7,
        vec![
            SnapshotItem::IncompleteMessage {
                fact_id: Id32::new([1; 32]),
                message_id: Id32::new([2; 32]),
                thread_id: Id32::new([3; 32]),
                sender_installation: Id32::new([4; 32]),
                sender_mailbox: Id32::new([5; 32]),
                recipient_installation: Some(Id32::new([6; 32])),
                recipient_mailbox: Some(Id32::new([7; 32])),
                content: "history missing".to_owned(),
                purpose: MessagePurposeDto::Question,
                presentation: PresentationKindDto::Message,
                correlation_provider: None,
                correlation_session: None,
                correlation_operation: None,
                project_id: None,
                missing_dependencies: vec![Id32::new([8; 32])],
                unusable_dependencies: Vec::new(),
            },
            SnapshotItem::IncompleteMessagesTruncated,
        ],
    )
    .expect("diagnostic snapshot validates");
    round_trip(&WireMessage::Response(ResponseEnvelope::success(
        RequestId::new(1).expect("nonzero"),
        ResponseResult::AuthoritativeSnapshot(snapshot),
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
            context: ConversationContextDto::Direct {
                participant: ConversationParticipantDto {
                    agent: Some(id(12)),
                    installation: Some(id(1)),
                    mailbox: Some(id(2)),
                    name: Some("helper".to_owned()),
                },
            },
            local_human: MailboxAddressDto {
                installation_id: id(1),
                mailbox_id: id(13),
            },
            root_message: None,
            preview: Some("Can we ship?".to_owned()),
            latest_fact: Some(id(11)),
            open_messages: 1,
            archived_messages: 0,
            sent_messages: 1,
        },
        SnapshotItem::Agent {
            agent_id: id(12),
            claims: vec![id(35)],
            names: vec!["helper".to_owned()],
            mailboxes: vec![MailboxAddressDto {
                installation_id: id(1),
                mailbox_id: id(13),
            }],
            retirements: vec![],
            lifecycle: "active".to_owned(),
            runnable: true,
        },
        SnapshotItem::AgentSession {
            provider: "fake".to_owned(),
            session: "session-1".to_owned(),
            bindings: vec![AgentSessionBindingDto {
                fact_id: id(36),
                mailbox: MailboxAddressDto {
                    installation_id: id(1),
                    mailbox_id: id(13),
                },
            }],
            mailbox_installation: Some(id(1)),
            mailbox_id: Some(id(13)),
            conflicted: false,
        },
        SnapshotItem::AgentSelection {
            agent_id: id(12),
            candidates: vec![AgentSelectionCandidateDto {
                fact_id: id(37),
                provider: "fake".to_owned(),
                session: "session-1".to_owned(),
            }],
            provider: Some("fake".to_owned()),
            session: Some("session-1".to_owned()),
            frontier: vec![id(37)],
            conflicted: false,
        },
        SnapshotItem::AgentSessionName {
            agent_id: id(12),
            provider: "fake".to_owned(),
            session: "session-1".to_owned(),
            candidates: vec![AgentSessionNameCandidateDto {
                fact_id: id(38),
                display_name: Some("work".to_owned()),
            }],
            frontier: vec![id(38)],
            resolved: true,
            display_name: Some("work".to_owned()),
        },
        SnapshotItem::Project {
            project_id: id(14),
            home: id(1),
            account_id: id(12),
            mailbox_id: id(13),
            name: "rewrite".to_owned(),
            lifecycle: "open".to_owned(),
            archived: false,
            claimable: true,
            head: id(15),
            input_sequence: 1,
        },
        SnapshotItem::ProjectAssignment {
            project_id: id(14),
            assignment_id: id(39),
            agent_id: id(12),
            provider: "fake".to_owned(),
            session: Some("session-1".to_owned()),
            phase: "runnable".to_owned(),
            thread_id: Some(id(40)),
            launch_directory: Some(
                ResourceLocatorDto::new(
                    ResourceSchemeDto::WorkingTree,
                    "/workspace/rewrite".to_owned(),
                )
                .expect("launch directory validates"),
            ),
            blocked: None,
            cardinality_conflicted: true,
            runnable: false,
            support: vec![id(39)],
        },
        SnapshotItem::ProjectThread {
            project_id: id(14),
            agent_id: id(12),
            provider: "fake".to_owned(),
            session: "session-1".to_owned(),
            thread_id: id(40),
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
            thread_id: id(15),
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
            expected_head: Some(id(25)),
            operation_provider: "hq".to_owned(),
            operation_session: "project-control-v1".to_owned(),
            operation_id: id(26),
            body: "hq-project-action-v1:open".to_owned(),
            issued_at_unix_millis: 1_700_000_000_123,
            request_fact: id(27),
            progress: Box::new(RemoteCommandProgressDto::Received {
                receipt_fact: id(28),
                received_head: Some(id(25)),
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
fn project_assignment_snapshot_rejects_inconsistent_phase_fields() {
    let id = |byte| Id32::new([byte; 32]);
    let assignment = SnapshotItem::ProjectAssignment {
        project_id: id(1),
        assignment_id: id(2),
        agent_id: id(3),
        provider: "fake".to_owned(),
        session: None,
        phase: "configuring".to_owned(),
        thread_id: Some(id(4)),
        launch_directory: None,
        blocked: None,
        cardinality_conflicted: false,
        runnable: false,
        support: vec![id(2)],
    };
    assert_eq!(
        AuthoritativeSnapshotDto::new(1, vec![assignment]),
        Err(ValueError::InvalidValueCombination)
    );
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
