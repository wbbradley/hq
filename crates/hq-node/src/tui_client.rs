//! Reconnecting local-client mapping and the single TUI effect executor.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hq_domain::ProviderId;
use hq_local_api::{
    BlockingClientError, ClientConnectionState, ClientEvent, ClientReconnectCause,
    ClientTransportFailureKind,
    protocol::v1::{
        ActivityStatusDto, AuthoritativeConversationViewDto, AuthoritativeSnapshotDto,
        CompletedItemPresentationDto, ConversationActivityDto, ConversationActivityKindDto,
        ConversationContextDto, ConversationEntryDto, ConversationKeyDto, ConversationMessageDto,
        ConversationPageRequest, ConversationPageSelectionDto, ConversationParticipantDto, Id32,
        InstallationConfigurationDto, InstallationConfigurationPatchDto,
        InteractionAnswerOutcomeDto, InteractionAnswerRequestDto, InteractionKindDto,
        InteractionResponseDto, MailboxAddressDto, MailboxCommandActionDto,
        MailboxCommandRequestDto, MailboxDraftDto, MailboxDraftSaveOutcomeDto,
        MailboxDraftSaveRequestDto, MailboxDraftTargetDto, MessagePurposeDto, MutationAttemptDto,
        MutationOutcomeDto, PendingInteractionDto, PresentationKindDto, ProviderCatalogDto,
        Request, ResponseResult, SnapshotItem,
    },
};
use hq_tui::{
    EffectId, UiActivityStatus, UiAgent, UiAgentAction, UiAgentAssignmentPhase,
    UiAgentAttentionReason, UiAgentLifecycle, UiAgentMailbox, UiAgentProjectAssignment,
    UiAgentSession, UiAgentStatus, UiCompletedFileChange, UiCompletedItemPresentation,
    UiConfigField, UiConfiguration, UiConnectionState, UiConversationActivityKind,
    UiConversationAuthor, UiConversationEntry, UiConversationEntryPresentation, UiConversationPage,
    UiConversationTarget, UiDirectTarget, UiEffect, UiEvent, UiFailure, UiHumanIssue,
    UiHumanMembershipEvidence, UiHumanMembershipStatus, UiHumanSelectionEvidence, UiHumanState,
    UiInteraction, UiInteractionAnswerOutcome, UiInteractionChoice, UiInteractionKind,
    UiInteractionResponse, UiInteractionTarget, UiInteractionTargetIssue, UiMailboxAction,
    UiMailboxCommandResult, UiMailboxDraft, UiMailboxDraftTarget, UiManagedSessionAction,
    UiManagedSessionOutcome, UiManagedSessionResult, UiMaterializedConversationView,
    UiMessageDelivery, UiMessageState, UiMessageTarget, UiProject, UiProjectAction,
    UiProjectAssignment, UiProjectConversationSetup, UiProjectExternalWarning, UiProjectOutcome,
    UiProjectResource, UiProjectResourceCheck, UiProjectResourceConflict, UiProjectResult,
    UiProjectThread, UiProvider, UiReconnectCause, UiReconnectFailureKind, UiReconnectOperation,
    UiRow, UiRowKind, UiRowState, UiSection, UiSnapshot, UiTechnicalSection, UiTheme,
    UiThemeChoice, UiTimerKind,
};
use sha2::{Digest, Sha256};

use crate::{
    LocalCodexConfiguration, LocalConfiguration, LocalNodeClient, LocalNodeClientError,
    LocalNodeEventClient, StatePaths, ThemeSelection, TuiThemeEnvironment, UnixClientInterrupt,
    UnixClientWake, list_tui_themes,
    local_client::{
        LocalManagedSessionCommand, LocalManagedSessionOutcome, LocalNamedAgentCommand,
        LocalProject, LocalProjectCommand, LocalProjectOutcome, continue_project_command,
        execute_managed_session_command, execute_named_agent_command, execute_project_command,
        tui_named_agent_catalog, tui_project_catalog,
    },
    resolve_tui_theme,
};

const CLIENT_COMMAND_CAPACITY: usize = 8;
const CLIENT_EVENT_CAPACITY: usize = 16;

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
    /// One subscribed snapshot and selected first page became current together.
    MaterializedView(Box<UiMaterializedConversationView>),
    /// Complete bounded pending provider interactions.
    Interactions(Vec<UiInteraction>),
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
        /// Typed cause when the state begins a reconnect.
        cause: Option<UiReconnectCause>,
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

    /// Loads all installation-local settings and discoverable themes.
    fn load_configuration(&mut self) -> Result<UiConfiguration, UiFailure> {
        Err(UiFailure {
            code: "configuration_unavailable".to_owned(),
            action: "restart HQ and reopen Config".to_owned(),
        })
    }

    /// Validates and persists one complete installation-local replacement.
    fn save_configuration(
        &mut self,
        _field: UiConfigField,
        _configuration: UiConfiguration,
        _apply_theme: bool,
    ) -> Result<(UiConfiguration, Option<UiTheme>), UiFailure> {
        Err(UiFailure {
            code: "configuration_unavailable".to_owned(),
            action: "restart HQ and reopen Config".to_owned(),
        })
    }

    /// Loads one bounded reducer-ordered page for an exact snapshot row identity.
    fn load_conversation(
        &mut self,
        row_id: &str,
        cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure>;

    /// Executes or reconciles one exact terminal provider-interaction response.
    fn answer_interaction(
        &mut self,
        _interaction: UiInteraction,
        _response: UiInteractionResponse,
    ) -> Result<UiInteractionAnswerOutcome, UiFailure> {
        Err(UiFailure {
            code: "interaction_response_unavailable".to_owned(),
            action: "reconnect to HQ and try the response again".to_owned(),
        })
    }

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

    /// Continues one exact nonterminal project operation.
    fn continue_project_command(
        &mut self,
        _operation: UiProjectResult,
    ) -> Result<UiProjectResult, UiFailure> {
        Err(UiFailure {
            code: "project_operation_unavailable".to_owned(),
            action: "keep the operation open while HQ reconnects".to_owned(),
        })
    }
}

fn interaction_response_command_id(
    interaction: &UiInteraction,
    response: &InteractionResponseDto,
) -> Id32 {
    let mut digest = Sha256::new();
    digest.update(b"hq-tui-interaction-command-v1\0");
    digest.update(interaction.agent_id);
    digest.update(interaction.request_id);
    match response {
        InteractionResponseDto::Text(value) => {
            digest.update([1]);
            digest.update(value.as_bytes());
        }
        InteractionResponseDto::Choice(value) => {
            digest.update([2]);
            digest.update(value.as_bytes());
        }
        InteractionResponseDto::Approval(value) => digest.update([3, u8::from(*value)]),
        InteractionResponseDto::Cancelled => digest.update([4]),
    }
    Id32::new(digest.finalize().into())
}

/// Cross-thread capability that wakes one blocking observation read during shutdown.
pub trait TuiObservationInterrupt: Send + Sync {
    /// Interrupts the active observation wait. Repeated calls are harmless.
    fn interrupt(&self);
}

/// Cross-thread latest-value control for the dedicated observation owner.
pub trait TuiObservationControl: Send + Sync {
    /// Replaces the desired selected Inbox row without waiting for a response.
    fn select_conversation(&self, row_id: Option<String>);
}

#[derive(Clone, Copy, Debug, Default)]
struct NoopTuiObservationControl;

impl TuiObservationControl for NoopTuiObservationControl {
    fn select_conversation(&self, _row_id: Option<String>) {}
}

/// Dedicated subscribed observation capability owned independently from TUI commands.
pub trait TuiObservationPort: Send {
    /// Takes the coherent view obtained during subscription activation, when available.
    fn take_initial_view(&mut self) -> Option<UiMaterializedConversationView> {
        None
    }

    /// Blocks until subscribed work produces observations or a transport workflow boundary.
    fn next_observations(&mut self) -> Vec<TuiClientObservation>;

    /// Returns a capability that can wake the blocking observation owner from another thread.
    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt>;

    /// Returns a latest-value selection control independent from the command worker.
    fn control_handle(&self) -> Arc<dyn TuiObservationControl> {
        Arc::new(NoopTuiObservationControl)
    }
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
    client: LocalNodeClient,
    state: StatePaths,
    presentation: SharedTuiPresentation,
    project_operations: BTreeMap<[u8; 32], crate::local_client::LocalProjectResult>,
}

/// Subscribed local-API observation adapter with no command or query authority.
pub struct LocalTuiObserver {
    client: LocalNodeEventClient,
    observed_connection: Option<ClientConnectionState>,
    presentation: SharedTuiPresentation,
    initial_view: Option<UiMaterializedConversationView>,
    selection: Arc<Mutex<PendingConversationSelection>>,
    control: LocalTuiObservationControl,
    prior_inbox_rows: Vec<String>,
    desired_row: Option<String>,
    pending_interactions: Option<Vec<PendingInteractionDto>>,
}

#[derive(Clone)]
struct ConversationPresentationContext {
    context: ConversationContextDto,
    local_human: MailboxAddressDto,
}

#[derive(Clone, Default)]
struct SharedTuiPresentation {
    inner: Arc<Mutex<TuiPresentationData>>,
}

struct TuiPresentationData {
    conversation_keys: BTreeMap<String, ConversationKeyDto>,
    conversation_presentations: BTreeMap<String, ConversationPresentationContext>,
    providers: ProviderCatalogDto,
    agent_names: BTreeMap<[u8; 32], String>,
    project_names: BTreeMap<[u8; 32], String>,
    project_threads: Vec<ProjectThreadPresentation>,
    running_operations: BTreeMap<String, Vec<RunningOperationPresentation>>,
    mailbox_drafts: Vec<MailboxDraftDto>,
}

#[derive(Clone)]
struct ProjectThreadPresentation {
    project_id: [u8; 32],
    agent_id: [u8; 32],
    provider: String,
    session: String,
    thread_id: [u8; 32],
}

#[derive(Clone)]
struct RunningOperationPresentation {
    provider: String,
    session: String,
    operation_id: [u8; 32],
}

impl Default for TuiPresentationData {
    fn default() -> Self {
        Self {
            conversation_keys: BTreeMap::new(),
            conversation_presentations: BTreeMap::new(),
            providers: ProviderCatalogDto {
                providers: Vec::new(),
                default_provider: None,
            },
            agent_names: BTreeMap::new(),
            project_names: BTreeMap::new(),
            project_threads: Vec::new(),
            running_operations: BTreeMap::new(),
            mailbox_drafts: Vec::new(),
        }
    }
}

impl SharedTuiPresentation {
    fn replace_snapshot(&self, snapshot: &AuthoritativeSnapshotDto) {
        let Ok(mut presentation) = self.inner.lock() else {
            return;
        };
        presentation.conversation_keys = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::Conversation { key, .. } => {
                    Some((conversation_identity(key.clone()), key.clone()))
                }
                _ => None,
            })
            .collect();
        presentation.conversation_presentations = snapshot
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
        presentation.agent_names = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::Agent {
                    agent_id, names, ..
                } => Some((
                    agent_id.bytes(),
                    names.first().map_or_else(
                        || format!("Agent {}", short_id(*agent_id)),
                        |name| terminal_text(name),
                    ),
                )),
                _ => None,
            })
            .collect();
        presentation.project_names = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::Project {
                    project_id, name, ..
                } => Some((project_id.bytes(), terminal_text(name))),
                _ => None,
            })
            .collect();
        presentation.project_threads = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                SnapshotItem::ProjectThread {
                    project_id,
                    agent_id,
                    provider,
                    session,
                    thread_id,
                } => Some(ProjectThreadPresentation {
                    project_id: project_id.bytes(),
                    agent_id: agent_id.bytes(),
                    provider: provider.clone(),
                    session: session.clone(),
                    thread_id: thread_id.bytes(),
                }),
                _ => None,
            })
            .collect();
        let conversation_rows = presentation
            .conversation_keys
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        presentation
            .running_operations
            .retain(|row_id, _| conversation_rows.contains(row_id));
    }

    fn replace_mailbox_drafts(&self, drafts: Vec<MailboxDraftDto>) {
        if let Ok(mut presentation) = self.inner.lock() {
            presentation.mailbox_drafts = drafts;
        }
    }

    fn mailbox_drafts(&self) -> Vec<MailboxDraftDto> {
        self.inner.lock().map_or_else(
            |_| Vec::new(),
            |presentation| presentation.mailbox_drafts.clone(),
        )
    }

    fn upsert_mailbox_draft(&self, draft: MailboxDraftDto) {
        if let Ok(mut presentation) = self.inner.lock() {
            presentation
                .mailbox_drafts
                .retain(|candidate| candidate.draft_id != draft.draft_id);
            presentation.mailbox_drafts.push(draft);
        }
    }

    fn replace_providers(&self, providers: ProviderCatalogDto) {
        if let Ok(mut presentation) = self.inner.lock() {
            presentation.providers = providers;
        }
    }

    fn providers(&self) -> ProviderCatalogDto {
        self.inner.lock().map_or_else(
            |_| ProviderCatalogDto {
                providers: Vec::new(),
                default_provider: None,
            },
            |presentation| presentation.providers.clone(),
        )
    }

    fn conversation(
        &self,
        row_id: &str,
    ) -> Option<(ConversationKeyDto, ConversationPresentationContext)> {
        let presentation = self.inner.lock().ok()?;
        Some((
            presentation.conversation_keys.get(row_id)?.clone(),
            presentation.conversation_presentations.get(row_id)?.clone(),
        ))
    }

    fn agent_name(&self, agent_id: [u8; 32]) -> String {
        self.inner
            .lock()
            .ok()
            .and_then(|presentation| presentation.agent_names.get(&agent_id).cloned())
            .unwrap_or_else(|| format!("Agent {}", short_id(Id32::new(agent_id))))
    }

    fn project_name(&self, project_id: [u8; 32]) -> String {
        self.inner
            .lock()
            .ok()
            .and_then(|presentation| presentation.project_names.get(&project_id).cloned())
            .unwrap_or_else(|| format!("Project {}", short_id(Id32::new(project_id))))
    }

    fn replace_running_operations(&self, row_id: String, entries: &[ConversationEntryDto]) {
        let operations = entries
            .iter()
            .filter_map(|entry| match entry {
                ConversationEntryDto::Activity(activity)
                    if matches!(activity.status, ActivityStatusDto::Running) =>
                {
                    Some(RunningOperationPresentation {
                        provider: activity.provider.clone(),
                        session: activity.session.clone(),
                        operation_id: activity.operation.bytes(),
                    })
                }
                ConversationEntryDto::Message(_) | ConversationEntryDto::Activity(_) => None,
            })
            .collect();
        if let Ok(mut presentation) = self.inner.lock() {
            presentation.running_operations.insert(row_id, operations);
        }
    }

    fn interaction_target(&self, interaction: &PendingInteractionDto) -> UiInteractionTarget {
        let Ok(presentation) = self.inner.lock() else {
            return UiInteractionTarget::Unresolved {
                reason: UiInteractionTargetIssue::Missing,
            };
        };
        let mut candidates = BTreeSet::new();
        if let Some(project_id) = interaction.project_id.map(Id32::bytes) {
            for thread in &presentation.project_threads {
                if thread.project_id == project_id
                    && thread.agent_id == interaction.agent_id.bytes()
                    && thread.provider == interaction.provider
                    && thread.session == interaction.session
                {
                    let row_id = conversation_identity(ConversationKeyDto::ProjectThread {
                        project: Id32::new(thread.project_id),
                        thread: Id32::new(thread.thread_id),
                    });
                    if presentation.conversation_keys.contains_key(&row_id) {
                        candidates.insert(row_id);
                    }
                }
            }
        } else {
            for (row_id, key) in &presentation.conversation_keys {
                let ConversationKeyDto::ProviderSession {
                    provider, session, ..
                } = key
                else {
                    continue;
                };
                let Some(context) = presentation.conversation_presentations.get(row_id) else {
                    continue;
                };
                let ConversationContextDto::Direct { participant } = &context.context else {
                    continue;
                };
                if provider == &interaction.provider
                    && session == &interaction.session
                    && participant.agent.map(Id32::bytes) == Some(interaction.agent_id.bytes())
                {
                    candidates.insert(row_id.clone());
                }
            }
        }
        let mut candidates = candidates.into_iter();
        let Some(row_id) = candidates.next() else {
            return UiInteractionTarget::Unresolved {
                reason: UiInteractionTargetIssue::Missing,
            };
        };
        if candidates.next().is_some() {
            return UiInteractionTarget::Unresolved {
                reason: UiInteractionTargetIssue::Ambiguous,
            };
        }
        if let Some(operations) = presentation.running_operations.get(&row_id)
            && !operations.iter().any(|operation| {
                operation.provider == interaction.provider
                    && operation.session == interaction.session
                    && operation.operation_id == interaction.operation_id.bytes()
            })
        {
            return UiInteractionTarget::Unresolved {
                reason: UiInteractionTargetIssue::OperationMismatch,
            };
        }
        UiInteractionTarget::Conversation { row_id }
    }
}

#[derive(Clone)]
struct LocalTuiObservationControl {
    selection: Arc<Mutex<PendingConversationSelection>>,
    wake: UnixClientWake,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum PendingConversationSelection {
    #[default]
    Unchanged,
    Replace(Option<String>),
}

impl TuiObservationControl for LocalTuiObservationControl {
    fn select_conversation(&self, row_id: Option<String>) {
        if let Ok(mut selection) = self.selection.lock() {
            *selection = PendingConversationSelection::Replace(row_id);
        }
        self.wake.wake();
    }
}

impl LocalTuiClient {
    /// Wraps one already-ready ordinary local API command client.
    pub fn new(client: LocalNodeClient, state: StatePaths) -> Self {
        Self {
            client,
            state,
            presentation: SharedTuiPresentation {
                inner: Arc::new(Mutex::new(TuiPresentationData {
                    conversation_keys: BTreeMap::new(),
                    conversation_presentations: BTreeMap::new(),
                    providers: ProviderCatalogDto {
                        providers: Vec::new(),
                        default_provider: None,
                    },
                    agent_names: BTreeMap::new(),
                    project_names: BTreeMap::new(),
                    project_threads: Vec::new(),
                    running_operations: BTreeMap::new(),
                    mailbox_drafts: Vec::new(),
                })),
            },
            project_operations: BTreeMap::new(),
        }
    }

    fn with_presentation(
        client: LocalNodeClient,
        state: StatePaths,
        presentation: SharedTuiPresentation,
    ) -> Self {
        Self {
            client,
            state,
            presentation,
            project_operations: BTreeMap::new(),
        }
    }
}

impl LocalTuiObserver {
    /// Wraps one activated broad-invalidation subscription.
    pub fn new(client: LocalNodeEventClient) -> Self {
        Self::with_presentation(
            client,
            SharedTuiPresentation::default(),
            None,
            Vec::new(),
            None,
        )
    }

    fn with_presentation(
        client: LocalNodeEventClient,
        presentation: SharedTuiPresentation,
        initial_view: Option<UiMaterializedConversationView>,
        prior_inbox_rows: Vec<String>,
        desired_row: Option<String>,
    ) -> Self {
        let wake = client.wake_handle();
        let selection = Arc::new(Mutex::new(PendingConversationSelection::Unchanged));
        Self {
            client,
            observed_connection: None,
            presentation,
            initial_view,
            selection: Arc::clone(&selection),
            control: LocalTuiObservationControl { selection, wake },
            prior_inbox_rows,
            desired_row,
            pending_interactions: None,
        }
    }

    fn apply_latest_selection(&mut self) -> Result<(), UiFailure> {
        let replacement = std::mem::take(
            &mut *self
                .selection
                .lock()
                .map_err(|_| observation_control_failure())?,
        );
        let PendingConversationSelection::Replace(row_id) = replacement else {
            return Ok(());
        };
        let conversation = row_id
            .as_deref()
            .map(|row_id| {
                let (key, _) = self
                    .presentation
                    .conversation(row_id)
                    .ok_or_else(observation_control_failure)?;
                ConversationPageSelectionDto::new(key, 100)
                    .map_err(|_| observation_control_failure())
            })
            .transpose()?;
        self.client
            .update_subscription_conversation(conversation)
            .map_err(|error| client_failure(&error))?;
        self.desired_row = row_id;
        Ok(())
    }

    fn map_observation_result(
        &mut self,
        result: Result<Option<ClientEvent>, LocalNodeClientError>,
        state: ClientConnectionState,
        observations: &mut Vec<TuiClientObservation>,
    ) {
        match result {
            Ok(Some(ClientEvent::Snapshot(snapshot))) => {
                let local_installation = *self.client.installation_id().as_bytes();
                self.presentation.replace_snapshot(&snapshot);
                let providers = self.presentation.providers();
                let drafts = self.presentation.mailbox_drafts();
                let projects = match tui_project_catalog(&snapshot) {
                    Ok(projects) => projects,
                    Err(error) => {
                        observations.push(TuiClientObservation::Failure {
                            generation: connection_generation(state),
                            failure: project_failure(&error),
                        });
                        return;
                    }
                };
                observations.push(TuiClientObservation::MaterializedView(Box::new(
                    UiMaterializedConversationView {
                        snapshot: tui_snapshot_with_projects_and_drafts(
                            local_installation,
                            &snapshot,
                            projects,
                            &providers,
                            &drafts,
                        ),
                        conversation: None,
                    },
                )));
                self.remap_pending_interactions(observations);
            }
            Ok(Some(ClientEvent::AuthoritativeConversationView(view))) => {
                self.map_authoritative_conversation_view(view, state, observations);
            }
            Ok(Some(ClientEvent::Response {
                result: ResponseResult::PendingInteractions(interactions),
                ..
            })) => {
                self.pending_interactions = Some(interactions);
                self.remap_pending_interactions(observations);
            }
            Ok(
                Some(
                    ClientEvent::Response {
                        result: ResponseResult::InteractionResponder(_),
                        ..
                    }
                    | ClientEvent::IncompatibleVersion,
                )
                | None,
            ) => {}
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
    }

    fn map_authoritative_conversation_view(
        &mut self,
        view: AuthoritativeConversationViewDto,
        state: ClientConnectionState,
        observations: &mut Vec<TuiClientObservation>,
    ) {
        let local_installation = *self.client.installation_id().as_bytes();
        let next_rows = view
            .snapshot
            .items
            .iter()
            .filter_map(|item| snapshot_row(UiSection::Inbox, item).map(|row| row.id))
            .collect::<Vec<_>>();
        self.presentation.replace_snapshot(&view.snapshot);
        if let Some(desired) = self.desired_row.clone()
            && !next_rows.iter().any(|row_id| row_id == &desired)
            && !next_rows.is_empty()
        {
            self.select_successor_after_removal(&desired, next_rows, state, observations);
            self.remap_pending_interactions(observations);
            return;
        }
        if next_rows.is_empty() {
            self.desired_row = None;
        }
        self.prior_inbox_rows = next_rows;
        match tui_materialized_conversation_view(local_installation, view, &self.presentation) {
            Ok(view) => observations.push(TuiClientObservation::MaterializedView(Box::new(view))),
            Err(failure) => observations.push(TuiClientObservation::Failure {
                generation: connection_generation(state),
                failure,
            }),
        }
        self.remap_pending_interactions(observations);
    }

    fn remap_pending_interactions(&self, observations: &mut Vec<TuiClientObservation>) {
        let Some(interactions) = &self.pending_interactions else {
            return;
        };
        observations.push(TuiClientObservation::Interactions(
            interactions
                .iter()
                .cloned()
                .map(|interaction| tui_interaction(interaction, &self.presentation))
                .collect(),
        ));
    }

    fn select_successor_after_removal(
        &mut self,
        desired: &str,
        next_rows: Vec<String>,
        state: ClientConnectionState,
        observations: &mut Vec<TuiClientObservation>,
    ) {
        let prior_index = self
            .prior_inbox_rows
            .iter()
            .position(|row_id| row_id == desired)
            .unwrap_or(0);
        let successor = next_rows
            .iter()
            .skip(prior_index.min(next_rows.len() - 1))
            .chain(next_rows.iter())
            .find(|candidate| self.presentation.conversation(candidate).is_some())
            .cloned();
        let Some(successor) = successor else {
            if let Err(error) = self.client.update_subscription_conversation(None) {
                observations.push(TuiClientObservation::Failure {
                    generation: connection_generation(state),
                    failure: client_failure(&error),
                });
            } else {
                self.desired_row = None;
            }
            self.prior_inbox_rows = next_rows;
            return;
        };
        let selection = conversation_selection(&self.presentation, &successor);
        if let Err(failure) = selection.and_then(|selection| {
            self.client
                .update_subscription_conversation(Some(selection))
                .map_err(|error| client_failure(&error))
        }) {
            observations.push(TuiClientObservation::Failure {
                generation: connection_generation(state),
                failure,
            });
        } else {
            self.desired_row = Some(successor);
        }
        self.prior_inbox_rows = next_rows;
    }
}

/// Builds the installed command and observation adapters around one retained subscription base.
pub(crate) fn compose_tui_clients(
    mut command_client: LocalNodeClient,
    mut event_client: LocalNodeEventClient,
    state: StatePaths,
    subscription_base: &AuthoritativeSnapshotDto,
) -> Result<(LocalTuiClient, LocalTuiObserver), UiFailure> {
    let presentation = SharedTuiPresentation::default();
    presentation.replace_snapshot(subscription_base);
    let providers = request_provider_catalog(&mut command_client)?;
    presentation.replace_providers(providers.clone());
    let drafts = request_mailbox_drafts(&mut command_client)?;
    presentation.replace_mailbox_drafts(drafts.clone());
    let local_installation = *event_client.installation_id().as_bytes();
    let projects =
        tui_project_catalog(subscription_base).map_err(|error| project_failure(&error))?;
    let initial_snapshot = tui_snapshot_with_projects_and_drafts(
        local_installation,
        subscription_base,
        projects,
        &providers,
        &drafts,
    );
    let initial = activate_initial_tui_view(
        &mut event_client,
        &presentation,
        local_installation,
        initial_snapshot,
    )?;
    let client = LocalTuiClient::with_presentation(command_client, state, presentation.clone());
    let observer = LocalTuiObserver::with_presentation(
        event_client,
        presentation,
        Some(initial.view),
        initial.inbox_rows,
        initial.desired_row,
    );
    Ok((client, observer))
}

struct InitialTuiView {
    view: UiMaterializedConversationView,
    inbox_rows: Vec<String>,
    desired_row: Option<String>,
}

fn activate_initial_tui_view(
    event_client: &mut LocalNodeEventClient,
    presentation: &SharedTuiPresentation,
    local_installation: [u8; 32],
    initial_snapshot: UiSnapshot,
) -> Result<InitialTuiView, UiFailure> {
    let mut inbox_rows = initial_snapshot
        .inbox_rows
        .iter()
        .map(|row| row.id.clone())
        .collect::<Vec<_>>();
    let mut desired_row = initial_snapshot
        .inbox_rows
        .iter()
        .find(|row| row.kind == UiRowKind::Conversation)
        .map(|row| row.id.clone());
    let view = if let Some(row_id) = desired_row.clone() {
        let selection = conversation_selection(presentation, &row_id)?;
        event_client
            .update_subscription_conversation(Some(selection))
            .map_err(|error| client_failure(&error))?;
        loop {
            match event_client
                .next_observation()
                .map_err(|error| client_failure(&error))?
            {
                Some(ClientEvent::AuthoritativeConversationView(view)) => {
                    let materialized =
                        tui_materialized_conversation_view(local_installation, view, presentation)?;
                    if materialized
                        .conversation
                        .as_ref()
                        .is_some_and(|conversation| {
                            Some(&conversation.row_id) == desired_row.as_ref()
                        })
                    {
                        break materialized;
                    }
                    let next_rows = materialized
                        .snapshot
                        .inbox_rows
                        .iter()
                        .map(|row| row.id.clone())
                        .collect::<Vec<_>>();
                    if desired_row
                        .as_ref()
                        .is_some_and(|row_id| !next_rows.contains(row_id))
                    {
                        let prior_index = desired_row
                            .as_ref()
                            .and_then(|row_id| {
                                inbox_rows.iter().position(|candidate| candidate == row_id)
                            })
                            .unwrap_or(0);
                        let Some(successor) = next_rows
                            .iter()
                            .skip(prior_index.min(next_rows.len().saturating_sub(1)))
                            .chain(next_rows.iter())
                            .find(|candidate| presentation.conversation(candidate).is_some())
                            .cloned()
                        else {
                            desired_row = None;
                            inbox_rows = next_rows;
                            break materialized;
                        };
                        let selection = conversation_selection(presentation, &successor)?;
                        event_client
                            .update_subscription_conversation(Some(selection))
                            .map_err(|error| client_failure(&error))?;
                        desired_row = Some(successor);
                    }
                    inbox_rows = next_rows;
                }
                Some(ClientEvent::Snapshot(snapshot)) => presentation.replace_snapshot(&snapshot),
                Some(ClientEvent::IncompatibleVersion | ClientEvent::Error { .. }) => {
                    return Err(UiFailure {
                        code: "subscription_activation_failed".to_owned(),
                        action: "restart HQ and reopen the Inbox".to_owned(),
                    });
                }
                Some(
                    ClientEvent::Mutation(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_),
                )
                | None => {}
            }
        }
    } else {
        UiMaterializedConversationView {
            snapshot: initial_snapshot,
            conversation: None,
        }
    };
    Ok(InitialTuiView {
        view,
        inbox_rows,
        desired_row,
    })
}

fn conversation_selection(
    presentation: &SharedTuiPresentation,
    row_id: &str,
) -> Result<ConversationPageSelectionDto, UiFailure> {
    let (key, _) = presentation
        .conversation(row_id)
        .ok_or_else(observation_control_failure)?;
    ConversationPageSelectionDto::new(key, 100).map_err(|_| observation_control_failure())
}

fn tui_interaction(
    interaction: PendingInteractionDto,
    presentation: &SharedTuiPresentation,
) -> UiInteraction {
    let agent_id = interaction.agent_id.bytes();
    let project_id = interaction.project_id.map(Id32::bytes);
    let kind = match interaction.kind {
        InteractionKindDto::Question => UiInteractionKind::Question,
        InteractionKindDto::CommandApproval => UiInteractionKind::CommandApproval,
        InteractionKindDto::FileApproval => UiInteractionKind::FileApproval,
        InteractionKindDto::Permission => UiInteractionKind::Permission,
        InteractionKindDto::McpUrl => UiInteractionKind::McpUrl,
        InteractionKindDto::McpForm => UiInteractionKind::McpForm,
    };
    let target = if kind == UiInteractionKind::CommandApproval {
        presentation.interaction_target(&interaction)
    } else {
        UiInteractionTarget::Modal
    };
    UiInteraction {
        agent_id,
        agent_name: presentation.agent_name(agent_id),
        project_id,
        project_name: project_id.map(|project_id| presentation.project_name(project_id)),
        provider: terminal_text(&interaction.provider),
        session: terminal_text(&interaction.session),
        request_id: interaction.request_id.bytes(),
        operation_id: interaction.operation_id.bytes(),
        kind,
        prompt: terminal_text(&interaction.prompt),
        choices: interaction
            .choices
            .into_iter()
            .map(|choice| UiInteractionChoice {
                label: interaction_choice_label(&choice.value, &choice.label),
                value: choice.value,
            })
            .collect(),
        allow_text: interaction.allow_text,
        target,
    }
}

fn interaction_choice_label(value: &str, provider_label: &str) -> String {
    match value {
        "accept" => "Allow once".to_owned(),
        "acceptForSession" | "grantSession" => "Allow for this session".to_owned(),
        "acceptWithExecpolicyAmendment" => "Allow with the proposed command rule".to_owned(),
        "decline" => "Deny".to_owned(),
        "cancel" => "Cancel".to_owned(),
        "grantTurn" => "Allow for this turn".to_owned(),
        _ => terminal_text(provider_label),
    }
}

fn request_provider_catalog(client: &mut LocalNodeClient) -> Result<ProviderCatalogDto, UiFailure> {
    let ClientEvent::Response {
        result: ResponseResult::ProviderCatalog(providers),
        ..
    } = client
        .request(Request::ProviderCatalog)
        .map_err(|error| client_failure(&error))?
    else {
        return Err(UiFailure {
            code: "provider_catalog_protocol".to_owned(),
            action: "restart HQ and reload the available agent services".to_owned(),
        });
    };
    Ok(providers)
}

fn request_mailbox_drafts(client: &mut LocalNodeClient) -> Result<Vec<MailboxDraftDto>, UiFailure> {
    let ClientEvent::Response {
        result: ResponseResult::MailboxDrafts(drafts),
        ..
    } = client
        .request(Request::MailboxDrafts)
        .map_err(|error| client_failure(&error))?
    else {
        return Err(UiFailure {
            code: "mailbox_drafts_protocol".to_owned(),
            action: "restart HQ and reload saved message drafts".to_owned(),
        });
    };
    Ok(drafts)
}

fn observation_control_failure() -> UiFailure {
    UiFailure {
        code: "conversation_selection_stale".to_owned(),
        action: "reload the Inbox and select the conversation again".to_owned(),
    }
}

impl TuiObservationInterrupt for UnixClientInterrupt {
    fn interrupt(&self) {
        UnixClientInterrupt::interrupt(self);
    }
}

impl TuiClientPort for LocalTuiClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        let local_installation = *self.client.installation_id().as_bytes();
        let snapshot = self
            .client
            .snapshot()
            .map_err(|error| client_failure(&error))?;
        self.presentation.replace_snapshot(&snapshot);
        let projects = tui_project_catalog(&snapshot).map_err(|error| project_failure(&error))?;
        let providers = request_provider_catalog(&mut self.client)?;
        self.presentation.replace_providers(providers.clone());
        let drafts = request_mailbox_drafts(&mut self.client)?;
        self.presentation.replace_mailbox_drafts(drafts.clone());
        Ok(tui_snapshot_with_projects_and_drafts(
            local_installation,
            &snapshot,
            projects,
            &providers,
            &drafts,
        ))
    }

    fn load_configuration(&mut self) -> Result<UiConfiguration, UiFailure> {
        let configuration = local_configuration(
            self.client
                .configuration()
                .map_err(|_| configuration_unavailable())?,
        )?;
        tui_configuration(&configuration)
    }

    fn save_configuration(
        &mut self,
        field: UiConfigField,
        configuration: UiConfiguration,
        apply_theme: bool,
    ) -> Result<(UiConfiguration, Option<UiTheme>), UiFailure> {
        let patch = match field {
            UiConfigField::Theme => {
                InstallationConfigurationPatchDto::Theme(configuration.theme.clone())
            }
            UiConfigField::DefaultProvider => InstallationConfigurationPatchDto::DefaultProvider(
                configuration.default_provider.clone(),
            ),
            UiConfigField::CodexModel => {
                InstallationConfigurationPatchDto::CodexModel(configuration.codex_model.clone())
            }
            UiConfigField::CodexYolo => {
                InstallationConfigurationPatchDto::CodexYolo(configuration.codex_yolo)
            }
        };
        if let InstallationConfigurationPatchDto::Theme(Some(selection)) = &patch {
            ThemeSelection::new(selection.clone()).map_err(|_| configuration_failure())?;
        }
        let persisted = local_configuration(
            self.client
                .update_configuration(patch)
                .map_err(|_| configuration_unavailable())?,
        )?;
        let environment = TuiThemeEnvironment::from_environment();
        let theme = apply_theme
            .then(|| resolve_tui_theme(persisted.theme.as_ref(), &environment))
            .transpose()
            .map_err(|_| configuration_failure())?;
        Ok((tui_configuration(&persisted)?, theme))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        let (key, presentation) =
            self.presentation
                .conversation(row_id)
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

    fn answer_interaction(
        &mut self,
        interaction: UiInteraction,
        response: UiInteractionResponse,
    ) -> Result<UiInteractionAnswerOutcome, UiFailure> {
        let response = match response {
            UiInteractionResponse::Text(value) => InteractionResponseDto::Text(value),
            UiInteractionResponse::Choice(value) => InteractionResponseDto::Choice(value),
            UiInteractionResponse::Approval(value) => InteractionResponseDto::Approval(value),
            UiInteractionResponse::Cancelled => InteractionResponseDto::Cancelled,
        };
        let command_id = interaction_response_command_id(&interaction, &response);
        let event = self
            .client
            .request(Request::AnswerInteraction(InteractionAnswerRequestDto {
                command_id,
                agent_id: Id32::new(interaction.agent_id),
                request_id: Id32::new(interaction.request_id),
                response,
            }))
            .map_err(|error| client_failure(&error))?;
        match event {
            ClientEvent::Response {
                result: ResponseResult::InteractionAnswer(InteractionAnswerOutcomeDto::Answered),
                ..
            } => Ok(UiInteractionAnswerOutcome::Answered),
            ClientEvent::Response {
                result: ResponseResult::InteractionAnswer(InteractionAnswerOutcomeDto::Stale),
                ..
            } => Ok(UiInteractionAnswerOutcome::Stale),
            _ => Err(UiFailure {
                code: "interaction_response_invalid".to_owned(),
                action: "reload pending requests and try again".to_owned(),
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
            self.presentation.upsert_mailbox_draft(draft.clone());
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
            } => {
                self.presentation.upsert_mailbox_draft(draft.clone());
                Ok(tui_draft(draft))
            }
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
            } => {
                self.presentation.upsert_mailbox_draft(draft.clone());
                Ok(tui_draft(draft))
            }
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
        let authors_message = matches!(
            action,
            UiMailboxAction::Reply { .. }
                | UiMailboxAction::Direct { .. }
                | UiMailboxAction::SelfNote
                | UiMailboxAction::Project { .. }
        );
        let message_id = if authors_message {
            Id32::new(
                draft
                    .as_ref()
                    .ok_or_else(|| UiFailure {
                        code: "mailbox_draft_missing".to_owned(),
                        action: "reopen the message editor and try again".to_owned(),
                    })?
                    .draft_id,
            )
        } else {
            Id32::new([0; 32])
        };
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
        let result = execute_project_command(&mut self.client, &self.state, command)
            .map_err(|error| project_failure(&error))?;
        if matches!(result.outcome, LocalProjectOutcome::Running { .. }) {
            self.project_operations
                .insert(result.command_id, result.clone());
        }
        Ok(ui_project_result(action, result))
    }

    fn continue_project_command(
        &mut self,
        operation: UiProjectResult,
    ) -> Result<UiProjectResult, UiFailure> {
        let current = self
            .project_operations
            .get(&operation.command_id)
            .filter(|current| {
                current.operation_id == operation.operation_id
                    && current.project_id == operation.project_id
                    && current.command == local_project_command(&operation.action)
            })
            .cloned()
            .ok_or_else(|| UiFailure {
                code: "project_operation_stale".to_owned(),
                action: "keep the operation open while HQ reloads its exact status".to_owned(),
            })?;
        let result = continue_project_command(&mut self.client, &current)
            .map_err(|error| project_failure(&error))?;
        if matches!(result.outcome, LocalProjectOutcome::Running { .. }) {
            self.project_operations
                .insert(result.command_id, result.clone());
        } else {
            self.project_operations.remove(&result.command_id);
        }
        Ok(ui_project_result(operation.action, result))
    }
}

fn tui_configuration(configuration: &LocalConfiguration) -> Result<UiConfiguration, UiFailure> {
    let environment = TuiThemeEnvironment::from_environment();
    let mut themes = vec![UiThemeChoice {
        selector: None,
        name: "Automatic".to_owned(),
        source: "default".to_owned(),
        error: None,
    }];
    themes.extend(
        list_tui_themes(configuration.theme.as_ref(), &environment)
            .map_err(|_| configuration_failure())?
            .into_iter()
            .map(|entry| UiThemeChoice {
                selector: Some(entry.selector),
                name: entry.name,
                source: entry.source,
                error: entry.error,
            }),
    );
    Ok(UiConfiguration {
        default_provider: configuration
            .default_provider
            .as_ref()
            .map(|provider| provider.as_str().to_owned()),
        theme: configuration
            .theme
            .as_ref()
            .map(|theme| theme.as_str().to_owned()),
        codex_model: configuration.codex.model.clone(),
        codex_yolo: configuration.codex.yolo,
        themes,
    })
}

fn local_configuration(
    configuration: InstallationConfigurationDto,
) -> Result<LocalConfiguration, UiFailure> {
    let provider = configuration
        .default_provider
        .map(ProviderId::new)
        .transpose()
        .map_err(|_| configuration_failure())?;
    let theme = configuration
        .theme
        .map(ThemeSelection::new)
        .transpose()
        .map_err(|_| configuration_failure())?;
    let codex = LocalCodexConfiguration::new(configuration.codex_yolo, configuration.codex_model)
        .map_err(|_| configuration_failure())?;
    LocalConfiguration::from_parts(provider, theme, codex).map_err(|_| configuration_failure())
}

fn configuration_failure() -> UiFailure {
    UiFailure {
        code: "configuration_invalid".to_owned(),
        action: "review the edited setting and try again".to_owned(),
    }
}

fn configuration_unavailable() -> UiFailure {
    UiFailure {
        code: "configuration_unavailable".to_owned(),
        action: "reconnect to HQ and reload Config before trying again".to_owned(),
    }
}

impl TuiObservationPort for LocalTuiObserver {
    fn take_initial_view(&mut self) -> Option<UiMaterializedConversationView> {
        self.initial_view.take()
    }

    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        if let Err(failure) = self.apply_latest_selection() {
            return vec![TuiClientObservation::Failure {
                generation: connection_generation(self.client.connection_state()),
                failure,
            }];
        }
        let current = self.client.connection_state();
        if self.observed_connection != Some(current) {
            self.observed_connection = Some(current);
            let (generation, state) = connection_observation(current);
            return vec![TuiClientObservation::Connection {
                generation,
                state,
                cause: None,
            }];
        }
        let result = self.client.next_observation();
        let state = self.client.connection_state();
        let reconnect_cause = self.client.take_reconnect_cause().map(ui_reconnect_cause);
        let mut observations = Vec::new();
        if self.observed_connection != Some(state) {
            self.observed_connection = Some(state);
            let (generation, state) = connection_observation(state);
            observations.push(TuiClientObservation::Connection {
                generation,
                state,
                cause: reconnect_cause,
            });
        }
        self.map_observation_result(result, state, &mut observations);
        observations
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.client.interrupt_handle())
    }

    fn control_handle(&self) -> Arc<dyn TuiObservationControl> {
        Arc::new(self.control.clone())
    }
}

const fn ui_reconnect_cause(cause: ClientReconnectCause) -> UiReconnectCause {
    let (operation, kind) = match cause {
        ClientReconnectCause::Connect(kind) => (UiReconnectOperation::Connect, kind),
        ClientReconnectCause::Read(kind) => (UiReconnectOperation::Read, kind),
        ClientReconnectCause::Write(kind) => (UiReconnectOperation::Write, kind),
    };
    let kind = match kind {
        ClientTransportFailureKind::Unavailable => UiReconnectFailureKind::Unavailable,
        ClientTransportFailureKind::Transport => UiReconnectFailureKind::Transport,
        ClientTransportFailureKind::Protocol => UiReconnectFailureKind::Protocol,
    };
    UiReconnectCause { operation, kind }
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
        UiMailboxDraftTarget::ProjectSetup {
            project_id,
            agent_id,
            provider,
        } => MailboxDraftTargetDto::ProjectSetup {
            project_id: Id32::new(*project_id),
            agent_id: Id32::new(*agent_id),
            provider: provider.clone(),
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
        MailboxDraftTargetDto::ProjectSetup {
            project_id,
            agent_id,
            provider,
        } => UiMailboxDraftTarget::ProjectSetup {
            project_id: project_id.bytes(),
            agent_id: agent_id.bytes(),
            provider: provider.clone(),
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
    LoadConfiguration {
        id: EffectId,
    },
    SaveConfiguration {
        id: EffectId,
        field: UiConfigField,
        configuration: UiConfiguration,
        apply_theme: bool,
    },
    LoadConversation {
        id: EffectId,
        row_id: String,
        cursor: Option<String>,
    },
    AnswerInteraction {
        id: EffectId,
        interaction: UiInteraction,
        response: UiInteractionResponse,
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
    ContinueProjectCommand {
        id: EffectId,
        operation: UiProjectResult,
    },
    Shutdown,
}

/// Single bounded executor for client, timer, redraw, and exit effects.
pub struct TuiEffectExecutor<C: TuiClock> {
    clock: C,
    commands: SyncSender<WorkerCommand>,
    events: Receiver<UiEvent>,
    event_wake: TuiEventWake,
    workers: Option<TuiWorkers>,
    cancellation: Arc<AtomicBool>,
    worker_stopped: Arc<AtomicBool>,
    observation_interrupt: Arc<dyn TuiObservationInterrupt>,
    observation_control: Arc<dyn TuiObservationControl>,
    timers: Vec<ScheduledTimer>,
    outstanding_snapshots: Vec<EffectId>,
    redraw_pending: bool,
    exit_requested: bool,
}

/// Readable OS wake source paired with the executor's worker-event queue.
pub struct TuiEventWake {
    reader: UnixStream,
}

#[derive(Clone)]
struct TuiEventNotifier {
    writer: Arc<UnixStream>,
}

impl TuiEventWake {
    fn pair() -> Result<(Self, TuiEventNotifier), TuiExecutorError> {
        let (reader, writer) = UnixStream::pair().map_err(|_| TuiExecutorError::WorkerSpawn)?;
        reader
            .set_nonblocking(true)
            .map_err(|_| TuiExecutorError::WorkerSpawn)?;
        writer
            .set_nonblocking(true)
            .map_err(|_| TuiExecutorError::WorkerSpawn)?;
        Ok((
            Self { reader },
            TuiEventNotifier {
                writer: Arc::new(writer),
            },
        ))
    }

    /// Borrows the readable descriptor for an outer event-loop wait.
    pub fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.reader.as_fd()
    }

    fn drain(&mut self) {
        let mut bytes = [0_u8; 64];
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
}

impl TuiEventNotifier {
    fn notify(&self) {
        let _ = (&*self.writer).write(&[1]);
    }
}

struct TuiWorkerExitWake {
    notifier: TuiEventNotifier,
    stopped: Arc<AtomicBool>,
    cancellation: Arc<AtomicBool>,
}

impl Drop for TuiWorkerExitWake {
    fn drop(&mut self) {
        if !self.cancellation.load(Ordering::SeqCst) {
            self.stopped.store(true, Ordering::SeqCst);
            self.notifier.notify();
        }
    }
}

struct TuiWorkers {
    commands: JoinHandle<()>,
    observations: JoinHandle<()>,
}

impl<C: TuiClock> TuiEffectExecutor<C> {
    /// Starts a command worker with an interruptible parked observation owner.
    pub fn spawn<P: TuiClientPort + 'static>(
        client: P,
        clock: C,
    ) -> Result<Self, TuiExecutorError> {
        Self::spawn_with_observer(client, ParkedTuiObserver::default(), clock)
    }

    /// Starts independent named workers for commands and subscribed observations.
    pub fn spawn_with_observer<P: TuiClientPort + 'static, O: TuiObservationPort + 'static>(
        client: P,
        observer: O,
        clock: C,
    ) -> Result<Self, TuiExecutorError> {
        let (commands, command_receiver) = mpsc::sync_channel(CLIENT_COMMAND_CAPACITY);
        let (event_sender, events) = mpsc::sync_channel(CLIENT_EVENT_CAPACITY);
        let (event_wake, event_notifier) = TuiEventWake::pair()?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::new(AtomicBool::new(false));
        let command_cancellation = Arc::clone(&cancellation);
        let command_events = event_sender.clone();
        let command_notifier = event_notifier.clone();
        let command_stopped = Arc::clone(&worker_stopped);
        let command_worker = thread::Builder::new()
            .name("hq-tui-commands".to_owned())
            .spawn(move || {
                let _exit_wake = TuiWorkerExitWake {
                    notifier: command_notifier.clone(),
                    stopped: command_stopped,
                    cancellation: Arc::clone(&command_cancellation),
                };
                client_worker(
                    client,
                    &command_receiver,
                    &command_events,
                    &command_cancellation,
                    &command_notifier,
                );
            })
            .map_err(|_| TuiExecutorError::WorkerSpawn)?;
        let observation_interrupt = observer.interrupt_handle();
        let observation_control = observer.control_handle();
        let observation_cancellation = Arc::clone(&cancellation);
        let observation_notifier = event_notifier;
        let observation_stopped = Arc::clone(&worker_stopped);
        let Ok(observation_worker) = thread::Builder::new()
            .name("hq-tui-observations".to_owned())
            .spawn(move || {
                let _exit_wake = TuiWorkerExitWake {
                    notifier: observation_notifier.clone(),
                    stopped: observation_stopped,
                    cancellation: Arc::clone(&observation_cancellation),
                };
                observation_worker(
                    observer,
                    &event_sender,
                    &observation_cancellation,
                    &observation_notifier,
                );
            })
        else {
            drop(commands);
            let _ = command_worker.join();
            return Err(TuiExecutorError::WorkerSpawn);
        };
        Ok(Self {
            clock,
            commands,
            events,
            event_wake,
            workers: Some(TuiWorkers {
                commands: command_worker,
                observations: observation_worker,
            }),
            cancellation,
            worker_stopped,
            observation_interrupt,
            observation_control,
            timers: Vec::new(),
            outstanding_snapshots: Vec::new(),
            redraw_pending: false,
            exit_requested: false,
        })
    }

    /// Executes ordered pure-model effects without changing the model.
    #[allow(clippy::too_many_lines)]
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
                UiEffect::LoadConfiguration { id } => {
                    self.enqueue_client_effect(id, WorkerCommand::LoadConfiguration { id })?;
                }
                UiEffect::SaveConfiguration {
                    id,
                    field,
                    configuration,
                    apply_theme,
                } => {
                    self.enqueue_client_effect(
                        id,
                        WorkerCommand::SaveConfiguration {
                            id,
                            field,
                            configuration,
                            apply_theme,
                        },
                    )?;
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
                UiEffect::ObserveConversation { row_id } => {
                    self.observation_control.select_conversation(row_id);
                }
                UiEffect::AnswerInteraction {
                    id,
                    interaction,
                    response,
                } => {
                    self.enqueue_client_effect(
                        id,
                        WorkerCommand::AnswerInteraction {
                            id,
                            interaction,
                            response,
                        },
                    )?;
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
                UiEffect::ContinueProjectCommand { id, operation } => {
                    self.enqueue_client_effect(
                        id,
                        WorkerCommand::ContinueProjectCommand { id, operation },
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
        self.event_wake.drain();
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

    /// Borrows the worker-event wake source used by the outer event loop.
    pub const fn event_wake(&self) -> &TuiEventWake {
        &self.event_wake
    }

    /// Reports an unexpected worker exit before executor cancellation.
    pub fn worker_stopped(&self) -> bool {
        self.worker_stopped.load(Ordering::SeqCst)
    }

    /// Returns the exact delay until the next scheduled timer, if any.
    pub fn time_until_event(&self) -> Option<Duration> {
        self.timers
            .first()
            .map(|timer| timer.deadline.saturating_sub(self.clock.now()))
    }

    /// Stops and joins the worker, draining bounded results while it exits.
    pub fn shutdown(&mut self) -> Result<(), TuiExecutorError> {
        let Some(workers) = self.workers.take() else {
            return Ok(());
        };
        self.cancellation.store(true, Ordering::SeqCst);
        self.observation_interrupt.interrupt();
        let mut command = WorkerCommand::Shutdown;
        loop {
            match self.commands.try_send(command) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(returned)) => {
                    command = returned;
                    while self.events.try_recv().is_ok() {}
                    if workers.commands.is_finished() {
                        break;
                    }
                    thread::yield_now();
                }
            }
        }
        while !workers.commands.is_finished() || !workers.observations.is_finished() {
            while self.events.try_recv().is_ok() {}
            thread::yield_now();
        }
        let command_result = workers.commands.join();
        let observation_result = workers.observations.join();
        if command_result.is_err() || observation_result.is_err() {
            Err(TuiExecutorError::WorkerPanicked)
        } else {
            Ok(())
        }
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
            | UiEvent::ConfigurationLoaded { effect_id, .. }
            | UiEvent::ConfigurationSaved { effect_id, .. }
            | UiEvent::ConfigurationFailed { effect_id, .. }
            | UiEvent::ConversationLoaded { effect_id, .. }
            | UiEvent::ConversationFailed { effect_id, .. }
            | UiEvent::InteractionAnswered { effect_id, .. }
            | UiEvent::InteractionAnswerFailed { effect_id, .. }
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
            | UiEvent::ConversationViewportObserved { .. }
            | UiEvent::TimerElapsed { .. }
            | UiEvent::MaterializedViewObserved { .. }
            | UiEvent::InteractionsObserved { .. }
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

#[derive(Clone, Default)]
struct ParkedTuiInterrupt {
    interrupted: Arc<(Mutex<bool>, Condvar)>,
}

#[derive(Default)]
struct ParkedTuiObserver {
    interrupt: ParkedTuiInterrupt,
}

impl TuiObservationInterrupt for ParkedTuiInterrupt {
    fn interrupt(&self) {
        let (interrupted, wake) = &*self.interrupted;
        if let Ok(mut interrupted) = interrupted.lock() {
            *interrupted = true;
            wake.notify_all();
        }
    }
}

impl TuiObservationPort for ParkedTuiObserver {
    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        let (interrupted, wake) = &*self.interrupt.interrupted;
        if let Ok(interrupted) = interrupted.lock() {
            drop(wake.wait_while(interrupted, |interrupted| !*interrupted));
        }
        Vec::new()
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.interrupt.clone())
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
    notifier: &TuiEventNotifier,
) {
    loop {
        if cancellation.load(Ordering::SeqCst) {
            break;
        }
        match commands.recv() {
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
                if !send_tui_event(events, notifier, event) {
                    break;
                }
            }
            Ok(WorkerCommand::LoadConfiguration { id }) => {
                let event = match client.load_configuration() {
                    Ok(configuration) => UiEvent::ConfigurationLoaded {
                        effect_id: id,
                        configuration,
                    },
                    Err(failure) => UiEvent::ConfigurationFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if !send_tui_event(events, notifier, event) {
                    break;
                }
            }
            Ok(WorkerCommand::SaveConfiguration {
                id,
                field,
                configuration,
                apply_theme,
            }) => {
                let event = match client.save_configuration(field, configuration, apply_theme) {
                    Ok((configuration, theme)) => UiEvent::ConfigurationSaved {
                        effect_id: id,
                        configuration,
                        theme,
                    },
                    Err(failure) => UiEvent::ConfigurationFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if !send_tui_event(events, notifier, event) {
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
                if !send_tui_event(events, notifier, event) {
                    break;
                }
            }
            Ok(WorkerCommand::AnswerInteraction {
                id,
                interaction,
                response,
            }) => {
                let request_id = interaction.request_id;
                let event = match client.answer_interaction(interaction, response) {
                    Ok(outcome) => UiEvent::InteractionAnswered {
                        effect_id: id,
                        request_id,
                        outcome,
                    },
                    Err(failure) => UiEvent::InteractionAnswerFailed {
                        effect_id: id,
                        request_id,
                        failure,
                    },
                };
                if !send_tui_event(events, notifier, event) {
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
                if !send_tui_event(events, notifier, event) {
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
                if !send_tui_event(events, notifier, event) {
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
                if !send_tui_event(events, notifier, event) {
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
                if !send_tui_event(events, notifier, event) {
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
                if !send_tui_event(events, notifier, event) {
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
                if !send_tui_event(events, notifier, event) {
                    break;
                }
            }
            Ok(WorkerCommand::ContinueProjectCommand { id, operation }) => {
                let event = match client.continue_project_command(operation) {
                    Ok(result) => UiEvent::ProjectCommandCompleted {
                        effect_id: id,
                        result,
                    },
                    Err(failure) => UiEvent::ProjectCommandFailed {
                        effect_id: id,
                        failure,
                    },
                };
                if !send_tui_event(events, notifier, event) {
                    break;
                }
            }
            Ok(WorkerCommand::Shutdown) | Err(mpsc::RecvError) => break,
        }
    }
}

fn observation_worker<O: TuiObservationPort>(
    mut observer: O,
    events: &SyncSender<UiEvent>,
    cancellation: &AtomicBool,
    notifier: &TuiEventNotifier,
) {
    while !cancellation.load(Ordering::SeqCst) {
        let observations = observer.next_observations();
        if cancellation.load(Ordering::SeqCst) {
            break;
        }
        for observation in observations {
            let event = match observation {
                TuiClientObservation::MaterializedView(view) => {
                    UiEvent::MaterializedViewObserved { view: *view }
                }
                TuiClientObservation::Interactions(interactions) => {
                    UiEvent::InteractionsObserved { interactions }
                }
                TuiClientObservation::Invalidated { revision } => UiEvent::Invalidated { revision },
                TuiClientObservation::Connection {
                    generation,
                    state,
                    cause,
                } => UiEvent::ConnectionObserved {
                    generation,
                    state,
                    cause,
                },
                TuiClientObservation::Failure {
                    generation,
                    failure,
                } => UiEvent::ClientFailed {
                    generation,
                    failure,
                },
            };
            if !send_tui_event(events, notifier, event) {
                return;
            }
        }
    }
}

fn send_tui_event(
    events: &SyncSender<UiEvent>,
    notifier: &TuiEventNotifier,
    event: UiEvent,
) -> bool {
    if events.send(event).is_err() {
        return false;
    }
    notifier.notify();
    true
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

fn tui_materialized_conversation_view(
    local_installation: [u8; 32],
    view: AuthoritativeConversationViewDto,
    presentation: &SharedTuiPresentation,
) -> Result<UiMaterializedConversationView, UiFailure> {
    presentation.replace_snapshot(&view.snapshot);
    let providers = presentation.providers();
    let drafts = presentation.mailbox_drafts();
    let projects = tui_project_catalog(&view.snapshot).map_err(|error| project_failure(&error))?;
    let snapshot = tui_snapshot_with_projects_and_drafts(
        local_installation,
        &view.snapshot,
        projects,
        &providers,
        &drafts,
    );
    let conversation = view
        .conversation
        .map(|selected| {
            let row_id = conversation_identity(selected.key.clone());
            presentation.replace_running_operations(row_id.clone(), &selected.page.items);
            let (_, context) = presentation
                .conversation(&row_id)
                .ok_or_else(observation_control_failure)?;
            Ok(tui_conversation_page(
                &row_id,
                &context.context,
                &context.local_human,
                selected.page,
            ))
        })
        .transpose()?;
    Ok(UiMaterializedConversationView {
        snapshot,
        conversation,
    })
}

fn tui_snapshot_with_projects(
    local_installation: [u8; 32],
    snapshot: &AuthoritativeSnapshotDto,
    projects: Vec<LocalProject>,
    provider_catalog: &ProviderCatalogDto,
) -> UiSnapshot {
    tui_snapshot_with_projects_and_drafts(
        local_installation,
        snapshot,
        projects,
        provider_catalog,
        &[],
    )
}

#[allow(clippy::too_many_lines)]
fn tui_snapshot_with_projects_and_drafts(
    local_installation: [u8; 32],
    snapshot: &AuthoritativeSnapshotDto,
    projects: Vec<LocalProject>,
    provider_catalog: &ProviderCatalogDto,
    drafts: &[MailboxDraftDto],
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
    let mut inbox_rows: Vec<UiRow> = rows(UiSection::Inbox);
    let project_setups = tui_project_setups(drafts, &projects, &agents, &providers);
    let setup_rows = tui_project_setup_rows(&project_setups, &inbox_rows);
    inbox_rows.extend(setup_rows);
    UiSnapshot {
        revision: snapshot.revision,
        human_state,
        inbox_rows,
        sent_rows: rows(UiSection::Sent),
        archived_rows: rows(UiSection::Archived),
        agent_rows: agents.iter().map(agent_row).collect(),
        project_rows: rows(UiSection::Projects),
        direct_targets,
        providers,
        agents,
        projects,
        project_setups,
    }
}

fn tui_project_setup_rows(
    setups: &[UiProjectConversationSetup],
    authoritative_rows: &[UiRow],
) -> Vec<UiRow> {
    setups
        .iter()
        .filter(|setup| {
            !authoritative_rows.iter().any(|row| {
                matches!(
                    (&setup.draft.target, row.conversation_target),
                    (
                        UiMailboxDraftTarget::ProjectSetup { project_id, .. },
                        Some(UiConversationTarget::Project {
                            project_id: candidate_project,
                            root_message,
                            ..
                        })
                    ) if *project_id == candidate_project && setup.draft.draft_id == root_message
                )
            })
        })
        .map(|setup| UiRow {
            id: format!("project-setup:{}", full_id(Id32::new(setup.draft.draft_id))),
            title: format!("{} · {}", setup.agent_name, setup.project_name),
            detail: "Conversation not started".to_owned(),
            state: UiRowState::Open,
            kind: UiRowKind::ConversationSetup,
            conversation_target: None,
        })
        .collect()
}

fn tui_project_setups(
    drafts: &[MailboxDraftDto],
    projects: &[UiProject],
    agents: &[UiAgent],
    providers: &[UiProvider],
) -> Vec<UiProjectConversationSetup> {
    drafts
        .iter()
        .filter_map(|draft| {
            let MailboxDraftTargetDto::ProjectSetup {
                project_id,
                agent_id,
                provider,
            } = &draft.target
            else {
                return None;
            };
            let project = projects
                .iter()
                .find(|project| project.project_id == project_id.bytes())?;
            if project.assignment.as_ref().is_some_and(|assignment| {
                assignment.agent_id == agent_id.bytes() && assignment.runnable
            }) {
                return None;
            }
            let project_name = project.name.clone();
            let agent_name = agents
                .iter()
                .find(|agent| agent.agent_id == agent_id.bytes())?
                .names
                .first()?
                .clone();
            let provider_name = providers
                .iter()
                .find(|candidate| candidate.provider == *provider)
                .map_or_else(|| provider.clone(), |candidate| candidate.name.clone());
            Some(UiProjectConversationSetup {
                draft: tui_draft(draft.clone()),
                project_name,
                agent_name,
                provider_name,
            })
        })
        .collect()
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

fn ui_project_result(
    action: UiProjectAction,
    result: crate::local_client::LocalProjectResult,
) -> UiProjectResult {
    UiProjectResult {
        action,
        command_id: result.command_id,
        operation_id: result.operation_id,
        project_id: result.project_id,
        runtime_state: result.runtime_state,
        runtime_code: result.runtime_code,
        outcome: ui_project_outcome(result.outcome),
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
            UiSection::Agents | UiSection::Projects | UiSection::Config => false,
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
        UiSection::Agents | UiSection::Projects | UiSection::Config => return None,
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
        ConversationEntryDto::Activity(activity) => {
            let ConversationActivityDto {
                fact_id,
                activity_kind,
                sequence,
                source_installation,
                source_mailbox,
                provider,
                session,
                operation,
                item,
                logical_key,
                runtime,
                occurred_at_unix_ms,
                status,
                content,
                truncated,
                completed,
            } = *activity;
            let status = tui_activity_status(status);
            let kind = tui_activity_kind(activity_kind);
            UiConversationEntry {
                id: full_id(fact_id),
                presentation: UiConversationEntryPresentation::Activity {
                    kind,
                    summary: activity_summary(kind, &status, &content, completed.as_ref()),
                    detail: terminal_structured_text(&content),
                    status: status.clone(),
                    truncated,
                    completed: completed.map(tui_completed_item),
                },
                message_state: None,
                delivery: None,
                message_target: None,
                technical: vec![UiTechnicalSection::Activity {
                    sequence,
                    source_installation: full_id(source_installation),
                    source_mailbox: full_id(source_mailbox),
                    provider: terminal_text(&provider),
                    session: terminal_text(&session),
                    operation: full_id(operation),
                    item: item.map(|value| terminal_text(&value)),
                    logical_key: terminal_text(&logical_key),
                    runtime: terminal_text(&runtime),
                    occurred_at_unix_ms,
                    status,
                    truncated,
                }],
            }
        }
    }
}

fn tui_completed_item(value: CompletedItemPresentationDto) -> UiCompletedItemPresentation {
    match value {
        CompletedItemPresentationDto::Command {
            command,
            output,
            exit_code,
            command_truncated,
            output_truncated,
        } => UiCompletedItemPresentation::Command {
            command: terminal_structured_text(&command),
            output: output.map(|value| terminal_structured_text(&value)),
            exit_code,
            command_truncated,
            output_truncated,
        },
        CompletedItemPresentationDto::FileChange {
            changes,
            changes_truncated,
        } => UiCompletedItemPresentation::FileChange {
            changes: changes
                .into_iter()
                .map(|change| UiCompletedFileChange {
                    path: terminal_structured_text(&change.path).replace('\n', " "),
                    diff: change.diff.map(|value| terminal_structured_text(&value)),
                    path_truncated: change.path_truncated,
                    diff_truncated: change.diff_truncated,
                })
                .collect(),
            changes_truncated,
        },
        CompletedItemPresentationDto::Tool {
            name,
            name_truncated,
        } => UiCompletedItemPresentation::Tool {
            name: terminal_structured_text(&name).replace('\n', " "),
            name_truncated,
        },
        CompletedItemPresentationDto::WebSearch {
            query,
            query_truncated,
        } => UiCompletedItemPresentation::WebSearch {
            query: terminal_structured_text(&query),
            query_truncated,
        },
        CompletedItemPresentationDto::Unknown => UiCompletedItemPresentation::Unknown,
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

fn activity_summary(
    kind: UiConversationActivityKind,
    status: &UiActivityStatus,
    content: &str,
    completed: Option<&CompletedItemPresentationDto>,
) -> String {
    if kind == UiConversationActivityKind::Progress && matches!(status, UiActivityStatus::Running) {
        let progress = terminal_structured_text(content).replace('\n', " ");
        if !progress.trim().is_empty() {
            return progress;
        }
    }
    if kind == UiConversationActivityKind::CompletedItem {
        return match completed {
            Some(CompletedItemPresentationDto::Command { .. }) => match status {
                UiActivityStatus::Succeeded => "Command completed".to_owned(),
                UiActivityStatus::Failed { .. } => "Command failed".to_owned(),
                UiActivityStatus::Interrupted => "Command interrupted".to_owned(),
                UiActivityStatus::Snapshot | UiActivityStatus::Running => {
                    "Command activity".to_owned()
                }
            },
            Some(CompletedItemPresentationDto::FileChange { changes, .. }) => format!(
                "Changed {} file{}",
                changes.len(),
                if changes.len() == 1 { "" } else { "s" }
            ),
            Some(CompletedItemPresentationDto::Tool { name, .. }) => {
                format!(
                    "Tool: {}",
                    terminal_structured_text(name).replace('\n', " ")
                )
            }
            Some(CompletedItemPresentationDto::WebSearch { query, .. }) => format!(
                "Web search: {}",
                terminal_structured_text(query).replace('\n', " ")
            ),
            Some(CompletedItemPresentationDto::Unknown) | None => match status {
                UiActivityStatus::Succeeded => "Completed an item".to_owned(),
                UiActivityStatus::Failed { .. } => "An item failed".to_owned(),
                UiActivityStatus::Interrupted => "An item was interrupted".to_owned(),
                UiActivityStatus::Snapshot | UiActivityStatus::Running => {
                    "Item activity".to_owned()
                }
            },
        };
    }
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
        (UiConversationActivityKind::CompletedItem, _) => unreachable!(),
    }
    .to_owned()
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
            body: terminal_message_body(&message.content),
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

fn terminal_message_body(value: &str) -> String {
    const TAB_REPLACEMENT: &str = "    ";

    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                output.push('\n');
            }
            '\n' => output.push('\n'),
            '\t' => output.push_str(TAB_REPLACEMENT),
            character if character.is_control() => output.push(' '),
            character => output.push(character),
        }
    }
    output
}

fn terminal_structured_text(value: &str) -> String {
    #[derive(Clone, Copy)]
    enum EscapeState {
        Plain,
        Escape,
        Csi,
        Osc,
        OscEscape,
    }

    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    let mut state = EscapeState::Plain;
    while let Some(character) = characters.next() {
        state = match state {
            EscapeState::Escape => match character {
                '[' => EscapeState::Csi,
                ']' => EscapeState::Osc,
                _ => EscapeState::Plain,
            },
            EscapeState::Csi => {
                if ('@'..='~').contains(&character) {
                    EscapeState::Plain
                } else {
                    EscapeState::Csi
                }
            }
            EscapeState::Osc => match character {
                '\u{7}' => EscapeState::Plain,
                '\u{1b}' => EscapeState::OscEscape,
                _ => EscapeState::Osc,
            },
            EscapeState::OscEscape => {
                if character == '\\' {
                    EscapeState::Plain
                } else {
                    EscapeState::Osc
                }
            }
            EscapeState::Plain => match character {
                '\u{1b}' => EscapeState::Escape,
                '\u{9b}' => EscapeState::Csi,
                '\u{9d}' => EscapeState::Osc,
                '\r' => {
                    if characters.peek() == Some(&'\n') {
                        characters.next();
                    }
                    output.push('\n');
                    EscapeState::Plain
                }
                '\n' => {
                    output.push('\n');
                    EscapeState::Plain
                }
                '\t' => {
                    output.push_str("    ");
                    EscapeState::Plain
                }
                character if character.is_control() => {
                    output.push(' ');
                    EscapeState::Plain
                }
                character => {
                    output.push(character);
                    EscapeState::Plain
                }
            },
        };
    }
    output
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
        UiTimerKind::RetrySnapshot => 0,
        UiTimerKind::AutosaveDraft => 1,
        UiTimerKind::DismissCompletion => 2,
        UiTimerKind::ContinueProject => 3,
        UiTimerKind::RefreshCreatedProject => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConversationPresentationContext, ProjectThreadPresentation, RunningOperationPresentation,
        SharedTuiPresentation, conversation_identity, conversation_title, local_project_command,
        terminal_structured_text, tui_interaction, ui_project_outcome,
    };
    use crate::local_client::{
        LocalProjectCommand, LocalProjectOutcome, LocalProjectResourceCheck,
        LocalProjectResourceConflict,
    };
    use hq_local_api::protocol::v1::{
        ConversationContextDto, ConversationKeyDto, ConversationParticipantDto, Id32,
        InteractionChoiceDto, InteractionKindDto, MailboxAddressDto, PendingInteractionDto,
    };
    use hq_tui::{
        UiInteractionTarget, UiInteractionTargetIssue, UiProjectAction, UiProjectOutcome,
    };

    fn pending_command(project_id: Option<Id32>) -> PendingInteractionDto {
        PendingInteractionDto {
            agent_id: Id32::new([1; 32]),
            project_id,
            provider: "codex".to_owned(),
            session: "session-1".to_owned(),
            request_id: Id32::new([2; 32]),
            operation_id: Id32::new([3; 32]),
            kind: InteractionKindDto::CommandApproval,
            prompt: "Run tests?".to_owned(),
            choices: vec![InteractionChoiceDto {
                value: "accept".to_owned(),
                label: "Accept".to_owned(),
            }],
            allow_text: false,
        }
    }

    fn participant() -> ConversationParticipantDto {
        ConversationParticipantDto {
            agent: Some(Id32::new([1; 32])),
            installation: Some(Id32::new([4; 32])),
            mailbox: Some(Id32::new([5; 32])),
            name: Some("alice".to_owned()),
        }
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn command_approval_target_requires_one_exact_direct_conversation_and_operation() {
        let presentation = SharedTuiPresentation::default();
        let key = ConversationKeyDto::ProviderSession {
            counterparty_installation: Id32::new([4; 32]),
            counterparty_mailbox: Id32::new([5; 32]),
            provider: "codex".to_owned(),
            session: "session-1".to_owned(),
        };
        let row_id = conversation_identity(key.clone());
        {
            let mut data = presentation.inner.lock().expect("presentation lock");
            data.conversation_keys.insert(row_id.clone(), key);
            data.conversation_presentations.insert(
                row_id.clone(),
                ConversationPresentationContext {
                    context: ConversationContextDto::Direct {
                        participant: participant(),
                    },
                    local_human: MailboxAddressDto {
                        installation_id: Id32::new([8; 32]),
                        mailbox_id: Id32::new([9; 32]),
                    },
                },
            );
            data.running_operations.insert(
                row_id.clone(),
                vec![RunningOperationPresentation {
                    provider: "codex".to_owned(),
                    session: "session-1".to_owned(),
                    operation_id: [3; 32],
                }],
            );
        }

        let mapped = tui_interaction(pending_command(None), &presentation);
        assert_eq!(
            mapped.target,
            UiInteractionTarget::Conversation {
                row_id: row_id.clone()
            }
        );

        presentation
            .inner
            .lock()
            .expect("presentation lock")
            .running_operations
            .get_mut(&row_id)
            .expect("loaded operations")[0]
            .operation_id = [7; 32];
        let mismatched = tui_interaction(pending_command(None), &presentation);
        assert_eq!(
            mismatched.target,
            UiInteractionTarget::Unresolved {
                reason: UiInteractionTargetIssue::OperationMismatch
            }
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn command_approval_target_rejects_ambiguous_direct_and_resolves_exact_project_thread() {
        let presentation = SharedTuiPresentation::default();
        for mailbox in [[5; 32], [6; 32]] {
            let key = ConversationKeyDto::ProviderSession {
                counterparty_installation: Id32::new([4; 32]),
                counterparty_mailbox: Id32::new(mailbox),
                provider: "codex".to_owned(),
                session: "session-1".to_owned(),
            };
            let row_id = conversation_identity(key.clone());
            let mut data = presentation.inner.lock().expect("presentation lock");
            data.conversation_keys.insert(row_id.clone(), key);
            data.conversation_presentations.insert(
                row_id,
                ConversationPresentationContext {
                    context: ConversationContextDto::Direct {
                        participant: participant(),
                    },
                    local_human: MailboxAddressDto {
                        installation_id: Id32::new([8; 32]),
                        mailbox_id: Id32::new([9; 32]),
                    },
                },
            );
        }
        assert_eq!(
            tui_interaction(pending_command(None), &presentation).target,
            UiInteractionTarget::Unresolved {
                reason: UiInteractionTargetIssue::Ambiguous
            }
        );

        let project = Id32::new([10; 32]);
        let thread = Id32::new([11; 32]);
        let key = ConversationKeyDto::ProjectThread { project, thread };
        let row_id = conversation_identity(key.clone());
        {
            let mut data = presentation.inner.lock().expect("presentation lock");
            data.conversation_keys.insert(row_id.clone(), key);
            data.project_threads.push(ProjectThreadPresentation {
                project_id: project.bytes(),
                agent_id: [1; 32],
                provider: "codex".to_owned(),
                session: "session-1".to_owned(),
                thread_id: thread.bytes(),
            });
        }
        assert_eq!(
            tui_interaction(pending_command(Some(project)), &presentation).target,
            UiInteractionTarget::Conversation { row_id }
        );
    }

    #[test]
    fn structured_terminal_text_strips_escape_sequences_and_preserves_lines() {
        let value = "one\r\ntwo\rthree\t\x1b[31mred\x1b[0m\x1b]8;;https://bad\x07link\x1b]8;;\x07\u{009b}2Jfour\u{007f}";
        let rendered = terminal_structured_text(value);
        assert_eq!(rendered, "one\ntwo\nthree    redlinkfour ");
        assert!(!rendered.contains("31m"));
        assert!(!rendered.contains("https://bad"));
        assert!(
            rendered
                .chars()
                .all(|character| !character.is_control() || character == '\n')
        );
    }

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
