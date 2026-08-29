//! Bounded, versioned local client protocol and reconnecting client boundary.

mod client;
mod conversion;
mod hub;
pub mod protocol;
mod server;

pub use client::{
    BlockingClientConfig, BlockingClientError, BlockingClientRunner, ClientAction,
    ClientConnectionState, ClientError, ClientEvent, ClientOperation, ClientTransition,
    ClientTransport, ConnectionGeneration, InitialView, MAX_IN_FLIGHT_RETRYABLE_COMMANDS,
    ReconnectPolicy, ReconnectingClient,
};
pub use conversion::{
    application_error_to_v1, canonical_evidence_from_v1, canonical_evidence_to_v1,
    evidence_ingest_to_v1, mutation_from_v1, mutation_to_v1, page_request_from_v1, page_to_v1,
    project_command_from_v1, project_command_request_to_v1, project_command_to_v1, snapshot_to_v1,
    subscription_from_v1, topic_from_v1, topic_to_v1,
};
pub use hub::{
    DEFAULT_MAX_SUBSCRIPTIONS, FanoutDisposition, HubConfigError, RevisionHub, RevisionNotice,
};
pub use server::{
    LifecycleControl, OutboundMessage, ServerSession, ServerSessionError, ServerWriteDisposition,
    WriteTicket,
};
