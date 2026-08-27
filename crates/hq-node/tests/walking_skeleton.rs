//! End-to-end tests for the in-memory workspace walking skeleton.

use hq_node::{InMemoryRunError, run_in_memory};
use hq_protocol::InMemoryFrame;
use hq_reducer::FactSummary;

#[test]
fn a_fact_crosses_protocol_domain_application_and_reducer_boundaries() {
    let summary = run_in_memory([
        InMemoryFrame::new(2, "second"),
        InMemoryFrame::new(1, "first"),
    ]);

    assert_eq!(summary.as_ref().map(FactSummary::unique_fact_count), Ok(2));
    assert_eq!(
        summary
            .as_ref()
            .map(FactSummary::ordered_fact_ids)
            .map(<[_]>::len),
        Ok(2)
    );
}

#[test]
fn an_invalid_frame_stops_before_application_reduction() {
    assert_eq!(
        run_in_memory([InMemoryFrame::new(1, "")]),
        Err(InMemoryRunError::Decode(
            hq_protocol::DecodeError::EmptyPayload
        ))
    );
}
