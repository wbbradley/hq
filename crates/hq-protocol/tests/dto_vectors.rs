//! Public contracts for strict full-content DTO verification.

#![allow(clippy::expect_used, clippy::panic)]

use hq_protocol::{Bip340Signer, DispatchOutcome, ProtocolNamespace};

mod support;

use support::{A, B, C, CANONICAL_CONTENT, CONTROL_CONTENT, valid_bodies};

#[test]
fn published_contents_advance_to_owned_verified_dtos_and_reencode_exactly() {
    for (content, created_at, namespace, family) in [
        (CANONICAL_CONTENT, 0, ProtocolNamespace::Canonical, 1),
        (CONTROL_CONTENT, 1, ProtocolNamespace::Control, 46),
    ] {
        let verified = signer()
            .sign(created_at, content.as_bytes(), [11; 32])
            .expect("published DTO content signs");
        let DispatchOutcome::Supported(prefix) =
            verified.dispatch().expect("published prefix dispatches")
        else {
            panic!("published content must be supported");
        };
        let record = prefix.decode_v1().expect("complete DTO verifies");
        assert_eq!(record.namespace(), namespace);
        assert_eq!(record.family(), family);
        assert_eq!(record.content_bytes(), content.as_bytes());
        assert_eq!(record.encode_content().expect("DTO reencodes"), content);
    }
}

#[test]
fn every_normative_family_body_has_one_executable_exact_dto() {
    for (family, body) in valid_bodies() {
        let namespace = if family <= 45 || family == 49 {
            ProtocolNamespace::Canonical
        } else {
            ProtocolNamespace::Control
        };
        let scope = match family {
            7..=9 => format!(r#"["peer","{A}","{B}"]"#),
            12..=14 | 27..=45 | 49 => format!(r#"["account","{C}"]"#),
            46..=48 => format!(r#"["control","{C}","{A}"]"#),
            _ => format!(r#"["local","{A}"]"#),
        };
        let protocol = match namespace {
            ProtocolNamespace::Canonical => "hq/canonical",
            ProtocolNamespace::Control => "hq/control",
        };
        let content = format!(
            r#"{{"p":"{protocol}","v":1,"f":{family},"author":"{A}","time":0,"scope":{scope},"parents":[],"auth":[],"body":{body}}}"#
        );
        let verified = signer()
            .sign(
                0,
                content.as_bytes(),
                [u8::try_from(family).expect("catalog family fits u8"); 32],
            )
            .expect("catalog DTO signs");
        let DispatchOutcome::Supported(prefix) = verified.dispatch().expect("prefix dispatches")
        else {
            panic!("known catalog family must be supported");
        };
        let record = prefix.decode_v1().expect("catalog DTO verifies");
        assert_eq!(record.family(), family);
        assert_eq!(record.namespace(), namespace);
        assert_eq!(record.encode_content().expect("DTO reencodes"), content);
    }
}

fn signer() -> Bip340Signer {
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    Bip340Signer::from_secret_bytes(secret).expect("fixture key is valid")
}
