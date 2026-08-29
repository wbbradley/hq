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
}

/// Passive complete UI snapshot produced from one authoritative local-API snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiSnapshot {
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
}

/// Closed side effects emitted by pure UI transitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiEffect {
    /// Request one complete authoritative snapshot through the ordinary client.
    LoadSnapshot {
        /// Identity required on the completion event.
        id: EffectId,
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
    minimum_revision: u64,
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
    required_revision: Option<u64>,
    pending_snapshot: Option<PendingSnapshot>,
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
            required_revision: None,
            pending_snapshot: None,
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
            minimum_revision,
        });
        effects.push(UiEffect::LoadSnapshot { id });
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

    fn move_selection(&mut self, forward: bool) -> bool {
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
            true
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
        self.snapshot = Some(snapshot);
    }
}

/// Applies one event without performing I/O or domain mutation.
pub fn update(mut model: UiModel, event: UiEvent) -> Result<UiTransition, UiError> {
    let mut effects = Vec::new();
    match event {
        UiEvent::Started => start(&mut model, &mut effects)?,
        UiEvent::Input(value) => apply_input(&mut model, value, &mut effects),
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
        UiEvent::Invalidated { revision } => invalidated(&mut model, revision, &mut effects)?,
        UiEvent::ConnectionObserved { generation, state } => {
            connection_observed(&mut model, generation, state, &mut effects)?;
        }
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

fn apply_input(model: &mut UiModel, input: UiInput, effects: &mut Vec<UiEffect>) {
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
        UiInput::NextFocus | UiInput::PreviousFocus => {
            model.focus = match model.focus {
                UiFocus::Navigation => UiFocus::Content,
                UiFocus::Content => UiFocus::Navigation,
            };
            true
        }
        UiInput::NextSection => {
            model.section = model.section.next();
            true
        }
        UiInput::PreviousSection => {
            model.section = model.section.previous();
            true
        }
        UiInput::NextItem => model.move_selection(true),
        UiInput::PreviousItem => model.move_selection(false),
        UiInput::Activate | UiInput::Escape => false,
    };
    if changed {
        effects.push(UiEffect::RequestRedraw);
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
    model.pending_snapshot = None;
    model.retry_timer = None;
    model.connection = UiConnectionState::Ready;
    model.last_failure = None;
    let current_revision = model.snapshot.as_ref().map_or(0, |value| value.revision);
    if snapshot.revision >= current_revision {
        model.apply_snapshot(snapshot);
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
    effects.push(UiEffect::RequestRedraw);
    Ok(())
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
