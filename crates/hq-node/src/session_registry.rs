//! Bounded ownership for authenticated local protocol sessions.

use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use hq_application::{Application, ApplicationPorts};
use hq_local_api::{
    LifecycleControl, OutboundMessage, RevisionHub, ServerSession, ServerSessionError,
    ServerWriteDisposition,
    protocol::v1::{BuildMetadata, Id32, Request, WireMessage},
};
use tokio::{sync::mpsc, task::JoinSet};

use crate::{
    AcceptedLocalStream, LocalSessionClose, LocalSessionEvent, LocalSessionHandle,
    LocalSessionSendError, LocalSessionStartError, prepare_local_session_io,
};

type RequestTaskOutput = (
    Id32,
    Option<(ServerSession, Result<OutboundMessage, ServerSessionError>)>,
);
type RequestTaskJoin = Result<(tokio::task::Id, RequestTaskOutput), tokio::task::JoinError>;

type SessionTaskOutput = (Id32, LocalSessionClose);
type SessionTaskJoin = Result<(tokio::task::Id, SessionTaskOutput), tokio::task::JoinError>;

/// Fixed capacities applied by one local-session registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalSessionRegistryConfig {
    /// Maximum number of concurrently owned local sessions.
    pub session_capacity: NonZeroUsize,
    /// Maximum number of decoded/completion events awaiting dispatch.
    pub event_capacity: NonZeroUsize,
    /// Maximum number of complete frames awaiting each session writer.
    pub write_capacity: NonZeroUsize,
}

/// Immediate rejection before an authenticated stream becomes registry-owned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSessionAdmissionError {
    /// New session intake has been explicitly closed.
    Closed,
    /// This connection identity is already active.
    Duplicate,
    /// The fixed session capacity is occupied.
    Full,
    /// The authenticated descriptor could not be attached to the async runtime.
    Start(LocalSessionStartError),
}

impl fmt::Display for LocalSessionAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local session admission failed: {self:?}")
    }
}

impl Error for LocalSessionAdmissionError {}

/// Stable class for one local-session task that did not finish normally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSessionTaskFailureKind {
    /// The task was cancelled before it returned.
    Cancelled,
    /// The task panicked while owning the session descriptor.
    Panicked,
}

/// Diagnostic identity and class for one failed session task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalSessionTaskFailure {
    /// Connection identity, when it remained recoverable from task bookkeeping.
    pub session_id: Option<Id32>,
    /// Stable task termination class.
    pub kind: LocalSessionTaskFailureKind,
}

/// Plain diagnostic outcome from a complete registry drain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSessionShutdownReport {
    /// Sessions explicitly asked to close during this drain.
    pub closed_sessions: usize,
    /// Session tasks observed to completion by this drain.
    pub joined_tasks: usize,
    /// Tasks that did not return normally.
    pub task_failures: Vec<LocalSessionTaskFailure>,
    /// Session entries retained after the drain completed.
    pub retained_sessions: usize,
    /// Session tasks retained after the drain completed.
    pub retained_tasks: usize,
}

/// One invalidation that could not enter a session's fixed write queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalSessionInvalidationFailure {
    /// Slow or failed connection that was closed.
    pub session_id: Id32,
    /// Immediate bounded-queue rejection.
    pub error: LocalSessionSendError,
}

/// Plain outcome from one bounded invalidation-delivery pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalSessionInvalidationReport {
    /// Invalidation frames accepted by per-session write queues.
    pub delivered: usize,
    /// Boot-local identities whose writer accepted an invalidation frame.
    pub delivered_sessions: Vec<Id32>,
    /// Saturated or failed sessions closed by this pass.
    pub failures: Vec<LocalSessionInvalidationFailure>,
}

/// Session-local reason that caused registry-owned transport closure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalSessionDisconnectCause {
    /// The byte driver ended with this stable terminal cause.
    Transport(LocalSessionClose),
    /// The decoded message or completed ticket violated server-session state.
    Protocol(ServerSessionError),
    /// A response could not enter the fixed encoded-write queue.
    Response(LocalSessionSendError),
    /// Decoded request intake was closed for an orderly node drain.
    RequestIntakeClosed,
    /// A final protocol response completed and requires orderly close.
    PostWriteClose,
    /// The owned byte task did not return normally.
    Task(LocalSessionTaskFailureKind),
}

/// One bounded unit of progress made by the central session dispatcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalSessionDispatch {
    /// A request was admitted to independent execution for this session.
    RequestDispatched {
        /// Connection exclusively owned by the admitted request.
        session_id: Id32,
    },
    /// Session capability cleanup completed on its owned worker.
    SessionCleanupJoined {
        /// Connection whose responder and subscription capabilities were released.
        session_id: Id32,
    },
    /// A decoded request was handled and its exact response was queued.
    MessageHandled {
        /// Connection that owned the decoded message.
        session_id: Id32,
    },
    /// An exact completed response ticket was confirmed.
    WriteConfirmed {
        /// Connection that owned the ticket.
        session_id: Id32,
    },
    /// Session closure began without affecting sibling sessions.
    SessionClosing {
        /// Connection being closed.
        session_id: Id32,
        /// Stable session-local closure cause.
        cause: LocalSessionDisconnectCause,
    },
    /// A normally completed byte task was joined and its entry removed.
    TaskJoined {
        /// Connection formerly owned by the task.
        session_id: Id32,
        /// Stable terminal cause returned by the joined byte task.
        cause: LocalSessionClose,
    },
    /// A failed byte task was joined and its entry removed.
    TaskFailed {
        /// Stable task failure diagnostics.
        failure: LocalSessionTaskFailure,
    },
    /// A terminal event arrived after its session had already been removed.
    StaleEvent {
        /// Connection named by the stale event.
        session_id: Id32,
    },
}

#[derive(Debug)]
struct SessionSlot {
    session: Option<ServerSession>,
    io: LocalSessionHandle,
    closing: bool,
    pending_response: bool,
}

/// Sole bounded owner of active local server sessions and their I/O tasks.
pub struct LocalSessionRegistry {
    config: LocalSessionRegistryConfig,
    hub: RevisionHub,
    build: BuildMetadata,
    sessions: BTreeMap<Id32, SessionSlot>,
    event_tx: mpsc::Sender<LocalSessionEvent>,
    events: mpsc::Receiver<LocalSessionEvent>,
    tasks: JoinSet<SessionTaskOutput>,
    task_sessions: BTreeMap<tokio::task::Id, Id32>,
    accepting: bool,
    accepting_requests: bool,
    executor: Option<Arc<dyn crate::LocalRequestExecutor>>,
    requests: JoinSet<RequestTaskOutput>,
    request_sessions: BTreeMap<tokio::task::Id, Id32>,
}

impl fmt::Debug for LocalSessionRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalSessionRegistry")
            .field("sessions", &self.sessions.len())
            .field("requests", &self.requests.len())
            .finish_non_exhaustive()
    }
}

impl LocalSessionRegistry {
    /// Installs owned capabilities for independent request execution.
    #[must_use]
    pub fn with_request_executor(mut self, executor: Arc<dyn crate::LocalRequestExecutor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Constructs an empty registry with fixed capacities.
    pub fn new(config: LocalSessionRegistryConfig, hub: RevisionHub, build: BuildMetadata) -> Self {
        let (event_tx, events) = mpsc::channel(config.event_capacity.get());
        Self {
            config,
            hub,
            build,
            sessions: BTreeMap::new(),
            event_tx,
            events,
            tasks: JoinSet::new(),
            task_sessions: BTreeMap::new(),
            accepting: true,
            accepting_requests: true,
            executor: None,
            requests: JoinSet::new(),
            request_sessions: BTreeMap::new(),
        }
    }

    /// Admits one peer-validated stream without exceeding fixed ownership bounds.
    pub fn admit(
        &mut self,
        session_id: Id32,
        accepted: AcceptedLocalStream,
    ) -> Result<(), LocalSessionAdmissionError> {
        if !self.accepting {
            return Err(LocalSessionAdmissionError::Closed);
        }
        if self.sessions.contains_key(&session_id)
            || self.request_sessions.values().any(|id| *id == session_id)
        {
            return Err(LocalSessionAdmissionError::Duplicate);
        }
        let orphaned_requests = self
            .request_sessions
            .values()
            .filter(|id| !self.sessions.contains_key(id))
            .count();
        if self.sessions.len() + orphaned_requests >= self.config.session_capacity.get() {
            return Err(LocalSessionAdmissionError::Full);
        }

        let (io, driver) = prepare_local_session_io(
            accepted,
            session_id,
            self.config.write_capacity,
            self.event_tx.clone(),
        )
        .map_err(LocalSessionAdmissionError::Start)?;
        let session = ServerSession::new(self.hub.clone(), self.build.clone(), session_id);
        self.sessions.insert(
            session_id,
            SessionSlot {
                session: Some(session),
                io,
                closing: false,
                pending_response: false,
            },
        );
        let task = self.tasks.spawn(async move {
            let cause = driver.await;
            (session_id, cause)
        });
        self.task_sessions.insert(task.id(), session_id);
        debug_assert_eq!(self.sessions.len(), self.tasks.len());
        debug_assert_eq!(self.sessions.len(), self.task_sessions.len());
        Ok(())
    }

    /// Processes one decoded, write-completion, transport-close, or task-join event.
    pub async fn dispatch_next<P, L>(
        &mut self,
        application: &Application<P>,
        lifecycle: &L,
    ) -> Option<LocalSessionDispatch>
    where
        P: ApplicationPorts,
        L: LifecycleControl,
    {
        if self.tasks.is_empty() && self.requests.is_empty() {
            return None;
        }
        tokio::select! {
            biased;
            event = self.events.recv() => {
                event.map(|event| self.dispatch_event(event, application, lifecycle))
            }
            joined = self.requests.join_next_with_id(), if !self.requests.is_empty() => self.dispatch_request_join(joined),
            joined = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => self.dispatch_join(joined),
        }
    }

    /// Stops admission while retaining all currently owned sessions for explicit drain.
    pub fn close_intake(&mut self) {
        self.accepting = false;
    }

    /// Stops decoded request dispatch while retaining accepted response writes.
    pub fn close_request_intake(&mut self) {
        self.accepting_requests = false;
    }

    /// Delivers at most one coalesced invalidation per active session without waiting.
    pub fn flush_invalidations(&mut self) -> LocalSessionInvalidationReport {
        let mut report = LocalSessionInvalidationReport::default();
        let session_ids = self.sessions.keys().copied().collect::<Vec<_>>();
        for session_id in session_ids {
            let attempted = self.sessions.get_mut(&session_id).and_then(|slot| {
                if slot.closing {
                    return None;
                }
                slot.session
                    .as_mut()?
                    .poll_invalidation()
                    .map(|message| slot.io.try_send_invalidation(&message))
            });
            match attempted {
                Some(Ok(())) => {
                    report.delivered += 1;
                    report.delivered_sessions.push(session_id);
                }
                Some(Err(error)) => {
                    report
                        .failures
                        .push(LocalSessionInvalidationFailure { session_id, error });
                    self.begin_close(session_id);
                }
                None => {}
            }
        }
        report
    }

    /// Returns the number of centrally owned session entries.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Reports whether the registry owns no live session entries.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Returns the number of session I/O and request tasks awaiting a join.
    pub fn task_count(&self) -> usize {
        self.tasks.len() + self.requests.len()
    }

    /// Returns responses accepted by session writers but not yet confirmed written.
    pub fn pending_response_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|slot| slot.pending_response)
            .count()
    }

    /// Closes intake and every descriptor, consumes terminal events, and joins all tasks.
    pub async fn shutdown(mut self) -> LocalSessionShutdownReport {
        self.accepting = false;
        let closed_sessions = self.sessions.len();
        for slot in self.sessions.values() {
            slot.io.close();
        }

        let mut joined_tasks = 0;
        let mut task_failures = Vec::new();
        while !self.tasks.is_empty() {
            tokio::select! {
                event = self.events.recv() => {
                    if let Some(LocalSessionEvent::Closed { session_id, .. }) = event {
                        self.begin_close(session_id);
                    }
                }
                joined = self.tasks.join_next_with_id() => {
                    joined_tasks += 1;
                    match joined {
                        Some(Ok((task_id, (session_id, _cause)))) => {
                            self.task_sessions.remove(&task_id);
                            self.remove_session(session_id);
                        }
                        Some(Err(error)) => {
                            let session_id = self.task_sessions.remove(&error.id());
                            if let Some(session_id) = session_id {
                                self.remove_session(session_id);
                            }
                            task_failures.push(LocalSessionTaskFailure {
                                session_id,
                                kind: if error.is_cancelled() {
                                    LocalSessionTaskFailureKind::Cancelled
                                } else {
                                    LocalSessionTaskFailureKind::Panicked
                                },
                            });
                        }
                        None => break,
                    }
                }
            }
        }
        // Blocking request tasks cannot be detached: they retain application capabilities.
        while let Some(joined) = self.requests.join_next_with_id().await {
            joined_tasks += 1;
            if let Some(LocalSessionDispatch::TaskFailed { failure }) =
                self.dispatch_request_join(Some(joined))
            {
                task_failures.push(failure);
            }
        }
        self.sessions.clear();

        LocalSessionShutdownReport {
            closed_sessions,
            joined_tasks,
            task_failures,
            retained_sessions: self.sessions.len(),
            retained_tasks: self.tasks.len() + self.requests.len(),
        }
    }

    fn dispatch_message<P: ApplicationPorts, L: LifecycleControl>(
        &mut self,
        session_id: Id32,
        message: WireMessage,
        application: &Application<P>,
        lifecycle: &L,
    ) -> LocalSessionDispatch {
        let Some(slot) = self.sessions.get_mut(&session_id) else {
            return LocalSessionDispatch::StaleEvent { session_id };
        };
        if slot.closing {
            return LocalSessionDispatch::StaleEvent { session_id };
        }
        if !self.accepting_requests {
            self.begin_close(session_id);
            return LocalSessionDispatch::SessionClosing {
                session_id,
                cause: LocalSessionDisconnectCause::RequestIntakeClosed,
            };
        }
        if slot.session.is_none() {
            self.begin_close(session_id);
            return LocalSessionDispatch::SessionClosing {
                session_id,
                cause: LocalSessionDisconnectCause::Protocol(ServerSessionError::WritePending),
            };
        }
        if independently_executable(&message)
            && let Some(executor) = self.executor.as_ref()
            && let Some(mut session) = slot.session.take()
        {
            let executor = Arc::clone(executor);
            slot.pending_response = true;
            let task = self.requests.spawn_blocking(move || {
                let result = executor.execute(&mut session, message);
                (session_id, Some((session, result)))
            });
            self.request_sessions.insert(task.id(), session_id);
            return LocalSessionDispatch::RequestDispatched { session_id };
        }
        let routed = slot
            .session
            .as_mut()
            .ok_or(ServerSessionError::Disconnected)
            .and_then(|session| session.receive(message, application, lifecycle))
            .map_err(LocalSessionDisconnectCause::Protocol)
            .and_then(|outbound| {
                slot.io
                    .try_send_response(outbound)
                    .map_err(LocalSessionDisconnectCause::Response)
            });
        match routed {
            Ok(()) => {
                slot.pending_response = true;
                LocalSessionDispatch::MessageHandled { session_id }
            }
            Err(cause) => {
                self.begin_close(session_id);
                LocalSessionDispatch::SessionClosing { session_id, cause }
            }
        }
    }

    fn dispatch_event<P, L>(
        &mut self,
        event: LocalSessionEvent,
        application: &Application<P>,
        lifecycle: &L,
    ) -> LocalSessionDispatch
    where
        P: ApplicationPorts,
        L: LifecycleControl,
    {
        match event {
            LocalSessionEvent::Message {
                session_id,
                message,
            } => self.dispatch_message(session_id, *message, application, lifecycle),
            LocalSessionEvent::Written { session_id, ticket } => {
                let Some(slot) = self.sessions.get_mut(&session_id) else {
                    return LocalSessionDispatch::StaleEvent { session_id };
                };
                if slot.closing {
                    return LocalSessionDispatch::StaleEvent { session_id };
                }
                match slot
                    .session
                    .as_mut()
                    .ok_or(ServerSessionError::WritePending)
                    .and_then(|session| session.confirm_written(ticket))
                {
                    Ok(ServerWriteDisposition::Continue) => {
                        slot.pending_response = false;
                        LocalSessionDispatch::WriteConfirmed { session_id }
                    }
                    Ok(ServerWriteDisposition::Close) => {
                        self.begin_close(session_id);
                        LocalSessionDispatch::SessionClosing {
                            session_id,
                            cause: LocalSessionDisconnectCause::PostWriteClose,
                        }
                    }
                    Err(error) => {
                        let cause = LocalSessionDisconnectCause::Protocol(error);
                        self.begin_close(session_id);
                        LocalSessionDispatch::SessionClosing { session_id, cause }
                    }
                }
            }
            LocalSessionEvent::Closed { session_id, cause } => {
                if !self.sessions.contains_key(&session_id) {
                    return LocalSessionDispatch::StaleEvent { session_id };
                }
                self.begin_close(session_id);
                LocalSessionDispatch::SessionClosing {
                    session_id,
                    cause: LocalSessionDisconnectCause::Transport(cause),
                }
            }
        }
    }

    fn dispatch_request_join(
        &mut self,
        joined: Option<RequestTaskJoin>,
    ) -> Option<LocalSessionDispatch> {
        match joined? {
            Ok((task_id, (session_id, completion))) => {
                self.request_sessions.remove(&task_id);
                let Some((session, result)) = completion else {
                    return Some(LocalSessionDispatch::SessionCleanupJoined { session_id });
                };
                if self
                    .sessions
                    .get(&session_id)
                    .is_none_or(|slot| slot.closing)
                {
                    self.disconnect_session(session_id, session);
                    return Some(LocalSessionDispatch::StaleEvent { session_id });
                }
                let slot = self.sessions.get_mut(&session_id)?;
                slot.session = Some(session);
                let routed = result
                    .map_err(LocalSessionDisconnectCause::Protocol)
                    .and_then(|outbound| {
                        slot.io
                            .try_send_response(outbound)
                            .map_err(LocalSessionDisconnectCause::Response)
                    });
                Some(match routed {
                    Ok(()) => LocalSessionDispatch::MessageHandled { session_id },
                    Err(cause) => {
                        self.begin_close(session_id);
                        LocalSessionDispatch::SessionClosing { session_id, cause }
                    }
                })
            }
            Err(error) => {
                let session_id = self.request_sessions.remove(&error.id());
                if let Some(session_id) = session_id {
                    self.begin_close(session_id);
                }
                Some(LocalSessionDispatch::TaskFailed {
                    failure: LocalSessionTaskFailure {
                        session_id,
                        kind: if error.is_cancelled() {
                            LocalSessionTaskFailureKind::Cancelled
                        } else {
                            LocalSessionTaskFailureKind::Panicked
                        },
                    },
                })
            }
        }
    }

    fn dispatch_join(&mut self, joined: Option<SessionTaskJoin>) -> Option<LocalSessionDispatch> {
        match joined? {
            Ok((task_id, (session_id, cause))) => {
                self.task_sessions.remove(&task_id);
                self.remove_session(session_id);
                Some(LocalSessionDispatch::TaskJoined { session_id, cause })
            }
            Err(error) => {
                let session_id = self.task_sessions.remove(&error.id());
                let kind = if error.is_cancelled() {
                    LocalSessionTaskFailureKind::Cancelled
                } else {
                    LocalSessionTaskFailureKind::Panicked
                };
                if let Some(session_id) = session_id {
                    self.remove_session(session_id);
                }
                Some(LocalSessionDispatch::TaskFailed {
                    failure: LocalSessionTaskFailure { session_id, kind },
                })
            }
        }
    }

    fn disconnect_session(&mut self, session_id: Id32, mut session: ServerSession) {
        if let Some(executor) = &self.executor {
            let executor = Arc::clone(executor);
            let task = self.requests.spawn_blocking(move || {
                executor.disconnect(&mut session);
                (session_id, None)
            });
            self.request_sessions.insert(task.id(), session_id);
        } else {
            session.disconnect();
        }
    }

    fn begin_close(&mut self, session_id: Id32) {
        let session = self.sessions.get_mut(&session_id).and_then(|slot| {
            slot.closing = true;
            slot.pending_response = false;
            slot.io.close();
            slot.session.take()
        });
        if let Some(session) = session {
            self.disconnect_session(session_id, session);
        }
    }

    fn remove_session(&mut self, session_id: Id32) {
        if let Some(mut slot) = self.sessions.remove(&session_id) {
            slot.io.close();
            if let Some(session) = slot.session.take() {
                self.disconnect_session(session_id, session);
            }
        }
    }
}

fn independently_executable(message: &WireMessage) -> bool {
    matches!(message, WireMessage::Request(envelope) if !matches!(envelope.request,
        Request::Lifecycle(_) | Request::InstallationConfiguration | Request::UpdateInstallationConfiguration(_)))
}
