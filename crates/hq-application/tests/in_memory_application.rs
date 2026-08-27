//! In-memory application boundary acceptance tests.

use hq_application::InMemoryApplication;
use hq_domain::{BoundedText, Fact, FactId, SKELETON_PAYLOAD_MAX_BYTES, ValidatedValueError};

#[test]
fn duplicate_submission_does_not_change_the_projection() -> Result<(), ValidatedValueError> {
    let fact = Fact::new(
        FactId::from_bytes([7; 32]),
        BoundedText::<SKELETON_PAYLOAD_MAX_BYTES>::new("hello")?,
    );
    let mut application = InMemoryApplication::default();

    application.submit(fact.clone());
    application.submit(fact);

    assert_eq!(application.summary().unique_fact_count(), 1);
    Ok(())
}
