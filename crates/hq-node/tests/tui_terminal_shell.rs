//! Terminal normalization, event-loop, and restoration contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use hq_node::{
    TuiClientObservation, TuiClientPort, TuiClock, TuiTerminalError, TuiTerminalEvent,
    TuiTerminalPort, normalize_crossterm_event, run_tui_shell,
};
use hq_tui::{UiInput, UiModel, UiSection, UiSize, UiSnapshot};

#[test]
fn crossterm_events_normalize_to_the_closed_ui_vocabulary() {
    let cases = [
        (KeyCode::Char('q'), KeyModifiers::NONE, UiInput::Quit),
        (KeyCode::Tab, KeyModifiers::NONE, UiInput::NextFocus),
        (
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
            UiInput::PreviousFocus,
        ),
        (KeyCode::Right, KeyModifiers::NONE, UiInput::NextSection),
        (KeyCode::Char('l'), KeyModifiers::NONE, UiInput::NextSection),
        (KeyCode::Left, KeyModifiers::NONE, UiInput::PreviousSection),
        (
            KeyCode::Char('h'),
            KeyModifiers::NONE,
            UiInput::PreviousSection,
        ),
        (KeyCode::Down, KeyModifiers::NONE, UiInput::NextItem),
        (KeyCode::Char('j'), KeyModifiers::NONE, UiInput::NextItem),
        (KeyCode::Up, KeyModifiers::NONE, UiInput::PreviousItem),
        (
            KeyCode::Char('k'),
            KeyModifiers::NONE,
            UiInput::PreviousItem,
        ),
        (KeyCode::Enter, KeyModifiers::NONE, UiInput::Activate),
        (KeyCode::Esc, KeyModifiers::NONE, UiInput::Escape),
    ];
    for (code, modifiers, expected) in cases {
        assert_eq!(
            normalize_crossterm_event(&Event::Key(KeyEvent::new(code, modifiers))),
            Some(TuiTerminalEvent::Input(expected))
        );
    }
    assert_eq!(
        normalize_crossterm_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ))),
        Some(TuiTerminalEvent::Cancelled)
    );
    assert_eq!(
        normalize_crossterm_event(&Event::Resize(91, 27)),
        Some(TuiTerminalEvent::Resized(UiSize {
            width: 91,
            height: 27,
        }))
    );
    assert_eq!(
        normalize_crossterm_event(&Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ))),
        None
    );
    assert_eq!(
        normalize_crossterm_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        ))),
        None
    );
}

#[test]
fn normal_quit_and_ctrl_c_cancellation_restore_exactly_once() {
    for terminal_event in [
        TuiTerminalEvent::Input(UiInput::Quit),
        TuiTerminalEvent::Cancelled,
    ] {
        let log = Arc::new(Mutex::new(Vec::new()));
        let terminal = ScriptedTerminal::new(Arc::clone(&log), [Ok(Some(terminal_event))]);

        run_tui_shell(terminal, EmptyClient, FixedClock).expect("shell exits cleanly");

        let log = log.lock().expect("terminal log");
        assert_eq!(log.iter().filter(|entry| **entry == "restore").count(), 1);
        assert_eq!(log.first(), Some(&"activate"));
        assert!(log.contains(&"draw"));
    }
}

#[test]
fn terminal_errors_and_partial_activation_restore_exactly_once() {
    let poll_log = Arc::new(Mutex::new(Vec::new()));
    let poll_terminal = ScriptedTerminal::new(Arc::clone(&poll_log), [Err(TuiTerminalError::Poll)]);
    assert!(run_tui_shell(poll_terminal, EmptyClient, FixedClock).is_err());
    assert_eq!(
        poll_log
            .lock()
            .expect("poll log")
            .iter()
            .filter(|entry| **entry == "restore")
            .count(),
        1
    );

    let activation_log = Arc::new(Mutex::new(Vec::new()));
    let mut activation_terminal = ScriptedTerminal::new(
        Arc::clone(&activation_log),
        std::iter::empty::<Result<Option<TuiTerminalEvent>, TuiTerminalError>>(),
    );
    activation_terminal.activation_fails = true;
    assert!(run_tui_shell(activation_terminal, EmptyClient, FixedClock).is_err());
    assert_eq!(
        activation_log.lock().expect("activation log").as_slice(),
        &["activate", "restore"]
    );
}

#[test]
fn panic_unwinding_restores_the_terminal_exactly_once() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut terminal = ScriptedTerminal::new(
        Arc::clone(&log),
        [Ok(Some(TuiTerminalEvent::Input(UiInput::Quit)))],
    );
    terminal.draw_panics = true;

    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = run_tui_shell(terminal, EmptyClient, FixedClock);
        }))
        .is_err()
    );
    assert_eq!(
        log.lock()
            .expect("panic log")
            .iter()
            .filter(|entry| **entry == "restore")
            .count(),
        1
    );
}

#[test]
fn client_worker_failure_restores_the_terminal_exactly_once() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let terminal = ScriptedTerminal::new(
        Arc::clone(&log),
        [Ok(Some(TuiTerminalEvent::Input(UiInput::Quit)))],
    );

    assert!(run_tui_shell(terminal, PanickingClient, FixedClock).is_err());
    assert_eq!(
        log.lock()
            .expect("client failure log")
            .iter()
            .filter(|entry| **entry == "restore")
            .count(),
        1
    );
}

struct ScriptedTerminal {
    log: Arc<Mutex<Vec<&'static str>>>,
    events: VecDeque<Result<Option<TuiTerminalEvent>, TuiTerminalError>>,
    activation_fails: bool,
    draw_panics: bool,
}

impl ScriptedTerminal {
    fn new(
        log: Arc<Mutex<Vec<&'static str>>>,
        events: impl IntoIterator<Item = Result<Option<TuiTerminalEvent>, TuiTerminalError>>,
    ) -> Self {
        Self {
            log,
            events: events.into_iter().collect(),
            activation_fails: false,
            draw_panics: false,
        }
    }

    fn record(&self, event: &'static str) {
        self.log.lock().expect("terminal log").push(event);
    }
}

impl TuiTerminalPort for ScriptedTerminal {
    fn activate(&mut self) -> Result<(), TuiTerminalError> {
        self.record("activate");
        if self.activation_fails {
            Err(TuiTerminalError::Activate)
        } else {
            Ok(())
        }
    }

    fn size(&mut self) -> Result<UiSize, TuiTerminalError> {
        self.record("size");
        Ok(UiSize {
            width: 80,
            height: 24,
        })
    }

    fn poll(&mut self, _wait: Duration) -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
        self.record("poll");
        self.events
            .pop_front()
            .unwrap_or(Ok(Some(TuiTerminalEvent::Input(UiInput::Quit))))
    }

    fn draw(&mut self, _model: &UiModel) -> Result<(), TuiTerminalError> {
        self.record("draw");
        assert!(!self.draw_panics, "scripted draw panic");
        Ok(())
    }

    fn restore(&mut self) -> Result<(), TuiTerminalError> {
        self.record("restore");
        Ok(())
    }
}

struct EmptyClient;

impl TuiClientPort for EmptyClient {
    fn load_snapshot(&mut self, section: UiSection) -> Result<UiSnapshot, hq_tui::UiFailure> {
        Ok(UiSnapshot {
            section,
            revision: 1,
            rows: Vec::new(),
        })
    }

    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation> {
        thread::sleep(wait);
        Vec::new()
    }
}

struct PanickingClient;

impl TuiClientPort for PanickingClient {
    fn load_snapshot(&mut self, _section: UiSection) -> Result<UiSnapshot, hq_tui::UiFailure> {
        panic!("scripted client failure")
    }

    fn poll(&mut self, _wait: Duration) -> Vec<TuiClientObservation> {
        Vec::new()
    }
}

#[derive(Clone, Copy)]
struct FixedClock;

impl TuiClock for FixedClock {
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}
