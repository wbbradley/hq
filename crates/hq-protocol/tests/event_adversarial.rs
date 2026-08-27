//! Boundary and adversarial corpus for event framing, verification, and prefix dispatch.

#![allow(clippy::expect_used)]

use hq_protocol::{Bip340Signer, FailureClass, MAX_CONTENT_BYTES, MAX_EVENT_BYTES, RawEventBytes};
use sha2::{Digest, Sha256};

const AUTHOR: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PUBLIC_KEY: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const CONTENT: &str = r#"{"p":"hq/canonical","v":1,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[],"auth":[],"body":null}"#;

#[test]
fn content_syntax_limits_and_prefix_policy_fail_before_supported_content() {
    let signer = fixture_signer();
    let mut deep = "null".to_owned();
    for _ in 0..17 {
        deep = format!("[{deep}]");
    }
    let too_deep = CONTENT.replacen("null}", &format!("{deep}}}"), 1);
    let too_many = CONTENT.replacen(
        "null}",
        &format!(
            "[{}]}}",
            std::iter::repeat_n("null", 65)
                .collect::<Vec<_>>()
                .join(",")
        ),
        1,
    );
    let cases = [
        (
            CONTENT.replacen("hq/canonical", "hq\\/canonical", 1),
            FailureClass::ContentNonCanonical,
        ),
        (
            CONTENT.replacen(
                "{\"p\":\"hq/canonical\",",
                "{\"p\":\"hq/canonical\",\"p\":\"hq/canonical\",",
                1,
            ),
            FailureClass::ContentMalformed,
        ),
        (
            CONTENT.replacen("{\"p\":", "{\"unknown\":null,\"p\":", 1),
            FailureClass::ContentMalformed,
        ),
        (
            CONTENT.replacen(
                "{\"p\":\"hq/canonical\",\"v\":1,",
                "{\"v\":1,\"p\":\"hq/canonical\",",
                1,
            ),
            FailureClass::ContentMalformed,
        ),
        (
            CONTENT.replacen("\"body\":null", "\"body\":1.0", 1),
            FailureClass::ContentMalformed,
        ),
        (
            CONTENT.replacen("\"body\":null", "\"body\":-1", 1),
            FailureClass::ContentMalformed,
        ),
        (format!("{CONTENT}x"), FailureClass::ContentMalformed),
        (too_deep, FailureClass::ContentTooDeep),
        (too_many, FailureClass::ContentTooManyItems),
        (
            CONTENT.replacen(AUTHOR, &"A".repeat(64), 1),
            FailureClass::ContentNonCanonical,
        ),
    ];

    for (content, expected) in cases {
        let verified = signer
            .sign(0, content.as_bytes(), [9; 32])
            .expect("bounded bytes can be signed before content dispatch");
        assert_eq!(
            verified
                .dispatch()
                .expect_err("adversarial content must not become supported")
                .class(),
            expected,
            "content = {content}"
        );
    }
}

#[test]
fn authored_milliseconds_must_agree_with_verified_outer_seconds() {
    let content = CONTENT.replacen("\"time\":0", "\"time\":1000", 1);
    let verified = fixture_signer()
        .sign(0, content.as_bytes(), [3; 32])
        .expect("content is signed at an intentionally wrong outer second");
    assert_eq!(
        verified
            .dispatch()
            .expect_err("time disagreement must stop dispatch")
            .class(),
        FailureClass::AuthoredTimeMismatch
    );
}

#[test]
fn noncanonical_outer_strings_hex_and_crypto_encodings_are_rejected() {
    let signed = fixture_signer()
        .sign(0, CONTENT.as_bytes(), [4; 32])
        .expect("fixture signs");
    let wire = std::str::from_utf8(signed.exact_event_bytes()).expect("local event is UTF-8");
    let cases = [
        (
            wire.replacen("hq/canonical", "hq\\/canonical", 1),
            FailureClass::OuterNonCanonical,
        ),
        (
            wire.replacen(
                &hex(&signed.event_id()),
                &hex(&signed.event_id()).to_uppercase(),
                1,
            ),
            FailureClass::OuterFieldShape,
        ),
        (
            wire.replacen("\"created_at\":0", "\"created_at\":00", 1),
            FailureClass::OuterNonCanonical,
        ),
        (
            wire.replacen("\"created_at\":0", "\"created_at\":-1", 1),
            FailureClass::OuterFieldShape,
        ),
    ];
    for (wire, expected) in cases {
        assert_eq!(
            RawEventBytes::new(wire.into_bytes())
                .expect("bounded")
                .parse()
                .expect_err("outer event must be rejected")
                .class(),
            expected
        );
    }

    let invalid_key = outer_event(
        "fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc30",
        CONTENT,
        &"0".repeat(128),
    );
    assert_eq!(
        RawEventBytes::new(invalid_key.into_bytes())
            .expect("bounded")
            .parse()
            .expect("shape is valid")
            .verify()
            .expect_err("invalid x coordinate")
            .class(),
        FailureClass::InvalidPublicKey
    );

    let invalid_signature = outer_event(PUBLIC_KEY, CONTENT, &"f".repeat(128));
    assert_eq!(
        RawEventBytes::new(invalid_signature.into_bytes())
            .expect("bounded")
            .parse()
            .expect("shape is valid")
            .verify()
            .expect_err("signature scalars are out of range")
            .class(),
        FailureClass::InvalidSignatureEncoding
    );
}

#[test]
fn raw_and_decoded_limits_are_inclusive_and_checked_before_unbounded_work() {
    assert!(RawEventBytes::new(vec![b'a'; MAX_EVENT_BYTES]).is_ok());
    assert_eq!(
        RawEventBytes::new(vec![b'a'; MAX_EVENT_BYTES + 1])
            .expect_err("one byte over raw limit")
            .class(),
        FailureClass::EventTooLarge
    );

    let signer = fixture_signer();
    let exact = vec![b'a'; MAX_CONTENT_BYTES];
    assert_eq!(
        signer
            .sign(0, &exact, [5; 32])
            .expect("exact decoded limit signs")
            .content_bytes()
            .len(),
        MAX_CONTENT_BYTES
    );
    assert_eq!(
        signer
            .sign(0, &vec![b'a'; MAX_CONTENT_BYTES + 1], [5; 32])
            .expect_err("one byte over decoded content limit")
            .class(),
        FailureClass::ContentTooLarge
    );
}

fn fixture_signer() -> Bip340Signer {
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    Bip340Signer::from_secret_bytes(secret).expect("fixture key is valid")
}

fn outer_event(public_key: &str, content: &str, signature: &str) -> String {
    let escaped = content.replace('\\', "\\\\").replace('\"', "\\\"");
    let preimage = format!("[0,\"{public_key}\",0,6000,[],\"{escaped}\"]");
    let id = hex(&Sha256::digest(preimage.as_bytes()));
    format!(
        "{{\"id\":\"{id}\",\"pubkey\":\"{public_key}\",\"created_at\":0,\"kind\":6000,\"tags\":[],\"content\":\"{escaped}\",\"sig\":\"{signature}\"}}"
    )
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}
