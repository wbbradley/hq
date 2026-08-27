//! Bounded, versioned local client protocol and reconnecting client boundary.

mod client;
mod conversion;
mod hub;
pub mod protocol;
mod server;

pub use client::{
    ClientAction, ClientError, ClientEvent, ClientTransition, ClientTransport,
    ConnectionGeneration, MAX_IN_FLIGHT_MUTATIONS, ReconnectPolicy, ReconnectingClient,
};
pub use conversion::{
    application_error_to_v1, mutation_from_v1, mutation_to_v1, page_request_from_v1, page_to_v1,
    snapshot_to_v1, subscription_from_v1, topic_from_v1, topic_to_v1,
};
pub use hub::{
    DEFAULT_MAX_SUBSCRIPTIONS, FanoutDisposition, HubConfigError, RevisionHub, RevisionNotice,
};
pub use server::{
    LifecycleControl, OutboundMessage, ServerSession, ServerSessionError, WriteTicket,
};
