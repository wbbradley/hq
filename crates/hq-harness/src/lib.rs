//! Provider-neutral managed-runtime contract and registration boundary.

mod buffer;
mod contract;
mod environment;
mod registry;
mod supervisor;

pub use buffer::{HarnessBufferPush, HarnessBufferedEvent, HarnessEventBuffer, HarnessSnapshotKey};
pub use contract::{
    HarnessActivity, HarnessCancellationOutcome, HarnessCapabilities, HarnessCapability,
    HarnessDrainOutcome, HarnessError, HarnessErrorClass, HarnessEvent, HarnessEventPoll,
    HarnessFactory, HarnessInstance, HarnessInstanceRequest, HarnessInteractiveAnswer,
    HarnessInteractiveRequest, HarnessInteractiveResponse, HarnessOutput, HarnessOutputKind,
    HarnessRequestChoice, HarnessRequestId, HarnessRequestKind, HarnessSession,
    HarnessSessionRequest, HarnessSubmission, HarnessSubmissionLookup, HarnessSubmissionOutcome,
    OpenedHarnessSession,
};
pub use environment::{
    HarnessEnvironment, MAX_HARNESS_ENVIRONMENT_BYTES, MAX_HARNESS_ENVIRONMENT_ENTRIES,
    MAX_HARNESS_ENVIRONMENT_NAME_BYTES, MAX_HARNESS_ENVIRONMENT_VALUE_BYTES,
};
pub use registry::{HarnessRegistry, RegisteredProviderView};
pub use supervisor::{
    HarnessClock, HarnessDeliveryRecord, HarnessDeliveryState, HarnessEventCheckpoint,
    HarnessEventPumpReport, HarnessLaunchRequest, HarnessLeaseOutcome, HarnessOwnerToken,
    HarnessPendingInteraction, HarnessPersistencePort, HarnessProjectDelivery, HarnessReadySession,
    HarnessResponderId, HarnessSessionControlOutcome, HarnessSessionOperation,
    HarnessSessionOperationKind, HarnessSessionOperationState, HarnessStateMutation,
    HarnessStatePort, HarnessStateSnapshot, HarnessSupervisor, HarnessSupervisorConfig,
    HarnessSupervisorDependencies, HarnessSupervisorReport, HarnessTokenSource, HarnessWorkerLease,
    MAX_HARNESS_SUPERVISOR_STATE_ITEMS,
};
