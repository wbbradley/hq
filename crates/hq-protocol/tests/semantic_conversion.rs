//! Public contracts for converting complete verified DTOs into domain facts.

#![allow(clippy::expect_used, clippy::panic)]

use hq_domain::{
    ActivityKind, ActivityStatus, AgentId, AssignmentBinding, AssignmentId, AuthorityRole,
    DispatchId, FactId, FactKind, InstallationId, MessagePurpose, ProjectActivityAttribution,
    ProjectId, ProviderId, ProviderSessionId, RemoteCommandResult, ResourceHealth,
    RuntimeObservation, SemanticPayload, SigningPublicKey, ThreadId,
};
use hq_protocol::{
    Bip340Signer, CanonicalEventPlan, DispatchOutcome, FailureClass, ProtocolNamespace,
};

mod support;

use support::{A, B, C, CANONICAL_CONTENT, CONTROL_CONTENT, D, KEY, valid_bodies};

#[test]
fn every_verified_v1_dto_converts_to_its_exact_semantic_family() {
    for (family, body) in valid_bodies() {
        let record = verified_record(family, &body);
        let event_id = record.verified_event().event_id();
        let content = record.content_bytes().to_vec();
        let semantic = record
            .into_semantic_fact()
            .expect("valid catalog DTO converts to a semantic fact");

        let index = usize::try_from(family - 1).expect("catalog index fits usize");
        assert_eq!(semantic.fact().kind(), FactKind::ALL[index]);
        assert_eq!(semantic.fact().id().as_bytes(), &event_id);
        assert_eq!(
            semantic.fact().author().installation_id(),
            InstallationId::from_bytes([0x11; 32])
        );
        assert_eq!(
            semantic.fact().author().signing_key(),
            SigningPublicKey::from_bytes(hex(KEY))
        );
        assert_eq!(semantic.content_bytes(), content);
        assert_eq!(semantic.verified_event().event_id(), event_id);
    }
}

#[test]
fn published_vectors_reach_semantics_with_exact_signed_time_and_evidence() {
    for (content, created_at, expected_family, expected_millis) in [
        (CANONICAL_CONTENT, 0, 1, 0),
        (CONTROL_CONTENT, 1, 46, 1_000),
    ] {
        let record = record_from_content(content, created_at).expect("published vector verifies");
        let exact_event = record.verified_event().exact_event_bytes().to_vec();
        let semantic = record
            .into_semantic_fact()
            .expect("published vector converts");

        assert_eq!(semantic.fact().kind(), FactKind::ALL[expected_family - 1]);
        assert_eq!(
            semantic.fact().authored_at().as_unix_millis(),
            expected_millis
        );
        assert_eq!(semantic.content_bytes(), content.as_bytes());
        assert_eq!(semantic.verified_event().exact_event_bytes(), exact_event);
    }
}

#[test]
fn conversion_preserves_deep_root_and_remote_command_semantics() {
    let declared = verified_record(1, &valid_bodies()[0].1)
        .into_semantic_fact()
        .expect("installation root converts");
    let SemanticPayload::InstallationDeclared {
        installation_id,
        signing_key,
        label,
        ..
    } = declared.fact().payload()
    else {
        panic!("family one must remain an installation declaration");
    };
    assert_eq!(*installation_id, InstallationId::from_bytes([0x11; 32]));
    assert_eq!(*signing_key, SigningPublicKey::from_bytes(hex(KEY)));
    assert!(label.is_none());

    let requested = verified_record(46, &valid_bodies()[45].1)
        .into_semantic_fact()
        .expect("remote command converts");
    let SemanticPayload::RemoteProjectCommandRequested {
        target_home, body, ..
    } = requested.fact().payload()
    else {
        panic!("family 46 must remain a remote command request");
    };
    assert_eq!(*target_home, InstallationId::from_bytes([0x11; 32]));
    assert_eq!(body.as_str(), "open");
}

#[test]
fn conversion_preserves_nested_activity_project_output_and_result_values() {
    let activity = verified_record(22, &valid_bodies()[21].1)
        .into_semantic_fact()
        .expect("activity converts");
    let SemanticPayload::HarnessActivityRecorded {
        correlation,
        kind,
        sequence,
        status,
        content,
        truncated,
        ..
    } = activity.fact().payload()
    else {
        panic!("family 22 must remain activity");
    };
    assert_eq!(correlation.provider().as_str(), "provider");
    assert_eq!(*kind, ActivityKind::Progress);
    assert_eq!(sequence.get(), 1);
    assert_eq!(*status, ActivityStatus::Running);
    assert_eq!(content.as_str(), "content");
    assert!(!truncated);

    let project = verified_record(27, &valid_bodies()[26].1)
        .into_semantic_fact()
        .expect("project root converts");
    let SemanticPayload::ProjectCreated {
        resources, primary, ..
    } = project.fact().payload()
    else {
        panic!("family 27 must remain project creation");
    };
    assert_eq!(resources.as_slice().len(), 1);
    assert_eq!(resources.as_slice()[0].health, ResourceHealth::Unknown);
    assert_eq!(Some(resources.as_slice()[0].resource_id), *primary);

    let output = verified_record(45, &valid_bodies()[44].1)
        .into_semantic_fact()
        .expect("project output converts");
    let SemanticPayload::ProjectOutputRecorded {
        output_id, message, ..
    } = output.fact().payload()
    else {
        panic!("family 45 must remain project output");
    };
    assert_eq!(message.message_id, *output_id);
    assert_eq!(message.purpose, MessagePurpose::ProjectOutput);
    assert!(message.recipient.is_some());

    let outcome = verified_record(48, &valid_bodies()[47].1)
        .into_semantic_fact()
        .expect("remote outcome converts");
    let SemanticPayload::RemoteProjectCommandOutcome {
        result, runtime, ..
    } = outcome.fact().payload()
    else {
        panic!("family 48 must remain remote outcome");
    };
    assert!(matches!(result, RemoteCommandResult::Committed(_)));
    assert_eq!(*runtime, Some(RuntimeObservation::Succeeded));
}

#[test]
fn activity_project_attribution_is_additive_and_historical_bytes_remain_exact() {
    let historical = verified_record(22, &valid_bodies()[21].1)
        .into_semantic_fact()
        .expect("historical activity converts");
    assert!(matches!(
        historical.fact().payload(),
        SemanticPayload::HarnessActivityRecorded { project: None, .. }
    ));
    assert_eq!(
        CanonicalEventPlan::from_fact(historical.fact())
            .encode_content()
            .expect("historical activity re-encodes"),
        historical.content_bytes()
    );

    let SemanticPayload::HarnessActivityRecorded {
        source,
        correlation,
        item,
        kind,
        logical_key,
        runtime,
        sequence,
        occurred_at,
        status,
        content,
        truncated,
        ..
    } = historical.fact().payload()
    else {
        panic!("family 22 must remain activity");
    };
    let attribution = ProjectActivityAttribution {
        project_id: ProjectId::from_bytes([0x21; 32]),
        dispatch_id: DispatchId::from_bytes([0x22; 32]),
        binding: AssignmentBinding {
            assignment_id: AssignmentId::from_bytes([0x23; 32]),
            agent_id: AgentId::from_bytes([0x24; 32]),
            provider: ProviderId::new("provider").expect("provider"),
            session: ProviderSessionId::new("session").expect("session"),
        },
        thread_id: ThreadId::from_bytes([0x25; 32]),
    };
    let payload = SemanticPayload::HarnessActivityRecorded {
        project: Some(attribution.clone()),
        source: *source,
        correlation: correlation.clone(),
        item: item.clone(),
        kind: *kind,
        logical_key: logical_key.clone(),
        runtime: runtime.clone(),
        sequence: *sequence,
        occurred_at: *occurred_at,
        status: status.clone(),
        content: content.clone(),
        truncated: *truncated,
    };
    let encoded = CanonicalEventPlan::new(
        historical.fact().author().installation_id(),
        historical.fact().authored_at(),
        historical.fact().scope().clone(),
        historical.fact().causal().clone(),
        payload,
    )
    .encode_content()
    .expect("attributed activity encodes");
    let decoded = CanonicalEventPlan::decode_content(&encoded).expect("attribution decodes");
    assert!(matches!(
        decoded.into_parts().4,
        SemanticPayload::HarnessActivityRecorded {
            project: Some(actual),
            ..
        } if actual == attribution
    ));
}

#[test]
fn conversion_preserves_namespaced_parents_and_typed_authorities_without_aliasing() {
    let body = &valid_bodies()[45].1;
    let parents = format!(r#"[["c","{B}"],["c","{C}"],["r","{D}"]]"#);
    let authorities = format!(r#"[["active-human","c","{B}"],["project-home","c","{C}"]]"#);
    let record = verified_record_with(46, body, None, &parents, &authorities)
        .expect("valid namespaced references decode");
    let semantic = record
        .into_semantic_fact()
        .expect("distinct IDs survive namespace erasure");
    let causal = semantic.fact().causal();

    assert_eq!(causal.parents().iter().len(), 3);
    assert_eq!(
        causal.authority(AuthorityRole::ActiveHuman),
        Some(FactId::from_bytes([0x22; 32]))
    );
    assert_eq!(
        causal.authority(AuthorityRole::ProjectHome),
        Some(FactId::from_bytes([0x33; 32]))
    );
}

#[test]
fn semantic_conversion_rejects_intrinsic_subject_scope_and_domain_bound_failures() {
    let peer_self = format!(
        r#"{{"peer":{{"installation":"{A}","signing":"{KEY}"}},"encryption":"{B}","label":null,"relays":[]}}"#
    );
    assert_semantic_failure(5, &peer_self, None, FailureClass::AuthorSubjectMismatch);

    let wrong_mailbox = format!(
        r#"{{"grant":"{C}","mailbox":{{"installation":"{A}","mailbox":"{C}"}},"grantee":{{"installation":"{A}","signing":"{KEY}"}}}}"#
    );
    assert_semantic_failure(7, &wrong_mailbox, None, FailureClass::ScopePayloadMismatch);

    let wrong_creator = format!(
        r#"{{"account":"{C}","creator":{{"installation":"{B}","signing":"{KEY}"}},"label":null}}"#
    );
    assert_semantic_failure(
        10,
        &wrong_creator,
        None,
        FailureClass::AuthorSubjectMismatch,
    );

    let wrong_device = format!(
        r#"{{"account":"{C}","grant":"4444444444444444444444444444444444444444444444444444444444444444","device":{{"installation":"{B}","signing":"{KEY}"}}}}"#
    );
    assert_semantic_failure(13, &wrong_device, None, FailureClass::AuthorSubjectMismatch);

    let long_code = "x".repeat(97);
    let narrow_domain = format!(r#"{{"peer":"{B}","reason":"{long_code}"}}"#);
    assert_semantic_failure(6, &narrow_domain, None, FailureClass::DomainValueInvalid);

    let account_direct = format!(
        r#"{{"id":"{B}","sender":{{"installation":"{A}","mailbox":"{B}"}},"recipient":{{"installation":"{A}","mailbox":"{B}"}},"body":"body","purpose":"question","presentation":"message","correlation":null,"project":null}}"#
    );
    let account_scope = format!(r#"["account","{C}"]"#);
    assert_semantic_failure(
        15,
        &account_direct,
        Some(&account_scope),
        FailureClass::ScopePayloadMismatch,
    );
}

#[test]
fn conversion_rejects_parent_ids_that_collide_only_after_namespace_erasure() {
    let parents = format!(r#"[["c","{B}"],["r","{B}"]]"#);
    let record = verified_record_with(46, &valid_bodies()[45].1, None, &parents, "[]")
        .expect("wire references remain distinct by namespace");
    let error = record
        .into_semantic_fact()
        .expect_err("namespace erasure must not alias parents");
    assert_eq!(error.class(), FailureClass::DomainValueInvalid);
}

fn verified_record(family: u64, body: &str) -> hq_protocol::VerifiedSupportedRecord {
    verified_record_with(family, body, None, "[]", "[]").expect("catalog DTO verifies")
}

fn verified_record_with(
    family: u64,
    body: &str,
    scope_override: Option<&str>,
    parents: &str,
    authorities: &str,
) -> Result<hq_protocol::VerifiedSupportedRecord, hq_protocol::ProtocolError> {
    let namespace = if family <= 45 {
        ProtocolNamespace::Canonical
    } else {
        ProtocolNamespace::Control
    };
    let default_scope = match family {
        7..=9 => format!(r#"["peer","{A}","{B}"]"#),
        12..=14 | 27..=45 => format!(r#"["account","{C}"]"#),
        46..=48 => format!(r#"["control","{C}","{A}"]"#),
        _ => format!(r#"["local","{A}"]"#),
    };
    let scope = scope_override.unwrap_or(&default_scope);
    let protocol = match namespace {
        ProtocolNamespace::Canonical => "hq/canonical",
        ProtocolNamespace::Control => "hq/control",
    };
    let content = format!(
        r#"{{"p":"{protocol}","v":1,"f":{family},"author":"{A}","time":0,"scope":{scope},"parents":{parents},"auth":{authorities},"body":{body}}}"#
    );
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    let signer = Bip340Signer::from_secret_bytes(secret).expect("fixture key is valid");
    let family_byte = u8::try_from(family).expect("catalog family fits u8");
    let verified = signer.sign(0, content.as_bytes(), [family_byte; 32])?;
    let DispatchOutcome::Supported(prefix) = verified.dispatch()? else {
        panic!("known catalog family must be supported");
    };
    prefix.decode_v1()
}

fn record_from_content(
    content: &str,
    created_at: u64,
) -> Result<hq_protocol::VerifiedSupportedRecord, hq_protocol::ProtocolError> {
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    let signer = Bip340Signer::from_secret_bytes(secret)?;
    let verified = signer.sign(created_at, content.as_bytes(), [11; 32])?;
    let DispatchOutcome::Supported(prefix) = verified.dispatch()? else {
        panic!("published family must be supported");
    };
    prefix.decode_v1()
}

fn assert_semantic_failure(family: u64, body: &str, scope: Option<&str>, expected: FailureClass) {
    let record = verified_record_with(family, body, scope, "[]", "[]")
        .expect("adversarial case passes DTO verification");
    let error = record
        .into_semantic_fact()
        .expect_err("intrinsic semantic mismatch is rejected");
    assert_eq!(error.class(), expected);
}

fn hex(value: &str) -> [u8; 32] {
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let text = std::str::from_utf8(pair).expect("fixture hex is utf8");
        output[index] = u8::from_str_radix(text, 16).expect("fixture hex decodes");
    }
    output
}
