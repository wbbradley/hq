//! Executable consistency checks between local API v1 code and its normative specification.

use hq_local_api::protocol::v1::{
    MAX_BUFFERED_BYTES, MAX_BUILD_FIELD_BYTES, MAX_CANONICAL_EVIDENCE_BYTES,
    MAX_CANONICAL_EVIDENCE_ITEMS, MAX_CURSOR_BYTES, MAX_FRAME_BYTES,
    MAX_MATERIALIZED_CONVERSATION_PAGE_ITEMS, MAX_PAGE_ITEMS, MAX_SNAPSHOT_ITEMS, MAX_TOPICS, V1,
};

const SPEC: &str = include_str!("../../../docs/protocol/local-api-v1.md");

#[test]
fn normative_spec_names_the_independent_version_and_every_local_bound() {
    for expected in [
        format!("local API v{V1}"),
        MAX_FRAME_BYTES.to_string(),
        MAX_BUFFERED_BYTES.to_string(),
        MAX_BUILD_FIELD_BYTES.to_string(),
        MAX_PAGE_ITEMS.to_string(),
        MAX_MATERIALIZED_CONVERSATION_PAGE_ITEMS.to_string(),
        MAX_CURSOR_BYTES.to_string(),
        MAX_TOPICS.to_string(),
        MAX_SNAPSHOT_ITEMS.to_string(),
        MAX_CANONICAL_EVIDENCE_ITEMS.to_string(),
        MAX_CANONICAL_EVIDENCE_BYTES.to_string(),
    ] {
        assert!(
            SPEC.replace(',', "").contains(&expected),
            "spec must name {expected}"
        );
    }
}

#[test]
fn normative_spec_pins_replay_subscription_and_trust_invariants() {
    for required in [
        "grants no domain authority",
        "highest common version",
        "repeats the exact frame payload",
        "registers a subscription as pending before reading",
        "only after the acknowledgement frame is confirmed written",
        "never blocks a commit",
        "authoritative materialized view",
    ] {
        assert!(
            SPEC.contains(required),
            "missing normative rule: {required}"
        );
    }
}
