//! Crossterm terminal ownership around the pure TUI reducer and effect executor.

use std::{
    collections::BTreeSet,
    fmt,
    io::Stdout,
    os::fd::{AsFd, BorrowedFd},
    time::{Duration, Instant},
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
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    unistd::{Uid, User},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    BoundaryIds, BoundaryKind, BoundaryProcess, BoundaryTrace, IdentityError, IdentityErrorClass,
    LocalNodeClient, LocalNodeEventClient, MonotonicTuiClock, StatePaths, TuiClientPort, TuiClock,
    TuiEffectExecutor, TuiObservationPort, TuiTerminalIoKind, TuiTerminalPhase,
    TuiThemeEnvironment, TuiThemeError, local_client::installed_local_client_config,
    resolve_tui_theme, tui_client::compose_tui_clients,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalPollReadiness {
    TimedOut,
    Executor,
    Input,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalPollDeadline {
    Infinite,
    Finite(Instant),
}

impl TerminalPollDeadline {
    fn from_wait<N>(wait: Option<Duration>, now: &mut N) -> Result<Self, TuiTerminalError>
    where
        N: FnMut() -> Instant,
    {
        wait.map_or(Ok(Self::Infinite), |wait| {
            now()
                .checked_add(wait)
                .map(Self::Finite)
                .ok_or(TuiTerminalError::Poll)
        })
    }

    fn timeout_at<N>(self, now: &mut N) -> Result<PollTimeout, TuiTerminalError>
    where
        N: FnMut() -> Instant,
    {
        match self {
            Self::Infinite => Ok(PollTimeout::NONE),
            Self::Finite(deadline) => {
                PollTimeout::try_from(deadline.saturating_duration_since(now()))
                    .map_err(|_| TuiTerminalError::Poll)
            }
        }
    }
}

fn poll_terminal_with<N, E, P>(
    wait: Option<Duration>,
    mut now: N,
    mut next_event: E,
    mut poll_ready: P,
) -> Result<Option<TuiTerminalEvent>, TuiTerminalError>
where
    N: FnMut() -> Instant,
    E: FnMut() -> Result<Option<TuiTerminalEvent>, TuiTerminalError>,
    P: FnMut(PollTimeout) -> Result<TerminalPollReadiness, Errno>,
{
    let deadline = TerminalPollDeadline::from_wait(wait, &mut now)?;
    loop {
        if let Some(event) = next_event()? {
            return Ok(Some(event));
        }
        let timeout = deadline.timeout_at(&mut now)?;
        match poll_ready(timeout) {
            Ok(TerminalPollReadiness::TimedOut | TerminalPollReadiness::Executor) => {
                return Ok(None);
            }
            Ok(TerminalPollReadiness::Input) => return next_event(),
            Err(Errno::EINTR) => {}
            Err(error) => {
                return Err(terminal_io_error(
                    TuiTerminalPhase::Poll,
                    &std::io::Error::from_raw_os_error(error as i32),
                ));
            }
        }
    }
}

fn next_crossterm_event() -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
    if !event::poll(Duration::ZERO)
        .map_err(|error| terminal_io_error(TuiTerminalPhase::Poll, &error))?
    {
        return Ok(None);
    }
    event::read()
        .map(|observation| normalize_crossterm_event(&observation))
        .map_err(|error| terminal_io_error(TuiTerminalPhase::Poll, &error))
}

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
    /// A real terminal operation failed with structured operating-system evidence.
    OperatingSystem {
        /// Exact terminal phase that failed.
        phase: TuiTerminalPhase,
        /// Stable operating-system error category.
        kind: TuiTerminalIoKind,
        /// Platform error number when supplied by the operating system.
        code: Option<i32>,
    },
}

impl fmt::Display for TuiTerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TUI terminal failed: {self:?}")
    }
}

impl std::error::Error for TuiTerminalError {}

impl TuiTerminalError {
    fn action(self) -> String {
        match self {
            Self::OperatingSystem { phase, kind, code } => {
                let code = code.map_or_else(|| "none".to_owned(), |value| value.to_string());
                format!(
                    "the interactive terminal failed during {} (error kind: {}; OS code: {code})",
                    phase.as_str(),
                    kind.as_str()
                )
            }
            Self::Activate => "the interactive terminal failed during activation".to_owned(),
            Self::Size => "the interactive terminal failed while reading its size".to_owned(),
            Self::Poll => "the interactive terminal failed while polling input".to_owned(),
            Self::Draw => "the interactive terminal failed while drawing a frame".to_owned(),
            Self::Restore => "the interactive terminal failed during restoration".to_owned(),
        }
    }

    fn diagnostic_parts(self) -> (TuiTerminalPhase, Option<TuiTerminalIoKind>, Option<i32>) {
        match self {
            Self::Activate => (TuiTerminalPhase::Activate, None, None),
            Self::Size => (TuiTerminalPhase::Size, None, None),
            Self::Poll => (TuiTerminalPhase::Poll, None, None),
            Self::Draw => (TuiTerminalPhase::Draw, None, None),
            Self::Restore => (TuiTerminalPhase::Restore, None, None),
            Self::OperatingSystem { phase, kind, code } => (phase, Some(kind), code),
        }
    }
}

impl TuiTerminalPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Activate => "activation",
            Self::Size => "size observation",
            Self::Poll => "input polling",
            Self::Draw => "frame drawing",
            Self::Restore => "restoration",
        }
    }
}

impl TuiTerminalIoKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::ConnectionReset => "connection_reset",
            Self::BrokenPipe => "broken_pipe",
            Self::WouldBlock => "would_block",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
            Self::Unsupported => "unsupported",
            Self::UnexpectedEof => "unexpected_eof",
            Self::Other => "other",
        }
    }
}

fn terminal_io_error(phase: TuiTerminalPhase, error: &std::io::Error) -> TuiTerminalError {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => TuiTerminalIoKind::NotFound,
        std::io::ErrorKind::PermissionDenied => TuiTerminalIoKind::PermissionDenied,
        std::io::ErrorKind::ConnectionReset => TuiTerminalIoKind::ConnectionReset,
        std::io::ErrorKind::BrokenPipe => TuiTerminalIoKind::BrokenPipe,
        std::io::ErrorKind::WouldBlock => TuiTerminalIoKind::WouldBlock,
        std::io::ErrorKind::TimedOut => TuiTerminalIoKind::TimedOut,
        std::io::ErrorKind::Interrupted => TuiTerminalIoKind::Interrupted,
        std::io::ErrorKind::Unsupported => TuiTerminalIoKind::Unsupported,
        std::io::ErrorKind::UnexpectedEof => TuiTerminalIoKind::UnexpectedEof,
        _ => TuiTerminalIoKind::Other,
    };
    TuiTerminalError::OperatingSystem {
        phase,
        kind,
        code: error.raw_os_error(),
    }
}

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
            Self::Terminal(error) => (
                "tui.terminal_failed",
                error.action(),
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
        let terminal = Terminal::new(backend)
            .map_err(|error| terminal_io_error(TuiTerminalPhase::Activate, &error))?;
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
        enable_raw_mode().map_err(|error| terminal_io_error(TuiTerminalPhase::Activate, &error))?;
        self.activation = TerminalActivation::Raw;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)
            .map_err(|error| terminal_io_error(TuiTerminalPhase::Activate, &error))?;
        self.activation = TerminalActivation::AlternateScreen;
        execute!(self.terminal.backend_mut(), EnableMouseCapture)
            .map_err(|error| terminal_io_error(TuiTerminalPhase::Activate, &error))?;
        self.activation = TerminalActivation::MouseCapture;
        execute!(self.terminal.backend_mut(), EnableBracketedPaste)
            .map_err(|error| terminal_io_error(TuiTerminalPhase::Activate, &error))?;
        self.activation = TerminalActivation::BracketedPaste;
        execute!(self.terminal.backend_mut(), Hide)
            .map_err(|error| terminal_io_error(TuiTerminalPhase::Activate, &error))?;
        self.activation = TerminalActivation::CursorHidden;
        Ok(())
    }

    fn size(&mut self) -> Result<UiSize, TuiTerminalError> {
        let size = self
            .terminal
            .size()
            .map_err(|error| terminal_io_error(TuiTerminalPhase::Size, &error))?;
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
        let interest = PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR;
        poll_terminal_with(wait, Instant::now, next_crossterm_event, |timeout| {
            let stdin = std::io::stdin();
            let mut descriptors = [
                PollFd::new(stdin.as_fd(), interest),
                PollFd::new(executor_wake, interest),
            ];
            let ready = poll(&mut descriptors, timeout)?;
            if ready == 0 {
                return Ok(TerminalPollReadiness::TimedOut);
            }
            if descriptors[0]
                .revents()
                .is_some_and(|ready| ready.intersects(interest))
            {
                Ok(TerminalPollReadiness::Input)
            } else {
                Ok(TerminalPollReadiness::Executor)
            }
        })
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
            .map_err(|error| terminal_io_error(TuiTerminalPhase::Draw, &error))?;
        Ok(observation)
    }

    fn set_theme(&mut self, theme: UiTheme) -> Result<(), TuiTerminalError> {
        self.theme = theme;
        self.render_cache = UiRenderCache::new();
        Ok(())
    }

    fn restore(&mut self) -> Result<(), TuiTerminalError> {
        let mut failure = None;
        if self.activation >= TerminalActivation::CursorHidden
            && let Err(error) = execute!(self.terminal.backend_mut(), Show)
        {
            failure.get_or_insert_with(|| terminal_io_error(TuiTerminalPhase::Restore, &error));
        }
        if self.activation >= TerminalActivation::BracketedPaste
            && let Err(error) = execute!(self.terminal.backend_mut(), DisableBracketedPaste)
        {
            failure.get_or_insert_with(|| terminal_io_error(TuiTerminalPhase::Restore, &error));
        }
        if self.activation >= TerminalActivation::MouseCapture
            && let Err(error) = execute!(self.terminal.backend_mut(), DisableMouseCapture)
        {
            failure.get_or_insert_with(|| terminal_io_error(TuiTerminalPhase::Restore, &error));
        }
        if self.activation >= TerminalActivation::AlternateScreen
            && let Err(error) = execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
        {
            failure.get_or_insert_with(|| terminal_io_error(TuiTerminalPhase::Restore, &error));
        }
        if self.activation >= TerminalActivation::Raw
            && let Err(error) = disable_raw_mode()
        {
            failure.get_or_insert_with(|| terminal_io_error(TuiTerminalPhase::Restore, &error));
        }
        self.activation = TerminalActivation::Inactive;
        failure.map_or(Ok(()), Err)
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
    if key.modifiers == KeyModifiers::CONTROL {
        let input = match key.code {
            KeyCode::Char('a' | 'A') => UiInput::MoveCursorHome,
            KeyCode::Char('e' | 'E') => UiInput::MoveCursorEnd,
            KeyCode::Char('u' | 'U') => UiInput::DeleteToLineEnd,
            KeyCode::Char('k' | 'K') => UiInput::DeleteToLineStart,
            KeyCode::Char('d' | 'D') => UiInput::Delete,
            _ => return None,
        };
        return Some(TuiTerminalEvent::Input(input));
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
    run_tui_shell_with_trace(
        terminal,
        client,
        observer,
        clock,
        &BoundaryTrace::from_environment(BoundaryProcess::Tui),
    )
}

fn run_tui_shell_with_trace<T, P, O, C>(
    terminal: T,
    client: P,
    observer: O,
    clock: C,
    trace: &BoundaryTrace,
) -> Result<(), TuiShellError>
where
    T: TuiTerminalPort,
    P: TuiClientPort + 'static,
    O: TuiObservationPort + 'static,
    C: TuiClock,
{
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
            trace_tui_event(trace, BoundaryKind::TuiObservationReceived, &event);
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
            trace_current_interaction(trace, BoundaryKind::TuiModelUpdated, &model);
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
    let trace = BoundaryTrace::from_state(state.root(), BoundaryProcess::Tui);
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
    let terminal = CrosstermTerminal::new(theme).map_err(|error| {
        record_terminal_failure(&trace, error);
        TuiShellError::Terminal(error)
    })?;
    let result = run_tui_shell_with_trace(
        terminal,
        client,
        observer,
        MonotonicTuiClock::default(),
        &trace,
    );
    if let Err(TuiShellError::Terminal(error)) = &result {
        record_terminal_failure(&trace, *error);
    }
    result
}

fn record_terminal_failure(trace: &BoundaryTrace, error: TuiTerminalError) {
    let (phase, kind, code) = error.diagnostic_parts();
    trace.record(
        BoundaryKind::TuiTerminalFailed,
        BoundaryIds {
            terminal_phase: Some(phase),
            terminal_io_kind: kind,
            terminal_os_code: code,
            ..BoundaryIds::default()
        },
    );
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[test]
    fn interrupted_poll_rechecks_the_terminal_event_queue() {
        let started = Instant::now();
        let mut observations = VecDeque::from([
            None,
            None,
            Some(TuiTerminalEvent::Resized(UiSize {
                width: 100,
                height: 24,
            })),
        ]);
        let mut waits = VecDeque::from([Err(Errno::EINTR), Err(Errno::EINTR)]);

        let event = poll_terminal_with(
            None,
            || started,
            || Ok(observations.pop_front().expect("bounded event probes")),
            |timeout| {
                assert_eq!(timeout, PollTimeout::NONE);
                waits.pop_front().expect("bounded poll attempts")
            },
        )
        .expect("interruptions are retryable");

        assert_eq!(
            event,
            Some(TuiTerminalEvent::Resized(UiSize {
                width: 100,
                height: 24,
            }))
        );
        assert!(waits.is_empty());
    }

    #[test]
    fn interrupted_poll_preserves_one_finite_deadline() {
        let started = Instant::now();
        let mut times = VecDeque::from([
            started,
            started,
            started + Duration::from_millis(40),
            started + Duration::from_millis(90),
        ]);
        let mut results = VecDeque::from([
            Err(Errno::EINTR),
            Err(Errno::EINTR),
            Ok(TerminalPollReadiness::TimedOut),
        ]);
        let mut timeouts = Vec::new();

        let event = poll_terminal_with(
            Some(Duration::from_millis(100)),
            || times.pop_front().expect("bounded clock observations"),
            || Ok(None),
            |timeout| {
                timeouts.push(timeout);
                results.pop_front().expect("bounded poll attempts")
            },
        )
        .expect("interruptions are retryable");

        assert_eq!(event, None);
        assert_eq!(
            timeouts,
            [
                PollTimeout::try_from(Duration::from_millis(100)).unwrap(),
                PollTimeout::try_from(Duration::from_millis(60)).unwrap(),
                PollTimeout::try_from(Duration::from_millis(10)).unwrap(),
            ]
        );
    }

    #[test]
    fn interrupted_poll_checks_readiness_once_at_an_expired_deadline() {
        let started = Instant::now();
        let mut times = VecDeque::from([started, started, started + Duration::from_millis(101)]);
        let mut results = VecDeque::from([Err(Errno::EINTR), Ok(TerminalPollReadiness::TimedOut)]);
        let mut timeouts = Vec::new();

        let event = poll_terminal_with(
            Some(Duration::from_millis(100)),
            || times.pop_front().expect("bounded clock observations"),
            || Ok(None),
            |timeout| {
                timeouts.push(timeout);
                results.pop_front().expect("bounded poll attempts")
            },
        )
        .expect("deadline expiry is orderly");

        assert_eq!(event, None);
        assert_eq!(
            timeouts,
            [
                PollTimeout::try_from(Duration::from_millis(100)).unwrap(),
                PollTimeout::ZERO,
            ]
        );
    }

    #[test]
    fn interrupted_infinite_poll_remains_infinite_until_ready() {
        let started = Instant::now();
        let mut results = VecDeque::from([
            Err(Errno::EINTR),
            Err(Errno::EINTR),
            Ok(TerminalPollReadiness::Executor),
        ]);
        let mut timeouts = Vec::new();

        let event = poll_terminal_with(
            None,
            || started,
            || Ok(None),
            |timeout| {
                timeouts.push(timeout);
                results.pop_front().expect("bounded poll attempts")
            },
        )
        .expect("interruptions are retryable");

        assert_eq!(event, None);
        assert_eq!(timeouts, [PollTimeout::NONE; 3]);
    }

    #[test]
    fn executor_wake_racing_an_interruption_returns_control_to_the_shell() {
        let started = Instant::now();
        let mut results = VecDeque::from([Err(Errno::EINTR), Ok(TerminalPollReadiness::Executor)]);

        let event = poll_terminal_with(
            None,
            || started,
            || Ok(None),
            |_| results.pop_front().expect("bounded poll attempts"),
        )
        .expect("executor readiness survives an interruption");

        assert_eq!(event, None);
        assert!(results.is_empty());
    }

    #[test]
    fn non_interruption_poll_error_preserves_typed_os_evidence() {
        let started = Instant::now();

        let error = poll_terminal_with(None, || started, || Ok(None), |_| Err(Errno::EIO))
            .expect_err("real poll failures remain fatal");

        assert_eq!(
            error,
            TuiTerminalError::OperatingSystem {
                phase: TuiTerminalPhase::Poll,
                kind: TuiTerminalIoKind::Other,
                code: Some(Errno::EIO as i32),
            }
        );
    }
}
