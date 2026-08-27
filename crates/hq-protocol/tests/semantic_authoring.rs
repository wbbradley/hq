//! Public contracts for typed semantic event planning and local signing.

#![allow(clippy::expect_used, clippy::panic)]

use hq_domain::FactKind;
use hq_protocol::{Bip340Signer, CanonicalEventPlan, DispatchOutcome, ProtocolNamespace};

#[allow(dead_code)]
mod support;

use support::{A, B, C, valid_bodies};

#[test]
fn every_semantic_family_authors_back_to_the_exact_verified_v1_record() {
    for (family, body) in valid_bodies() {
        let auxiliary_randomness = [u8::try_from(family).expect("family fits u8"); 32];
        let original = verified_record(family, &body, auxiliary_randomness);
        let expected_event = original.verified_event().exact_event_bytes().to_vec();
        let expected_content = original.content_bytes().to_vec();
        let semantic = original.into_semantic_fact().expect("catalog DTO converts");

        let authored = CanonicalEventPlan::from_fact(semantic.fact())
            .sign(&signer(), auxiliary_randomness)
            .expect("typed plan signs and verifies");

        assert_eq!(authored.fact(), semantic.fact());
        let index = usize::try_from(family - 1).expect("family index fits usize");
        assert_eq!(authored.fact().kind(), FactKind::ALL[index]);
        assert_eq!(authored.content_bytes(), expected_content);
        assert_eq!(
            authored.verified_event().exact_event_bytes(),
            expected_event
        );
    }
}

#[test]
fn every_semantic_family_round_trips_through_unsigned_local_planning_content() {
    for (family, body) in valid_bodies() {
        let original = verified_record(
            family,
            &body,
            [u8::try_from(family).expect("family fits u8"); 32],
        );
        let semantic = original.into_semantic_fact().expect("catalog DTO converts");
        let plan = CanonicalEventPlan::from_fact(semantic.fact());
        let content = plan.clone().encode_content().expect("plan encodes");
        assert_eq!(
            CanonicalEventPlan::decode_content(&content)
                .unwrap_or_else(|error| panic!("family {family} plan failed: {error}")),
            plan
        );
    }
}

#[test]
fn typed_authoring_preserves_canonical_and_control_authority_namespaces() {
    let bodies = valid_bodies();
    for (family, body, parents, authorities) in [
        (
            2,
            bodies[1].1.as_str(),
            format!(r#"[["c","{B}"]]"#),
            format!(r#"[["local-installation","c","{B}"]]"#),
        ),
        (
            47,
            bodies[46].1.as_str(),
            format!(r#"[["r","{B}"]]"#),
            format!(r#"[["request","r","{B}"]]"#),
        ),
    ] {
        let auxiliary_randomness = [u8::try_from(family).expect("family fits u8"); 32];
        let original = verified_record_with_references(
            family,
            body,
            auxiliary_randomness,
            &parents,
            &authorities,
        );
        let expected = original.verified_event().exact_event_bytes().to_vec();
        let semantic = original.into_semantic_fact().expect("record converts");
        let authored = CanonicalEventPlan::from_fact(semantic.fact())
            .sign(&signer(), auxiliary_randomness)
            .expect("typed plan signs");
        assert_eq!(authored.verified_event().exact_event_bytes(), expected);
    }
}

fn verified_record(
    family: u64,
    body: &str,
    auxiliary_randomness: [u8; 32],
) -> hq_protocol::VerifiedSupportedRecord {
    verified_record_with_references(family, body, auxiliary_randomness, "[]", "[]")
}

fn verified_record_with_references(
    family: u64,
    body: &str,
    auxiliary_randomness: [u8; 32],
    parents: &str,
    authorities: &str,
) -> hq_protocol::VerifiedSupportedRecord {
    let namespace = if family <= 45 {
        ProtocolNamespace::Canonical
    } else {
        ProtocolNamespace::Control
    };
    let scope = match family {
        7..=9 => format!(r#"["peer","{A}","{B}"]"#),
        12..=14 | 27..=45 => format!(r#"["account","{C}"]"#),
        46..=48 => format!(r#"["control","{C}","{A}"]"#),
        _ => format!(r#"["local","{A}"]"#),
    };
    let protocol = match namespace {
        ProtocolNamespace::Canonical => "hq/canonical",
        ProtocolNamespace::Control => "hq/control",
    };
    let content = format!(
        r#"{{"p":"{protocol}","v":1,"f":{family},"author":"{A}","time":0,"scope":{scope},"parents":{parents},"auth":{authorities},"body":{body}}}"#
    );
    let verified = signer()
        .sign(0, content.as_bytes(), auxiliary_randomness)
        .expect("catalog DTO signs");
    let DispatchOutcome::Supported(prefix) = verified.dispatch().expect("prefix dispatches") else {
        panic!("known family must be supported");
    };
    prefix.decode_v1().expect("catalog DTO verifies")
}

fn signer() -> Bip340Signer {
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    Bip340Signer::from_secret_bytes(secret).expect("fixture key is valid")
}
