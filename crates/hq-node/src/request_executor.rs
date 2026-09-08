//! Owned application execution for requests that may wait on independent component work.

use hq_application::{Application, ApplicationError, ApplicationErrorCode, ApplicationPorts};
use hq_local_api::{
    LifecycleControl, OutboundMessage, ServerSession, ServerSessionError,
    protocol::v1::{LifecycleRequest, LifecycleStatus, WireMessage},
};

/// Owned request capabilities that may run independently of the session event loop.
pub trait LocalRequestExecutor: Send + Sync {
    /// Releases responder and subscription capabilities outside the coordinator.
    fn disconnect(&self, session: &mut ServerSession) {
        session.disconnect();
    }

    /// Executes one request while its session is exclusively owned by this worker.
    fn execute(
        &self,
        session: &mut ServerSession,
        message: WireMessage,
    ) -> Result<OutboundMessage, ServerSessionError>;
}

/// Shares an owned application among independently dispatched local sessions.
pub struct ApplicationRequestExecutor<P> {
    application: Application<P>,
}

impl<P> ApplicationRequestExecutor<P> {
    /// Retains the application until all admitted session work has joined.
    pub const fn new(application: Application<P>) -> Self {
        Self { application }
    }
}

impl<P: ApplicationPorts + Send + Sync> LocalRequestExecutor for ApplicationRequestExecutor<P> {
    fn execute(
        &self,
        session: &mut ServerSession,
        message: WireMessage,
    ) -> Result<OutboundMessage, ServerSessionError> {
        session.receive(message, &self.application, &NoLifecycle)
    }
}

struct NoLifecycle;
impl LifecycleControl for NoLifecycle {
    fn lifecycle(&self, _request: LifecycleRequest) -> Result<LifecycleStatus, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCode::AdapterUnavailable,
        ))
    }
}
