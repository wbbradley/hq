//! Crossterm terminal ownership around the pure TUI reducer and effect executor.

use std::{
    collections::BTreeSet,
    fmt,
    io::Stdout,
    os::fd::{AsFd, BorrowedFd},
    time::Duration,
};

use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hq_local_api::{InitialView, protocol::v1::BuildMetadata};
use hq_tui::{
    UiConversationViewportObservation, UiEvent, UiInput, UiModel, UiRenderCache, UiSize, UiTheme,
    update,
};
use nix::{
    poll::{PollFd, PollFlags, PollTimeout, poll},
    unistd::{Uid, User},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    BoundaryIds, BoundaryKind, BoundaryProcess, BoundaryTrace, IdentityError, IdentityErrorClass,
    LocalNodeClient, LocalNodeEventClient, MonotonicTuiClock, StatePaths, TuiClientPort, TuiClock,
    TuiEffectExecutor, TuiObservationPort, TuiThemeEnvironment, TuiThemeError,
    local_client::installed_local_client_config, resolve_tui_theme,
    tui_client::compose_tui_clients,
};

/// Passive terminal observation after backend-specific normalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiTerminalEvent {
    /// One input action understood by the pure UI model.
    Input(UiInput),
    /// Complete new terminal dimensions.
    Resized(UiSize),
    /// Shell cancellation, treated as an orderly quit request.
    Cancelled,
}

/// Closed terminal operation failure without backend or operating-system prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiTerminalError {
    /// Terminal modes could not be activated completely.
    Activate,
    /// Terminal dimensions could not be observed.
    Size,
    /// Terminal input could not be polled or read.
    Poll,
    /// A complete borrowed frame could not be drawn.
    Draw,
    /// One or more terminal modes could not be restored.
    Restore,
}

impl fmt::Display for TuiTerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TUI terminal failed: {self:?}")
    }
}

impl std::error::Error for TuiTerminalError {}

/// Closed outer-shell failure without terminal, transport, or model prose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiShellError {
    /// Installed identity state is absent, unsafe, or invalid.
    Identity(IdentityError),
    /// Unsigned local configuration could not be read safely.
    Configuration(IdentityError),
    /// The configured startup theme could not be resolved completely.
    Theme(TuiThemeError),
    /// The terminal boundary failed.
    Terminal(TuiTerminalError),
    /// The pure UI model rejected an event.
    Model,
    /// The bounded effect executor failed or its worker panicked.
    Executor,
    /// The independent ordinary and subscribed local clients could not be composed.
    Client,
    /// Installed build metadata was invalid.
    Build,
}

impl fmt::Display for TuiShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TUI shell failed: {self:?}")
    }
}

impl std::error::Error for TuiShellError {}

impl TuiShellError {
    /// Returns the stable installed diagnostic code and actionable human message.
    pub fn diagnostic(&self) -> (&'static str, String) {
        match self {
            Self::Identity(error)
                if matches!(error.class(), IdentityErrorClass::IdentityMissing) =>
            {
                (
                    "setup.identity_required",
                    "HQ needs a device identity before it can protect your account and messages.\nRun `hq identity init` to create it, or import an existing identity.\nThen run `hq` again; the next screen will guide account setup.".to_owned(),
                )
            }
            Self::Identity(_) => (
                "setup.identity_invalid",
                "inspect or restore the installation identity before starting the TUI".to_owned(),
            ),
            Self::Configuration(_) => (
                "tui.configuration_invalid",
                "the local configuration is invalid; inspect it with `hq config get`".to_owned(),
            ),
            Self::Theme(error) => (
                "tui.theme_invalid",
                format!(
                    "cannot load the selected theme: {error}\nRun `hq config themes` to inspect choices, or `hq config set theme none` to restore automatic selection."
                ),
            ),
            Self::Terminal(_) => (
                "tui.terminal_failed",
                "the interactive terminal could not be activated, drawn, or restored".to_owned(),
            ),
            Self::Model => (
                "tui.model_failed",
                "the interactive model rejected a terminal or client event".to_owned(),
            ),
            Self::Executor => (
                "tui.executor_failed",
                "the interactive client worker stopped unexpectedly".to_owned(),
            ),
            Self::Client => (
                "tui.client_failed",
                "the local node client could not be started or connected".to_owned(),
            ),
            Self::Build => (
                "tui.build_invalid",
                "the installed build metadata is invalid".to_owned(),
            ),
        }
    }
}

impl From<TuiTerminalError> for TuiShellError {
    fn from(error: TuiTerminalError) -> Self {
        Self::Terminal(error)
    }
}

/// Terminal capability owned exclusively by the outer shell guard.
pub trait TuiTerminalPort {
    /// Activates raw input and the shell's isolated screen modes.
    fn activate(&mut self) -> Result<(), TuiTerminalError>;
    /// Returns complete current dimensions.
    fn size(&mut self) -> Result<UiSize, TuiTerminalError>;
    /// Waits for terminal input, executor readiness, or an optional exact deadline.
    fn poll(
        &mut self,
        executor_wake: BorrowedFd<'_>,
        wait: Option<Duration>,
    ) -> Result<Option<TuiTerminalEvent>, TuiTerminalError>;
    /// Draws the latest complete model by immutable borrow.
    fn draw(
        &mut self,
        model: &UiModel,
    ) -> Result<Option<UiConversationViewportObservation>, TuiTerminalError>;
    /// Replaces the resolved semantic theme used by subsequent draws.
    fn set_theme(&mut self, _theme: UiTheme) -> Result<(), TuiTerminalError> {
        Ok(())
    }
    /// Restores every mode activated by this capability; repeated calls are safe.
    fn restore(&mut self) -> Result<(), TuiTerminalError>;
}

/// Real Crossterm/Ratatui terminal capability for the installed executable.
pub struct CrosstermTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    theme: UiTheme,
    render_cache: UiRenderCache,
    activation: TerminalActivation,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TerminalActivation {
    Inactive,
    Raw,
    AlternateScreen,
    MouseCapture,
    BracketedPaste,
    CursorHidden,
}

impl CrosstermTerminal {
    /// Constructs a terminal backend without changing process terminal modes.
    pub fn new(theme: UiTheme) -> Result<Self, TuiTerminalError> {
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::new(backend).map_err(|_| TuiTerminalError::Activate)?;
        Ok(Self {
            terminal,
            theme,
            render_cache: UiRenderCache::new(),
            activation: TerminalActivation::Inactive,
        })
    }
}

impl TuiTerminalPort for CrosstermTerminal {
    fn activate(&mut self) -> Result<(), TuiTerminalError> {
        enable_raw_mode().map_err(|_| TuiTerminalError::Activate)?;
        self.activation = TerminalActivation::Raw;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)
            .map_err(|_| TuiTerminalError::Activate)?;
        self.activation = TerminalActivation::AlternateScreen;
        execute!(self.terminal.backend_mut(), EnableMouseCapture)
            .map_err(|_| TuiTerminalError::Activate)?;
        self.activation = TerminalActivation::MouseCapture;
        execute!(self.terminal.backend_mut(), EnableBracketedPaste)
            .map_err(|_| TuiTerminalError::Activate)?;
        self.activation = TerminalActivation::BracketedPaste;
        execute!(self.terminal.backend_mut(), Hide).map_err(|_| TuiTerminalError::Activate)?;
        self.activation = TerminalActivation::CursorHidden;
        Ok(())
    }

    fn size(&mut self) -> Result<UiSize, TuiTerminalError> {
        let size = self.terminal.size().map_err(|_| TuiTerminalError::Size)?;
        Ok(UiSize {
            width: size.width,
            height: size.height,
        })
    }

    fn poll(
        &mut self,
        executor_wake: BorrowedFd<'_>,
        wait: Option<Duration>,
    ) -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
        if event::poll(Duration::ZERO).map_err(|_| TuiTerminalError::Poll)? {
            return event::read()
                .map(|observation| normalize_crossterm_event(&observation))
                .map_err(|_| TuiTerminalError::Poll);
        }
        let stdin = std::io::stdin();
        let interest = PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR;
        let mut descriptors = [
            PollFd::new(stdin.as_fd(), interest),
            PollFd::new(executor_wake, interest),
        ];
        let timeout = wait
            .map_or(Ok(PollTimeout::NONE), PollTimeout::try_from)
            .map_err(|_| TuiTerminalError::Poll)?;
        if poll(&mut descriptors, timeout).map_err(|_| TuiTerminalError::Poll)? == 0 {
            return Ok(None);
        }
        if !descriptors[0]
            .revents()
            .is_some_and(|ready| ready.intersects(interest))
        {
            return Ok(None);
        }
        if !event::poll(Duration::ZERO).map_err(|_| TuiTerminalError::Poll)? {
            return Ok(None);
        }
        event::read()
            .map(|observation| normalize_crossterm_event(&observation))
            .map_err(|_| TuiTerminalError::Poll)
    }

    fn draw(
        &mut self,
        model: &UiModel,
    ) -> Result<Option<UiConversationViewportObservation>, TuiTerminalError> {
        let theme = &self.theme;
        let cache = &mut self.render_cache;
        let mut observation = None;
        self.terminal
            .draw(|frame| {
                observation = hq_tui::render_with_cache(frame, model, theme, cache);
            })
            .map_err(|_| TuiTerminalError::Draw)?;
        Ok(observation)
    }

    fn set_theme(&mut self, theme: UiTheme) -> Result<(), TuiTerminalError> {
        self.theme = theme;
        self.render_cache = UiRenderCache::new();
        Ok(())
    }

    fn restore(&mut self) -> Result<(), TuiTerminalError> {
        let mut failed = false;
        if self.activation >= TerminalActivation::CursorHidden {
            failed |= execute!(self.terminal.backend_mut(), Show).is_err();
        }
        if self.activation >= TerminalActivation::BracketedPaste {
            failed |= execute!(self.terminal.backend_mut(), DisableBracketedPaste).is_err();
        }
        if self.activation >= TerminalActivation::MouseCapture {
            failed |= execute!(self.terminal.backend_mut(), DisableMouseCapture).is_err();
        }
        if self.activation >= TerminalActivation::AlternateScreen {
            failed |= execute!(self.terminal.backend_mut(), LeaveAlternateScreen).is_err();
        }
        if self.activation >= TerminalActivation::Raw {
            failed |= disable_raw_mode().is_err();
        }
        self.activation = TerminalActivation::Inactive;
        if failed {
            Err(TuiTerminalError::Restore)
        } else {
            Ok(())
        }
    }
}

/// Normalizes one Crossterm observation without changing model or terminal state.
pub fn normalize_crossterm_event(observation: &Event) -> Option<TuiTerminalEvent> {
    match observation {
        Event::Key(key) if key.kind != KeyEventKind::Release => normalize_key(*key),
        Event::Resize(width, height) => Some(TuiTerminalEvent::Resized(UiSize {
            width: *width,
            height: *height,
        })),
        Event::Paste(value) => Some(TuiTerminalEvent::Input(UiInput::Paste(value.clone()))),
        Event::FocusGained | Event::FocusLost | Event::Key(_) | Event::Mouse(_) => None,
    }
}

fn normalize_key(key: KeyEvent) -> Option<TuiTerminalEvent> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(TuiTerminalEvent::Cancelled);
    }
    if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('j' | 'J')) {
        return Some(TuiTerminalEvent::Input(UiInput::InsertNewline));
    }
    let plain = !key.modifiers.intersects(
        KeyModifiers::CONTROL
            | KeyModifiers::ALT
            | KeyModifiers::SUPER
            | KeyModifiers::HYPER
            | KeyModifiers::META,
    );
    let input = match key.code {
        KeyCode::Tab if plain => UiInput::NextFocus,
        KeyCode::BackTab if plain => UiInput::PreviousFocus,
        KeyCode::Right if plain => UiInput::MoveCursorRight,
        KeyCode::Left if plain => UiInput::MoveCursorLeft,
        KeyCode::Down if plain => UiInput::NextItem,
        KeyCode::Up if plain => UiInput::PreviousItem,
        KeyCode::Enter if key.modifiers == KeyModifiers::SHIFT => UiInput::InsertNewline,
        KeyCode::Enter if plain => UiInput::Activate,
        KeyCode::F(1) if plain => UiInput::Help,
        KeyCode::F(5) if plain => UiInput::Refresh,
        KeyCode::PageDown if plain => UiInput::LoadMore,
        KeyCode::Backspace if plain => UiInput::Backspace,
        KeyCode::Delete if plain => UiInput::Delete,
        KeyCode::Home if plain => UiInput::MoveCursorHome,
        KeyCode::End if plain => UiInput::MoveCursorEnd,
        KeyCode::Char(character) if plain => UiInput::Character(character),
        KeyCode::Esc => UiInput::Escape,
        _ => return None,
    };
    Some(TuiTerminalEvent::Input(input))
}

/// Runs the complete terminal loop over injected terminal, client, and monotonic clock ports.
pub fn run_tui_shell<T, P, O, C>(
    terminal: T,
    client: P,
    observer: O,
    clock: C,
) -> Result<(), TuiShellError>
where
    T: TuiTerminalPort,
    P: TuiClientPort + 'static,
    O: TuiObservationPort + 'static,
    C: TuiClock,
{
    let trace = BoundaryTrace::from_environment(BoundaryProcess::Tui);
    let mut drawn_interactions = BTreeSet::new();
    let home_directory = current_user_home_directory();
    let mut observer = observer;
    let initial_view = observer.take_initial_view();
    let mut terminal = TerminalGuard::activate(terminal)?;
    let mut executor = TuiEffectExecutor::spawn_with_observer(client, observer, clock)
        .map_err(|_| TuiShellError::Executor)?;
    let mut model = UiModel::new(terminal.size()?).with_home_directory(home_directory);
    if let Some(view) = initial_view {
        let initial_transition = update(model, UiEvent::MaterializedViewObserved { view })
            .map_err(|_| TuiShellError::Model)?;
        model = initial_transition.model;
        executor
            .execute(initial_transition.effects)
            .map_err(|_| TuiShellError::Executor)?;
    }
    let started = update(model, UiEvent::Started).map_err(|_| TuiShellError::Model)?;
    let mut model = started.model;
    executor
        .execute(started.effects)
        .map_err(|_| TuiShellError::Executor)?;

    while !executor.exit_requested() {
        while let Some(event) = executor.poll_event() {
            trace_tui_event(&trace, BoundaryKind::TuiObservationReceived, &event);
            if let UiEvent::ConfigurationSaved {
                effect_id,
                theme: Some(theme),
                ..
            } = &event
                && model.configuration_effect_pending(*effect_id)
            {
                terminal.set_theme(theme.clone())?;
            }
            let transition = update(model, event).map_err(|_| TuiShellError::Model)?;
            model = transition.model;
            trace_current_interaction(&trace, BoundaryKind::TuiModelUpdated, &model);
            executor
                .execute(transition.effects)
                .map_err(|_| TuiShellError::Executor)?;
        }
        if executor.take_redraw_request() {
            let observation = terminal.draw(&model)?;
            if let Some(interaction) = model.current_interaction()
                && drawn_interactions.insert(interaction.request_id)
            {
                trace.record(
                    BoundaryKind::TuiDialogDrawn,
                    BoundaryIds {
                        operation: Some(interaction.operation_id),
                        provider_request: Some(interaction.request_id),
                        ..BoundaryIds::default()
                    },
                );
            }
            if let Some(observation) = observation {
                let transition =
                    update(model, UiEvent::ConversationViewportObserved { observation })
                        .map_err(|_| TuiShellError::Model)?;
                model = transition.model;
                executor
                    .execute(transition.effects)
                    .map_err(|_| TuiShellError::Executor)?;
            }
        }
        if executor.exit_requested() {
            break;
        }
        if executor.worker_stopped() {
            return Err(TuiShellError::Executor);
        }
        let wait = executor.time_until_event();
        if let Some(event) = terminal.poll(executor.event_wake().as_fd(), wait)? {
            let event = match event {
                TuiTerminalEvent::Input(input) => UiEvent::Input(input),
                TuiTerminalEvent::Resized(size) => UiEvent::Resized(size),
                TuiTerminalEvent::Cancelled => UiEvent::Input(UiInput::Quit),
            };
            let transition = update(model, event).map_err(|_| TuiShellError::Model)?;
            model = transition.model;
            executor
                .execute(transition.effects)
                .map_err(|_| TuiShellError::Executor)?;
        }
    }

    executor.shutdown().map_err(|_| TuiShellError::Executor)?;
    terminal.finish()?;
    Ok(())
}

fn trace_tui_event(trace: &BoundaryTrace, kind: BoundaryKind, event: &UiEvent) {
    match event {
        UiEvent::InteractionsObserved { interactions } => {
            for interaction in interactions {
                trace.record(
                    kind,
                    BoundaryIds {
                        operation: Some(interaction.operation_id),
                        provider_request: Some(interaction.request_id),
                        ..BoundaryIds::default()
                    },
                );
            }
        }
        UiEvent::Invalidated { revision } => trace.record(
            kind,
            BoundaryIds {
                revision: Some(*revision),
                ..BoundaryIds::default()
            },
        ),
        UiEvent::ConnectionObserved { generation, .. }
        | UiEvent::ClientFailed { generation, .. } => {
            trace.record(
                kind,
                BoundaryIds {
                    subscription_generation: Some(*generation),
                    ..BoundaryIds::default()
                },
            );
        }
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
        | UiEvent::ProjectCommandFailed { effect_id, .. } => trace.record(
            kind,
            BoundaryIds {
                tui_effect: Some(effect_id.value()),
                ..BoundaryIds::default()
            },
        ),
        _ => {}
    }
}

fn trace_current_interaction(trace: &BoundaryTrace, kind: BoundaryKind, model: &UiModel) {
    if let Some(interaction) = model.current_interaction() {
        trace.record(
            kind,
            BoundaryIds {
                operation: Some(interaction.operation_id),
                provider_request: Some(interaction.request_id),
                ..BoundaryIds::default()
            },
        );
    }
}

fn current_user_home_directory() -> Option<String> {
    User::from_uid(Uid::effective())
        .ok()
        .flatten()
        .and_then(|user| user.dir.into_os_string().into_string().ok())
}

/// Composes the installed subscribed client and real terminal shell for one state root.
pub fn run_installed_tui(state: StatePaths) -> Result<(), TuiShellError> {
    state.validate_identity().map_err(TuiShellError::Identity)?;
    let theme = resolve_installed_tui_theme(&state, &TuiThemeEnvironment::from_environment())?;
    let build = BuildMetadata::new(
        "hq",
        env!("CARGO_PKG_VERSION"),
        option_env!("HQ_BUILD_COMMIT"),
    )
    .map_err(|_| TuiShellError::Build)?;
    let client_config = installed_local_client_config(state.clone(), build, InitialView::OnDemand);
    let mut event_client =
        LocalNodeEventClient::connect(client_config.clone()).map_err(|_| TuiShellError::Client)?;
    let subscription_base = event_client
        .activate_subscription()
        .map_err(|_| TuiShellError::Client)?;
    let command_client =
        LocalNodeClient::connect(client_config).map_err(|_| TuiShellError::Client)?;
    let (client, observer) =
        compose_tui_clients(command_client, event_client, state, &subscription_base)
            .map_err(|_| TuiShellError::Client)?;
    let terminal = CrosstermTerminal::new(theme)?;
    run_tui_shell(terminal, client, observer, MonotonicTuiClock::default())
}

/// Loads and resolves the immutable startup theme before any terminal mode is activated.
pub fn resolve_installed_tui_theme(
    state: &StatePaths,
    environment: &TuiThemeEnvironment,
) -> Result<UiTheme, TuiShellError> {
    let configuration = state
        .load_configuration()
        .map_err(TuiShellError::Configuration)?;
    resolve_tui_theme(configuration.theme.as_ref(), environment).map_err(TuiShellError::Theme)
}

struct TerminalGuard<T: TuiTerminalPort> {
    terminal: T,
    armed: bool,
}

impl<T: TuiTerminalPort> TerminalGuard<T> {
    fn activate(terminal: T) -> Result<Self, TuiTerminalError> {
        let mut guard = Self {
            terminal,
            armed: true,
        };
        guard.terminal.activate()?;
        Ok(guard)
    }

    fn size(&mut self) -> Result<UiSize, TuiTerminalError> {
        self.terminal.size()
    }

    fn poll(
        &mut self,
        executor_wake: BorrowedFd<'_>,
        wait: Option<Duration>,
    ) -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
        self.terminal.poll(executor_wake, wait)
    }

    fn draw(
        &mut self,
        model: &UiModel,
    ) -> Result<Option<UiConversationViewportObservation>, TuiTerminalError> {
        self.terminal.draw(model)
    }

    fn set_theme(&mut self, theme: UiTheme) -> Result<(), TuiTerminalError> {
        self.terminal.set_theme(theme)
    }

    fn finish(mut self) -> Result<(), TuiTerminalError> {
        let restored = self.terminal.restore();
        self.armed = false;
        restored
    }
}

impl<T: TuiTerminalPort> Drop for TerminalGuard<T> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.terminal.restore();
            self.armed = false;
        }
    }
}
