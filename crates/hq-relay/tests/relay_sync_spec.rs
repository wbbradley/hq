//! Normative relay synchronization specification and public-record contracts.

#![allow(clippy::expect_used)]

use hq_application::{RelayAccess, RelayAuthentication};
use hq_domain::{CommandDigest, OperationId};
use hq_relay::{
    CLIENT_AUTH_KIND, DesiredRelayPolicy, EnvelopeCodec, RelayEnvelopePort, RelayPolicyChange,
    RelayUrl,
};

const SPEC: &str = include_str!("../../../docs/protocol/relay-sync-v1.md");

#[test]
fn normative_spec_pins_durability_bounds_and_non_authority() {
    for required in [
        "untrusted transport services",
        "Preparation plus the uniqueness claim is one transaction",
        "Resume repeats the boundary page",
        "typed keyset position",
        "never skips a possibly unbounded",
        "Relay `OK` text is never durable",
        "Staging is FIFO by first-received time then wrapper digest and evicts nothing",
        "Quarantine is diagnostic and may evict",
        "Relay URL/order/time/acceptance is audit input only",
        "joins every session owner",
    ] {
        assert!(
            SPEC.contains(required),
            "missing normative rule: {required}"
        );
    }
    for bound in ["1,024", "64 MiB", "4 MiB", "262,144", "4,096"] {
        assert!(SPEC.contains(bound), "missing normative bound: {bound}");
    }
}

#[test]
fn passive_policy_records_expose_fields_while_url_validation_remains_opaque() {
    let desired = DesiredRelayPolicy {
        url: RelayUrl::new("wss://relay.example".to_owned()).expect("URL validates"),
        access: RelayAccess::ReadWrite,
        authentication: RelayAuthentication::Required,
        enabled: true,
    };
    let change = RelayPolicyChange {
        operation_id: OperationId::from_bytes([1; 32]),
        request_digest: CommandDigest::from_bytes([2; 32]),
        desired,
    };
    assert_eq!(change.desired.url.as_str(), "wss://relay.example");
    assert_eq!(change.desired.access, RelayAccess::ReadWrite);
    assert_eq!(change.desired.authentication, RelayAuthentication::Required);
    assert!(change.desired.enabled);
}

#[test]
fn generic_nostr_authentication_uses_transport_verification_not_semantic_fact_parsing() {
    let codec = EnvelopeCodec::from_secret_bytes([4; 32]).expect("codec constructs");
    let url = RelayUrl::new("wss://relay.example".to_owned()).expect("URL validates");
    let prepared = RelayEnvelopePort::authenticate(&codec, &url, "relay-challenge", 1_800_000_000)
        .expect("generic Nostr authentication signs and verifies");
    let event: serde_json::Value =
        serde_json::from_slice(&prepared.exact_event).expect("authentication JSON parses");
    assert_eq!(event["kind"], CLIENT_AUTH_KIND);
    assert_eq!(event["id"].as_str().map(str::len), Some(64));
    assert_ne!(prepared.event_id, [0; 32]);
}
