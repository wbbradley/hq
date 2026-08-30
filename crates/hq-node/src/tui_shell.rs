//! Crossterm terminal ownership around the pure TUI reducer and effect executor.

use std::{fmt, io::Stdout, time::Duration};

use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hq_local_api::{InitialView, protocol::v1::BuildMetadata};
use hq_tui::{UiEvent, UiInput, UiModel, UiSize, UiTheme, update};
use nix::unistd::{Uid, User};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    IdentityError, IdentityErrorClass, LocalNodeEventClient, LocalTuiClient, MonotonicTuiClock,
    StatePaths, TuiClientPort, TuiClock, TuiEffectExecutor,
    local_client::installed_local_client_config,
};

const MAX_TERMINAL_WAIT: Duration = Duration::from_millis(50);

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiShellError {
    /// Installed identity state is absent, unsafe, or invalid.
    Identity(IdentityError),
    /// The terminal boundary failed.
    Terminal(TuiTerminalError),
    /// The pure UI model rejected an event.
    Model,
    /// The bounded effect executor failed or its worker panicked.
    Executor,
    /// The ordinary subscribed local client could not be composed.
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
    pub const fn diagnostic(&self) -> (&'static str, &'static str) {
        match self {
            Self::Identity(error)
                if matches!(error.class(), IdentityErrorClass::IdentityMissing) =>
            {
                (
                    "setup.identity_required",
                    "HQ needs a device identity before it can protect your account and messages.\nRun `hq identity init` to create it, or import an existing identity.\nThen run `hq` again; the next screen will guide account setup.",
                )
            }
            Self::Identity(_) => (
                "setup.identity_invalid",
                "inspect or restore the installation identity before starting the TUI",
            ),
            Self::Terminal(_) => (
                "tui.terminal_failed",
                "the interactive terminal could not be activated, drawn, or restored",
            ),
            Self::Model => (
                "tui.model_failed",
                "the interactive model rejected a terminal or client event",
            ),
            Self::Executor => (
                "tui.executor_failed",
                "the interactive client worker stopped unexpectedly",
            ),
            Self::Client => (
                "tui.client_failed",
                "the local node client could not be started or connected",
            ),
            Self::Build => (
                "tui.build_invalid",
                "the installed build metadata is invalid",
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
    /// Polls one normalized terminal event for a bounded interval.
    fn poll(&mut self, wait: Duration) -> Result<Option<TuiTerminalEvent>, TuiTerminalError>;
    /// Draws the latest complete model by immutable borrow.
    fn draw(&mut self, model: &UiModel) -> Result<(), TuiTerminalError>;
    /// Restores every mode activated by this capability; repeated calls are safe.
    fn restore(&mut self) -> Result<(), TuiTerminalError>;
}

/// Real Crossterm/Ratatui terminal capability for the installed executable.
pub struct CrosstermTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    theme: UiTheme,
    activation: TerminalActivation,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TerminalActivation {
    Inactive,
    Raw,
    AlternateScreen,
    MouseCapture,
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

    fn poll(&mut self, wait: Duration) -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
        if !event::poll(wait).map_err(|_| TuiTerminalError::Poll)? {
            return Ok(None);
        }
        event::read()
            .map(|observation| normalize_crossterm_event(&observation))
            .map_err(|_| TuiTerminalError::Poll)
    }

    fn draw(&mut self, model: &UiModel) -> Result<(), TuiTerminalError> {
        self.terminal
            .draw(|frame| hq_tui::render(frame, model, &self.theme))
            .map(|_| ())
            .map_err(|_| TuiTerminalError::Draw)
    }

    fn restore(&mut self) -> Result<(), TuiTerminalError> {
        let mut failed = false;
        if self.activation >= TerminalActivation::CursorHidden {
            failed |= execute!(self.terminal.backend_mut(), Show).is_err();
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
pub fn run_tui_shell<T, P, C>(terminal: T, client: P, clock: C) -> Result<(), TuiShellError>
where
    T: TuiTerminalPort,
    P: TuiClientPort + 'static,
    C: TuiClock,
{
    let home_directory = current_user_home_directory();
    let mut terminal = TerminalGuard::activate(terminal)?;
    let mut executor =
        TuiEffectExecutor::spawn(client, clock).map_err(|_| TuiShellError::Executor)?;
    let model = UiModel::new(terminal.size()?).with_home_directory(home_directory);
    let started = update(model, UiEvent::Started).map_err(|_| TuiShellError::Model)?;
    let mut model = started.model;
    executor
        .execute(started.effects)
        .map_err(|_| TuiShellError::Executor)?;

    while !executor.exit_requested() {
        while let Some(event) = executor.poll_event() {
            let transition = update(model, event).map_err(|_| TuiShellError::Model)?;
            model = transition.model;
            executor
                .execute(transition.effects)
                .map_err(|_| TuiShellError::Executor)?;
        }
        if executor.take_redraw_request() {
            terminal.draw(&model)?;
        }
        if executor.exit_requested() {
            break;
        }
        let wait = executor.time_until_event(MAX_TERMINAL_WAIT);
        if let Some(event) = terminal.poll(wait)? {
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

fn current_user_home_directory() -> Option<String> {
    User::from_uid(Uid::effective())
        .ok()
        .flatten()
        .and_then(|user| user.dir.into_os_string().into_string().ok())
}

/// Composes the installed subscribed client and real terminal shell for one state root.
pub fn run_installed_tui(state: StatePaths) -> Result<(), TuiShellError> {
    state.validate_identity().map_err(TuiShellError::Identity)?;
    let build = BuildMetadata::new(
        "hq",
        env!("CARGO_PKG_VERSION"),
        option_env!("HQ_BUILD_COMMIT"),
    )
    .map_err(|_| TuiShellError::Build)?;
    let event_client = LocalNodeEventClient::connect(installed_local_client_config(
        state.clone(),
        build,
        InitialView::OnDemand,
    ))
    .map_err(|_| TuiShellError::Client)?;
    let terminal = CrosstermTerminal::new(UiTheme::terminal())?;
    run_tui_shell(
        terminal,
        LocalTuiClient::new(event_client, state),
        MonotonicTuiClock::default(),
    )
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

    fn poll(&mut self, wait: Duration) -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
        self.terminal.poll(wait)
    }

    fn draw(&mut self, model: &UiModel) -> Result<(), TuiTerminalError> {
        self.terminal.draw(model)
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
