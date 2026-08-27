//! Composition root and runtime ownership boundary.

use hq_application::InMemoryApplication;
use hq_protocol::{DecodeError, InMemoryFrame};
use hq_reducer::FactSummary;

/// Errors produced while composing the in-memory workspace skeleton.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InMemoryRunError {
    /// A frame failed protocol validation.
    Decode(DecodeError),
}

/// Runs trusted in-memory frames through protocol, application, and reducer boundaries.
pub fn run_in_memory(
    frames: impl IntoIterator<Item = InMemoryFrame>,
) -> Result<FactSummary, InMemoryRunError> {
    let mut application = InMemoryApplication::default();
    for frame in frames {
        let fact = frame.decode().map_err(InMemoryRunError::Decode)?;
        application.submit(fact);
    }

    Ok(application.summary())
}
