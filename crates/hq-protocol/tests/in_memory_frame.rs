//! In-memory protocol boundary acceptance tests.

use hq_protocol::{DecodeError, InMemoryFrame};

#[test]
fn rejects_an_empty_fact_payload() {
    assert_eq!(
        InMemoryFrame::new(7, "").decode(),
        Err(DecodeError::EmptyPayload)
    );
}
