//! Adversarial full-content DTO verification tests.

#![allow(clippy::expect_used, clippy::panic)]

use hq_protocol::{Bip340Signer, DispatchOutcome, FailureClass, ProtocolError};

const A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const B: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const C: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const KEY: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const BASE: &str = r#"{"p":"hq/canonical","v":1,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[],"auth":[],"body":{"installation":"1111111111111111111111111111111111111111111111111111111111111111","signing":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","encryption":"2222222222222222222222222222222222222222222222222222222222222222","label":"alpha"}}"#;

#[test]
fn complete_envelope_and_body_shape_must_be_exact() {
    let cases = [
        (
            BASE.replacen("\"scope\":", "\"parents\":[],\"scope\":", 1)
                .replacen("\"parents\":[],\"auth\"", "\"auth\"", 1),
            FailureClass::ContentNonCanonical,
        ),
        (
            BASE.replacen("\"body\":", "\"extension\":null,\"body\":", 1),
            FailureClass::ContentMalformed,
        ),
        (
            BASE.replacen("\"label\":\"alpha\"", "\"label\":\"alpha\",\"label\":\"alpha\"", 1),
            FailureClass::ContentMalformed,
        ),
        (
            BASE.replacen("\"label\":\"alpha\"", "\"label\":\"alpha\",\"extension\":null", 1),
            FailureClass::ContentMalformed,
        ),
        (
            BASE.replacen(
                &format!(r#""encryption":"{B}","label":"alpha""#),
                &format!(r#""label":"alpha","encryption":"{B}""#),
                1,
            ),
            FailureClass::ContentNonCanonical,
        ),
        (
            BASE.replacen(",\"label\":\"alpha\"", "", 1),
            FailureClass::ContentMalformed,
        ),
        (
            BASE.replacen(&format!(r#""signing":"{KEY}""#), r#""signing":"bad""#, 1),
            FailureClass::ContentMalformed,
        ),
        (
            BASE.replacen(&format!(r#""encryption":"{B}""#), r#""encryption":"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF""#, 1),
            FailureClass::ContentMalformed,
        ),
        (
            BASE.replacen(
                &format!(r#""installation":"{A}","signing":"{KEY}""#),
                &format!(r#""mailbox":"{B}","kind":"agent""#),
                1,
            ),
            FailureClass::ContentMalformed,
        ),
    ];
    for (content, expected) in cases {
        assert_eq!(dto_error(&content).class(), expected, "{content}");
    }
}

#[test]
fn decoded_values_collections_and_intrinsic_agreement_are_bounded() {
    let long = "x".repeat(129);
    let too_many_relays = std::iter::repeat_n(r#"{"scheme":"opaque","value":"relay"}"#, 9)
        .collect::<Vec<_>>()
        .join(",");
    let cases = [
        (
            BASE.replacen("\"label\":\"alpha\"", &format!(r#""label":"{long}""#), 1),
            FailureClass::ContentMalformed,
        ),
        (
            envelope(
                2,
                &format!(r#"{{"mailbox":"{B}","kind":"unknown","label":null}}"#),
                &format!(r#"["local","{A}"]"#),
                "[]",
                "[]",
            ),
            FailureClass::ContentMalformed,
        ),
        (
            envelope(
                22,
                &format!(
                    r#"{{"source":{{"installation":"{A}","mailbox":"{B}"}},"operation":{{"provider":"p","session":"s","id":"{C}"}},"item":null,"kind":"progress","logical_key":"key","runtime":"runtime","sequence":0,"occurred_at":0,"status":{{"state":"running"}},"content":"content","truncated":false}}"#
                ),
                &format!(r#"["local","{A}"]"#),
                "[]",
                "[]",
            ),
            FailureClass::ContentMalformed,
        ),
        (
            envelope(
                22,
                &format!(
                    r#"{{"source":{{"installation":"{A}","mailbox":"{B}"}},"operation":{{"provider":"p","session":"s","id":"{C}"}},"item":null,"kind":"progress","logical_key":"key","runtime":"runtime","sequence":1,"occurred_at":9223372036854775808,"status":{{"state":"running"}},"content":"content","truncated":false}}"#
                ),
                &format!(r#"["local","{A}"]"#),
                "[]",
                "[]",
            ),
            FailureClass::ContentMalformed,
        ),
        (
            envelope(
                5,
                &format!(
                    r#"{{"peer":{{"installation":"{B}","signing":"{KEY}"}},"encryption":"{C}","label":null,"relays":[{too_many_relays}]}}"#
                ),
                &format!(r#"["local","{A}"]"#),
                "[]",
                "[]",
            ),
            FailureClass::ContentMalformed,
        ),
        (
            envelope(
                5,
                &format!(
                    r#"{{"peer":{{"installation":"{B}","signing":"{KEY}"}},"encryption":"{C}","label":null,"relays":[{{"scheme":"opaque","value":"relay"}},{{"scheme":"opaque","value":"relay"}}]}}"#
                ),
                &format!(r#"["local","{A}"]"#),
                "[]",
                "[]",
            ),
            FailureClass::ContentMalformed,
        ),
        (
            BASE.replacen(
                &format!(r#"["local","{A}"]"#),
                &format!(r#"["local","{B}"]"#),
                1,
            ),
            FailureClass::ScopeAuthorMismatch,
        ),
        (
            envelope(
                2,
                &format!(r#"{{"mailbox":"{B}","kind":"agent","label":null}}"#),
                &format!(r#"["peer","{A}","{B}"]"#),
                "[]",
                "[]",
            ),
            FailureClass::NamespaceConfusion,
        ),
        (
            BASE.replacen(
                &format!(r#""installation":"{A}""#),
                &format!(r#""installation":"{B}""#),
                1,
            ),
            FailureClass::ScopeAuthorMismatch,
        ),
    ];
    for (content, expected) in cases {
        assert_eq!(dto_error(&content).class(), expected, "{content}");
    }
}

#[test]
fn named_decoded_text_bounds_are_inclusive() {
    for (maximum, field) in [(64, "provider"), (256, "session"), (16_384, "content")] {
        let exact = "x".repeat(maximum);
        let over = "x".repeat(maximum + 1);
        assert!(
            verify_dto(&activity_content(field, &exact)).is_ok(),
            "{field}"
        );
        assert_eq!(
            dto_error(&activity_content(field, &over)).class(),
            FailureClass::ContentMalformed,
            "{field}"
        );
    }

    let exact = "x".repeat(4_096);
    let over = "x".repeat(4_097);
    assert!(verify_dto(&context_content(&exact)).is_ok());
    assert_eq!(
        dto_error(&context_content(&over)).class(),
        FailureClass::ContentMalformed
    );
}

#[test]
fn typed_references_require_namespace_order_role_uniqueness_and_parent_membership() {
    let parent_a = format!(r#"["c","{A}"]"#);
    let parent_b = format!(r#"["c","{B}"]"#);
    let cases = [
        (
            envelope(
                2,
                &format!(r#"{{"mailbox":"{B}","kind":"agent","label":null}}"#),
                &format!(r#"["local","{A}"]"#),
                &format!("[{parent_b},{parent_a}]"),
                "[]",
            ),
            FailureClass::ContentNonCanonical,
        ),
        (
            envelope(
                2,
                &format!(r#"{{"mailbox":"{B}","kind":"agent","label":null}}"#),
                &format!(r#"["local","{A}"]"#),
                &format!(r#"[["r","{A}"]]"#),
                "[]",
            ),
            FailureClass::NamespaceConfusion,
        ),
        (
            envelope(
                2,
                &format!(r#"{{"mailbox":"{B}","kind":"agent","label":null}}"#),
                &format!(r#"["local","{A}"]"#),
                "[]",
                &format!(r#"[["local-installation","c","{A}"]]"#),
            ),
            FailureClass::AuthorityNotParent,
        ),
        (
            envelope(
                11,
                &format!(r#"{{"account":"{C}"}}"#),
                &format!(r#"["local","{A}"]"#),
                &format!("[{parent_a},{parent_b}]"),
                &format!(r#"[["local-installation","c","{A}"],["local-installation","c","{B}"]]"#),
            ),
            FailureClass::DuplicateAuthorityRole,
        ),
        (
            envelope(
                2,
                &format!(r#"{{"mailbox":"{B}","kind":"agent","label":null}}"#),
                &format!(r#"["local","{A}"]"#),
                &format!("[{parent_a}]"),
                &format!(r#"[["project-home","c","{A}"]]"#),
            ),
            FailureClass::ContentMalformed,
        ),
    ];
    for (content, expected) in cases {
        assert_eq!(dto_error(&content).class(), expected, "{content}");
    }
}

fn dto_error(content: &str) -> ProtocolError {
    verify_dto(content).expect_err("adversarial DTO must not verify")
}

fn verify_dto(content: &str) -> Result<(), ProtocolError> {
    let verified = signer()
        .sign(0, content.as_bytes(), [17; 32])
        .expect("bounded adversarial DTO signs");
    match verified.dispatch() {
        Err(error) => Err(error),
        Ok(DispatchOutcome::Supported(prefix)) => prefix.decode_v1().map(|_| ()),
        Ok(DispatchOutcome::Unsupported(_)) => panic!("catalog family unexpectedly unsupported"),
    }
}

fn activity_content(field: &str, value: &str) -> String {
    let provider = if field == "provider" {
        value
    } else {
        "provider"
    };
    let session = if field == "session" { value } else { "session" };
    let content = if field == "content" { value } else { "content" };
    envelope(
        22,
        &format!(
            r#"{{"source":{{"installation":"{A}","mailbox":"{B}"}},"operation":{{"provider":"{provider}","session":"{session}","id":"{C}"}},"item":null,"kind":"progress","logical_key":"key","runtime":"runtime","sequence":1,"occurred_at":0,"status":{{"state":"running"}},"content":"{content}","truncated":false}}"#
        ),
        &format!(r#"["local","{A}"]"#),
        "[]",
        "[]",
    )
}

fn context_content(locator: &str) -> String {
    envelope(
        4,
        &format!(
            r#"{{"mailbox":"{B}","context":{{"directory":{{"scheme":"opaque","value":"{locator}"}},"repository":null,"worktree":null,"branch":null}}}}"#
        ),
        &format!(r#"["local","{A}"]"#),
        "[]",
        "[]",
    )
}

fn envelope(family: u64, body: &str, scope: &str, parents: &str, auth: &str) -> String {
    format!(
        r#"{{"p":"hq/canonical","v":1,"f":{family},"author":"{A}","time":0,"scope":{scope},"parents":{parents},"auth":{auth},"body":{body}}}"#
    )
}

fn signer() -> Bip340Signer {
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    Bip340Signer::from_secret_bytes(secret).expect("fixture key is valid")
}
