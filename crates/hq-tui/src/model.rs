//! Pure identity-aware TUI transition algebra.

use std::{num::NonZeroU64, time::Duration};

const PERIODIC_REFRESH: Duration = Duration::from_secs(300);
const RETRY_DELAY: Duration = Duration::from_millis(250);

/// Stable identity attached to an asynchronous UI effect and its completion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EffectId(NonZeroU64);

/// Current shell connection state presented by the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiConnectionState {
    /// No connection attempt has started.
    Disconnected,
    /// An initial snapshot request is in flight.
    Connecting,
    /// A complete authoritative snapshot is available.
    Ready,
    /// The shell is recovering connectivity or a lost refresh.
    Reconnecting,
    /// The local endpoint has no compatible protocol version.
    Incompatible,
}

/// Top-level semantic section selected by the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiSection {
    /// Open human mailbox work.
    Inbox,
    /// Human-authored sent work.
    Sent,
    /// Archived human mailbox work.
    Archived,
    /// Named agents and sessions.
    Agents,
    /// Projects and resources.
    Projects,
}

impl UiSection {
    pub(crate) const ALL: [Self; 5] = [
        Self::Inbox,
        Self::Sent,
        Self::Archived,
        Self::Agents,
        Self::Projects,
    ];

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Logical focus independent of terminal coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiFocus {
    /// Top-level section navigation.
    Navigation,
    /// Current section content.
    Content,
    /// Open conversation history.
    Conversation,
}

/// Shell-normalized terminal input understood by the pure model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiInput {
    /// Exit the UI.
    Quit,
    /// Move focus forward.
    NextFocus,
    /// Move focus backward.
    PreviousFocus,
    /// Select the next top-level section.
    NextSection,
    /// Select the previous top-level section.
    PreviousSection,
    /// Select the next logical row.
    NextItem,
    /// Select the previous logical row.
    PreviousItem,
    /// Activate the selected row.
    Activate,
    /// Request the next reducer-ordered conversation page.
    LoadMore,
    /// Dismiss the current transient interaction.
    Escape,
}

/// Passive terminal dimensions supplied by the shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiSize {
    /// Terminal columns.
    pub width: u16,
    /// Terminal rows.
    pub height: u16,
}

/// Passive shell-normalized status for one summary row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiRowState {
    /// Actionable current work.
    Open,
    /// Work awaiting another actor or external effect.
    Waiting,
    /// Work retained outside the active view.
    Archived,
    /// Work whose current truth is incomplete or conflicted.
    Attention,
}

/// Passive semantic kind for one summary row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiRowKind {
    /// A conversation whose entries are available by bounded page query.
    Conversation,
    /// Inert diagnostic state that cannot be used as an action target.
    Diagnostic,
    /// A named-agent summary.
    Agent,
    /// A project summary.
    Project,
}

/// Passive shell-normalized summary row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiRow {
    /// Stable logical identity used to preserve selection across reloads.
    pub id: String,
    /// Primary bounded display text.
    pub title: String,
    /// Secondary bounded display text.
    pub detail: String,
    /// Typed presentation state.
    pub state: UiRowState,
    /// Typed semantic row kind; never inferred from display text.
    pub kind: UiRowKind,
}

/// Closed message state presented without interpreting prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiMessageState {
    /// The message remains open and actionable where its type permits.
    Open,
    /// The message is reversibly archived.
    Archived,
    /// The message was absorbing-rejected.
    Rejected,
}

/// Closed reducer-owned conversation entry family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiConversationEntryKind {
    /// Durable message presentation.
    Message,
    /// Non-actionable durable or coalesced activity presentation.
    Activity,
}

/// Closed activity status presented without parsing a display string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiActivityStatus {
    /// Informational snapshot without a lifecycle claim.
    Snapshot,
    /// Correlated work remains active.
    Running,
    /// Correlated work completed successfully.
    Succeeded,
    /// Correlated work failed with a stable reason code.
    Failed {
        /// Stable bounded failure reason.
        reason: String,
    },
    /// Correlated work was explicitly interrupted.
    Interrupted,
}

/// One typed namespaced technical disclosure section.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiTechnicalSection {
    /// Exact message routing identities.
    Routing {
        /// Full sender mailbox address.
        sender: String,
        /// Full recipient mailbox address when directly addressed.
        recipient: Option<String>,
    },
    /// Typed purpose, presentation, and optional operation correlation.
    Semantics {
        /// Stable purpose label from the protocol enum.
        purpose: String,
        /// Stable presentation label from the protocol enum.
        presentation: String,
        /// Provider namespace when correlated.
        provider: Option<String>,
        /// Provider session when correlated.
        session: Option<String>,
        /// Operation identity when correlated.
        operation: Option<String>,
        /// Project identity when associated.
        project: Option<String>,
    },
    /// Exact causal and delivery evidence identities.
    Evidence {
        /// Stable public message identity.
        message_id: String,
        /// Stable causal thread identity.
        thread_id: String,
        /// Causal-maximal reversible-state frontier.
        state_frontier: Vec<String>,
        /// Peer-authored children proving receipt.
        peer_received_by: Vec<String>,
        /// Normalized question root when present.
        root_fact: Option<String>,
        /// Normalized root public message when present.
        root_message: Option<String>,
        /// Whether the message is currently a ready answer.
        ready_answer: bool,
        /// Whether its question thread has a valid cancellation.
        thread_cancelled: bool,
    },
    /// Typed non-actionable activity metadata.
    Activity {
        /// Positive source sequence selected by the reducer.
        sequence: u64,
        /// Closed activity status.
        status: UiActivityStatus,
        /// Whether content was explicitly truncated at authoring.
        truncated: bool,
    },
}

/// Passive reducer-ordered conversation presentation entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationEntry {
    /// Stable canonical fact identity used as the logical scroll anchor.
    pub id: String,
    /// Typed entry family.
    pub kind: UiConversationEntryKind,
    /// Bounded sanitized display content.
    pub content: String,
    /// Stable display source or status summary.
    pub summary: String,
    /// Typed message state; absent for non-actionable activity.
    pub message_state: Option<UiMessageState>,
    /// Namespaced technical sections, already bounded by the local protocol.
    pub technical: Vec<UiTechnicalSection>,
}

/// Passive bounded page returned by the ordinary local API client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversationPage {
    /// Stable summary-row identity requested by the model.
    pub row_id: String,
    /// Reducer-ordered entries for this page.
    pub entries: Vec<UiConversationEntry>,
    /// Opaque continuation cursor when more reducer-ordered entries exist.
    pub next_cursor: Option<String>,
}

/// Passive accumulated conversation presentation held by the pure model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiConversation {
    /// Stable summary-row identity.
    pub row_id: String,
    /// Reducer-ordered entries loaded so far.
    pub entries: Vec<UiConversationEntry>,
    /// Opaque next-page cursor.
    pub next_cursor: Option<String>,
}

/// Passive complete UI snapshot produced from one authoritative local-API snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiSnapshot {
    /// Semantic section represented by these rows.
    pub section: UiSection,
    /// Serialized authoritative revision.
    pub revision: u64,
    /// Reducer-ordered shell-normalized rows for the selected foundation view.
    pub rows: Vec<UiRow>,
}

/// Passive stable actionable failure shown without behavioral prose parsing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiFailure {
    /// Stable machine-readable failure code.
    pub code: String,
    /// Bounded safe operator action.
    pub action: String,
}

/// Closed timer purpose owned by the shell effect executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiTimerKind {
    /// Periodic full-snapshot repair.
    PeriodicRefresh,
    /// Bounded retry after a failed snapshot request.
    RetrySnapshot,
}

/// Closed event vocabulary accepted by the pure UI model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiEvent {
    /// Start the model exactly once.
    Started,
    /// Normalized terminal input.
    Input(UiInput),
    /// Complete terminal resize.
    Resized(UiSize),
    /// One scheduled timer elapsed.
    TimerElapsed {
        /// Identity of the completed timer effect.
        effect_id: EffectId,
    },
    /// One authoritative snapshot request completed.
    SnapshotLoaded {
        /// Identity of the completed snapshot effect.
        effect_id: EffectId,
        /// Complete shell-normalized snapshot.
        snapshot: UiSnapshot,
    },
    /// One authoritative snapshot request failed.
    SnapshotFailed {
        /// Identity of the completed snapshot effect.
        effect_id: EffectId,
        /// Stable actionable failure.
        failure: UiFailure,
    },
    /// One reducer-ordered conversation page request completed.
    ConversationLoaded {
        /// Identity of the completed page effect.
        effect_id: EffectId,
        /// Complete passive page for the requested row.
        page: UiConversationPage,
    },
    /// One conversation page request failed.
    ConversationFailed {
        /// Identity of the completed page effect.
        effect_id: EffectId,
        /// Stable actionable failure.
        failure: UiFailure,
    },
    /// A revision-only wake marked the current snapshot stale.
    Invalidated {
        /// Greatest revision known to the shell.
        revision: u64,
    },
    /// The reconnecting client reported a generation-scoped state.
    ConnectionObserved {
        /// Monotonic shell connection generation.
        generation: u64,
        /// State observed for that generation.
        state: UiConnectionState,
    },
    /// The reconnecting client reported a stable generation-scoped failure.
    ClientFailed {
        /// Monotonic shell connection generation.
        generation: u64,
        /// Stable actionable client failure.
        failure: UiFailure,
    },
}

/// Closed side effects emitted by pure UI transitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiEffect {
    /// Request one complete authoritative snapshot through the ordinary client.
    LoadSnapshot {
        /// Identity required on the completion event.
        id: EffectId,
        /// Semantic section that the complete snapshot must represent.
        section: UiSection,
    },
    /// Request one bounded reducer-ordered conversation page.
    LoadConversation {
        /// Identity required on the completion event.
        id: EffectId,
        /// Stable summary-row identity selected by the model.
        row_id: String,
        /// Opaque continuation cursor; absent for the first page.
        cursor: Option<String>,
    },
    /// Schedule one bounded timer through the shell clock.
    ScheduleTimer {
        /// Identity required on the completion event.
        id: EffectId,
        /// Closed timer purpose.
        kind: UiTimerKind,
        /// Exact delay requested from the shell.
        after: Duration,
    },
    /// Coalescible request to render the latest borrowed model.
    RequestRedraw,
    /// Leave the terminal loop after restoration ownership unwinds.
    Exit,
}

/// Pure transition result with the complete next model and ordered effects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiTransition {
    /// Complete next model.
    pub model: UiModel,
    /// Ordered effects for the shell executor.
    pub effects: Vec<UiEffect>,
}

/// Closed model transition failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiError {
    /// The one-time start event was repeated.
    AlreadyStarted,
    /// The process-local effect identity space was exhausted.
    EffectIdentityExhausted,
    /// A shell returned snapshot rows for a section other than the requested section.
    SnapshotSectionMismatch,
    /// A shell returned a page for a row other than the exact requested row.
    ConversationRowMismatch,
}

impl std::fmt::Display for UiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "TUI transition failed: {self:?}")
    }
}

impl std::error::Error for UiError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingSnapshot {
    id: EffectId,
    section: UiSection,
    minimum_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingConversation {
    id: EffectId,
    row_id: String,
    cursor: Option<String>,
}

/// Complete invariant-bearing TUI application state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiModel {
    viewport: UiSize,
    connection: UiConnectionState,
    connection_generation: u64,
    section: UiSection,
    focus: UiFocus,
    snapshot: Option<UiSnapshot>,
    selected_row: Option<String>,
    conversation: Option<UiConversation>,
    conversation_anchor: Option<String>,
    technical_visible: bool,
    required_revision: Option<u64>,
    pending_snapshot: Option<PendingSnapshot>,
    pending_conversation: Option<PendingConversation>,
    periodic_timer: Option<EffectId>,
    retry_timer: Option<EffectId>,
    next_effect_id: Option<NonZeroU64>,
    last_failure: Option<UiFailure>,
    started: bool,
    should_exit: bool,
}

impl UiModel {
    /// Constructs a disconnected model without performing any effects.
    pub const fn new(viewport: UiSize) -> Self {
        Self {
            viewport,
            connection: UiConnectionState::Disconnected,
            connection_generation: 0,
            section: UiSection::Inbox,
            focus: UiFocus::Navigation,
            snapshot: None,
            selected_row: None,
            conversation: None,
            conversation_anchor: None,
            technical_visible: false,
            required_revision: None,
            pending_snapshot: None,
            pending_conversation: None,
            periodic_timer: None,
            retry_timer: None,
            next_effect_id: NonZeroU64::new(1),
            last_failure: None,
            started: false,
            should_exit: false,
        }
    }

    /// Returns the latest terminal dimensions.
    pub const fn viewport(&self) -> UiSize {
        self.viewport
    }

    /// Returns the currently presented connection state.
    pub const fn connection(&self) -> UiConnectionState {
        self.connection
    }

    /// Returns the selected semantic section.
    pub const fn section(&self) -> UiSection {
        self.section
    }

    /// Returns the current logical focus.
    pub const fn focus(&self) -> UiFocus {
        self.focus
    }

    /// Borrows the latest complete snapshot.
    pub const fn snapshot(&self) -> Option<&UiSnapshot> {
        self.snapshot.as_ref()
    }

    /// Returns the selected stable row identity.
    pub fn selected_row(&self) -> Option<&str> {
        self.selected_row.as_deref()
    }

    /// Borrows the reducer-ordered conversation loaded for the selected row.
    pub const fn conversation(&self) -> Option<&UiConversation> {
        self.conversation.as_ref()
    }

    /// Returns the stable selected conversation-entry identity.
    pub fn conversation_anchor(&self) -> Option<&str> {
        self.conversation_anchor.as_deref()
    }

    /// Reports whether typed technical disclosure is expanded.
    pub const fn technical_visible(&self) -> bool {
        self.technical_visible
    }

    /// Returns the greatest revision required by coalesced invalidations.
    pub const fn required_revision(&self) -> Option<u64> {
        self.required_revision
    }

    /// Returns the current authoritative snapshot effect identity.
    pub const fn pending_snapshot(&self) -> Option<EffectId> {
        match self.pending_snapshot {
            Some(pending) => Some(pending.id),
            None => None,
        }
    }

    /// Returns the current conversation-page effect identity.
    pub const fn pending_conversation(&self) -> Option<EffectId> {
        match &self.pending_conversation {
            Some(pending) => Some(pending.id),
            None => None,
        }
    }

    /// Borrows the latest matching failure.
    pub const fn last_failure(&self) -> Option<&UiFailure> {
        self.last_failure.as_ref()
    }

    /// Reports whether the model requested loop exit.
    pub const fn should_exit(&self) -> bool {
        self.should_exit
    }

    fn allocate_effect(&mut self) -> Result<EffectId, UiError> {
        let current = self
            .next_effect_id
            .ok_or(UiError::EffectIdentityExhausted)?;
        self.next_effect_id = current.get().checked_add(1).and_then(NonZeroU64::new);
        Ok(EffectId(current))
    }

    fn request_snapshot(&mut self, effects: &mut Vec<UiEffect>) -> Result<(), UiError> {
        if self.pending_snapshot.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        let minimum_revision = self.required_revision.unwrap_or_else(|| {
            self.snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.revision)
        });
        self.pending_snapshot = Some(PendingSnapshot {
            id,
            section: self.section,
            minimum_revision,
        });
        effects.push(UiEffect::LoadSnapshot {
            id,
            section: self.section,
        });
        Ok(())
    }

    fn request_conversation(
        &mut self,
        row_id: String,
        cursor: Option<String>,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        if self.pending_conversation.is_some() {
            return Ok(());
        }
        let id = self.allocate_effect()?;
        self.pending_conversation = Some(PendingConversation {
            id,
            row_id: row_id.clone(),
            cursor: cursor.clone(),
        });
        effects.push(UiEffect::LoadConversation { id, row_id, cursor });
        Ok(())
    }

    fn schedule_timer(
        &mut self,
        kind: UiTimerKind,
        after: Duration,
        effects: &mut Vec<UiEffect>,
    ) -> Result<(), UiError> {
        let id = self.allocate_effect()?;
        match kind {
            UiTimerKind::PeriodicRefresh => self.periodic_timer = Some(id),
            UiTimerKind::RetrySnapshot => self.retry_timer = Some(id),
        }
        effects.push(UiEffect::ScheduleTimer { id, kind, after });
        Ok(())
    }

    fn move_row_selection(&mut self, forward: bool) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        if snapshot.rows.is_empty() {
            return false;
        }
        let current = self
            .selected_row
            .as_deref()
            .and_then(|selected| snapshot.rows.iter().position(|row| row.id == selected));
        let next = match (current, forward) {
            (Some(index), true) => (index + 1).min(snapshot.rows.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
            (None, _) => 0,
        };
        let selected = snapshot.rows[next].id.clone();
        if self.selected_row.as_ref() == Some(&selected) {
            false
        } else {
            self.selected_row = Some(selected);
            self.close_conversation();
            true
        }
    }

    fn move_conversation_anchor(&mut self, forward: bool) -> bool {
        let Some(conversation) = &self.conversation else {
            return false;
        };
        if conversation.entries.is_empty() {
            return false;
        }
        let current = self.conversation_anchor.as_deref().and_then(|selected| {
            conversation
                .entries
                .iter()
                .position(|entry| entry.id == selected)
        });
        let next = match (current, forward) {
            (Some(index), true) => (index + 1).min(conversation.entries.len() - 1),
            (Some(index), false) => index.saturating_sub(1),
            (None, _) => 0,
        };
        let selected = conversation.entries[next].id.clone();
        if self.conversation_anchor.as_ref() == Some(&selected) {
            false
        } else {
            self.conversation_anchor = Some(selected);
            self.technical_visible = false;
            true
        }
    }

    fn selected_row_is_conversation(&self) -> bool {
        self.selected_row.as_ref().is_some_and(|selected| {
            self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .rows
                    .iter()
                    .any(|row| &row.id == selected && row.kind == UiRowKind::Conversation)
            })
        })
    }

    fn close_conversation(&mut self) {
        self.conversation = None;
        self.conversation_anchor = None;
        self.technical_visible = false;
        self.pending_conversation = None;
        if self.focus == UiFocus::Conversation {
            self.focus = UiFocus::Content;
        }
    }

    fn apply_snapshot(&mut self, snapshot: UiSnapshot) {
        let keep = self.selected_row.as_ref().and_then(|selected| {
            snapshot
                .rows
                .iter()
                .find(|row| &row.id == selected)
                .map(|row| row.id.clone())
        });
        self.selected_row = keep.or_else(|| snapshot.rows.first().map(|row| row.id.clone()));
        let conversation_survives = self.conversation.as_ref().is_some_and(|conversation| {
            self.selected_row.as_ref() == Some(&conversation.row_id)
                && snapshot
                    .rows
                    .iter()
                    .any(|row| row.id == conversation.row_id && row.kind == UiRowKind::Conversation)
        });
        if !conversation_survives {
            self.close_conversation();
        }
        self.snapshot = Some(snapshot);
    }
}

/// Applies one event without performing I/O or domain mutation.
pub fn update(mut model: UiModel, event: UiEvent) -> Result<UiTransition, UiError> {
    let mut effects = Vec::new();
    match event {
        UiEvent::Started => start(&mut model, &mut effects)?,
        UiEvent::Input(value) => apply_input(&mut model, value, &mut effects)?,
        UiEvent::Resized(viewport) => {
            if model.viewport != viewport {
                model.viewport = viewport;
                effects.push(UiEffect::RequestRedraw);
            }
        }
        UiEvent::TimerElapsed { effect_id } => {
            timer_elapsed(&mut model, effect_id, &mut effects)?;
        }
        UiEvent::SnapshotLoaded {
            effect_id,
            snapshot,
        } => snapshot_loaded(&mut model, effect_id, snapshot, &mut effects)?,
        UiEvent::SnapshotFailed { effect_id, failure } => {
            snapshot_failed(&mut model, effect_id, failure, &mut effects)?;
        }
        UiEvent::ConversationLoaded { effect_id, page } => {
            conversation_loaded(&mut model, effect_id, page, &mut effects)?;
        }
        UiEvent::ConversationFailed { effect_id, failure } => {
            conversation_failed(&mut model, effect_id, failure, &mut effects);
        }
        UiEvent::Invalidated { revision } => invalidated(&mut model, revision, &mut effects)?,
        UiEvent::ConnectionObserved { generation, state } => {
            connection_observed(&mut model, generation, state, &mut effects)?;
        }
        UiEvent::ClientFailed {
            generation,
            failure,
        } => client_failed(&mut model, generation, failure, &mut effects),
    }
    Ok(UiTransition { model, effects })
}

fn start(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<(), UiError> {
    if model.started {
        return Err(UiError::AlreadyStarted);
    }
    model.started = true;
    model.connection = UiConnectionState::Connecting;
    model.request_snapshot(effects)?;
    model.schedule_timer(UiTimerKind::PeriodicRefresh, PERIODIC_REFRESH, effects)?;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn apply_input(
    model: &mut UiModel,
    input: UiInput,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let changed = match input {
        UiInput::Quit => {
            if model.should_exit {
                false
            } else {
                model.should_exit = true;
                effects.push(UiEffect::Exit);
                false
            }
        }
        UiInput::NextFocus => {
            model.focus = match model.focus {
                UiFocus::Navigation => UiFocus::Content,
                UiFocus::Content if model.conversation.is_some() => UiFocus::Conversation,
                UiFocus::Content | UiFocus::Conversation => UiFocus::Navigation,
            };
            true
        }
        UiInput::PreviousFocus => {
            model.focus = match model.focus {
                UiFocus::Navigation if model.conversation.is_some() => UiFocus::Conversation,
                UiFocus::Navigation | UiFocus::Conversation => UiFocus::Content,
                UiFocus::Content => UiFocus::Navigation,
            };
            true
        }
        UiInput::NextSection => {
            model.section = model.section.next();
            model.snapshot = None;
            model.selected_row = None;
            model.close_conversation();
            model.request_snapshot(effects)?;
            true
        }
        UiInput::PreviousSection => {
            model.section = model.section.previous();
            model.snapshot = None;
            model.selected_row = None;
            model.close_conversation();
            model.request_snapshot(effects)?;
            true
        }
        UiInput::NextItem => match model.focus {
            UiFocus::Conversation => model.move_conversation_anchor(true),
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(true),
        },
        UiInput::PreviousItem => match model.focus {
            UiFocus::Conversation => model.move_conversation_anchor(false),
            UiFocus::Navigation | UiFocus::Content => model.move_row_selection(false),
        },
        UiInput::Activate => activate(model, effects)?,
        UiInput::LoadMore => load_more(model, effects)?,
        UiInput::Escape => escape(model),
    };
    if changed {
        effects.push(UiEffect::RequestRedraw);
    }
    Ok(())
}

fn activate(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<bool, UiError> {
    if model.focus == UiFocus::Conversation && model.conversation_anchor.is_some() {
        model.technical_visible = !model.technical_visible;
        return Ok(true);
    }
    if !model.selected_row_is_conversation() {
        return Ok(false);
    }
    let row_id = model.selected_row.clone().unwrap_or_default();
    if model
        .conversation
        .as_ref()
        .is_some_and(|conversation| conversation.row_id == row_id)
    {
        model.focus = UiFocus::Conversation;
    } else {
        model.request_conversation(row_id, None, effects)?;
    }
    Ok(true)
}

fn load_more(model: &mut UiModel, effects: &mut Vec<UiEffect>) -> Result<bool, UiError> {
    let request = model.conversation.as_ref().and_then(|conversation| {
        conversation
            .next_cursor
            .clone()
            .map(|cursor| (conversation.row_id.clone(), cursor))
    });
    let Some((row_id, cursor)) = request else {
        return Ok(false);
    };
    model.request_conversation(row_id, Some(cursor), effects)?;
    Ok(true)
}

fn escape(model: &mut UiModel) -> bool {
    if model.technical_visible {
        model.technical_visible = false;
        true
    } else if model.conversation.is_some() {
        model.close_conversation();
        true
    } else {
        false
    }
}

fn timer_elapsed(
    model: &mut UiModel,
    effect_id: EffectId,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.periodic_timer == Some(effect_id) {
        model.periodic_timer = None;
        model.schedule_timer(UiTimerKind::PeriodicRefresh, PERIODIC_REFRESH, effects)?;
        model.request_snapshot(effects)?;
        effects.push(UiEffect::RequestRedraw);
    } else if model.retry_timer == Some(effect_id) {
        model.retry_timer = None;
        model.connection = UiConnectionState::Connecting;
        model.request_snapshot(effects)?;
        effects.push(UiEffect::RequestRedraw);
    }
    Ok(())
}

fn snapshot_loaded(
    model: &mut UiModel,
    effect_id: EffectId,
    snapshot: UiSnapshot,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let Some(pending) = model
        .pending_snapshot
        .filter(|pending| pending.id == effect_id)
    else {
        return Ok(());
    };
    if snapshot.section != pending.section {
        return Err(UiError::SnapshotSectionMismatch);
    }
    model.pending_snapshot = None;
    model.retry_timer = None;
    model.connection = UiConnectionState::Ready;
    model.last_failure = None;
    if pending.section == model.section {
        let current_revision = model.snapshot.as_ref().map_or(0, |value| value.revision);
        if snapshot.revision >= current_revision {
            model.apply_snapshot(snapshot);
        }
    } else {
        model.request_snapshot(effects)?;
        effects.push(UiEffect::RequestRedraw);
        return Ok(());
    }
    let observed_revision = model.snapshot.as_ref().map_or(0, |value| value.revision);
    let required_revision = model
        .required_revision
        .unwrap_or(pending.minimum_revision)
        .max(pending.minimum_revision);
    if observed_revision >= required_revision {
        model.required_revision = None;
    } else {
        model.required_revision = Some(required_revision);
        model.request_snapshot(effects)?;
    }
    if model.required_revision.is_none()
        && let Some(row_id) = model
            .conversation
            .as_ref()
            .map(|conversation| conversation.row_id.clone())
    {
        model.request_conversation(row_id, None, effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn conversation_loaded(
    model: &mut UiModel,
    effect_id: EffectId,
    page: UiConversationPage,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let Some(pending) = model
        .pending_conversation
        .as_ref()
        .filter(|pending| pending.id == effect_id)
        .cloned()
    else {
        return Ok(());
    };
    if page.row_id != pending.row_id {
        return Err(UiError::ConversationRowMismatch);
    }
    model.pending_conversation = None;
    if model.selected_row.as_deref() != Some(page.row_id.as_str())
        || !model.selected_row_is_conversation()
    {
        return Ok(());
    }
    let previous_anchor = model.conversation_anchor.clone();
    if pending.cursor.is_some()
        && let Some(conversation) = &mut model.conversation
        && conversation.row_id == page.row_id
    {
        conversation.entries.extend(page.entries);
        conversation.next_cursor = page.next_cursor;
    } else {
        model.conversation = Some(UiConversation {
            row_id: page.row_id,
            entries: page.entries,
            next_cursor: page.next_cursor,
        });
    }
    model.conversation_anchor = model.conversation.as_ref().and_then(|conversation| {
        previous_anchor
            .filter(|anchor| conversation.entries.iter().any(|entry| &entry.id == anchor))
            .or_else(|| conversation.entries.first().map(|entry| entry.id.clone()))
    });
    model.focus = UiFocus::Conversation;
    model.last_failure = None;
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn conversation_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if model
        .pending_conversation
        .as_ref()
        .map(|pending| pending.id)
        != Some(effect_id)
    {
        return;
    }
    model.pending_conversation = None;
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

fn snapshot_failed(
    model: &mut UiModel,
    effect_id: EffectId,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if model.pending_snapshot.map(|pending| pending.id) != Some(effect_id) {
        return Ok(());
    }
    model.pending_snapshot = None;
    model.connection = UiConnectionState::Reconnecting;
    model.last_failure = Some(failure);
    if model.retry_timer.is_none() {
        model.schedule_timer(UiTimerKind::RetrySnapshot, RETRY_DELAY, effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn invalidated(
    model: &mut UiModel,
    revision: u64,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    let current = model
        .snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.revision);
    let required = model.required_revision.unwrap_or(current);
    if revision <= required {
        return Ok(());
    }
    model.required_revision = Some(revision);
    model.pending_conversation = None;
    if let Some(pending) = &mut model.pending_snapshot {
        pending.minimum_revision = pending.minimum_revision.max(revision);
    } else {
        model.request_snapshot(effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn connection_observed(
    model: &mut UiModel,
    generation: u64,
    state: UiConnectionState,
    effects: &mut Vec<UiEffect>,
) -> Result<(), UiError> {
    if generation < model.connection_generation
        || (generation == model.connection_generation && state == model.connection)
    {
        return Ok(());
    }
    let became_ready =
        state == UiConnectionState::Ready && model.connection != UiConnectionState::Ready;
    model.connection_generation = generation;
    model.connection = state;
    if became_ready {
        model.request_snapshot(effects)?;
    }
    effects.push(UiEffect::RequestRedraw);
    Ok(())
}

fn client_failed(
    model: &mut UiModel,
    generation: u64,
    failure: UiFailure,
    effects: &mut Vec<UiEffect>,
) {
    if generation < model.connection_generation {
        return;
    }
    model.connection_generation = generation;
    model.connection = UiConnectionState::Reconnecting;
    model.pending_conversation = None;
    model.last_failure = Some(failure);
    effects.push(UiEffect::RequestRedraw);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::num::NonZeroU64;

    use super::{UiEffect, UiError, UiEvent, UiModel, UiSize, update};

    #[test]
    fn effect_identity_exhaustion_is_explicit() {
        let mut model = UiModel::new(UiSize {
            width: 80,
            height: 24,
        });
        model.next_effect_id = NonZeroU64::new(u64::MAX);
        let error = update(model, UiEvent::Started).expect_err("second allocation exhausts");
        assert_eq!(error, UiError::EffectIdentityExhausted);
    }

    #[test]
    fn repeated_start_is_rejected() {
        let started = update(
            UiModel::new(UiSize {
                width: 80,
                height: 24,
            }),
            UiEvent::Started,
        )
        .expect("first start");
        assert!(matches!(started.effects[0], UiEffect::LoadSnapshot { .. }));
        assert_eq!(
            update(started.model, UiEvent::Started),
            Err(UiError::AlreadyStarted)
        );
    }
}
