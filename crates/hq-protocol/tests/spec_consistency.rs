//! Consistency checks for the normative protocol specification package.

use std::collections::BTreeSet;

use hq_domain::{FactKind, ProtocolClass};

const MAPPING: &str = include_str!("../../../docs/protocol/payload-mapping-v1.md");
const CANONICAL: &str = include_str!("../../../docs/protocol/canonical-fact-v1.md");
const CONTROL: &str = include_str!("../../../docs/protocol/remote-control-v1.md");
const TRUST: &str = include_str!("../../../docs/protocol/trust-transitions.md");
const ADR: &str = include_str!("../../../docs/adr/0004-canonical-fact-nostr-carriage.md");
const CANONICAL_VECTOR: &str =
    include_str!("../../../docs/protocol/vectors/canonical-installation-v1.json");
const CONTROL_VECTOR: &str = include_str!("../../../docs/protocol/vectors/remote-command-v1.json");
const ADVERSARIAL: &str = include_str!("../../../docs/protocol/vectors/adversarial-v1.json");

#[test]
fn every_fact_family_has_one_mapping_in_the_correct_namespace() {
    let mut numeric_ids = BTreeSet::new();

    for kind in FactKind::ALL {
        let marker = format!("| {} |", kind.catalog_id());
        assert_eq!(
            MAPPING.matches(&marker).count(),
            1,
            "{} must have exactly one mapping row",
            kind.catalog_id()
        );

        let numeric_id = kind
            .catalog_id()
            .strip_prefix("FCT-")
            .and_then(|value| value.parse::<u8>().ok());
        assert!(numeric_id.is_some(), "catalog ID must remain numeric");
        if let Some(numeric_id) = numeric_id {
            assert!(
                numeric_ids.insert(numeric_id),
                "protocol IDs must be unique"
            );
            let namespace = match kind.protocol_class() {
                ProtocolClass::Canonical => "hq/canonical",
                ProtocolClass::RemoteControl => "hq/control",
            };
            let registry_row = format!("| {numeric_id} | {} | {namespace} |", kind.name());
            assert!(
                MAPPING.contains(&registry_row),
                "{} must map to {namespace}",
                kind.catalog_id()
            );
        }
    }

    assert_eq!(numeric_ids, (1..=48).collect());
}

#[test]
fn protocol_ranges_are_disjoint_and_independently_versioned() {
    for id in 1..=45 {
        assert!(CANONICAL.contains(&format!("`{id}`")));
        assert!(!CONTROL.contains(&format!("| {id} |")));
    }
    for id in 46..=48 {
        assert!(CONTROL.contains(&format!("`{id}`")));
        assert!(!CANONICAL.contains(&format!("| {id} |")));
    }
    assert!(CANONICAL.contains("hq/canonical"));
    assert!(CONTROL.contains("hq/control"));
    assert!(CANONICAL.contains("independent version space"));
    assert!(CONTROL.contains("independent version space"));
}

#[test]
fn every_wire_bound_is_named_in_the_normative_specs() {
    for bound in [
        "MAX_EVENT_BYTES",
        "MAX_CONTENT_BYTES",
        "MAX_JSON_DEPTH",
        "MAX_OBJECT_MEMBERS",
        "MAX_COLLECTION_ITEMS",
        "MAX_PARENT_REFS",
        "MAX_AUTHORITY_REFS",
        "MAX_SHORT_TEXT_BYTES",
        "MAX_CONTENT_TEXT_BYTES",
        "MAX_LOCATOR_TEXT_BYTES",
        "MAX_RELAY_HINTS",
        "MAX_RESOURCE_ITEMS",
    ] {
        assert!(
            CANONICAL.contains(bound) || CONTROL.contains(bound),
            "missing named bound {bound}"
        );
    }
}

#[test]
fn vectors_are_exact_and_cover_required_adversarial_classes() {
    for vector in [CANONICAL_VECTOR, CONTROL_VECTOR] {
        assert!(vector.contains("\"content_bytes\""));
        assert!(vector.contains("\"event_preimage_bytes\""));
        assert!(vector.contains("\"event_id\""));
        assert!(vector.contains("\"signature\""));
        assert!(vector.contains("\"semantic_mapping\""));
        assert!(!vector.contains("..."));
        assert!(!vector.contains("<redacted>"));
    }

    for class in [
        "malformed-json",
        "noncanonical-escape",
        "duplicate-member",
        "unknown-member",
        "member-order",
        "decoded-bound",
        "encoded-bound",
        "invalid-hex",
        "wrong-kind",
        "unsupported-version",
        "unsupported-family",
        "namespace-confusion",
        "content-tamper",
        "bad-signature",
        "authority-scope-mismatch",
    ] {
        assert!(ADVERSARIAL.contains(&format!("\"class\": \"{class}\"")));
    }
}

#[test]
fn trust_states_and_primary_source_revisions_are_pinned() {
    for state in [
        "RawEventBytes",
        "ParsedOuterEvent",
        "CryptographicallyVerifiedEvent",
        "VerifiedSupportedRecord",
        "VerifiedUnsupportedRecord",
        "SemanticFact",
        "ReducerAdmission",
    ] {
        assert!(TRUST.contains(state), "missing trust state {state}");
    }
    assert!(TRUST.contains("never exposes `SemanticFact`"));

    assert!(ADR.contains("dabfcb2aaecf4fa374eda8b1232ab303a03f60ba"));
    assert!(ADR.contains("1159ee2f92af3d1b78f888528dcfb260a78baf80"));
    assert!(ADR.contains("kind `6000`"));
    for linked_spec in [
        "canonical-fact-v1.md",
        "remote-control-v1.md",
        "payload-mapping-v1.md",
        "trust-transitions.md",
    ] {
        assert!(ADR.contains(linked_spec), "ADR must link {linked_spec}");
    }
}
