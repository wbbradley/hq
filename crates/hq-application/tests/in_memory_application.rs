//! In-memory application boundary acceptance tests.

use hq_application::InMemoryApplication;
use hq_domain::{Fact, FactId};

#[test]
fn duplicate_submission_does_not_change_the_projection() {
    let fact = Fact::new(FactId::new(7), "hello");
    let mut application = InMemoryApplication::default();

    application.submit(fact.clone());
    application.submit(fact);

    assert_eq!(application.summary().unique_fact_count(), 1);
}
