//! Reusable provider-neutral conformance suite over the deterministic scripted adapter.

#![allow(clippy::expect_used)]

use hq_testkit::{HarnessConformanceScenario, ScriptedHarnessSubject, run_harness_conformance};

#[test]
fn scripted_provider_passes_every_neutral_harness_scenario() {
    let report = run_harness_conformance(&ScriptedHarnessSubject)
        .expect("scripted provider satisfies the neutral contract");
    assert_eq!(
        report.scenarios.as_slice(),
        HarnessConformanceScenario::ALL.as_slice()
    );
}
