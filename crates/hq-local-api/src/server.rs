//! Transport-independent local server session state machine.

use std::{collections::BTreeMap, collections::BTreeSet, error::Error, fmt};

use hq_application::{Application, ApplicationError, ApplicationPorts, ObserveRevisions};
use hq_domain::OperationId;

use crate::conversion::{
    agent_effect_from_v1, agent_effect_to_v1, agent_retirement_from_v1, agent_retirement_to_v1,
    empty_effect_to_v1, project_command_from_v1, project_command_to_v1, relay_effect_from_v1,
    relay_status_to_v1, resource_effect_from_v1, resource_effect_to_v1, state_health_to_v1,
    state_repair_to_v1, synchronization_effect_from_v1,
};
use crate::protocol::v1::{
    BuildMetadata, ErrorClass, ErrorResponse, Id32, LifecycleRequest, LifecycleStatus, Request,
    RequestEnvelope, ResponseEnvelope, ResponseResult, RevisionInvalidation, ServerHello, V1,
    VersionRange, VersionRejected, WireMessage, negotiate,
};
use crate::{
    RevisionHub, application_error_to_v1, canonical_evidence_from_v1, canonical_evidence_to_v1,
    evidence_ingest_to_v1, mutation_from_v1, mutation_to_v1, page_request_from_v1, page_to_v1,
    snapshot_to_v1, subscription_from_v1, topic_to_v1,
};

/// Node-owned lifecycle capability supplied by the later composition root.
pub trait LifecycleControl {
    /// Executes one local lifecycle query or control operation.
    fn lifecycle(&self, request: LifecycleRequest) -> Result<LifecycleStatus, ApplicationError>;
}

/// Opaque identity for one response write owned by one server session.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WriteTicket(u64);

impl WriteTicket {
    /// Returns the session-local diagnostic ticket number.
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// One message the transport must write before confirming its ticket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundMessage {
    ticket: WriteTicket,
    message: WireMessage,
}

impl OutboundMessage {
    /// Returns the ticket to confirm only after the full frame has been written.
    pub const fn ticket(&self) -> WriteTicket {
        self.ticket
    }

    /// Borrows the exact typed message to encode and write.
    pub const fn message(&self) -> &WireMessage {
        &self.message
    }

    /// Consumes the action into its ticket and exact wire message.
    pub fn into_parts(self) -> (WriteTicket, WireMessage) {
        (self.ticket, self.message)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AfterWrite {
    None,
    Activate(OperationId),
    Close,
}

/// Transport action required after one exact response write is confirmed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerWriteDisposition {
    /// The negotiated session remains available for another request.
    Continue,
    /// The final protocol response was delivered and the transport must close.
    Close,
}

/// Closed session-state failure requiring transport cleanup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerSessionError {
    /// The transport attempted to use a disconnected or closing session.
    Disconnected,
    /// A request or repeated hello violated negotiated protocol order.
    ProtocolOrder,
    /// The transport confirmed a ticket not owned by this session or already confirmed.
    UnknownWriteTicket,
    /// The transport attempted another request before confirming the prior response write.
    WritePending,
    /// The session-local write-ticket space was exhausted.
    TicketExhausted,
    /// Post-write observer activation failed and was cancelled.
    Activation(ApplicationError),
}

impl fmt::Display for ServerSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local server session failed: {self:?}")
    }
}

impl Error for ServerSessionError {}

/// Protocol state and revision registrations for one negotiated local connection.
pub struct ServerSession {
    hub: RevisionHub,
    build: BuildMetadata,
    session_id: Id32,
    negotiated: bool,
    closing: bool,
    disconnected: bool,
    next_ticket: u64,
    writes: BTreeMap<WriteTicket, AfterWrite>,
    subscriptions: BTreeSet<OperationId>,
}

impl fmt::Debug for ServerSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerSession")
            .field("session_id", &self.session_id)
            .field("negotiated", &self.negotiated)
            .field("closing", &self.closing)
            .field("disconnected", &self.disconnected)
            .field("pending_writes", &self.writes.len())
            .field("subscriptions", &self.subscriptions)
            .finish_non_exhaustive()
    }
}

impl ServerSession {
    /// Constructs one fresh unnegotiated session.
    pub const fn new(hub: RevisionHub, build: BuildMetadata, session_id: Id32) -> Self {
        Self {
            hub,
            build,
            session_id,
            negotiated: false,
            closing: false,
            disconnected: false,
            next_ticket: 1,
            writes: BTreeMap::new(),
            subscriptions: BTreeSet::new(),
        }
    }

    /// Handles one decoded message through capabilities borrowed only for this call.
    pub fn receive<P, L>(
        &mut self,
        message: WireMessage,
        application: &Application<P>,
        lifecycle: &L,
    ) -> Result<OutboundMessage, ServerSessionError>
    where
        P: ApplicationPorts,
        L: LifecycleControl,
    {
        self.ensure_open()?;
        if !self.writes.is_empty() {
            return Err(ServerSessionError::WritePending);
        }
        if !self.negotiated {
            return self.handle_hello(message);
        }
        let WireMessage::Request(envelope) = message else {
            return Err(ServerSessionError::ProtocolOrder);
        };
        self.handle_request(envelope, application, lifecycle)
    }

    /// Confirms a full successful frame write and performs its ordered post-write transition.
    pub fn confirm_written(
        &mut self,
        ticket: WriteTicket,
    ) -> Result<ServerWriteDisposition, ServerSessionError> {
        let after = self
            .writes
            .remove(&ticket)
            .ok_or(ServerSessionError::UnknownWriteTicket)?;
        match after {
            AfterWrite::None => Ok(ServerWriteDisposition::Continue),
            AfterWrite::Activate(operation_id) => {
                if let Err(error) = self.hub.activate_subscription(operation_id) {
                    let _ = self.hub.cancel_subscription(operation_id);
                    self.subscriptions.remove(&operation_id);
                    return Err(ServerSessionError::Activation(error));
                }
                Ok(ServerWriteDisposition::Continue)
            }
            AfterWrite::Close => {
                self.disconnect();
                Ok(ServerWriteDisposition::Close)
            }
        }
    }

    /// Takes one active coalesced invalidation owned by this session, if available.
    pub fn poll_invalidation(&mut self) -> Option<WireMessage> {
        if self.disconnected || !self.negotiated {
            return None;
        }
        for operation_id in self.subscriptions.iter().copied() {
            let Ok(Some(notice)) = self.hub.take(operation_id) else {
                continue;
            };
            let Ok(invalidation) = RevisionInvalidation::new(
                Id32::new(*operation_id.as_bytes()),
                notice.revision().value(),
                notice.topics().iter().copied().map(topic_to_v1).collect(),
                notice.full_snapshot(),
            ) else {
                continue;
            };
            return Some(WireMessage::Invalidation(invalidation));
        }
        None
    }

    /// Cancels all pending and active registrations and permanently closes this session.
    pub fn disconnect(&mut self) {
        if self.disconnected {
            return;
        }
        for operation_id in std::mem::take(&mut self.subscriptions) {
            let _ = self.hub.cancel_subscription(operation_id);
        }
        self.writes.clear();
        self.disconnected = true;
        self.closing = true;
    }

    /// Reports whether the protocol handshake completed successfully.
    pub const fn is_negotiated(&self) -> bool {
        self.negotiated
    }

    /// Returns the number of registrations owned by this session.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    fn handle_hello(
        &mut self,
        message: WireMessage,
    ) -> Result<OutboundMessage, ServerSessionError> {
        let WireMessage::ClientHello(hello) = message else {
            return Err(ServerSessionError::ProtocolOrder);
        };
        let Ok(server_versions) = VersionRange::new(V1, V1) else {
            return Err(ServerSessionError::ProtocolOrder);
        };
        if let Ok(version) = negotiate(hello.versions, server_versions) {
            self.negotiated = true;
            self.outbound(
                WireMessage::ServerHello(ServerHello::new(
                    version,
                    self.build.clone(),
                    self.session_id,
                )),
                AfterWrite::None,
            )
        } else {
            self.closing = true;
            self.outbound(
                WireMessage::VersionRejected(VersionRejected::new(
                    server_versions,
                    self.build.clone(),
                )),
                AfterWrite::Close,
            )
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_request<P, L>(
        &mut self,
        envelope: RequestEnvelope,
        application: &Application<P>,
        lifecycle: &L,
    ) -> Result<OutboundMessage, ServerSessionError>
    where
        P: ApplicationPorts,
        L: LifecycleControl,
    {
        let request_id = envelope.id;
        let mut after = AfterWrite::None;
        let result = match envelope.request {
            Request::Lifecycle(request) => {
                lifecycle.lifecycle(request).map(ResponseResult::Lifecycle)
            }
            Request::AuthoritativeSnapshot => application
                .authoritative_snapshot()
                .and_then(|snapshot| {
                    snapshot_to_v1(&snapshot).map_err(|_| internal_conversion_error())
                })
                .map(ResponseResult::AuthoritativeSnapshot),
            Request::ConversationPage(request) => page_request_from_v1(request)
                .map_err(|_| invalid_request_error())
                .and_then(|(key, limit, cursor)| {
                    application.conversation_entries(&key, limit, cursor.as_ref())
                })
                .and_then(|page| page_to_v1(&page).map_err(|_| internal_conversion_error()))
                .map(ResponseResult::ConversationPage),
            Request::Mutation(request) => mutation_from_v1(request)
                .map_err(|_| invalid_request_error())
                .and_then(|request| application.execute_mutation(request))
                .map(|completion| ResponseResult::Mutation(mutation_to_v1(completion.attempt()))),
            Request::CanonicalEvidence(request) => {
                let roots = request
                    .roots
                    .into_iter()
                    .map(|root| hq_domain::FactId::from_bytes(root.bytes()))
                    .collect::<BTreeSet<_>>();
                application
                    .canonical_evidence(
                        &roots,
                        crate::protocol::v1::MAX_CANONICAL_EVIDENCE_ITEMS,
                        crate::protocol::v1::MAX_CANONICAL_EVIDENCE_BYTES,
                    )
                    .and_then(|evidence| {
                        canonical_evidence_to_v1(&evidence).map_err(|_| internal_conversion_error())
                    })
                    .map(ResponseResult::CanonicalEvidence)
            }
            Request::IngestCanonicalEvidence(evidence) => canonical_evidence_from_v1(evidence)
                .map_err(|_| invalid_request_error())
                .and_then(|evidence| application.ingest_canonical_evidence(&evidence))
                .map(|outcomes| ResponseResult::EvidenceIngest(evidence_ingest_to_v1(&outcomes))),
            Request::ConfigureRelay(request) => relay_effect_from_v1(&request)
                .map_err(|_| invalid_request_error())
                .and_then(|request| application.configure_relay(&request))
                .map(|outcome| ResponseResult::EmptyEffect(empty_effect_to_v1(&outcome))),
            Request::Synchronize(request) => synchronization_effect_from_v1(&request)
                .map_err(|_| invalid_request_error())
                .and_then(|request| application.synchronize(&request))
                .map(|outcome| ResponseResult::EmptyEffect(empty_effect_to_v1(&outcome))),
            Request::RelayStatus => application
                .relay_status()
                .map(|status| ResponseResult::RelayStatus(relay_status_to_v1(&status))),
            Request::StateHealth => application
                .state_health()
                .map(|status| ResponseResult::StateHealth(state_health_to_v1(&status))),
            Request::RepairState { operation_id } => application
                .repair_state(OperationId::from_bytes(operation_id.bytes()))
                .map(|report| ResponseResult::StateRepair(state_repair_to_v1(&report))),
            Request::ControlAgentSession(request) => agent_effect_from_v1(&request)
                .map_err(|_| invalid_request_error())
                .and_then(|request| application.control_agent_session(&request))
                .map(|outcome| ResponseResult::AgentSession(agent_effect_to_v1(&outcome))),
            Request::InspectResource(request) => resource_effect_from_v1(&request)
                .map_err(|_| invalid_request_error())
                .and_then(|request| application.inspect_resource(&request))
                .map(|outcome| ResponseResult::ResourceInspection(resource_effect_to_v1(&outcome))),
            Request::ControlProject(request) => project_command_from_v1(*request)
                .map_err(|_| invalid_request_error())
                .and_then(|request| application.control_project(request))
                .map(|outcome| ResponseResult::ProjectCommand(project_command_to_v1(&outcome))),
            Request::RetireAgent(request) => application
                .retire_agent(agent_retirement_from_v1(*request))
                .map(|outcome| ResponseResult::AgentRetirement(agent_retirement_to_v1(&outcome))),
            Request::Subscribe(request) => {
                let subscription =
                    subscription_from_v1(request).map_err(|_| invalid_request_error());
                subscription.and_then(|subscription| {
                    let operation_id = subscription.operation_id();
                    self.hub.register_subscription(&subscription)?;
                    match application.authoritative_snapshot() {
                        Ok(snapshot) => {
                            if let Ok(snapshot) = snapshot_to_v1(&snapshot) {
                                self.subscriptions.insert(operation_id);
                                after = AfterWrite::Activate(operation_id);
                                Ok(ResponseResult::Subscription(
                                    crate::protocol::v1::SubscriptionAcknowledgement::new(
                                        Id32::new(*operation_id.as_bytes()),
                                        snapshot,
                                    ),
                                ))
                            } else {
                                let _ = self.hub.cancel_subscription(operation_id);
                                Err(internal_conversion_error())
                            }
                        }
                        Err(error) => {
                            let _ = self.hub.cancel_subscription(operation_id);
                            Err(error)
                        }
                    }
                })
            }
            Request::CancelSubscription { subscription_id } => {
                let operation_id = OperationId::from_bytes(subscription_id.bytes());
                self.hub.cancel_subscription(operation_id).map(|()| {
                    self.subscriptions.remove(&operation_id);
                    ResponseResult::Empty
                })
            }
        };

        let response = match result {
            Ok(result) => ResponseEnvelope::success(request_id, result),
            Err(error) => {
                after = AfterWrite::None;
                ResponseEnvelope::error(request_id, application_error_to_v1(error))
            }
        };
        self.outbound(WireMessage::Response(response), after)
    }

    fn outbound(
        &mut self,
        message: WireMessage,
        after: AfterWrite,
    ) -> Result<OutboundMessage, ServerSessionError> {
        let ticket = WriteTicket(self.next_ticket);
        self.next_ticket = self
            .next_ticket
            .checked_add(1)
            .ok_or(ServerSessionError::TicketExhausted)?;
        self.writes.insert(ticket, after);
        Ok(OutboundMessage { ticket, message })
    }

    fn ensure_open(&self) -> Result<(), ServerSessionError> {
        if self.disconnected || self.closing {
            Err(ServerSessionError::Disconnected)
        } else {
            Ok(())
        }
    }
}

impl Drop for ServerSession {
    fn drop(&mut self) {
        if self.disconnected {
            return;
        }
        for operation_id in std::mem::take(&mut self.subscriptions) {
            let _ = self.hub.cancel_subscription(operation_id);
        }
    }
}

fn invalid_request_error() -> ApplicationError {
    ApplicationError::new(hq_application::ApplicationErrorCode::InvalidRequest)
}

fn internal_conversion_error() -> ApplicationError {
    ApplicationError::new(hq_application::ApplicationErrorCode::InvariantViolation)
}

#[allow(dead_code)]
fn _pin_error_shape(_class: ErrorClass, _error: ErrorResponse) {}
