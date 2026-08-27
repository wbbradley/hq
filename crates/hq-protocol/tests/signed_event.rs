//! Public trust-boundary tests for signed HQ event carriage.

#![allow(clippy::expect_used, clippy::panic)]

use hq_protocol::{
    Bip340Signer, DispatchOutcome, FailureClass, MAX_EVENT_BYTES, ProtocolNamespace, RawEventBytes,
    UnsupportedReason, verify_bip340,
};

const CANONICAL_CONTENT: &str = r#"{"p":"hq/canonical","v":1,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[],"auth":[],"body":{"installation":"1111111111111111111111111111111111111111111111111111111111111111","signing":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","encryption":"2222222222222222222222222222222222222222222222222222222222222222","label":"alpha"}}"#;
const CANONICAL_EVENT: &str = r#"{"id":"4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022","pubkey":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","created_at":0,"kind":6000,"tags":[],"content":"{\"p\":\"hq/canonical\",\"v\":1,\"f\":1,\"author\":\"1111111111111111111111111111111111111111111111111111111111111111\",\"time\":0,\"scope\":[\"local\",\"1111111111111111111111111111111111111111111111111111111111111111\"],\"parents\":[],\"auth\":[],\"body\":{\"installation\":\"1111111111111111111111111111111111111111111111111111111111111111\",\"signing\":\"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"encryption\":\"2222222222222222222222222222222222222222222222222222222222222222\",\"label\":\"alpha\"}}","sig":"70ca60d3fcd430426d63b160b585ada5f5a13bdced5140382ee9ad8f98588774b596e8a2d6ede96cee57758b81139a16a783d4ae5a47bd666a6136b99da3cce3"}"#;
const CONTROL_CONTENT: &str = r#"{"p":"hq/control","v":1,"f":46,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":1000,"scope":["control","3333333333333333333333333333333333333333333333333333333333333333","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[["c","4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022"]],"auth":[["active-human","c","4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022"]],"body":{"command":"4444444444444444444444444444444444444444444444444444444444444444","digest":"5555555555555555555555555555555555555555555555555555555555555555","project":"6666666666666666666666666666666666666666666666666666666666666666","target_home":"1111111111111111111111111111111111111111111111111111111111111111","expected_head":"4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022","operation":{"provider":"hq-control","session":"test","id":"7777777777777777777777777777777777777777777777777777777777777777"},"body":"open"}}"#;
const CONTROL_EVENT: &str = r#"{"id":"0cd711332f29eebb49f54e8823d452d7ee547f581107fb5903d561d593c60db2","pubkey":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","created_at":1,"kind":6000,"tags":[],"content":"{\"p\":\"hq/control\",\"v\":1,\"f\":46,\"author\":\"1111111111111111111111111111111111111111111111111111111111111111\",\"time\":1000,\"scope\":[\"control\",\"3333333333333333333333333333333333333333333333333333333333333333\",\"1111111111111111111111111111111111111111111111111111111111111111\"],\"parents\":[[\"c\",\"4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022\"]],\"auth\":[[\"active-human\",\"c\",\"4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022\"]],\"body\":{\"command\":\"4444444444444444444444444444444444444444444444444444444444444444\",\"digest\":\"5555555555555555555555555555555555555555555555555555555555555555\",\"project\":\"6666666666666666666666666666666666666666666666666666666666666666\",\"target_home\":\"1111111111111111111111111111111111111111111111111111111111111111\",\"expected_head\":\"4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022\",\"operation\":{\"provider\":\"hq-control\",\"session\":\"test\",\"id\":\"7777777777777777777777777777777777777777777777777777777777777777\"},\"body\":\"open\"}}","sig":"05a01ad2e5823e14aac19364c0b260728789ad5b14c1c516c30649a97dd4e81980dfa584b1ed5ce5d8c9f7c42bbfdb17611bf641db4bed3311ac5c499f94e12a"}"#;
const LEGACY_GO_SCHEMA3_CONTENT: &str = r#"{"schema":3,"type":"question","installation_id":"0198c7ec-73b0-7cc3-a5f7-e31c77140d01","signer_key_id":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","parents":[],"scope":"installation-private","payload":{"body":"legacy"}}"#;

#[test]
fn published_events_advance_through_distinct_trust_states_and_retain_exact_bytes() {
    for (wire, content, namespace, family, id) in [
        (
            CANONICAL_EVENT,
            CANONICAL_CONTENT,
            ProtocolNamespace::Canonical,
            1,
            "4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022",
        ),
        (
            CONTROL_EVENT,
            CONTROL_CONTENT,
            ProtocolNamespace::Control,
            46,
            "0cd711332f29eebb49f54e8823d452d7ee547f581107fb5903d561d593c60db2",
        ),
    ] {
        let raw = RawEventBytes::new(wire.as_bytes().to_vec()).expect("bounded fixture");
        assert_eq!(raw.exact_bytes(), wire.as_bytes());

        let parsed = raw.parse().expect("strict outer event");
        assert_eq!(parsed.content_bytes(), content.as_bytes());
        assert_eq!(
            parsed.created_at(),
            u64::from(namespace == ProtocolNamespace::Control)
        );

        let verified = parsed.verify().expect("valid ID and BIP-340 signature");
        assert_eq!(verified.exact_event_bytes(), wire.as_bytes());
        assert_eq!(verified.content_bytes(), content.as_bytes());
        assert_eq!(hex(&verified.event_id()), id);
        assert_eq!(sha256(verified.event_preimage_bytes()), verified.event_id());

        let DispatchOutcome::Supported(supported) = verified.dispatch().expect("valid prefix")
        else {
            panic!("published vector must be supported");
        };
        assert_eq!(supported.namespace(), namespace);
        assert_eq!(supported.version(), 1);
        assert_eq!(supported.family(), family);
        assert_eq!(supported.content_bytes(), content.as_bytes());
    }
}

#[test]
fn signer_with_explicit_auxiliary_randomness_reproduces_the_published_identity() {
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    let signer = Bip340Signer::from_secret_bytes(secret).expect("valid fixture secret");
    assert_eq!(
        hex(&signer.public_key()),
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
    );

    let verified_event = signer
        .sign(0, CANONICAL_CONTENT.as_bytes(), [0; 32])
        .expect("fixture signs");
    assert_eq!(
        verified_event.event_id(),
        decode::<32>("4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022")
    );
    assert_eq!(verified_event.content_bytes(), CANONICAL_CONTENT.as_bytes());
    let DispatchOutcome::Supported(supported) = verified_event
        .dispatch()
        .expect("locally signed content dispatches")
    else {
        panic!("locally signed content must be supported");
    };
    assert_eq!(supported.namespace(), ProtocolNamespace::Canonical);
}

#[test]
fn official_bip340_vectors_pin_raw_32_byte_message_semantics_and_invalid_cases() {
    let valid = [
        (
            "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "e907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca821525f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0",
        ),
        (
            "dff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659",
            "243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89",
            "6896bd60eeae296db48a229ff71dfe071bde413e6d43f917dc8dcf8c78de33418906d11ac976abccb20b091292bff4ea897efcb639ea871cfa95f6de339e4b0a",
        ),
    ];
    for (public_key, message, signature) in valid {
        assert!(verify_bip340(
            decode::<32>(public_key),
            decode::<32>(message),
            decode::<64>(signature)
        ));
    }

    for (public_key, signature) in [
        (
            "eefdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34",
            "6cff5c3ba86c69ea4b7376f31a9bcb4f74c1976089b2d9963da2e5543e17776969e89b4c5564d00349106b8497785dd7d1d713a8ae82b32fa79d5f7fc407d39b",
        ),
        (
            "dff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659",
            "fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f69e89b4c5564d00349106b8497785dd7d1d713a8ae82b32fa79d5f7fc407d39b",
        ),
    ] {
        assert!(!verify_bip340(
            decode::<32>(public_key),
            decode::<32>("243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89"),
            decode::<64>(signature)
        ));
    }
}

#[test]
fn malformed_and_tampered_outer_events_stop_at_the_expected_boundary() {
    let cases = [
        (
            CANONICAL_EVENT.replacen("{\"id\"", "{ \"id\"", 1),
            FailureClass::OuterNonCanonical,
        ),
        (
            CANONICAL_EVENT.replacen("{\"id\":", "{\"extra\":null,\"id\":", 1),
            FailureClass::OuterMemberOrder,
        ),
        (
            CANONICAL_EVENT.replacen("\"kind\":6000", "\"kind\":6001", 1),
            FailureClass::WrongKind,
        ),
        (
            CANONICAL_EVENT.replacen("\"tags\":[]", "\"tags\":[[\"x\"]]", 1),
            FailureClass::NonemptyTags,
        ),
        (
            CANONICAL_EVENT.replacen("alpha", "bravo", 1),
            FailureClass::EventIdMismatch,
        ),
        (
            CANONICAL_EVENT.replacen("cce3\"}", "cce2\"}", 1),
            FailureClass::BadSignature,
        ),
    ];

    for (wire, expected) in cases {
        let raw = RawEventBytes::new(wire.into_bytes()).expect("case remains bounded");
        let actual = match raw.parse() {
            Ok(parsed) => parsed.verify().expect_err("case must not verify"),
            Err(error) => error,
        };
        assert_eq!(actual.class(), expected);
    }

    let mut invalid_utf8 = CANONICAL_EVENT.as_bytes().to_vec();
    invalid_utf8[0] = 0xff;
    assert_eq!(
        RawEventBytes::new(invalid_utf8)
            .expect("bounded")
            .parse()
            .expect_err("invalid UTF-8")
            .class(),
        FailureClass::OuterInvalidUtf8
    );
    assert_eq!(
        RawEventBytes::new(vec![b'a'; MAX_EVENT_BYTES + 1])
            .expect_err("one byte over limit")
            .class(),
        FailureClass::EventTooLarge
    );
}

#[test]
fn verified_prefix_dispatch_is_closed_and_keeps_unsupported_disjoint() {
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    let signer = Bip340Signer::from_secret_bytes(secret).expect("valid fixture secret");
    let cases = [
        (
            CANONICAL_CONTENT.replacen("\"v\":1", "\"v\":2", 1),
            Ok(UnsupportedReason::Version),
        ),
        (
            CANONICAL_CONTENT.replacen("\"f\":1", "\"f\":49", 1),
            Ok(UnsupportedReason::Family),
        ),
        (
            CANONICAL_CONTENT.replacen("hq/canonical", "example/unknown", 1),
            Ok(UnsupportedReason::Protocol),
        ),
        (
            CANONICAL_CONTENT.replacen("hq/canonical", "hq/control", 1),
            Err(FailureClass::NamespaceConfusion),
        ),
        (
            LEGACY_GO_SCHEMA3_CONTENT.to_owned(),
            Err(FailureClass::LegacySchema),
        ),
    ];

    for (content, expected) in cases {
        let verified = signer
            .sign(0, content.as_bytes(), [7; 32])
            .expect("arbitrary bounded content can be signed");
        match expected {
            Ok(reason) => {
                let DispatchOutcome::Unsupported(unsupported) =
                    verified.dispatch().expect("unsupported is a valid branch")
                else {
                    panic!("case must be unsupported");
                };
                assert_eq!(unsupported.reason(), reason);
                assert_eq!(unsupported.content_bytes(), content.as_bytes());
            }
            Err(class) => assert_eq!(
                verified
                    .dispatch()
                    .expect_err("malformed supported-kind content")
                    .class(),
                class
            ),
        }
    }
}

fn decode<const N: usize>(hexadecimal: &str) -> [u8; N] {
    assert_eq!(hexadecimal.len(), N * 2);
    let mut decoded = [0_u8; N];
    for (index, pair) in hexadecimal.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        decoded[index] = (nibble(pair[0]) << 4) | nibble(pair[1]);
    }
    decoded
}

fn nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => panic!("fixture must use lowercase hex"),
    }
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

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    Sha256::digest(bytes).into()
}
