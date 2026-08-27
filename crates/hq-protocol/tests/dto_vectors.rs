//! Public contracts for strict full-content DTO verification.

#![allow(clippy::expect_used, clippy::panic)]

use hq_protocol::{Bip340Signer, DispatchOutcome, ProtocolNamespace};

const CANONICAL_CONTENT: &str = r#"{"p":"hq/canonical","v":1,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[],"auth":[],"body":{"installation":"1111111111111111111111111111111111111111111111111111111111111111","signing":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","encryption":"2222222222222222222222222222222222222222222222222222222222222222","label":"alpha"}}"#;
const CONTROL_CONTENT: &str = r#"{"p":"hq/control","v":1,"f":46,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":1000,"scope":["control","3333333333333333333333333333333333333333333333333333333333333333","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[["c","4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022"]],"auth":[["active-human","c","4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022"]],"body":{"command":"4444444444444444444444444444444444444444444444444444444444444444","digest":"5555555555555555555555555555555555555555555555555555555555555555","project":"6666666666666666666666666666666666666666666666666666666666666666","target_home":"1111111111111111111111111111111111111111111111111111111111111111","expected_head":"4b63dbd21d18790058d9f0483fb06b7223594b843f235849456553f957a90022","operation":{"provider":"hq-control","session":"test","id":"7777777777777777777777777777777777777777777777777777777777777777"},"body":"open"}}"#;

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

const A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const B: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const C: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const D: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const KEY: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

#[allow(clippy::too_many_lines)]
fn valid_bodies() -> [(u64, String); 48] {
    let installation = format!(r#"{{"installation":"{A}","signing":"{KEY}"}}"#);
    let mailbox = format!(r#"{{"installation":"{A}","mailbox":"{B}"}}"#);
    let locator = r#"{"scheme":"worktree","value":"/repo"}"#;
    let context =
        format!(r#"{{"directory":{locator},"repository":null,"worktree":null,"branch":null}}"#);
    let operation = format!(r#"{{"provider":"provider","session":"session","id":"{D}"}}"#);
    let binding = format!(
        r#"{{"assignment":"{B}","agent":"{C}","provider":"provider","session":"session"}}"#
    );
    let resource = format!(r#"{{"id":"{B}","locator":{locator},"health":"unknown"}}"#);
    let message = |purpose: &str, project: &str, id: &str| {
        format!(
            r#"{{"id":"{id}","sender":{mailbox},"recipient":null,"body":"body","purpose":"{purpose}","presentation":"message","correlation":null,"project":{project}}}"#
        )
    };
    [
        (
            1,
            format!(
                r#"{{"installation":"{A}","signing":"{KEY}","encryption":"{B}","label":null}}"#
            ),
        ),
        (
            2,
            format!(r#"{{"mailbox":"{B}","kind":"agent","label":null}}"#),
        ),
        (
            3,
            format!(r#"{{"mailbox":"{B}","provider":"provider","session":"session"}}"#),
        ),
        (4, format!(r#"{{"mailbox":"{B}","context":{context}}}"#)),
        (
            5,
            format!(r#"{{"peer":{installation},"encryption":"{B}","label":null,"relays":[]}}"#),
        ),
        (6, format!(r#"{{"peer":"{B}","reason":"blocked"}}"#)),
        (
            7,
            format!(r#"{{"grant":"{C}","mailbox":{mailbox},"grantee":{installation}}}"#),
        ),
        (
            8,
            format!(r#"{{"grant":"{C}","mailbox":{mailbox},"grantee":"{A}"}}"#),
        ),
        (9, format!(r#"{{"grant":"{C}","action":"{D}"}}"#)),
        (
            10,
            format!(r#"{{"account":"{C}","creator":{installation},"label":null}}"#),
        ),
        (11, format!(r#"{{"account":"{C}"}}"#)),
        (
            12,
            format!(
                r#"{{"account":"{C}","grant":"{D}","device":{installation},"label":null,"relays":[]}}"#
            ),
        ),
        (
            13,
            format!(r#"{{"account":"{C}","grant":"{D}","device":{installation}}}"#),
        ),
        (
            14,
            format!(r#"{{"account":"{C}","grant":"{D}","device":"{A}"}}"#),
        ),
        (15, message("question", "null", B)),
        (16, message("asynchronous", "null", B)),
        (
            17,
            format!(
                r#"{{"thread":"{C}","message":{}}}"#,
                message("asynchronous", "null", B)
            ),
        ),
        (18, format!(r#"{{"thread":"{C}","reason":null}}"#)),
        (19, format!(r#"{{"message":"{B}"}}"#)),
        (20, format!(r#"{{"message":"{B}"}}"#)),
        (21, format!(r#"{{"message":"{B}","reason":"rejected"}}"#)),
        (
            22,
            format!(
                r#"{{"source":{mailbox},"operation":{operation},"item":null,"kind":"progress","logical_key":"key","runtime":"runtime","sequence":1,"occurred_at":0,"status":{{"state":"running"}},"content":"content","truncated":false}}"#
            ),
        ),
        (
            23,
            format!(r#"{{"agent":"{C}","mailbox":"{B}","name":"agent-one"}}"#),
        ),
        (24, format!(r#"{{"agent":"{C}","mailbox":"{B}"}}"#)),
        (
            25,
            format!(
                r#"{{"agent":"{C}","mailbox":"{B}","provider":"provider","session":"session","context":{context}}}"#
            ),
        ),
        (
            26,
            format!(
                r#"{{"agent":"{C}","provider":"provider","session":"session","display":null}}"#
            ),
        ),
        (
            27,
            format!(
                r#"{{"project":"{C}","mailbox":"{B}","home":"{A}","name":"project","brief":null,"predecessor":null,"resources":[{resource}],"primary":"{B}","state":"open"}}"#
            ),
        ),
        (28, format!(r#"{{"project":"{C}"}}"#)),
        (29, format!(r#"{{"project":"{C}"}}"#)),
        (
            30,
            format!(r#"{{"project":"{C}","forced":false,"runtime":null}}"#),
        ),
        (31, format!(r#"{{"project":"{C}"}}"#)),
        (32, format!(r#"{{"project":"{C}"}}"#)),
        (
            33,
            format!(r#"{{"project":"{C}","name":"project","brief":null}}"#),
        ),
        (
            34,
            format!(r#"{{"project":"{C}","resource":{resource},"primary":false}}"#),
        ),
        (
            35,
            format!(r#"{{"project":"{C}","resource":"{B}","force":false}}"#),
        ),
        (
            36,
            format!(r#"{{"project":"{C}","old_resource":"{D}","resource":{resource}}}"#),
        ),
        (37, format!(r#"{{"project":"{C}","resource":"{B}"}}"#)),
        (
            38,
            format!(
                r#"{{"project":"{C}","resource":"{B}","health":"healthy","details":null,"checked_at":0}}"#
            ),
        ),
        (39, format!(r#"{{"project":"{C}","binding":{binding}}}"#)),
        (
            40,
            format!(
                r#"{{"project":"{C}","binding":{binding},"thread":"{D}","launch_directory":{locator},"activation":{operation}}}"#
            ),
        ),
        (
            41,
            format!(r#"{{"project":"{C}","assignment":"{B}","cause":"blocked"}}"#),
        ),
        (
            42,
            format!(r#"{{"project":"{C}","assignment":"{B}","forced":false,"runtime":null}}"#),
        ),
        (
            43,
            format!(r#"{{"project":"{C}","message":"{B}","input_fact":"{D}","sequence":1}}"#),
        ),
        (
            44,
            format!(
                r#"{{"project":"{C}","message":"{B}","sequence":1,"dispatch":"{D}","binding":{binding},"thread":"{A}"}}"#
            ),
        ),
        (
            45,
            format!(
                r#"{{"project":"{C}","output":"{B}","dispatch":"{D}","binding":{binding},"thread":"{A}","message":{}}}"#,
                message("project-output", &format!(r#""{C}""#), B)
            ),
        ),
        (
            46,
            format!(
                r#"{{"command":"{D}","digest":"{B}","project":"{C}","target_home":"{A}","expected_head":"{B}","operation":{operation},"body":"open"}}"#
            ),
        ),
        (
            47,
            format!(
                r#"{{"command":"{D}","digest":"{B}","project":"{C}","received_head":"{B}","received_at":0}}"#
            ),
        ),
        (
            48,
            format!(
                r#"{{"command":"{D}","digest":"{B}","project":"{C}","result":{{"state":"committed","head":"{A}"}},"runtime":{{"state":"succeeded"}}}}"#
            ),
        ),
    ]
}
