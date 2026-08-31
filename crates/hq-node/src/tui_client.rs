//! Reconnecting local-client mapping and the single TUI effect executor.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hq_local_api::{
    BlockingClientError, ClientConnectionState, ClientEvent,
    protocol::v1::{
        ActivityStatusDto, AuthoritativeSnapshotDto, ConversationActivityKindDto,
        ConversationContextDto, ConversationEntryDto, ConversationKeyDto, ConversationMessageDto,
        ConversationPageRequest, ConversationParticipantDto, Id32, MailboxAddressDto,
        MailboxCommandActionDto, MailboxCommandRequestDto, MailboxDraftDto,
        MailboxDraftSaveOutcomeDto, MailboxDraftSaveRequestDto, MailboxDraftTargetDto,
        MessagePurposeDto, MutationAttemptDto, MutationOutcomeDto, PresentationKindDto,
        ProviderCatalogDto, Request, ResponseResult, SnapshotItem,
    },
};
use hq_tui::{
    EffectId, UiActivityStatus, UiAgent, UiAgentAction, UiAgentAssignmentPhase,
    UiAgentAttentionReason, UiAgentLifecycle, UiAgentMailbox, UiAgentProjectAssignment,
    UiAgentSession, UiAgentStatus, UiConnectionState, UiConversationActivityKind,
    UiConversationAuthor, UiConversationEntry, UiConversationEntryPresentation, UiConversationPage,
    UiConversationTarget, UiDirectTarget, UiEffect, UiEvent, UiFailure, UiHumanIssue,
    UiHumanMembershipEvidence, UiHumanMembershipStatus, UiHumanSelectionEvidence, UiHumanState,
    UiMailboxAction, UiMailboxCommandResult, UiMailboxDraft, UiMailboxDraftTarget,
    UiManagedSessionAction, UiManagedSessionOutcome, UiManagedSessionResult, UiMessageDelivery,
    UiMessageState, UiMessageTarget, UiProject, UiProjectAction, UiProjectAssignment,
    UiProjectExternalWarning, UiProjectOutcome, UiProjectResource, UiProjectResourceCheck,
    UiProjectResourceConflict, UiProjectResult, UiProjectThread, UiProvider, UiRow, UiRowKind,
    UiRowState, UiSection, UiSnapshot, UiTechnicalSection, UiTimerKind,
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
    /// Loads and maps one complete authoritative snapshot for every semantic section.
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure>;

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
    ) -> Result<UiMailboxCommandResult, UiFailure>;

    /// Executes or reconciles one stable named-agent administration command.
    fn submit_agent_command(&mut self, _action: UiAgentAction) -> Result<u64, UiFailure> {
        Err(UiFailure {
            code: "agent_command_unavailable".to_owned(),
            action: "restart HQ with support for agent changes".to_owned(),
        })
    }

    /// Executes or reconciles one stable provider-neutral managed-session command.
    fn submit_managed_session(
        &mut self,
        _action: UiManagedSessionAction,
    ) -> Result<UiManagedSessionResult, UiFailure> {
        Err(UiFailure {
            code: "managed_session_unavailable".to_owned(),
            action: "restart HQ with support for agent conversations".to_owned(),
        })
    }

    /// Executes or reconciles one stable project command.
    fn submit_project_command(
        &mut self,
        _action: UiProjectAction,
    ) -> Result<UiProjectResult, UiFailure> {
        Err(UiFailure {
            code: "project_command_unavailable".to_owned(),
            action: "restart HQ with support for project changes".to_owned(),
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
    conversation_presentations: BTreeMap<String, ConversationPresentationContext>,
}

#[derive(Clone)]
struct ConversationPresentationContext {
    context: ConversationContextDto,
    local_human: MailboxAddressDto,
}

impl LocalTuiClient {
    /// Wraps one already-ready subscribed ordinary local API client.
    pub const fn new(client: LocalNodeEventClient, state: StatePaths) -> Self {
        Self {
            client,
            state,
            observed_connection: None,
            conversation_keys: BTreeMap::new(),
            conversation_presentations: BTreeMap::new(),
        }
    }
}

impl TuiClientPort for LocalTuiClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        let local_installation = *self.client.installation_id().as_bytes();
        let snapshot = self
            .client
            .snapshot()
            .map_err(|error| client_failure(&error))?;
        self.conversation_keys = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::Conversation { key, .. } => {
                    let row_id = conversation_identity(key.clone());
                    Some((row_id, key.clone()))
                }
                _ => None,
            })
            .collect();
        self.conversation_presentations = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::Conversation {
                    key,
                    context,
                    local_human,
                    ..
                } => Some((
                    conversation_identity(key.clone()),
                    ConversationPresentationContext {
                        context: context.clone(),
                        local_human: local_human.clone(),
                    },
                )),
                _ => None,
            })
            .collect();
        let projects = tui_project_catalog(&snapshot).map_err(|error| project_failure(&error))?;
        let ClientEvent::Response {
            result: ResponseResult::ProviderCatalog(providers),
            ..
        } = self
            .client
            .request(Request::ProviderCatalog)
            .map_err(|error| client_failure(&error))?
        else {
            return Err(UiFailure {
                code: "provider_catalog_protocol".to_owned(),
                action: "restart HQ and reload the available agent services".to_owned(),
            });
        };
        Ok(tui_snapshot_with_projects(
            local_installation,
            &snapshot,
            projects,
            &providers,
        ))
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
                action: "reload the Inbox and select the conversation again".to_owned(),
            })?;
        let presentation = self
            .conversation_presentations
            .get(row_id)
            .cloned()
            .ok_or_else(|| UiFailure {
                code: "conversation_stale".to_owned(),
                action: "reload the Inbox and select the conversation again".to_owned(),
            })?;
        let request = ConversationPageRequest::new(key, 100, cursor).map_err(|_| UiFailure {
            code: "conversation_page_invalid".to_owned(),
            action: "reload the Inbox and select the conversation again".to_owned(),
        })?;
        match self
            .client
            .request(Request::ConversationPage(request))
            .map_err(|error| client_failure(&error))?
        {
            ClientEvent::Response {
                result: ResponseResult::ConversationPage(page),
                ..
            } => Ok(tui_conversation_page(
                row_id,
                &presentation.context,
                &presentation.local_human,
                page,
            )),
            _ => Err(UiFailure {
                code: "conversation_response_invalid".to_owned(),
                action: "reload the Inbox and select the conversation again".to_owned(),
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
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        let command_id = Id32::new(random_identity()?);
        let message_id = Id32::new(random_identity()?);
        let authors_message = matches!(
            action,
            UiMailboxAction::Reply { .. }
                | UiMailboxAction::Direct { .. }
                | UiMailboxAction::SelfNote
                | UiMailboxAction::Project { .. }
        );
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
            UiMailboxAction::Project {
                project_id,
                thread_id,
            } => MailboxCommandActionDto::Project {
                project_id: Id32::new(project_id),
                thread_id: thread_id.map(Id32::new),
                message_id,
            },
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
            }) => Ok(UiMailboxCommandResult {
                revision,
                message_id: authors_message.then_some(message_id.bytes()),
            }),
            ClientEvent::Mutation(MutationAttemptDto::Completed {
                outcome: MutationOutcomeDto::Rejected { code, .. },
                ..
            }) => Err(UiFailure {
                action: if code == "mailbox_target_stale" {
                    "reselect the target; the draft text is preserved".to_owned()
                } else {
                    "review the message or selected item and try again".to_owned()
                },
                code,
            }),
            _ => Err(UiFailure {
                code: "mailbox_command_uncertain".to_owned(),
                action: "keep this draft open while HQ checks whether it was sent".to_owned(),
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
                    "reload Agents and choose an active agent or saved conversation again"
                        .to_owned()
                }
                crate::cli::CliError::Arguments => {
                    "correct the agent or conversation name and try again".to_owned()
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
                        "choose an available agent service or saved conversation and try again"
                    }
                    crate::cli::CliError::AgentState => {
                        "reload Agents and choose an active agent again"
                    }
                    crate::cli::CliError::HarnessState => {
                        "reload the agent's saved conversations before trying again"
                    }
                    _ => "wait for HQ to recover, then retry the same conversation",
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
            runtime_state: result.runtime_state,
            runtime_code: result.runtime_code,
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
                    action: "waiting for HQ to reload your workspace".to_owned(),
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
            action: "close and reopen the draft after HQ reconnects".to_owned(),
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
        UiMailboxDraftTarget::Project {
            project_id,
            thread_id,
        } => MailboxDraftTargetDto::Project {
            project_id: Id32::new(*project_id),
            thread_id: thread_id.map(Id32::new),
        },
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
        MailboxDraftTargetDto::Project {
            project_id,
            thread_id,
        } => UiMailboxDraftTarget::Project {
            project_id: project_id.bytes(),
            thread_id: thread_id.as_ref().map(|id| id.bytes()),
        },
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
    cancellation: Arc<AtomicBool>,
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
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let worker = thread::Builder::new()
            .name("hq-tui-client".to_owned())
            .spawn(move || {
                client_worker(
                    client,
                    &command_receiver,
                    &event_sender,
                    &worker_cancellation,
                );
            })
            .map_err(|_| TuiExecutorError::WorkerSpawn)?;
        Ok(Self {
            clock,
            commands,
            events,
            worker: Some(worker),
            cancellation,
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
                UiEffect::LoadSnapshot { id } => {
                    if self.effect_is_outstanding(id) {
                        return Err(TuiExecutorError::DuplicateEffectIdentity);
                    }
                    self.commands
                        .try_send(WorkerCommand::LoadSnapshot { id })
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
        self.cancellation.store(true, Ordering::SeqCst);
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
    cancellation: &AtomicBool,
) {
    loop {
        if cancellation.load(Ordering::SeqCst) {
            break;
        }
        match commands.recv_timeout(COMMAND_WAIT) {
            Ok(_) if cancellation.load(Ordering::SeqCst) => break,
            Ok(WorkerCommand::LoadSnapshot { id }) => {
                let event = match client.load_snapshot() {
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
                    Ok(result) => UiEvent::MailboxCommandCommitted {
                        effect_id: id,
                        revision: result.revision,
                        message_id: result.message_id,
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
                if cancellation.load(Ordering::SeqCst) {
                    break;
                }
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

/// Maps one authoritative local API snapshot into one complete passive presentation snapshot.
pub fn tui_snapshot(
    local_installation: [u8; 32],
    snapshot: &AuthoritativeSnapshotDto,
) -> UiSnapshot {
    let projects = tui_project_catalog(snapshot).unwrap_or_default();
    let providers = ProviderCatalogDto {
        providers: Vec::new(),
        default_provider: None,
    };
    tui_snapshot_with_projects(local_installation, snapshot, projects, &providers)
}

/// Maps one authoritative snapshot together with the node's passive provider catalog.
pub fn tui_snapshot_with_provider_catalog(
    local_installation: [u8; 32],
    snapshot: &AuthoritativeSnapshotDto,
    providers: &ProviderCatalogDto,
) -> UiSnapshot {
    let projects = tui_project_catalog(snapshot).unwrap_or_default();
    tui_snapshot_with_projects(local_installation, snapshot, projects, providers)
}

fn tui_snapshot_with_projects(
    local_installation: [u8; 32],
    snapshot: &AuthoritativeSnapshotDto,
    projects: Vec<LocalProject>,
    provider_catalog: &ProviderCatalogDto,
) -> UiSnapshot {
    let human_state = tui_human_state(local_installation, snapshot);
    let projects = tui_projects(projects);
    let agents = tui_named_agent_catalog(snapshot)
        .into_iter()
        .map(|agent| {
            let agent_id = *agent.agent_id.as_bytes();
            let lifecycle = match agent.lifecycle.as_str() {
                "active" => UiAgentLifecycle::Active,
                "retired" => UiAgentLifecycle::Retired,
                _ => UiAgentLifecycle::Conflicted,
            };
            UiAgent {
                agent_id,
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
                lifecycle,
                runnable: agent.runnable,
                status: agent_status(agent_id, lifecycle, &projects),
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
            }
        })
        .collect::<Vec<_>>();
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
    let rows = |section| {
        snapshot
            .items
            .iter()
            .filter_map(|item| snapshot_row(section, item))
            .collect()
    };
    let providers = tui_providers(provider_catalog);
    UiSnapshot {
        revision: snapshot.revision,
        human_state,
        inbox_rows: rows(UiSection::Inbox),
        sent_rows: rows(UiSection::Sent),
        archived_rows: rows(UiSection::Archived),
        agent_rows: agents.iter().map(agent_row).collect(),
        project_rows: rows(UiSection::Projects),
        direct_targets,
        providers,
        agents,
        projects,
    }
}

fn tui_providers(provider_catalog: &ProviderCatalogDto) -> Vec<UiProvider> {
    let mut providers = provider_catalog
        .providers
        .iter()
        .map(|provider| UiProvider {
            provider: provider.provider.clone(),
            name: terminal_text(&provider.name),
            available: provider.available,
            configured_default: provider_catalog.default_provider.as_deref()
                == Some(provider.provider.as_str()),
        })
        .collect::<Vec<_>>();
    if let Some(default_provider) = &provider_catalog.default_provider
        && !providers
            .iter()
            .any(|provider| provider.provider == *default_provider)
    {
        providers.push(UiProvider {
            provider: default_provider.clone(),
            name: terminal_text(default_provider),
            available: false,
            configured_default: true,
        });
        providers.sort_by(|left, right| left.provider.cmp(&right.provider));
    }
    providers
}

fn tui_human_state(
    local_installation: [u8; 32],
    snapshot: &AuthoritativeSnapshotDto,
) -> UiHumanState {
    let selections = snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::AccountSelection {
                installation_id,
                candidates,
                active,
                frontier,
            } if installation_id.bytes() == local_installation => Some(UiHumanSelectionEvidence {
                candidates: candidates.iter().map(|id| id.bytes()).collect(),
                active: active.map(Id32::bytes),
                frontier: frontier.iter().map(|id| id.bytes()).collect(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    match selections.as_slice() {
        [] => UiHumanState::NeedsAttention(UiHumanIssue::NoAccountSelected),
        [selection] if selection.active.is_none() && selection.candidates.is_empty() => {
            UiHumanState::NeedsAttention(UiHumanIssue::NoAccountSelected)
        }
        [selection] if selection.active.is_none() => {
            UiHumanState::NeedsAttention(UiHumanIssue::SelectionCandidates {
                candidates: selection.candidates.clone(),
                frontier: selection.frontier.clone(),
            })
        }
        [selection] => {
            let Some(account) = selection.active else {
                return UiHumanState::NeedsAttention(UiHumanIssue::SelectionCandidates {
                    candidates: selection.candidates.clone(),
                    frontier: selection.frontier.clone(),
                });
            };
            let creator_authority = snapshot.items.iter().any(|item| {
                matches!(
                    item,
                    SnapshotItem::Account {
                        account_id,
                        creator_installation,
                        ..
                    } if account_id.bytes() == account
                        && creator_installation.bytes() == local_installation
                )
            });
            if creator_authority {
                return UiHumanState::Ready;
            }

            let memberships = local_human_memberships(snapshot, local_installation, account);
            match memberships.as_slice() {
                [] => UiHumanState::NeedsAttention(UiHumanIssue::SelectedWithoutAuthority {
                    account_id: account,
                    selection_frontier: selection.frontier.clone(),
                }),
                [membership] if membership.status == UiHumanMembershipStatus::Pending => {
                    UiHumanState::NeedsAttention(UiHumanIssue::MembershipPending(
                        membership.clone(),
                    ))
                }
                [membership] if membership.status == UiHumanMembershipStatus::Revoked => {
                    UiHumanState::NeedsAttention(UiHumanIssue::MembershipRevoked(
                        membership.clone(),
                    ))
                }
                [membership]
                    if membership.status == UiHumanMembershipStatus::Active
                        && membership.active_acceptances.len() == 1 =>
                {
                    UiHumanState::Ready
                }
                _ => UiHumanState::NeedsAttention(UiHumanIssue::MembershipAuthorityConflict {
                    records: memberships,
                }),
            }
        }
        _ => UiHumanState::NeedsAttention(UiHumanIssue::SelectionRecords {
            records: selections,
        }),
    }
}

fn local_human_memberships(
    snapshot: &AuthoritativeSnapshotDto,
    local_installation: [u8; 32],
    account: [u8; 32],
) -> Vec<UiHumanMembershipEvidence> {
    snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::Membership {
                account_id,
                device,
                state,
                frontier,
                active_acceptances,
                ..
            } if account_id.bytes() == account && device.bytes() == local_installation => {
                Some(UiHumanMembershipEvidence {
                    account_id: account,
                    status: match state.as_str() {
                        "pending" => UiHumanMembershipStatus::Pending,
                        "active" => UiHumanMembershipStatus::Active,
                        "revoked" => UiHumanMembershipStatus::Revoked,
                        _ => UiHumanMembershipStatus::Conflicted,
                    },
                    frontier: frontier.iter().map(|id| id.bytes()).collect(),
                    active_acceptances: active_acceptances.iter().map(|id| id.bytes()).collect(),
                })
            }
            _ => None,
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
fn local_project_command(action: &UiProjectAction) -> LocalProjectCommand {
    match action {
        UiProjectAction::PreviewCreateExisting { name, brief, path } => {
            LocalProjectCommand::PreviewCreateExisting {
                name: name.clone(),
                brief: brief.clone(),
                path: path.clone(),
            }
        }
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
        UiProjectAction::Activate {
            project_id,
            agent_id,
            provider,
            resume_session,
            resume_thread,
            launch_directory,
        } => LocalProjectCommand::Activate {
            project_id: *project_id,
            agent_id: *agent_id,
            provider: provider.clone(),
            resume_session: resume_session.clone(),
            resume_thread: *resume_thread,
            launch_directory: launch_directory.clone(),
        },
        UiProjectAction::DispatchPending { project_id } => LocalProjectCommand::DispatchPending {
            project_id: *project_id,
        },
        UiProjectAction::Handoff {
            project_id,
            agent_id,
            provider,
            resume_session,
            thread_id,
            launch_directory,
            force_takeover,
        } => LocalProjectCommand::Handoff {
            project_id: *project_id,
            agent_id: *agent_id,
            provider: provider.clone(),
            resume_session: resume_session.clone(),
            thread_id: *thread_id,
            launch_directory: launch_directory.clone(),
            force_takeover: *force_takeover,
        },
        UiProjectAction::Open { project_id } => LocalProjectCommand::Open {
            project_id: *project_id,
        },
        UiProjectAction::PreviewClose { project_id } => LocalProjectCommand::PreviewClose {
            project_id: *project_id,
        },
        UiProjectAction::Close { project_id, force } => LocalProjectCommand::Close {
            project_id: *project_id,
            force: *force,
        },
        UiProjectAction::SetArchived {
            project_id,
            archived,
        } => LocalProjectCommand::SetArchived {
            project_id: *project_id,
            archived: *archived,
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
        LocalProjectOutcome::InputSent { .. } => UiProjectOutcome::Completed { project_head: None },
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
            assignment: project.assignment.map(|assignment| UiProjectAssignment {
                assignment_id: assignment.assignment_id,
                agent_id: assignment.agent_id,
                provider: terminal_text(&assignment.provider),
                session: assignment.session.map(|session| terminal_text(&session)),
                phase: assignment.phase,
                thread_id: assignment.thread_id,
                launch_directory: assignment
                    .launch_directory
                    .map(|directory| terminal_text(&directory)),
                blocked: assignment.blocked.map(|blocked| terminal_text(&blocked)),
                cardinality_conflicted: assignment.cardinality_conflicted,
                runnable: assignment.runnable,
            }),
            threads: project
                .threads
                .into_iter()
                .map(|thread| UiProjectThread {
                    agent_id: thread.agent_id,
                    provider: terminal_text(&thread.provider),
                    session: terminal_text(&thread.session),
                    thread_id: thread.thread_id,
                })
                .collect(),
            pending_inputs: project
                .pending_inputs
                .into_iter()
                .map(|input| hq_tui::UiPendingProjectInput {
                    message_id: input.message_id,
                    thread_id: input.thread_id,
                    sequence: input.sequence,
                })
                .collect(),
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

fn snapshot_row(section: UiSection, item: &SnapshotItem) -> Option<UiRow> {
    match (section, item) {
        (
            section @ (UiSection::Inbox | UiSection::Sent | UiSection::Archived),
            SnapshotItem::Conversation {
                key,
                context,
                root_message,
                preview,
                open_messages,
                archived_messages,
                sent_messages,
                ..
            },
        ) if match section {
            UiSection::Inbox => *open_messages > 0,
            UiSection::Sent => *sent_messages > 0,
            UiSection::Archived => *archived_messages > 0,
            UiSection::Agents | UiSection::Projects => false,
        } =>
        {
            conversation_row(
                section,
                key.clone(),
                context,
                *root_message,
                preview.as_deref(),
                ConversationCounts {
                    open: *open_messages,
                    sent: *sent_messages,
                    archived: *archived_messages,
                },
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
            id: full_id(*message_id),
            title: terminal_text(content),
            detail: format!(
                "waiting for {} related records · {} records could not be used",
                missing_dependencies.len(),
                unusable_dependencies.len()
            ),
            state: UiRowState::Attention,
            kind: UiRowKind::Diagnostic,
            conversation_target: None,
        }),
        (UiSection::Inbox, SnapshotItem::IncompleteMessagesTruncated) => Some(UiRow {
            id: "incomplete-messages-truncated".to_owned(),
            title: "Additional incomplete messages".to_owned(),
            detail: "HQ will retry after more message history arrives".to_owned(),
            state: UiRowState::Attention,
            kind: UiRowKind::Diagnostic,
            conversation_target: None,
        }),
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
            id: full_id(*project_id),
            title: terminal_text(name),
            detail: if *archived {
                "archived".to_owned()
            } else if !*claimable {
                "needs attention · folder ownership conflict".to_owned()
            } else if lifecycle == "closed" {
                "closed".to_owned()
            } else {
                "open".to_owned()
            },
            state: if *archived {
                UiRowState::Archived
            } else if !*claimable {
                UiRowState::Attention
            } else {
                UiRowState::Open
            },
            kind: UiRowKind::Project,
            conversation_target: None,
        }),
        _ => None,
    }
}

fn agent_row(agent: &UiAgent) -> UiRow {
    let title = match agent.names.as_slice() {
        [name] => terminal_text(name),
        [] => format!("Agent {}", short_id(Id32::new(agent.agent_id))),
        _ => format!("Conflicted agent {}", short_id(Id32::new(agent.agent_id))),
    };
    let (state, detail) = match &agent.status {
        UiAgentStatus::Unassigned => (UiRowState::Open, "unassigned".to_owned()),
        UiAgentStatus::Assigned(assignment) => (
            UiRowState::Open,
            format!(
                "assigned to {} · {}",
                assignment.project_name,
                match assignment.phase {
                    UiAgentAssignmentPhase::SettingUp => "setting up",
                    UiAgentAssignmentPhase::Ready => "ready",
                    UiAgentAssignmentPhase::Blocked => "blocked",
                }
            ),
        ),
        UiAgentStatus::NeedsAttention {
            reason,
            assignments,
        } => {
            let detail = match reason {
                UiAgentAttentionReason::IdentityConflict => {
                    "needs attention · saved names disagree".to_owned()
                }
                UiAgentAttentionReason::AssignmentConflict => {
                    "needs attention · assigned to more than one project".to_owned()
                }
                UiAgentAttentionReason::AssignmentBlocked => assignments.first().map_or_else(
                    || "needs attention · project setup is blocked".to_owned(),
                    |assignment| format!("needs attention · {} blocked", assignment.project_name),
                ),
            };
            (UiRowState::Attention, detail)
        }
        UiAgentStatus::Retired => (UiRowState::Archived, "retired".to_owned()),
    };
    UiRow {
        id: full_id(Id32::new(agent.agent_id)),
        title,
        detail,
        state,
        kind: UiRowKind::Agent,
        conversation_target: None,
    }
}

fn agent_status(
    agent_id: [u8; 32],
    lifecycle: UiAgentLifecycle,
    projects: &[UiProject],
) -> UiAgentStatus {
    let assignments = projects
        .iter()
        .filter_map(|project| {
            let assignment = project
                .assignment
                .as_ref()
                .filter(|assignment| assignment.agent_id == agent_id)?;
            let phase = if assignment.blocked.is_some() {
                UiAgentAssignmentPhase::Blocked
            } else if assignment.runnable {
                UiAgentAssignmentPhase::Ready
            } else {
                UiAgentAssignmentPhase::SettingUp
            };
            Some(UiAgentProjectAssignment {
                project_id: project.project_id,
                project_name: project.name.clone(),
                assignment_id: assignment.assignment_id,
                provider: assignment.provider.clone(),
                session: assignment.session.clone(),
                phase,
                blocked: assignment.blocked.clone(),
                cardinality_conflicted: assignment.cardinality_conflicted,
            })
        })
        .collect::<Vec<_>>();

    match lifecycle {
        UiAgentLifecycle::Retired => UiAgentStatus::Retired,
        UiAgentLifecycle::Conflicted => UiAgentStatus::NeedsAttention {
            reason: UiAgentAttentionReason::IdentityConflict,
            assignments,
        },
        UiAgentLifecycle::Active => match assignments.as_slice() {
            [] => UiAgentStatus::Unassigned,
            [assignment] if assignment.cardinality_conflicted => UiAgentStatus::NeedsAttention {
                reason: UiAgentAttentionReason::AssignmentConflict,
                assignments,
            },
            [assignment] if assignment.phase == UiAgentAssignmentPhase::Blocked => {
                UiAgentStatus::NeedsAttention {
                    reason: UiAgentAttentionReason::AssignmentBlocked,
                    assignments,
                }
            }
            [assignment] => UiAgentStatus::Assigned(assignment.clone()),
            [_, _, ..] => UiAgentStatus::NeedsAttention {
                reason: UiAgentAttentionReason::AssignmentConflict,
                assignments,
            },
        },
    }
}

fn conversation_row(
    section: UiSection,
    key: ConversationKeyDto,
    context: &ConversationContextDto,
    root_message: Option<Id32>,
    preview: Option<&str>,
    counts: ConversationCounts,
) -> Option<UiRow> {
    let conversation_target = match &key {
        ConversationKeyDto::ProjectThread { project, thread } => {
            Some(UiConversationTarget::Project {
                project_id: project.bytes(),
                thread_id: thread.bytes(),
                root_message: root_message?.bytes(),
            })
        }
        ConversationKeyDto::Thread { .. } | ConversationKeyDto::ProviderSession { .. } => None,
    };
    let id = conversation_identity(key);
    let (count, label, state) = match section {
        UiSection::Inbox => (counts.open, "open messages", UiRowState::Open),
        UiSection::Sent => (counts.sent, "sent messages", UiRowState::Waiting),
        UiSection::Archived => (counts.archived, "archived messages", UiRowState::Archived),
        UiSection::Agents | UiSection::Projects => return None,
    };
    let fallback = format!("{count} {label}");
    Some(UiRow {
        id,
        title: conversation_title(context),
        detail: conversation_list_detail(context, preview, &fallback),
        state,
        kind: UiRowKind::Conversation,
        conversation_target,
    })
}

#[derive(Clone, Copy)]
struct ConversationCounts {
    open: u32,
    sent: u32,
    archived: u32,
}

/// Maps one bounded reducer-ordered local-API page into passive TUI presentation.
pub fn tui_conversation_page(
    row_id: &str,
    context: &ConversationContextDto,
    local_human: &MailboxAddressDto,
    page: hq_local_api::protocol::v1::ConversationPageDto,
) -> UiConversationPage {
    let heading = conversation_heading(context);
    UiConversationPage {
        row_id: row_id.to_owned(),
        title: heading.title,
        context: heading.context,
        entries: page
            .items
            .into_iter()
            .map(|entry| tui_conversation_entry(entry, local_human, heading.participant.as_ref()))
            .collect(),
        next_cursor: page.next_cursor,
    }
}

fn tui_conversation_entry(
    entry: ConversationEntryDto,
    local_human: &MailboxAddressDto,
    participant: Option<&ConversationParticipantPresentation>,
) -> UiConversationEntry {
    match entry {
        ConversationEntryDto::Message(message) => {
            tui_message_entry(*message, local_human, participant)
        }
        ConversationEntryDto::Activity {
            fact_id,
            activity_kind,
            sequence,
            status,
            content,
            truncated,
        } => {
            let status = tui_activity_status(status);
            let kind = tui_activity_kind(activity_kind);
            UiConversationEntry {
                id: full_id(fact_id),
                presentation: UiConversationEntryPresentation::Activity {
                    kind,
                    summary: activity_summary(kind, &status).to_owned(),
                    detail: terminal_text(&content),
                    status: status.clone(),
                    truncated,
                },
                message_state: None,
                delivery: None,
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

const fn tui_activity_kind(kind: ConversationActivityKindDto) -> UiConversationActivityKind {
    match kind {
        ConversationActivityKindDto::Status => UiConversationActivityKind::Status,
        ConversationActivityKindDto::AgentTurn => UiConversationActivityKind::AgentTurn,
        ConversationActivityKindDto::Progress => UiConversationActivityKind::Progress,
        ConversationActivityKindDto::Plan => UiConversationActivityKind::Plan,
        ConversationActivityKindDto::Diff => UiConversationActivityKind::Diff,
        ConversationActivityKindDto::CompletedItem => UiConversationActivityKind::CompletedItem,
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

const fn activity_summary(
    kind: UiConversationActivityKind,
    status: &UiActivityStatus,
) -> &'static str {
    match (kind, status) {
        (UiConversationActivityKind::AgentTurn, UiActivityStatus::Running) => "Agent is working…",
        (UiConversationActivityKind::AgentTurn, UiActivityStatus::Succeeded) => "Agent finished",
        (UiConversationActivityKind::AgentTurn, UiActivityStatus::Failed { .. }) => {
            "Agent stopped with an error"
        }
        (UiConversationActivityKind::AgentTurn, UiActivityStatus::Interrupted) => {
            "Agent was interrupted"
        }
        (UiConversationActivityKind::AgentTurn, UiActivityStatus::Snapshot) => "Agent status",
        (UiConversationActivityKind::Status, UiActivityStatus::Failed { .. }) => {
            "Status update failed"
        }
        (UiConversationActivityKind::Status, _) => "Status update",
        (UiConversationActivityKind::Progress, UiActivityStatus::Running) => "Work in progress…",
        (UiConversationActivityKind::Progress, UiActivityStatus::Failed { .. }) => {
            "Progress stopped with an error"
        }
        (UiConversationActivityKind::Progress, _) => "Progress updated",
        (UiConversationActivityKind::Plan, _) => "Plan updated",
        (UiConversationActivityKind::Diff, _) => "Changes updated",
        (UiConversationActivityKind::CompletedItem, UiActivityStatus::Succeeded) => {
            "Completed an item"
        }
        (UiConversationActivityKind::CompletedItem, UiActivityStatus::Failed { .. }) => {
            "An item failed"
        }
        (UiConversationActivityKind::CompletedItem, UiActivityStatus::Interrupted) => {
            "An item was interrupted"
        }
        (UiConversationActivityKind::CompletedItem, _) => "Item activity",
    }
}

fn tui_message_entry(
    message: ConversationMessageDto,
    local_human: &MailboxAddressDto,
    participant: Option<&ConversationParticipantPresentation>,
) -> UiConversationEntry {
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
    let author = conversation_author(&message, local_human, participant);
    let delivery = matches!(author, UiConversationAuthor::You).then_some(
        if message.peer_received_by.is_empty() {
            UiMessageDelivery::Sent
        } else {
            UiMessageDelivery::Received
        },
    );
    UiConversationEntry {
        id: full_id(message.fact_id),
        presentation: UiConversationEntryPresentation::Message {
            author,
            body: terminal_text(&message.content),
        },
        message_state: Some(state),
        delivery,
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

#[derive(Clone)]
struct ConversationParticipantPresentation {
    installation: Id32,
    mailbox: Id32,
    label: String,
}

struct ConversationHeading {
    title: String,
    context: Option<String>,
    participant: Option<ConversationParticipantPresentation>,
}

fn conversation_heading(context: &ConversationContextDto) -> ConversationHeading {
    match context {
        ConversationContextDto::Personal => ConversationHeading {
            title: "Personal notes".to_owned(),
            context: None,
            participant: None,
        },
        ConversationContextDto::Direct { participant } => {
            let label = participant
                .name
                .as_deref()
                .map_or_else(|| "Other participant".to_owned(), terminal_text);
            ConversationHeading {
                title: label.clone(),
                context: None,
                participant: conversation_participant(participant, label),
            }
        }
        ConversationContextDto::Project {
            name, participant, ..
        } => {
            let project = name
                .as_deref()
                .map_or_else(|| "Unnamed project".to_owned(), terminal_text);
            let label = participant
                .as_ref()
                .and_then(|value| value.name.as_deref())
                .map_or_else(|| "Project agent".to_owned(), terminal_text);
            ConversationHeading {
                title: label.clone(),
                context: Some(format!("Project · {project}")),
                participant: participant
                    .as_ref()
                    .and_then(|value| conversation_participant(value, label)),
            }
        }
    }
}

fn conversation_participant(
    participant: &ConversationParticipantDto,
    label: String,
) -> Option<ConversationParticipantPresentation> {
    participant
        .installation
        .zip(participant.mailbox)
        .map(
            |(installation, mailbox)| ConversationParticipantPresentation {
                installation,
                mailbox,
                label,
            },
        )
}

fn conversation_author(
    message: &ConversationMessageDto,
    local_human: &MailboxAddressDto,
    participant: Option<&ConversationParticipantPresentation>,
) -> UiConversationAuthor {
    if message.sender_installation == local_human.installation_id
        && message.sender_mailbox == local_human.mailbox_id
    {
        UiConversationAuthor::You
    } else if let Some(participant) = participant {
        if message.sender_installation == participant.installation
            && message.sender_mailbox == participant.mailbox
        {
            UiConversationAuthor::Participant(participant.label.clone())
        } else {
            UiConversationAuthor::Unknown
        }
    } else {
        UiConversationAuthor::Unknown
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

fn conversation_identity(key: ConversationKeyDto) -> String {
    match key {
        ConversationKeyDto::ProjectThread { project, thread } => {
            format!("project:{}:{}", full_id(project), full_id(thread))
        }
        ConversationKeyDto::Thread { thread, .. } => format!("thread:{}", full_id(thread)),
        ConversationKeyDto::ProviderSession {
            counterparty_installation,
            counterparty_mailbox,
            provider,
            session,
        } => format!(
            "session:{}:{}:{provider}:{session}",
            full_id(counterparty_installation),
            full_id(counterparty_mailbox)
        ),
    }
}

fn conversation_title(context: &ConversationContextDto) -> String {
    match context {
        ConversationContextDto::Personal => "Personal notes".to_owned(),
        ConversationContextDto::Direct { participant } => participant
            .name
            .as_deref()
            .map_or_else(|| "Other participant".to_owned(), terminal_text),
        ConversationContextDto::Project { participant, .. } => participant
            .as_ref()
            .and_then(|participant| participant.name.as_deref())
            .map_or_else(|| "Project agent".to_owned(), terminal_text),
    }
}

fn conversation_list_detail(
    context: &ConversationContextDto,
    preview: Option<&str>,
    fallback: &str,
) -> String {
    let message = preview.map_or_else(|| fallback.to_owned(), terminal_text);
    match context {
        ConversationContextDto::Project { name, .. } => format!(
            "{} · {message}",
            name.as_deref()
                .map_or_else(|| "Unnamed project".to_owned(), terminal_text),
        ),
        ConversationContextDto::Personal | ConversationContextDto::Direct { .. } => message,
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
            "reload Projects and select the current project or folder again",
        ),
        crate::cli::CliError::MessagingState => (
            "project_input_target_stale",
            "reload the project and select its current conversation again",
        ),
        _ => (
            "project_command_unavailable",
            "wait for HQ to recover, then retry the same change",
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
        UiTimerKind::DismissCompletion => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        conversation_identity, conversation_title, local_project_command, ui_project_outcome,
    };
    use crate::local_client::{
        LocalProjectCommand, LocalProjectOutcome, LocalProjectResourceCheck,
        LocalProjectResourceConflict,
    };
    use hq_local_api::protocol::v1::{
        ConversationContextDto, ConversationKeyDto, ConversationParticipantDto, Id32,
    };
    use hq_tui::{UiProjectAction, UiProjectOutcome};

    #[test]
    fn project_conversation_identity_retains_both_full_ids() {
        let identity = conversation_identity(ConversationKeyDto::ProjectThread {
            project: Id32::new([0x11; 32]),
            thread: Id32::new([0x22; 32]),
        });
        assert_eq!(
            identity,
            format!("project:{}:{}", "11".repeat(32), "22".repeat(32))
        );
    }

    #[test]
    fn conversation_titles_use_names_or_plain_unnamed_fallbacks_without_ids() {
        let participant = ConversationParticipantDto {
            agent: Some(Id32::new([0xaa; 32])),
            installation: Some(Id32::new([0xbb; 32])),
            mailbox: Some(Id32::new([0xcc; 32])),
            name: Some("Alice".to_owned()),
        };
        assert_eq!(
            conversation_title(&ConversationContextDto::Direct {
                participant: participant.clone(),
            }),
            "Alice"
        );
        assert_eq!(
            conversation_title(&ConversationContextDto::Project {
                project: Id32::new([0xdd; 32]),
                name: Some("Release".to_owned()),
                participant: Some(participant),
            }),
            "Alice"
        );

        let unnamed = conversation_title(&ConversationContextDto::Direct {
            participant: ConversationParticipantDto {
                agent: None,
                installation: Some(Id32::new([0xee; 32])),
                mailbox: Some(Id32::new([0xff; 32])),
                name: None,
            },
        });
        assert_eq!(unnamed, "Other participant");
        assert!(!unnamed.contains(&"ee".repeat(32)));
        assert!(!unnamed.contains(&"ff".repeat(32)));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn every_project_action_maps_to_the_exact_ordinary_client_command() {
        let project_id = [1; 32];
        let resource_id = [2; 32];
        let cases = [
            (
                UiProjectAction::PreviewCreateExisting {
                    name: "release".to_owned(),
                    brief: Some("ship it".to_owned()),
                    path: "/workspace/release".to_owned(),
                },
                LocalProjectCommand::PreviewCreateExisting {
                    name: "release".to_owned(),
                    brief: Some("ship it".to_owned()),
                    path: "/workspace/release".to_owned(),
                },
            ),
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
            (
                UiProjectAction::Activate {
                    project_id,
                    agent_id: [3; 32],
                    provider: "codex".to_owned(),
                    resume_session: Some("session".to_owned()),
                    resume_thread: Some([4; 32]),
                    launch_directory: "/work".to_owned(),
                },
                LocalProjectCommand::Activate {
                    project_id,
                    agent_id: [3; 32],
                    provider: "codex".to_owned(),
                    resume_session: Some("session".to_owned()),
                    resume_thread: Some([4; 32]),
                    launch_directory: "/work".to_owned(),
                },
            ),
            (
                UiProjectAction::DispatchPending { project_id },
                LocalProjectCommand::DispatchPending { project_id },
            ),
            (
                UiProjectAction::Handoff {
                    project_id,
                    agent_id: [5; 32],
                    provider: "codex".to_owned(),
                    resume_session: None,
                    thread_id: [6; 32],
                    launch_directory: "/handoff".to_owned(),
                    force_takeover: true,
                },
                LocalProjectCommand::Handoff {
                    project_id,
                    agent_id: [5; 32],
                    provider: "codex".to_owned(),
                    resume_session: None,
                    thread_id: [6; 32],
                    launch_directory: "/handoff".to_owned(),
                    force_takeover: true,
                },
            ),
            (
                UiProjectAction::Open { project_id },
                LocalProjectCommand::Open { project_id },
            ),
            (
                UiProjectAction::PreviewClose { project_id },
                LocalProjectCommand::PreviewClose { project_id },
            ),
            (
                UiProjectAction::Close {
                    project_id,
                    force: true,
                },
                LocalProjectCommand::Close {
                    project_id,
                    force: true,
                },
            ),
            (
                UiProjectAction::SetArchived {
                    project_id,
                    archived: true,
                },
                LocalProjectCommand::SetArchived {
                    project_id,
                    archived: true,
                },
            ),
            (
                UiProjectAction::SetArchived {
                    project_id,
                    archived: false,
                },
                LocalProjectCommand::SetArchived {
                    project_id,
                    archived: false,
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
