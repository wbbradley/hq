//! Provider-neutral managed-runtime contract and registration boundary.

mod contract;
mod registry;

pub use contract::{
    HarnessActivity, HarnessCancellationOutcome, HarnessCapabilities, HarnessCapability,
    HarnessDrainOutcome, HarnessError, HarnessErrorClass, HarnessEvent, HarnessEventPoll,
    HarnessFactory, HarnessInstance, HarnessInstanceRequest, HarnessInteractiveAnswer,
    HarnessInteractiveRequest, HarnessInteractiveResponse, HarnessOutput, HarnessOutputKind,
    HarnessRequestChoice, HarnessRequestId, HarnessRequestKind, HarnessSession,
    HarnessSessionRequest, HarnessSubmission, HarnessSubmissionLookup, HarnessSubmissionOutcome,
    OpenedHarnessSession,
};
pub use registry::HarnessRegistry;
