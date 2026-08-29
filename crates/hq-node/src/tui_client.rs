//! Reconnecting local-client mapping and the single TUI effect executor.

use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hq_local_api::{
    BlockingClientError, ClientConnectionState, ClientEvent,
    protocol::v1::{
        ActivityStatusDto, AuthoritativeSnapshotDto, ConversationEntryDto, ConversationKeyDto,
        ConversationMessageDto, ConversationPageRequest, Id32, MailboxCommandActionDto,
        MailboxCommandRequestDto, MailboxDraftDto, MailboxDraftSaveOutcomeDto,
        MailboxDraftSaveRequestDto, MailboxDraftTargetDto, MessagePurposeDto, MutationAttemptDto,
        MutationOutcomeDto, PresentationKindDto, Request, ResponseResult, SnapshotItem,
    },
};
use hq_tui::{
    EffectId, UiActivityStatus, UiAgent, UiAgentAction, UiAgentLifecycle, UiAgentMailbox,
    UiAgentSession, UiConnectionState, UiConversationEntry, UiConversationEntryKind,
    UiConversationPage, UiDirectTarget, UiEffect, UiEvent, UiFailure, UiMailboxAction,
    UiMailboxDraft, UiMailboxDraftTarget, UiManagedSessionAction, UiManagedSessionOutcome,
    UiManagedSessionResult, UiMessageState, UiMessageTarget, UiProject, UiProjectAction,
    UiProjectExternalWarning, UiProjectOutcome, UiProjectResource, UiProjectResourceCheck,
    UiProjectResourceConflict, UiProjectResult, UiRow, UiRowKind, UiRowState, UiSection,
    UiSnapshot, UiTechnicalSection, UiTimerKind,
};

use crate::{
    LocalNodeClientError, LocalNodeEventClient, StatePaths,
    local_client::{
        LocalManagedSessionCommand, LocalManagedSessionOutcome, LocalNamedAgentCommand,
        LocalProject, LocalProjectCommand, LocalProjectOutcome, execute_managed_session_command,
        execute_named_agent_command, execute_project_command, tui_named_agent_catalog,
        tui_project_catalog,
    },
};

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

    /// Loads one applicable durable draft, creating an empty draft when absent.
    fn open_draft(&mut self, target: UiMailboxDraftTarget)
    -> Result<UiMailboxDraft, TuiDraftError>;

    /// Autosaves one complete optimistic durable draft replacement.
    fn save_draft(&mut self, draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError>;

    /// Executes or reconciles one stable authoritative mailbox command.
    fn submit_mailbox_command(
        &mut self,
        draft: Option<UiMailboxDraft>,
        action: UiMailboxAction,
    ) -> Result<u64, UiFailure>;

    /// Executes or reconciles one stable named-agent administration command.
    fn submit_agent_command(&mut self, _action: UiAgentAction) -> Result<u64, UiFailure> {
        Err(UiFailure {
            code: "agent_command_unavailable".to_owned(),
            action: "use a client that supports named-agent administration".to_owned(),
        })
    }

    /// Executes or reconciles one stable provider-neutral managed-session command.
    fn submit_managed_session(
        &mut self,
        _action: UiManagedSessionAction,
    ) -> Result<UiManagedSessionResult, UiFailure> {
        Err(UiFailure {
            code: "managed_session_unavailable".to_owned(),
            action: "use a client that supports managed-session control".to_owned(),
        })
    }

    /// Executes or reconciles one stable project command.
    fn submit_project_command(
        &mut self,
        _action: UiProjectAction,
    ) -> Result<UiProjectResult, UiFailure> {
        Err(UiFailure {
            code: "project_command_unavailable".to_owned(),
            action: "use a client that supports project workflows".to_owned(),
        })
    }

    /// Polls subscribed invalidation and reconnect observations for a bounded interval.
    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation>;
}

/// Actionable draft failure with the current server value on optimistic conflict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiDraftError {
    /// Stable actionable failure.
    pub failure: UiFailure,
    /// Current server draft when a concurrent save won.
    pub current: Option<Box<UiMailboxDraft>>,
}

/// Ordinary local-API implementation of the TUI client capability.
pub struct LocalTuiClient {
    client: LocalNodeEventClient,
    state: StatePaths,
    observed_connection: Option<ClientConnectionState>,
    conversation_keys: BTreeMap<String, ConversationKeyDto>,
}

impl LocalTuiClient {
    /// Wraps one already-ready subscribed ordinary local API client.
    pub const fn new(client: LocalNodeEventClient, state: StatePaths) -> Self {
        Self {
            client,
            state,
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
        let projects = tui_project_catalog(&snapshot).map_err(|error| project_failure(&error))?;
        Ok(tui_snapshot_with_projects(section, snapshot, projects))
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

    fn open_draft(
        &mut self,
        target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        let ClientEvent::Response {
            result: ResponseResult::MailboxDrafts(drafts),
            ..
        } = self
            .client
            .request(Request::MailboxDrafts)
            .map_err(|error| draft_client_error(&error))?
        else {
            return Err(draft_protocol_error());
        };
        if let Some(draft) = drafts
            .into_iter()
            .find(|draft| tui_draft_target(&draft.target) == target)
        {
            return Ok(tui_draft(draft));
        }
        let request = MailboxDraftSaveRequestDto {
            draft_id: Id32::new(random_identity().map_err(|failure| TuiDraftError {
                failure,
                current: None,
            })?),
            target: mailbox_draft_target(&target),
            content: String::new(),
            expected_version: None,
        };
        match self
            .client
            .request(Request::SaveMailboxDraft(request))
            .map_err(|error| draft_client_error(&error))?
        {
            ClientEvent::Response {
                result: ResponseResult::MailboxDraftSave(MailboxDraftSaveOutcomeDto::Saved(draft)),
                ..
            } => Ok(tui_draft(draft)),
            ClientEvent::Response {
                result:
                    ResponseResult::MailboxDraftSave(MailboxDraftSaveOutcomeDto::Conflict(draft)),
                ..
            } => Err(TuiDraftError {
                failure: UiFailure {
                    code: "draft_conflict".to_owned(),
                    action: "reopen the current draft before editing".to_owned(),
                },
                current: Some(Box::new(tui_draft(draft))),
            }),
            _ => Err(draft_protocol_error()),
        }
    }

    fn save_draft(&mut self, draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        let request = MailboxDraftSaveRequestDto {
            draft_id: Id32::new(draft.draft_id),
            target: mailbox_draft_target(&draft.target),
            content: draft.content,
            expected_version: Some(draft.version),
        };
        match self
            .client
            .request(Request::SaveMailboxDraft(request))
            .map_err(|error| draft_client_error(&error))?
        {
            ClientEvent::Response {
                result: ResponseResult::MailboxDraftSave(MailboxDraftSaveOutcomeDto::Saved(draft)),
                ..
            } => Ok(tui_draft(draft)),
            ClientEvent::Response {
                result:
                    ResponseResult::MailboxDraftSave(MailboxDraftSaveOutcomeDto::Conflict(draft)),
                ..
            } => Err(TuiDraftError {
                failure: UiFailure {
                    code: "draft_conflict".to_owned(),
                    action: "edit the preserved text and retry against the current draft"
                        .to_owned(),
                },
                current: Some(Box::new(tui_draft(draft))),
            }),
            _ => Err(draft_protocol_error()),
        }
    }

    fn submit_mailbox_command(
        &mut self,
        draft: Option<UiMailboxDraft>,
        action: UiMailboxAction,
    ) -> Result<u64, UiFailure> {
        let command_id = Id32::new(random_identity()?);
        let message_id = Id32::new(random_identity()?);
        let action = match action {
            UiMailboxAction::Reply { target_message } => MailboxCommandActionDto::Reply {
                target_message: Id32::new(target_message),
                message_id,
            },
            UiMailboxAction::Direct {
                recipient_installation,
                recipient_mailbox,
            } => MailboxCommandActionDto::Direct {
                recipient_installation: Id32::new(recipient_installation),
                recipient_mailbox: Id32::new(recipient_mailbox),
                message_id,
            },
            UiMailboxAction::SelfNote => MailboxCommandActionDto::SelfNote { message_id },
            UiMailboxAction::Archive { target_message } => MailboxCommandActionDto::Archive {
                target_message: Id32::new(target_message),
            },
            UiMailboxAction::Restore { target_message } => MailboxCommandActionDto::Restore {
                target_message: Id32::new(target_message),
            },
        };
        let authored_at_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| UiFailure {
                code: "system_time_invalid".to_owned(),
                action: "correct the system clock and retry".to_owned(),
            })?
            .as_millis()
            .try_into()
            .map_err(|_| UiFailure {
                code: "system_time_invalid".to_owned(),
                action: "correct the system clock and retry".to_owned(),
            })?;
        let request = MailboxCommandRequestDto::new(
            command_id,
            draft.as_ref().map(|draft| Id32::new(draft.draft_id)),
            action,
            None,
            authored_at_millis,
            random_identity()?,
        );
        match self
            .client
            .mailbox_command(request)
            .map_err(|error| client_failure(&error))?
        {
            ClientEvent::Mutation(MutationAttemptDto::Completed {
                revision,
                outcome: MutationOutcomeDto::Committed,
                ..
            }) => Ok(revision),
            ClientEvent::Mutation(MutationAttemptDto::Completed {
                outcome: MutationOutcomeDto::Rejected { code, .. },
                ..
            }) => Err(UiFailure {
                action: if code == "mailbox_target_stale" {
                    "reselect the target; the draft text is preserved".to_owned()
                } else {
                    "correct the mailbox command and retry".to_owned()
                },
                code,
            }),
            _ => Err(UiFailure {
                code: "mailbox_command_uncertain".to_owned(),
                action: "keep the draft open while HQ reconciles the same command".to_owned(),
            }),
        }
    }

    fn submit_agent_command(&mut self, action: UiAgentAction) -> Result<u64, UiFailure> {
        let command = match action {
            UiAgentAction::Create { name } => LocalNamedAgentCommand::Create { name },
            UiAgentAction::RenameSession {
                agent_id,
                provider,
                session,
                display_name,
            } => LocalNamedAgentCommand::RenameSession {
                agent_id,
                provider,
                session,
                display_name,
            },
            UiAgentAction::Retire { agent_id, force } => {
                LocalNamedAgentCommand::Retire { agent_id, force }
            }
        };
        execute_named_agent_command(&self.state, command).map_err(|error| UiFailure {
            code: match error {
                crate::cli::CliError::AgentState => "agent_state_stale_or_uncertain",
                crate::cli::CliError::Arguments => "agent_action_invalid",
                _ => "agent_command_failed",
            }
            .to_owned(),
            action: match error {
                crate::cli::CliError::AgentState => {
                    "reload and reselect an active unconflicted agent or session".to_owned()
                }
                crate::cli::CliError::Arguments => {
                    "correct the agent name or session display name and retry".to_owned()
                }
                _ => "wait for the local node to recover, then retry".to_owned(),
            },
        })
    }

    fn submit_managed_session(
        &mut self,
        action: UiManagedSessionAction,
    ) -> Result<UiManagedSessionResult, UiFailure> {
        let command = match &action {
            UiManagedSessionAction::Start { agent_id, provider } => {
                LocalManagedSessionCommand::Start {
                    agent_id: *agent_id,
                    provider: provider.clone(),
                }
            }
            UiManagedSessionAction::Resume {
                agent_id,
                provider,
                session,
            } => LocalManagedSessionCommand::Resume {
                agent_id: *agent_id,
                provider: provider.clone(),
                session: session.clone(),
            },
            UiManagedSessionAction::Stop { agent_id, provider } => {
                LocalManagedSessionCommand::Stop {
                    agent_id: *agent_id,
                    provider: provider.clone(),
                }
            }
        };
        let result =
            execute_managed_session_command(&self.state, command).map_err(|error| UiFailure {
                code: match error {
                    crate::cli::CliError::Arguments => "managed_session_invalid",
                    crate::cli::CliError::AgentState => "managed_session_target_stale",
                    crate::cli::CliError::HarnessState => "managed_session_response_invalid",
                    _ => "managed_session_unavailable",
                }
                .to_owned(),
                action: match error {
                    crate::cli::CliError::Arguments => {
                        "correct the exact provider or session target and retry"
                    }
                    crate::cli::CliError::AgentState => {
                        "reload and reselect an active unconflicted named agent"
                    }
                    crate::cli::CliError::HarnessState => {
                        "reload durable sessions before retrying this operation"
                    }
                    _ => "wait for the local node to recover, then retry the same target",
                }
                .to_owned(),
            })?;
        let outcome = match result.outcome {
            LocalManagedSessionOutcome::Ready { session } => {
                UiManagedSessionOutcome::Ready { session }
            }
            LocalManagedSessionOutcome::Stopped => UiManagedSessionOutcome::Stopped,
            LocalManagedSessionOutcome::Rejected { category, code } => {
                UiManagedSessionOutcome::Rejected { category, code }
            }
            LocalManagedSessionOutcome::Uncertain { reconciliation_id } => {
                UiManagedSessionOutcome::Uncertain { reconciliation_id }
            }
        };
        Ok(UiManagedSessionResult {
            action,
            operation_id: result.operation_id,
            outcome,
        })
    }

    fn submit_project_command(
        &mut self,
        action: UiProjectAction,
    ) -> Result<UiProjectResult, UiFailure> {
        let command = local_project_command(&action);
        let result = execute_project_command(&self.state, command)
            .map_err(|error| project_failure(&error))?;
        let outcome = ui_project_outcome(result.outcome);
        Ok(UiProjectResult {
            action,
            command_id: result.command_id,
            operation_id: result.operation_id,
            project_id: result.project_id,
            outcome,
        })
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

fn random_identity() -> Result<[u8; 32], UiFailure> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| UiFailure {
        code: "entropy_unavailable".to_owned(),
        action: "restore operating-system randomness and retry".to_owned(),
    })?;
    Ok(bytes)
}

fn draft_client_error(error: &LocalNodeClientError) -> TuiDraftError {
    TuiDraftError {
        failure: client_failure(error),
        current: None,
    }
}

fn draft_protocol_error() -> TuiDraftError {
    TuiDraftError {
        failure: UiFailure {
            code: "draft_response_invalid".to_owned(),
            action: "reopen the draft from a fresh authoritative client".to_owned(),
        },
        current: None,
    }
}

fn mailbox_draft_target(target: &UiMailboxDraftTarget) -> MailboxDraftTargetDto {
    match target {
        UiMailboxDraftTarget::Reply { message_id } => MailboxDraftTargetDto::Reply {
            message_id: Id32::new(*message_id),
        },
        UiMailboxDraftTarget::Direct {
            installation_id,
            mailbox_id,
        } => MailboxDraftTargetDto::Direct {
            installation_id: Id32::new(*installation_id),
            mailbox_id: Id32::new(*mailbox_id),
        },
        UiMailboxDraftTarget::SelfNote => MailboxDraftTargetDto::SelfNote,
    }
}

fn tui_draft_target(target: &MailboxDraftTargetDto) -> UiMailboxDraftTarget {
    match target {
        MailboxDraftTargetDto::Reply { message_id } => UiMailboxDraftTarget::Reply {
            message_id: message_id.bytes(),
        },
        MailboxDraftTargetDto::Direct {
            installation_id,
            mailbox_id,
        } => UiMailboxDraftTarget::Direct {
            installation_id: installation_id.bytes(),
            mailbox_id: mailbox_id.bytes(),
        },
        MailboxDraftTargetDto::SelfNote => UiMailboxDraftTarget::SelfNote,
    }
}

fn tui_draft(draft: MailboxDraftDto) -> UiMailboxDraft {
    UiMailboxDraft {
        draft_id: draft.draft_id.bytes(),
        target: tui_draft_target(&draft.target),
        content: draft.content,
        version: draft.version,
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
    OpenDraft {
        id: EffectId,
        target: UiMailboxDraftTarget,
    },
    SaveDraft {
        id: EffectId,
        draft: UiMailboxDraft,
    },
    SubmitMailboxCommand {
        id: EffectId,
        draft: Option<UiMailboxDraft>,
        action: UiMailboxAction,
    },
    SubmitAgentCommand {
        id: EffectId,
        action: UiAgentAction,
    },
    SubmitManagedSession {
        id: EffectId,
        action: UiManagedSessionAction,
    },
    SubmitProjectCommand {
        id: EffectId,
        action: UiProjectAction,
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
                UiEffect::OpenDraft { id, target } => {
                    self.enqueue_client_effect(id, WorkerCommand::OpenDraft { id, target })?;
                }
                UiEffect::SaveDraft { id, draft } => {
                    self.enqueue_client_effect(id, WorkerCommand::SaveDraft { id, draft })?;
                }
                UiEffect::SubmitMailboxCommand { id, draft, action } => {
                    self.enqueue_client_effect(
                        id,
                        WorkerCommand::SubmitMailboxCommand { id, draft, action },
                    )?;
                }
                UiEffect::SubmitAgentCommand { id, action } => {
                    self.enqueue_client_effect(
                        id,
                        WorkerCommand::SubmitAgentCommand { id, action },
                    )?;
                }
                UiEffect::SubmitManagedSession { id, action } => {
                    self.enqueue_client_effect(
                        id,
                        WorkerCommand::SubmitManagedSession { id, action },
                    )?;
                }
                UiEffect::SubmitProjectCommand { id, action } => {
                    self.enqueue_client_effect(
                        id,
                        WorkerCommand::SubmitProjectCommand { id, action },
                    )?;
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
                    if kind == UiTimerKind::AutosaveDraft {
                        self.timers
                            .retain(|timer| timer.kind != UiTimerKind::AutosaveDraft);
                    }
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

    fn enqueue_client_effect(
        &mut self,
        id: EffectId,
        command: WorkerCommand,
    ) -> Result<(), TuiExecutorError> {
        if self.effect_is_outstanding(id) {
            return Err(TuiExecutorError::DuplicateEffectIdentity);
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) | TrySendError::Disconnected(_) => {
                    TuiExecutorError::WorkerUnavailable
                }
            })?;
        self.outstanding_snapshots.push(id);
        Ok(())
    }

    fn complete_snapshot_identity(&mut self, event: &UiEvent) {
        let completed = match event {
            UiEvent::SnapshotLoaded { effect_id, .. }
            | UiEvent::SnapshotFailed { effect_id, .. }
            | UiEvent::ConversationLoaded { effect_id, .. }
            | UiEvent::ConversationFailed { effect_id, .. }
            | UiEvent::DraftLoaded { effect_id, .. }
            | UiEvent::DraftSaved { effect_id, .. }
            | UiEvent::DraftFailed { effect_id, .. }
            | UiEvent::MailboxCommandCommitted { effect_id, .. }
            | UiEvent::MailboxCommandFailed { effect_id, .. }
            | UiEvent::AgentCommandCommitted { effect_id, .. }
            | UiEvent::AgentCommandFailed { effect_id, .. }
            | UiEvent::ManagedSessionCompleted { effect_id, .. }
            | UiEvent::ManagedSessionFailed { effect_id, .. }
            | UiEvent::ProjectCommandCompleted { effect_id, .. }
            | UiEvent::ProjectCommandFailed { effect_id, .. } => Some(*effect_id),
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

#[allow(clippy::too_many_lines)]
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
            Ok(WorkerCommand::OpenDraft { id, target }) => {
                let event = match client.open_draft(target) {
                    Ok(draft) => UiEvent::DraftLoaded {
                        effect_id: id,
                        draft,
                    },
                    Err(error) => UiEvent::DraftFailed {
                        effect_id: id,
                        failure: error.failure,
                        current: error.current.map(|draft| *draft),
                    },
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            Ok(WorkerCommand::SaveDraft { id, draft }) => {
                let event = match client.save_draft(draft) {
                    Ok(draft) => UiEvent::DraftSaved {
                        effect_id: id,
                        draft,
                    },
                    Err(error) => UiEvent::DraftFailed {
                        effect_id: id,
                        failure: error.failure,
                        current: error.current.map(|draft| *draft),
                    },
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            Ok(WorkerCommand::SubmitMailboxCommand { id, draft, action }) => {
                let event = match client.submit_mailbox_command(draft, action) {
                    Ok(revision) => UiEvent::MailboxCommandCommitted {
                        effect_id: id,
                        revision,
                    },
                    Err(failure) => UiEvent::MailboxCommandFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            Ok(WorkerCommand::SubmitAgentCommand { id, action }) => {
                let event = match client.submit_agent_command(action) {
                    Ok(revision) => UiEvent::AgentCommandCommitted {
                        effect_id: id,
                        revision,
                    },
                    Err(failure) => UiEvent::AgentCommandFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            Ok(WorkerCommand::SubmitManagedSession { id, action }) => {
                let event = match client.submit_managed_session(action) {
                    Ok(result) => UiEvent::ManagedSessionCompleted {
                        effect_id: id,
                        result,
                    },
                    Err(failure) => UiEvent::ManagedSessionFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if events.send(event).is_err() {
                    break;
                }
            }
            Ok(WorkerCommand::SubmitProjectCommand { id, action }) => {
                let event = match client.submit_project_command(action) {
                    Ok(result) => UiEvent::ProjectCommandCompleted {
                        effect_id: id,
                        result,
                    },
                    Err(failure) => UiEvent::ProjectCommandFailed {
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
    let projects = tui_project_catalog(&snapshot).unwrap_or_default();
    tui_snapshot_with_projects(section, snapshot, projects)
}

fn tui_snapshot_with_projects(
    section: UiSection,
    snapshot: AuthoritativeSnapshotDto,
    projects: Vec<LocalProject>,
) -> UiSnapshot {
    let agents = tui_named_agent_catalog(&snapshot)
        .into_iter()
        .map(|agent| UiAgent {
            agent_id: *agent.agent_id.as_bytes(),
            names: agent
                .names
                .into_iter()
                .map(|name| terminal_text(&name))
                .collect(),
            mailboxes: agent
                .mailboxes
                .into_iter()
                .map(|mailbox| UiAgentMailbox {
                    installation_id: *mailbox.installation_id().as_bytes(),
                    mailbox_id: *mailbox.mailbox_id().as_bytes(),
                })
                .collect(),
            lifecycle: match agent.lifecycle.as_str() {
                "active" => UiAgentLifecycle::Active,
                "retired" => UiAgentLifecycle::Retired,
                _ => UiAgentLifecycle::Conflicted,
            },
            runnable: agent.runnable,
            sessions: agent
                .sessions
                .into_iter()
                .map(|session| UiAgentSession {
                    provider: session.provider,
                    session: session.session,
                    mailbox: session.mailbox.map(|mailbox| UiAgentMailbox {
                        installation_id: *mailbox.installation_id().as_bytes(),
                        mailbox_id: *mailbox.mailbox_id().as_bytes(),
                    }),
                    conflicted: session.conflicted,
                    selected: session.selected,
                    name_resolved: session.name_resolved,
                    display_name: session.display_name.map(|name| terminal_text(&name)),
                })
                .collect(),
        })
        .collect();
    let projects = tui_projects(projects);
    let mut direct_targets = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Agent {
                names,
                mailboxes,
                retirements,
                ..
            } if retirements.is_empty() => match (names.as_slice(), mailboxes.as_slice()) {
                ([name], [mailbox]) => Some(UiDirectTarget {
                    installation_id: mailbox.installation_id.bytes(),
                    mailbox_id: mailbox.mailbox_id.bytes(),
                    label: terminal_text(name),
                }),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    direct_targets.sort_by(|left, right| {
        (&left.label, left.installation_id, left.mailbox_id).cmp(&(
            &right.label,
            right.installation_id,
            right.mailbox_id,
        ))
    });
    let rows = snapshot
        .items
        .into_iter()
        .filter_map(|item| snapshot_row(section, item))
        .collect();
    UiSnapshot {
        section,
        revision: snapshot.revision,
        rows,
        direct_targets,
        agents,
        projects,
    }
}

fn local_project_command(action: &UiProjectAction) -> LocalProjectCommand {
    match action {
        UiProjectAction::CreateExisting { name, brief, path } => {
            LocalProjectCommand::CreateExisting {
                name: name.clone(),
                brief: brief.clone(),
                path: path.clone(),
            }
        }
        UiProjectAction::CreateWorktree {
            name,
            brief,
            source,
            destination,
            branch,
            base,
        } => LocalProjectCommand::CreateWorktree {
            name: name.clone(),
            brief: brief.clone(),
            source: source.clone(),
            destination: destination.clone(),
            branch: branch.clone(),
            base: base.clone(),
        },
        UiProjectAction::SendInput {
            project_id,
            content,
        } => LocalProjectCommand::SendInput {
            project_id: *project_id,
            content: content.clone(),
        },
        UiProjectAction::PreviewAddResource {
            project_id,
            path,
            make_primary,
        } => LocalProjectCommand::PreviewAddResource {
            project_id: *project_id,
            path: path.clone(),
            make_primary: *make_primary,
        },
        UiProjectAction::AddResource {
            project_id,
            path,
            make_primary,
        } => LocalProjectCommand::AddResource {
            project_id: *project_id,
            path: path.clone(),
            make_primary: *make_primary,
        },
        UiProjectAction::PreviewReplaceResource {
            project_id,
            resource_id,
            path,
        } => LocalProjectCommand::PreviewReplaceResource {
            project_id: *project_id,
            resource_id: *resource_id,
            path: path.clone(),
        },
        UiProjectAction::ReplaceResource {
            project_id,
            resource_id,
            path,
        } => LocalProjectCommand::ReplaceResource {
            project_id: *project_id,
            resource_id: *resource_id,
            path: path.clone(),
        },
        UiProjectAction::RemoveResource {
            project_id,
            resource_id,
            force,
        } => LocalProjectCommand::RemoveResource {
            project_id: *project_id,
            resource_id: *resource_id,
            force: *force,
        },
        UiProjectAction::SetPrimaryResource {
            project_id,
            resource_id,
        } => LocalProjectCommand::SetPrimaryResource {
            project_id: *project_id,
            resource_id: *resource_id,
        },
        UiProjectAction::CheckResources {
            project_id,
            resource_id,
        } => LocalProjectCommand::CheckResources {
            project_id: *project_id,
            resource_id: *resource_id,
        },
    }
}

fn ui_project_outcome(outcome: LocalProjectOutcome) -> UiProjectOutcome {
    match outcome {
        LocalProjectOutcome::Completed { project_head } => {
            UiProjectOutcome::Completed { project_head }
        }
        LocalProjectOutcome::Running { stage } => UiProjectOutcome::Running { stage },
        LocalProjectOutcome::Rejected { category, code } => {
            UiProjectOutcome::Rejected { category, code }
        }
        LocalProjectOutcome::Reconcilable {
            stage,
            category,
            code,
            warning,
        } => UiProjectOutcome::Reconcilable {
            stage,
            category,
            code,
            warning: warning.map(|warning| UiProjectExternalWarning {
                kind: warning.kind,
                destination: warning.destination,
                branch: warning.branch,
            }),
        },
        LocalProjectOutcome::InputSent { message_id } => UiProjectOutcome::InputSent { message_id },
        LocalProjectOutcome::ResourcePreview {
            display_path,
            canonical_path,
            conflicts,
        } => UiProjectOutcome::ResourcePreview {
            display_path,
            canonical_path,
            conflicts: conflicts
                .into_iter()
                .map(|conflict| UiProjectResourceConflict {
                    project_id: conflict.project_id,
                    resource_id: conflict.resource_id,
                    display_path: conflict.display_path,
                    canonical_path: conflict.canonical_path,
                    relationship: conflict.relationship,
                })
                .collect(),
        },
        LocalProjectOutcome::ResourceChecks { checks } => UiProjectOutcome::ResourceChecks {
            checks: checks
                .into_iter()
                .map(|check| UiProjectResourceCheck {
                    resource_id: check.resource_id,
                    status: check.status,
                    health: check.health,
                    release: check.release,
                    observed_canonical_path: check.observed_canonical_path,
                    details: check.details,
                    error_category: check.error_category,
                    error_code: check.error_code,
                    reconciliation_id: check.reconciliation_id,
                })
                .collect(),
        },
    }
}

fn tui_projects(projects: Vec<LocalProject>) -> Vec<UiProject> {
    projects
        .into_iter()
        .map(|project| UiProject {
            project_id: project.project_id,
            home: project.home,
            name: terminal_text(&project.name),
            lifecycle: project.lifecycle,
            archived: project.archived,
            claimable: project.claimable,
            assigned: project.assigned,
            head: project.head,
            input_sequence: project.input_sequence,
            resources: project
                .resources
                .into_iter()
                .map(|resource| UiProjectResource {
                    resource_id: resource.resource_id,
                    display_path: terminal_text(&resource.display_path),
                    canonical_path: terminal_text(&resource.canonical_path),
                    health: resource.health,
                    primary: resource.primary,
                    active_claim: resource.active_claim,
                    conflicting_projects: resource.conflicting_projects,
                })
                .collect(),
        })
        .collect()
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
                message_target: None,
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
        message_target: Some(UiMessageTarget {
            message_id: message.message_id.bytes(),
            reply_allowed: message.purpose == MessagePurposeDto::Question,
        }),
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

fn project_failure(error: &crate::cli::CliError) -> UiFailure {
    let (code, action) = match error {
        crate::cli::CliError::Arguments => (
            "project_command_invalid",
            "correct the project name, text, path, branch, or base and retry",
        ),
        crate::cli::CliError::ProjectState => (
            "project_state_stale_or_uncertain",
            "reload and reselect the current project or working tree",
        ),
        crate::cli::CliError::MessagingState => (
            "project_input_target_stale",
            "reload and reselect the project's current mailbox",
        ),
        _ => (
            "project_command_unavailable",
            "wait for the local node to recover, then retry the same operation",
        ),
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
        UiTimerKind::AutosaveDraft => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::{local_project_command, ui_project_outcome};
    use crate::local_client::{
        LocalProjectCommand, LocalProjectOutcome, LocalProjectResourceCheck,
        LocalProjectResourceConflict,
    };
    use hq_tui::{UiProjectAction, UiProjectOutcome};

    #[test]
    fn every_resource_action_maps_to_the_exact_ordinary_client_command() {
        let project_id = [1; 32];
        let resource_id = [2; 32];
        let cases = [
            (
                UiProjectAction::PreviewAddResource {
                    project_id,
                    path: "/add".to_owned(),
                    make_primary: true,
                },
                LocalProjectCommand::PreviewAddResource {
                    project_id,
                    path: "/add".to_owned(),
                    make_primary: true,
                },
            ),
            (
                UiProjectAction::AddResource {
                    project_id,
                    path: "/add".to_owned(),
                    make_primary: true,
                },
                LocalProjectCommand::AddResource {
                    project_id,
                    path: "/add".to_owned(),
                    make_primary: true,
                },
            ),
            (
                UiProjectAction::PreviewReplaceResource {
                    project_id,
                    resource_id,
                    path: "/replace".to_owned(),
                },
                LocalProjectCommand::PreviewReplaceResource {
                    project_id,
                    resource_id,
                    path: "/replace".to_owned(),
                },
            ),
            (
                UiProjectAction::ReplaceResource {
                    project_id,
                    resource_id,
                    path: "/replace".to_owned(),
                },
                LocalProjectCommand::ReplaceResource {
                    project_id,
                    resource_id,
                    path: "/replace".to_owned(),
                },
            ),
            (
                UiProjectAction::RemoveResource {
                    project_id,
                    resource_id,
                    force: true,
                },
                LocalProjectCommand::RemoveResource {
                    project_id,
                    resource_id,
                    force: true,
                },
            ),
            (
                UiProjectAction::SetPrimaryResource {
                    project_id,
                    resource_id,
                },
                LocalProjectCommand::SetPrimaryResource {
                    project_id,
                    resource_id,
                },
            ),
            (
                UiProjectAction::CheckResources {
                    project_id,
                    resource_id: Some(resource_id),
                },
                LocalProjectCommand::CheckResources {
                    project_id,
                    resource_id: Some(resource_id),
                },
            ),
            (
                UiProjectAction::CheckResources {
                    project_id,
                    resource_id: None,
                },
                LocalProjectCommand::CheckResources {
                    project_id,
                    resource_id: None,
                },
            ),
        ];
        for (action, expected) in cases {
            assert_eq!(local_project_command(&action), expected);
        }
    }

    #[test]
    fn passive_resource_preview_and_check_evidence_maps_without_parsing_text() {
        assert_eq!(
            ui_project_outcome(LocalProjectOutcome::ResourcePreview {
                display_path: "/display".to_owned(),
                canonical_path: "/canonical".to_owned(),
                conflicts: vec![LocalProjectResourceConflict {
                    project_id: [3; 32],
                    resource_id: [4; 32],
                    display_path: "/other".to_owned(),
                    canonical_path: "/canonical/other".to_owned(),
                    relationship: "ancestor".to_owned(),
                }],
            }),
            UiProjectOutcome::ResourcePreview {
                display_path: "/display".to_owned(),
                canonical_path: "/canonical".to_owned(),
                conflicts: vec![hq_tui::UiProjectResourceConflict {
                    project_id: [3; 32],
                    resource_id: [4; 32],
                    display_path: "/other".to_owned(),
                    canonical_path: "/canonical/other".to_owned(),
                    relationship: "ancestor".to_owned(),
                }],
            }
        );
        assert_eq!(
            ui_project_outcome(LocalProjectOutcome::ResourceChecks {
                checks: vec![LocalProjectResourceCheck {
                    resource_id: [5; 32],
                    status: "uncertain".to_owned(),
                    health: None,
                    release: Some("dirty".to_owned()),
                    observed_canonical_path: Some("/observed".to_owned()),
                    details: Some("adapter detail".to_owned()),
                    error_category: Some("resource".to_owned()),
                    error_code: Some("uncertain".to_owned()),
                    reconciliation_id: Some([6; 32]),
                }],
            }),
            UiProjectOutcome::ResourceChecks {
                checks: vec![hq_tui::UiProjectResourceCheck {
                    resource_id: [5; 32],
                    status: "uncertain".to_owned(),
                    health: None,
                    release: Some("dirty".to_owned()),
                    observed_canonical_path: Some("/observed".to_owned()),
                    details: Some("adapter detail".to_owned()),
                    error_category: Some("resource".to_owned()),
                    error_code: Some("uncertain".to_owned()),
                    reconciliation_id: Some([6; 32]),
                }],
            }
        );
    }
}
