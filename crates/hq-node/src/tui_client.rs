//! Reconnecting local-client mapping and the single TUI effect executor.

use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use hq_local_api::{
    BlockingClientError, ClientConnectionState, ClientEvent,
    protocol::v1::{
        ActivityStatusDto, AuthoritativeSnapshotDto, ConversationEntryDto, ConversationKeyDto,
        ConversationMessageDto, ConversationPageRequest, Id32, MessagePurposeDto,
        PresentationKindDto, Request, ResponseResult, SnapshotItem,
    },
};
use hq_tui::{
    EffectId, UiActivityStatus, UiConnectionState, UiConversationEntry, UiConversationEntryKind,
    UiConversationPage, UiEffect, UiEvent, UiFailure, UiMessageState, UiRow, UiRowKind, UiRowState,
    UiSection, UiSnapshot, UiTechnicalSection, UiTimerKind,
};

use crate::{LocalNodeClientError, LocalNodeEventClient};

const CLIENT_COMMAND_CAPACITY: usize = 8;
const CLIENT_EVENT_CAPACITY: usize = 16;
const COMMAND_WAIT: Duration = Duration::from_millis(10);
const CLIENT_POLL_WAIT: Duration = Duration::from_millis(25);

/// Monotonic clock capability used only by the effect executor's timer queue.
pub trait TuiClock {
    /// Returns elapsed monotonic time from an arbitrary fixed origin.
    fn now(&self) -> Duration;
}

/// Process monotonic clock for the installed terminal shell.
#[derive(Clone, Debug)]
pub struct MonotonicTuiClock {
    origin: Instant,
}

impl Default for MonotonicTuiClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl TuiClock for MonotonicTuiClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// Closed observation emitted by a subscribed TUI client port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiClientObservation {
    /// A later authoritative revision is available.
    Invalidated {
        /// Greatest observed revision.
        revision: u64,
    },
    /// A generation-scoped reconnect state changed.
    Connection {
        /// Monotonic local-client connection generation.
        generation: u64,
        /// Presentation-safe connection state.
        state: UiConnectionState,
    },
    /// A generation-scoped client failure occurred while reconnect remains possible.
    Failure {
        /// Monotonic local-client connection generation.
        generation: u64,
        /// Stable actionable failure.
        failure: UiFailure,
    },
}

/// Capability boundary consumed by the worker-owned effect executor.
pub trait TuiClientPort: Send {
    /// Loads and maps one complete authoritative snapshot for the exact requested section.
    fn load_snapshot(&mut self, section: UiSection) -> Result<UiSnapshot, UiFailure>;

    /// Loads one bounded reducer-ordered page for an exact snapshot row identity.
    fn load_conversation(
        &mut self,
        row_id: &str,
        cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure>;

    /// Polls subscribed invalidation and reconnect observations for a bounded interval.
    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation>;
}

/// Ordinary local-API implementation of the TUI client capability.
pub struct LocalTuiClient {
    client: LocalNodeEventClient,
    observed_connection: Option<ClientConnectionState>,
    conversation_keys: BTreeMap<String, ConversationKeyDto>,
}

impl LocalTuiClient {
    /// Wraps one already-ready subscribed ordinary local API client.
    pub const fn new(client: LocalNodeEventClient) -> Self {
        Self {
            client,
            observed_connection: None,
            conversation_keys: BTreeMap::new(),
        }
    }
}

impl TuiClientPort for LocalTuiClient {
    fn load_snapshot(&mut self, section: UiSection) -> Result<UiSnapshot, UiFailure> {
        let snapshot = self
            .client
            .snapshot()
            .map_err(|error| client_failure(&error))?;
        self.conversation_keys = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::Conversation { key, .. } => {
                    let (row_id, _) = conversation_identity(key.clone());
                    Some((row_id, key.clone()))
                }
                _ => None,
            })
            .collect();
        Ok(tui_snapshot(section, snapshot))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        let key = self
            .conversation_keys
            .get(row_id)
            .cloned()
            .ok_or_else(|| UiFailure {
                code: "conversation_stale".to_owned(),
                action: "reload the authoritative mailbox snapshot".to_owned(),
            })?;
        let request = ConversationPageRequest::new(key, 100, cursor).map_err(|_| UiFailure {
            code: "conversation_page_invalid".to_owned(),
            action: "reload the authoritative mailbox snapshot".to_owned(),
        })?;
        match self
            .client
            .request(Request::ConversationPage(request))
            .map_err(|error| client_failure(&error))?
        {
            ClientEvent::Response {
                result: ResponseResult::ConversationPage(page),
                ..
            } => Ok(tui_conversation_page(row_id, page)),
            _ => Err(UiFailure {
                code: "conversation_response_invalid".to_owned(),
                action: "reload the authoritative mailbox snapshot".to_owned(),
            }),
        }
    }

    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation> {
        let result = self.client.poll_event(wait);
        let state = self.client.connection_state();
        let mut observations = Vec::new();
        if self.observed_connection != Some(state) {
            self.observed_connection = Some(state);
            let (generation, state) = connection_observation(state);
            observations.push(TuiClientObservation::Connection { generation, state });
        }
        match result {
            Ok(Some(ClientEvent::Snapshot(snapshot))) => {
                observations.push(TuiClientObservation::Invalidated {
                    revision: snapshot.revision,
                });
            }
            Ok(Some(ClientEvent::IncompatibleVersion) | None) => {}
            Ok(Some(
                ClientEvent::Mutation(_)
                | ClientEvent::ProjectCommand { .. }
                | ClientEvent::AgentRetirement { .. }
                | ClientEvent::AgentSession { .. }
                | ClientEvent::Response { .. }
                | ClientEvent::RequestLost(_)
                | ClientEvent::Error { .. },
            )) => observations.push(TuiClientObservation::Failure {
                generation: connection_generation(state),
                failure: UiFailure {
                    code: "unexpected_local_client_event".to_owned(),
                    action: "waiting for a fresh authoritative snapshot".to_owned(),
                },
            }),
            Err(error) => observations.push(TuiClientObservation::Failure {
                generation: connection_generation(state),
                failure: client_failure(&error),
            }),
        }
        observations
    }
}

/// Closed effect-executor lifecycle or bounded-queue failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiExecutorError {
    /// The client worker thread could not be created.
    WorkerSpawn,
    /// The bounded worker command queue is full or closed.
    WorkerUnavailable,
    /// One effect identity was scheduled more than once.
    DuplicateEffectIdentity,
    /// A timer deadline overflowed the supplied monotonic clock domain.
    TimerDeadlineOverflow,
    /// The joined client worker panicked.
    WorkerPanicked,
}

impl std::fmt::Display for TuiExecutorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "TUI effect executor failed: {self:?}")
    }
}

impl std::error::Error for TuiExecutorError {}

struct ScheduledTimer {
    id: EffectId,
    kind: UiTimerKind,
    deadline: Duration,
}

enum WorkerCommand {
    LoadSnapshot {
        id: EffectId,
        section: UiSection,
    },
    LoadConversation {
        id: EffectId,
        row_id: String,
        cursor: Option<String>,
    },
    Shutdown,
}

/// Single bounded executor for client, timer, redraw, and exit effects.
pub struct TuiEffectExecutor<C: TuiClock> {
    clock: C,
    commands: SyncSender<WorkerCommand>,
    events: Receiver<UiEvent>,
    worker: Option<JoinHandle<()>>,
    timers: Vec<ScheduledTimer>,
    outstanding_snapshots: Vec<EffectId>,
    redraw_pending: bool,
    exit_requested: bool,
}

impl<C: TuiClock> TuiEffectExecutor<C> {
    /// Starts one named worker that exclusively owns the supplied client capability.
    pub fn spawn<P: TuiClientPort + 'static>(
        client: P,
        clock: C,
    ) -> Result<Self, TuiExecutorError> {
        let (commands, command_receiver) = mpsc::sync_channel(CLIENT_COMMAND_CAPACITY);
        let (event_sender, events) = mpsc::sync_channel(CLIENT_EVENT_CAPACITY);
        let worker = thread::Builder::new()
            .name("hq-tui-client".to_owned())
            .spawn(move || client_worker(client, &command_receiver, &event_sender))
            .map_err(|_| TuiExecutorError::WorkerSpawn)?;
        Ok(Self {
            clock,
            commands,
            events,
            worker: Some(worker),
            timers: Vec::new(),
            outstanding_snapshots: Vec::new(),
            redraw_pending: false,
            exit_requested: false,
        })
    }

    /// Executes ordered pure-model effects without changing the model.
    pub fn execute(
        &mut self,
        effects: impl IntoIterator<Item = UiEffect>,
    ) -> Result<(), TuiExecutorError> {
        for effect in effects {
            match effect {
                UiEffect::LoadSnapshot { id, section } => {
                    if self.effect_is_outstanding(id) {
                        return Err(TuiExecutorError::DuplicateEffectIdentity);
                    }
                    self.commands
                        .try_send(WorkerCommand::LoadSnapshot { id, section })
                        .map_err(|error| match error {
                            TrySendError::Full(_) | TrySendError::Disconnected(_) => {
                                TuiExecutorError::WorkerUnavailable
                            }
                        })?;
                    self.outstanding_snapshots.push(id);
                }
                UiEffect::LoadConversation { id, row_id, cursor } => {
                    if self.effect_is_outstanding(id) {
                        return Err(TuiExecutorError::DuplicateEffectIdentity);
                    }
                    self.commands
                        .try_send(WorkerCommand::LoadConversation { id, row_id, cursor })
                        .map_err(|error| match error {
                            TrySendError::Full(_) | TrySendError::Disconnected(_) => {
                                TuiExecutorError::WorkerUnavailable
                            }
                        })?;
                    self.outstanding_snapshots.push(id);
                }
                UiEffect::ScheduleTimer { id, kind, after } => {
                    if self.effect_is_outstanding(id) {
                        return Err(TuiExecutorError::DuplicateEffectIdentity);
                    }
                    let deadline = self
                        .clock
                        .now()
                        .checked_add(after)
                        .ok_or(TuiExecutorError::TimerDeadlineOverflow)?;
                    self.timers.push(ScheduledTimer { id, kind, deadline });
                    self.timers.sort_by_key(|timer| {
                        (timer.deadline, timer.id, timer_kind_order(timer.kind))
                    });
                }
                UiEffect::RequestRedraw => self.redraw_pending = true,
                UiEffect::Exit => self.exit_requested = true,
            }
        }
        Ok(())
    }

    /// Returns one ready worker or timer event without blocking.
    pub fn poll_event(&mut self) -> Option<UiEvent> {
        match self.events.try_recv() {
            Ok(event) => {
                self.complete_snapshot_identity(&event);
                return Some(event);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return None,
        }
        let now = self.clock.now();
        if self
            .timers
            .first()
            .is_some_and(|timer| timer.deadline <= now)
        {
            let timer = self.timers.remove(0);
            return Some(UiEvent::TimerElapsed {
                effect_id: timer.id,
            });
        }
        None
    }

    /// Takes one coalesced redraw request.
    pub fn take_redraw_request(&mut self) -> bool {
        std::mem::take(&mut self.redraw_pending)
    }

    /// Reports whether an exit effect has been observed.
    pub const fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    /// Bounds a shell wait by the next scheduled timer.
    pub fn time_until_event(&self, maximum: Duration) -> Duration {
        self.timers.first().map_or(maximum, |timer| {
            timer.deadline.saturating_sub(self.clock.now()).min(maximum)
        })
    }

    /// Stops and joins the worker, draining bounded results while it exits.
    pub fn shutdown(&mut self) -> Result<(), TuiExecutorError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        let mut command = WorkerCommand::Shutdown;
        loop {
            match self.commands.try_send(command) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(returned)) => {
                    command = returned;
                    while self.events.try_recv().is_ok() {}
                    if worker.is_finished() {
                        break;
                    }
                    thread::yield_now();
                }
            }
        }
        while !worker.is_finished() {
            while self.events.try_recv().is_ok() {}
            thread::yield_now();
        }
        worker.join().map_err(|_| TuiExecutorError::WorkerPanicked)
    }

    fn effect_is_outstanding(&self, id: EffectId) -> bool {
        self.outstanding_snapshots.contains(&id) || self.timers.iter().any(|timer| timer.id == id)
    }

    fn complete_snapshot_identity(&mut self, event: &UiEvent) {
        let completed = match event {
            UiEvent::SnapshotLoaded { effect_id, .. }
            | UiEvent::SnapshotFailed { effect_id, .. }
            | UiEvent::ConversationLoaded { effect_id, .. }
            | UiEvent::ConversationFailed { effect_id, .. } => Some(*effect_id),
            UiEvent::Started
            | UiEvent::Input(_)
            | UiEvent::Resized(_)
            | UiEvent::TimerElapsed { .. }
            | UiEvent::Invalidated { .. }
            | UiEvent::ConnectionObserved { .. }
            | UiEvent::ClientFailed { .. } => None,
        };
        if let Some(completed) = completed {
            self.outstanding_snapshots
                .retain(|candidate| *candidate != completed);
        }
    }
}

impl<C: TuiClock> Drop for TuiEffectExecutor<C> {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn client_worker<P: TuiClientPort>(
    mut client: P,
    commands: &Receiver<WorkerCommand>,
    events: &SyncSender<UiEvent>,
) {
    loop {
        match commands.recv_timeout(COMMAND_WAIT) {
            Ok(WorkerCommand::LoadSnapshot { id, section }) => {
                let event = match client.load_snapshot(section) {
                    Ok(snapshot) => UiEvent::SnapshotLoaded {
                        effect_id: id,
                        snapshot,
                    },
                    Err(failure) => UiEvent::SnapshotFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            Ok(WorkerCommand::LoadConversation { id, row_id, cursor }) => {
                let event = match client.load_conversation(&row_id, cursor) {
                    Ok(page) => UiEvent::ConversationLoaded {
                        effect_id: id,
                        page,
                    },
                    Err(failure) => UiEvent::ConversationFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            Ok(WorkerCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                for observation in client.poll(CLIENT_POLL_WAIT) {
                    let event = match observation {
                        TuiClientObservation::Invalidated { revision } => {
                            UiEvent::Invalidated { revision }
                        }
                        TuiClientObservation::Connection { generation, state } => {
                            UiEvent::ConnectionObserved { generation, state }
                        }
                        TuiClientObservation::Failure {
                            generation,
                            failure,
                        } => UiEvent::ClientFailed {
                            generation,
                            failure,
                        },
                    };
                    if events.send(event).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Maps one authoritative local API snapshot into passive section-specific presentation rows.
pub fn tui_snapshot(section: UiSection, snapshot: AuthoritativeSnapshotDto) -> UiSnapshot {
    let rows = snapshot
        .items
        .into_iter()
        .filter_map(|item| snapshot_row(section, item))
        .collect();
    UiSnapshot {
        section,
        revision: snapshot.revision,
        rows,
    }
}

fn snapshot_row(section: UiSection, item: SnapshotItem) -> Option<UiRow> {
    match (section, item) {
        (
            section @ (UiSection::Inbox | UiSection::Sent | UiSection::Archived),
            SnapshotItem::Conversation {
                key,
                open_messages,
                archived_messages,
                sent_messages,
                ..
            },
        ) if match section {
            UiSection::Inbox => open_messages > 0,
            UiSection::Sent => sent_messages > 0,
            UiSection::Archived => archived_messages > 0,
            UiSection::Agents | UiSection::Projects => false,
        } =>
        {
            conversation_row(
                section,
                key,
                open_messages,
                sent_messages,
                archived_messages,
            )
        }
        (
            UiSection::Inbox,
            SnapshotItem::IncompleteMessage {
                message_id,
                content,
                missing_dependencies,
                unusable_dependencies,
                ..
            },
        ) => Some(UiRow {
            id: full_id(message_id),
            title: terminal_text(&content),
            detail: format!(
                "{} missing · {} unusable dependencies",
                missing_dependencies.len(),
                unusable_dependencies.len()
            ),
            state: UiRowState::Attention,
            kind: UiRowKind::Diagnostic,
        }),
        (UiSection::Inbox, SnapshotItem::IncompleteMessagesTruncated) => Some(UiRow {
            id: "incomplete-messages-truncated".to_owned(),
            title: "Additional incomplete messages".to_owned(),
            detail: "reload after causal history synchronizes".to_owned(),
            state: UiRowState::Attention,
            kind: UiRowKind::Diagnostic,
        }),
        (
            UiSection::Agents,
            SnapshotItem::Agent {
                agent_id,
                names,
                retirements,
                lifecycle,
                runnable,
                ..
            },
        ) => Some(agent_row(
            agent_id,
            &names,
            retirements.is_empty(),
            &lifecycle,
            runnable,
        )),
        (
            UiSection::Projects,
            SnapshotItem::Project {
                project_id,
                name,
                lifecycle,
                archived,
                claimable,
                ..
            },
        ) => Some(UiRow {
            id: full_id(project_id),
            title: terminal_text(&name),
            detail: terminal_text(&lifecycle),
            state: if archived {
                UiRowState::Archived
            } else if !claimable {
                UiRowState::Attention
            } else {
                UiRowState::Open
            },
            kind: UiRowKind::Project,
        }),
        _ => None,
    }
}

fn agent_row(
    agent_id: Id32,
    names: &[String],
    active: bool,
    lifecycle: &str,
    runnable: bool,
) -> UiRow {
    let title = match names {
        [name] => terminal_text(name),
        [] => format!("Agent {}", short_id(agent_id)),
        _ => format!("Conflicted agent {}", short_id(agent_id)),
    };
    let state = if names.len() > 1 {
        UiRowState::Attention
    } else if !active {
        UiRowState::Archived
    } else if runnable {
        UiRowState::Open
    } else {
        UiRowState::Waiting
    };
    UiRow {
        id: full_id(agent_id),
        title,
        detail: terminal_text(lifecycle),
        state,
        kind: UiRowKind::Agent,
    }
}

fn conversation_row(
    section: UiSection,
    key: ConversationKeyDto,
    open_messages: u32,
    sent_messages: u32,
    archived_messages: u32,
) -> Option<UiRow> {
    let (id, title) = conversation_identity(key);
    let (count, label, state) = match section {
        UiSection::Inbox => (open_messages, "open messages", UiRowState::Open),
        UiSection::Sent => (sent_messages, "sent messages", UiRowState::Waiting),
        UiSection::Archived => (archived_messages, "archived messages", UiRowState::Archived),
        UiSection::Agents | UiSection::Projects => return None,
    };
    Some(UiRow {
        id,
        title,
        detail: format!("{count} {label}"),
        state,
        kind: UiRowKind::Conversation,
    })
}

/// Maps one bounded reducer-ordered local-API page into passive TUI presentation.
pub fn tui_conversation_page(
    row_id: &str,
    page: hq_local_api::protocol::v1::ConversationPageDto,
) -> UiConversationPage {
    UiConversationPage {
        row_id: row_id.to_owned(),
        entries: page.items.into_iter().map(tui_conversation_entry).collect(),
        next_cursor: page.next_cursor,
    }
}

fn tui_conversation_entry(entry: ConversationEntryDto) -> UiConversationEntry {
    match entry {
        ConversationEntryDto::Message(message) => tui_message_entry(*message),
        ConversationEntryDto::Activity {
            fact_id,
            sequence,
            status,
            content,
            truncated,
        } => {
            let status = tui_activity_status(status);
            UiConversationEntry {
                id: full_id(fact_id),
                kind: UiConversationEntryKind::Activity,
                content: terminal_text(&content),
                summary: format!("activity · {}", activity_status_label(&status)),
                message_state: None,
                technical: vec![UiTechnicalSection::Activity {
                    sequence,
                    status,
                    truncated,
                }],
            }
        }
    }
}

fn tui_activity_status(status: ActivityStatusDto) -> UiActivityStatus {
    match status {
        ActivityStatusDto::Snapshot => UiActivityStatus::Snapshot,
        ActivityStatusDto::Running => UiActivityStatus::Running,
        ActivityStatusDto::Succeeded => UiActivityStatus::Succeeded,
        ActivityStatusDto::Failed { reason } => UiActivityStatus::Failed {
            reason: terminal_text(&reason),
        },
        ActivityStatusDto::Interrupted => UiActivityStatus::Interrupted,
    }
}

const fn activity_status_label(status: &UiActivityStatus) -> &str {
    match status {
        UiActivityStatus::Snapshot => "snapshot",
        UiActivityStatus::Running => "running",
        UiActivityStatus::Succeeded => "succeeded",
        UiActivityStatus::Failed { .. } => "failed",
        UiActivityStatus::Interrupted => "interrupted",
    }
}

fn tui_message_entry(message: ConversationMessageDto) -> UiConversationEntry {
    let state = if message.rejected {
        UiMessageState::Rejected
    } else if message.open {
        UiMessageState::Open
    } else {
        UiMessageState::Archived
    };
    let purpose = message_purpose_label(message.purpose).to_owned();
    let presentation = presentation_label(message.presentation).to_owned();
    let sender = mailbox_address(message.sender_installation, message.sender_mailbox);
    let recipient = message
        .recipient_installation
        .zip(message.recipient_mailbox)
        .map(|(installation, mailbox)| mailbox_address(installation, mailbox));
    UiConversationEntry {
        id: full_id(message.fact_id),
        kind: UiConversationEntryKind::Message,
        content: terminal_text(&message.content),
        summary: format!("{purpose} · {}", short_id(message.sender_mailbox)),
        message_state: Some(state),
        technical: vec![
            UiTechnicalSection::Routing { sender, recipient },
            UiTechnicalSection::Semantics {
                purpose,
                presentation,
                provider: message
                    .correlation_provider
                    .map(|value| terminal_text(&value)),
                session: message
                    .correlation_session
                    .map(|value| terminal_text(&value)),
                operation: message.correlation_operation.map(full_id),
                project: message.project_id.map(full_id),
            },
            UiTechnicalSection::Evidence {
                message_id: full_id(message.message_id),
                thread_id: full_id(message.thread_id),
                state_frontier: message.state_frontier.into_iter().map(full_id).collect(),
                peer_received_by: message.peer_received_by.into_iter().map(full_id).collect(),
                root_fact: message.root_fact.map(full_id),
                root_message: message.root_message.map(full_id),
                ready_answer: message.ready_answer,
                thread_cancelled: message.thread_cancelled,
            },
        ],
    }
}

fn mailbox_address(installation: Id32, mailbox: Id32) -> String {
    format!("{}:{}", full_id(installation), full_id(mailbox))
}

const fn message_purpose_label(purpose: MessagePurposeDto) -> &'static str {
    match purpose {
        MessagePurposeDto::Question => "question",
        MessagePurposeDto::Asynchronous => "asynchronous",
        MessagePurposeDto::ProjectOutput => "project output",
    }
}

const fn presentation_label(presentation: PresentationKindDto) -> &'static str {
    match presentation {
        PresentationKindDto::Message => "message",
        PresentationKindDto::FinalAnswer => "final answer",
        PresentationKindDto::Status => "status",
    }
}

fn conversation_identity(key: ConversationKeyDto) -> (String, String) {
    match key {
        ConversationKeyDto::Thread { thread, .. } => (
            format!("thread:{}", full_id(thread)),
            format!("Thread {}", short_id(thread)),
        ),
        ConversationKeyDto::ProviderSession {
            counterparty_mailbox,
            provider,
            session,
            ..
        } => (
            format!(
                "session:{}:{provider}:{session}",
                full_id(counterparty_mailbox)
            ),
            format!("{} · {}", terminal_text(&provider), terminal_text(&session)),
        ),
    }
}

fn terminal_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn full_id(id: Id32) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in id.bytes() {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn short_id(id: Id32) -> String {
    full_id(id).chars().take(12).collect()
}

const fn connection_observation(state: ClientConnectionState) -> (u64, UiConnectionState) {
    match state {
        ClientConnectionState::Idle => (0, UiConnectionState::Disconnected),
        ClientConnectionState::Connecting(generation)
        | ClientConnectionState::Negotiating(generation) => (
            generation.value(),
            if generation.value() == 1 {
                UiConnectionState::Connecting
            } else {
                UiConnectionState::Reconnecting
            },
        ),
        ClientConnectionState::Active(generation) => (generation.value(), UiConnectionState::Ready),
        ClientConnectionState::Incompatible(generation) => {
            (generation.value(), UiConnectionState::Incompatible)
        }
    }
}

const fn connection_generation(state: ClientConnectionState) -> u64 {
    connection_observation(state).0
}

fn client_failure(error: &LocalNodeClientError) -> UiFailure {
    let (code, action) = match error {
        LocalNodeClientError::Execution(BlockingClientError::Incompatible) => (
            "local_api_incompatible",
            "install a compatible HQ client and node",
        ),
        LocalNodeClientError::Coordinator(_)
        | LocalNodeClientError::Launcher(_)
        | LocalNodeClientError::RuntimePath
        | LocalNodeClientError::Transport(_)
        | LocalNodeClientError::Client
        | LocalNodeClientError::Execution(
            BlockingClientError::InvalidDeadline
            | BlockingClientError::Client(_)
            | BlockingClientError::Deadline
            | BlockingClientError::ConnectionAttemptsExhausted
            | BlockingClientError::ResponseLost,
        ) => ("local_client_unavailable", "waiting to reconnect"),
    };
    UiFailure {
        code: code.to_owned(),
        action: action.to_owned(),
    }
}

const fn timer_kind_order(kind: UiTimerKind) -> u8 {
    match kind {
        UiTimerKind::PeriodicRefresh => 0,
        UiTimerKind::RetrySnapshot => 1,
    }
}
