//! Reconnecting local-client mapping and the single TUI effect executor.

use std::{
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use hq_local_api::{
    BlockingClientError, ClientConnectionState, ClientEvent,
    protocol::v1::{AuthoritativeSnapshotDto, ConversationKeyDto, Id32, SnapshotItem},
};
use hq_tui::{
    EffectId, UiConnectionState, UiEffect, UiEvent, UiFailure, UiRow, UiRowState, UiSection,
    UiSnapshot, UiTimerKind,
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

    /// Polls subscribed invalidation and reconnect observations for a bounded interval.
    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation>;
}

/// Ordinary local-API implementation of the TUI client capability.
pub struct LocalTuiClient {
    client: LocalNodeEventClient,
    observed_connection: Option<ClientConnectionState>,
}

impl LocalTuiClient {
    /// Wraps one already-ready subscribed ordinary local API client.
    pub const fn new(client: LocalNodeEventClient) -> Self {
        Self {
            client,
            observed_connection: None,
        }
    }
}

impl TuiClientPort for LocalTuiClient {
    fn load_snapshot(&mut self, section: UiSection) -> Result<UiSnapshot, UiFailure> {
        self.client
            .snapshot()
            .map(|snapshot| tui_snapshot(section, snapshot))
            .map_err(|error| client_failure(&error))
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
    LoadSnapshot { id: EffectId, section: UiSection },
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
            | UiEvent::SnapshotFailed { effect_id, .. } => Some(*effect_id),
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
            UiSection::Inbox,
            SnapshotItem::Conversation {
                key, open_messages, ..
            },
        ) if open_messages > 0 => {
            let (id, title) = conversation_identity(key);
            Some(UiRow {
                id,
                title,
                detail: format!("{open_messages} open messages"),
                state: UiRowState::Open,
            })
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
        ) => {
            let title = match names.as_slice() {
                [name] => terminal_text(name),
                [] => format!("Agent {}", short_id(agent_id)),
                _ => format!("Conflicted agent {}", short_id(agent_id)),
            };
            let state = if names.len() > 1 {
                UiRowState::Attention
            } else if !retirements.is_empty() {
                UiRowState::Archived
            } else if runnable {
                UiRowState::Open
            } else {
                UiRowState::Waiting
            };
            Some(UiRow {
                id: full_id(agent_id),
                title,
                detail: terminal_text(&lifecycle),
                state,
            })
        }
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
        }),
        _ => None,
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
