//! Deterministic builders, scripted adapters, and test support.

mod fixture;
mod harness;
mod payloads;
mod sequence;
mod values;

pub use fixture::{FactBuilder, FixtureError};
pub use harness::{
    HarnessConformanceFailure, HarnessConformanceFixture, HarnessConformanceObservation,
    HarnessConformanceReport, HarnessConformanceScenario, HarnessConformanceSubject,
    HarnessConformanceTrace, ScriptedHarnessSubject, run_harness_conformance,
};
pub use sequence::{StateMachineSequence, arrival_permutations};
pub use values::{DeterministicClock, DeterministicValues};
