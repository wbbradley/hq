//! Composition root and runtime ownership boundary.

mod identity;

pub use identity::{
    BackupPassword, IdentityError, IdentityErrorClass, InstallationIdentity, LocalConfiguration,
    PublicIdentity, RelayEndpoint, StateDirectoryOwner, StatePaths,
};

use hq_application::InMemoryApplication;
use hq_protocol::{DecodeError, InMemoryFrame};
use hq_reducer::{GraphReductionReport, ReduceError};

/// Errors produced while composing the in-memory workspace skeleton.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InMemoryRunError {
    /// A frame failed protocol validation.
    Decode(DecodeError),
    /// Pure complete-batch reduction rejected an inconsistent domain stage result.
    Reduce(ReduceError),
}

/// Runs trusted in-memory frames through protocol, application, and reducer boundaries.
pub fn run_in_memory(
    frames: impl IntoIterator<Item = InMemoryFrame>,
) -> Result<GraphReductionReport, InMemoryRunError> {
    let mut application = InMemoryApplication::default();
    for frame in frames {
        let fact = frame.decode().map_err(InMemoryRunError::Decode)?;
        application.submit(fact);
    }

    application.summary().map_err(InMemoryRunError::Reduce)
}
