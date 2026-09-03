//! Terminal normalization, event-loop, and restoration contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::VecDeque,
    os::fd::BorrowedFd,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use hq_node::{
    TuiClientObservation, TuiClientPort, TuiClock, TuiDraftError, TuiObservationInterrupt,
    TuiObservationPort, TuiShellError, TuiTerminalError, TuiTerminalEvent, TuiTerminalIoKind,
    TuiTerminalPhase, TuiTerminalPort, normalize_crossterm_event, run_tui_shell,
};
use hq_tui::{
    UiConversationAuthor, UiConversationEntry, UiConversationEntryGeometry,
    UiConversationEntryPresentation, UiConversationPage, UiConversationViewportObservation,
    UiConversationViewportPosition, UiFailure, UiHumanState, UiInput, UiInteraction,
    UiInteractionKind, UiMailboxAction, UiMailboxCommandResult, UiMailboxDraft,
    UiMailboxDraftTarget, UiMaterializedConversationView, UiMessageState, UiModel, UiRow,
    UiRowKind, UiRowState, UiSize, UiSnapshot,
};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

#[test]
fn crossterm_events_normalize_to_the_closed_ui_vocabulary() {
    let cases = [
        (
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            UiInput::Character('q'),
        ),
        (
            KeyCode::Char('6'),
            KeyModifiers::NONE,
            UiInput::Character('6'),
        ),
        (KeyCode::Tab, KeyModifiers::NONE, UiInput::NextFocus),
        (
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
            UiInput::PreviousFocus,
        ),
        (KeyCode::Right, KeyModifiers::NONE, UiInput::MoveCursorRight),
        (
            KeyCode::Char('l'),
            KeyModifiers::NONE,
            UiInput::Character('l'),
        ),
        (KeyCode::Left, KeyModifiers::NONE, UiInput::MoveCursorLeft),
        (
            KeyCode::Char('h'),
            KeyModifiers::NONE,
            UiInput::Character('h'),
        ),
        (KeyCode::Down, KeyModifiers::NONE, UiInput::NextItem),
        (
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            UiInput::Character('j'),
        ),
        (KeyCode::Up, KeyModifiers::NONE, UiInput::PreviousItem),
        (
            KeyCode::Char('k'),
            KeyModifiers::NONE,
            UiInput::Character('k'),
        ),
        (KeyCode::Enter, KeyModifiers::NONE, UiInput::Activate),
        (KeyCode::Enter, KeyModifiers::SHIFT, UiInput::InsertNewline),
        (
            KeyCode::Char('j'),
            KeyModifiers::CONTROL,
            UiInput::InsertNewline,
        ),
        (KeyCode::PageDown, KeyModifiers::NONE, UiInput::LoadMore),
        (KeyCode::Esc, KeyModifiers::NONE, UiInput::Escape),
        (KeyCode::Backspace, KeyModifiers::NONE, UiInput::Backspace),
        (KeyCode::Delete, KeyModifiers::NONE, UiInput::Delete),
        (KeyCode::Home, KeyModifiers::NONE, UiInput::MoveCursorHome),
        (KeyCode::End, KeyModifiers::NONE, UiInput::MoveCursorEnd),
        (KeyCode::F(1), KeyModifiers::NONE, UiInput::Help),
        (KeyCode::F(5), KeyModifiers::NONE, UiInput::Refresh),
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
        Some(TuiTerminalEvent::Input(UiInput::Character('x')))
    );
    assert_eq!(
        normalize_crossterm_event(&Event::Paste("pasted text".to_owned())),
        Some(TuiTerminalEvent::Input(UiInput::Paste(
            "pasted text".to_owned()
        )))
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

        run_tui_shell(terminal, EmptyClient, idle_observer(), FixedClock)
            .expect("shell exits cleanly");

        let log = log.lock().expect("terminal log");
        assert_eq!(log.iter().filter(|entry| **entry == "restore").count(), 1);
        assert_eq!(log.first(), Some(&"activate"));
        assert!(log.contains(&"draw"));
    }
}

#[test]
fn retained_subscription_view_is_drawn_without_refetching_startup_state() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let terminal = ScriptedTerminal::new(
        Arc::clone(&log),
        [Ok(Some(TuiTerminalEvent::Input(UiInput::Quit)))],
    );
    let snapshot_loads = Arc::new(AtomicUsize::new(0));
    let client = CountingClient {
        snapshot_loads: Arc::clone(&snapshot_loads),
    };
    let snapshot = UiSnapshot {
        revision: 7,
        human_state: UiHumanState::Ready,
        inbox_rows: vec![UiRow {
            id: "thread-a".to_owned(),
            title: "Alice".to_owned(),
            detail: "ready".to_owned(),
            state: UiRowState::Open,
            kind: UiRowKind::Conversation,
            conversation_target: None,
        }],
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: Vec::new(),
        project_rows: Vec::new(),
        direct_targets: Vec::new(),
        providers: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    };
    let observer = InitialObserver {
        idle: idle_observer(),
        initial: Some(UiMaterializedConversationView {
            snapshot,
            conversation: Some(UiConversationPage {
                title: "Alice".to_owned(),
                context: None,
                row_id: "thread-a".to_owned(),
                entries: Vec::new(),
                next_cursor: None,
            }),
        }),
    };

    run_tui_shell(terminal, client, observer, FixedClock).expect("shell exits cleanly");

    assert_eq!(snapshot_loads.load(Ordering::SeqCst), 0);
    assert!(log.lock().expect("terminal log").contains(&"draw"));
}

#[test]
fn idle_shell_wakes_and_draws_a_provider_interaction_without_terminal_polling() {
    let (notify_observer, observer_wake) = mpsc::channel();
    let dialog_drawn = Arc::new(AtomicBool::new(false));
    let terminal = WakeDrivenTerminal {
        notify_observer: notify_observer.clone(),
        observer_notified: false,
        dialog_drawn: Arc::clone(&dialog_drawn),
    };
    let observer = InteractionObserver {
        wake: observer_wake,
        interrupt: IdleInterrupt(notify_observer),
        published: false,
    };

    assert_eq!(
        run_tui_shell(terminal, EmptyClient, observer, FixedClock),
        Err(hq_node::TuiShellError::Terminal(TuiTerminalError::Poll))
    );

    assert!(dialog_drawn.load(Ordering::SeqCst));
}

#[test]
fn passive_draw_geometry_is_reduced_before_the_next_frame() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let positions = Arc::new(Mutex::new(Vec::new()));
    let mut terminal = ScriptedTerminal::new(
        Arc::clone(&log),
        [Ok(None), Ok(Some(TuiTerminalEvent::Input(UiInput::Quit)))],
    );
    terminal.draw_positions = Some(Arc::clone(&positions));
    terminal
        .draw_observations
        .push_back(Some(UiConversationViewportObservation {
            conversation_id: "thread-a".to_owned(),
            width: 40,
            height: 4,
            entries: vec![UiConversationEntryGeometry {
                entry_id: "message-1".to_owned(),
                height: 7,
            }],
        }));
    let observer = InitialObserver {
        idle: idle_observer(),
        initial: Some(UiMaterializedConversationView {
            snapshot: UiSnapshot {
                revision: 1,
                human_state: UiHumanState::Ready,
                inbox_rows: vec![UiRow {
                    id: "thread-a".to_owned(),
                    title: "Alice".to_owned(),
                    detail: "ready".to_owned(),
                    state: UiRowState::Open,
                    kind: UiRowKind::Conversation,
                    conversation_target: None,
                }],
                sent_rows: Vec::new(),
                archived_rows: Vec::new(),
                agent_rows: Vec::new(),
                project_rows: Vec::new(),
                direct_targets: Vec::new(),
                providers: Vec::new(),
                agents: Vec::new(),
                projects: Vec::new(),
            },
            conversation: Some(UiConversationPage {
                title: "Alice".to_owned(),
                context: None,
                row_id: "thread-a".to_owned(),
                entries: vec![UiConversationEntry {
                    id: "message-1".to_owned(),
                    presentation: UiConversationEntryPresentation::Message {
                        author: UiConversationAuthor::Participant("Alice".to_owned()),
                        body: "long message".to_owned(),
                    },
                    message_state: Some(UiMessageState::Open),
                    delivery: None,
                    message_target: None,
                    technical: Vec::new(),
                }],
                next_cursor: None,
            }),
        }),
    };

    run_tui_shell(terminal, EmptyClient, observer, FixedClock).expect("shell exits cleanly");

    assert_eq!(
        positions.lock().expect("draw positions").as_slice(),
        &[
            None,
            Some(UiConversationViewportPosition {
                entry_id: "message-1".to_owned(),
                row: 3,
            }),
        ]
    );
}

#[test]
fn terminal_errors_and_partial_activation_restore_exactly_once() {
    let poll_log = Arc::new(Mutex::new(Vec::new()));
    let poll_terminal = ScriptedTerminal::new(Arc::clone(&poll_log), [Err(TuiTerminalError::Poll)]);
    assert!(run_tui_shell(poll_terminal, EmptyClient, idle_observer(), FixedClock).is_err());
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
    assert!(
        run_tui_shell(
            activation_terminal,
            EmptyClient,
            idle_observer(),
            FixedClock,
        )
        .is_err()
    );
    assert_eq!(
        activation_log.lock().expect("activation log").as_slice(),
        &["activate", "restore"]
    );
}

#[test]
fn terminal_diagnostic_preserves_phase_error_kind_and_os_code_without_backend_prose() {
    let error = TuiShellError::Terminal(TuiTerminalError::OperatingSystem {
        phase: TuiTerminalPhase::Restore,
        kind: TuiTerminalIoKind::BrokenPipe,
        code: Some(32),
    });

    assert_eq!(
        error.diagnostic(),
        (
            "tui.terminal_failed",
            "the interactive terminal failed during restoration (error kind: broken_pipe; OS code: 32)"
                .to_owned(),
        )
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
            let _ = run_tui_shell(terminal, EmptyClient, idle_observer(), FixedClock);
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
    let mut terminal = ScriptedTerminal::new(
        Arc::clone(&log),
        [Ok(Some(TuiTerminalEvent::Input(UiInput::Quit)))],
    );
    terminal.draw_delay = Duration::from_millis(50);

    assert!(run_tui_shell(terminal, PanickingClient, idle_observer(), FixedClock).is_err());
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
    draw_delay: Duration,
    draw_observations: VecDeque<Option<UiConversationViewportObservation>>,
    draw_positions: Option<Arc<Mutex<Vec<Option<UiConversationViewportPosition>>>>>,
}

struct WakeDrivenTerminal {
    notify_observer: mpsc::Sender<()>,
    observer_notified: bool,
    dialog_drawn: Arc<AtomicBool>,
}

impl TuiTerminalPort for WakeDrivenTerminal {
    fn activate(&mut self) -> Result<(), TuiTerminalError> {
        Ok(())
    }

    fn size(&mut self) -> Result<UiSize, TuiTerminalError> {
        Ok(UiSize {
            width: 80,
            height: 24,
        })
    }

    fn poll(
        &mut self,
        executor_wake: BorrowedFd<'_>,
        wait: Option<Duration>,
    ) -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
        if self.dialog_drawn.load(Ordering::SeqCst) {
            return Err(TuiTerminalError::Poll);
        }
        if !self.observer_notified {
            self.observer_notified = true;
            self.notify_observer.send(()).expect("wake observer");
        }
        let mut descriptors = [PollFd::new(
            executor_wake,
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
        )];
        let timeout = wait
            .unwrap_or(Duration::from_secs(2))
            .min(Duration::from_secs(2));
        let timeout = PollTimeout::try_from(timeout).expect("valid shell wait");
        let ready = poll(&mut descriptors, timeout).map_err(|_| TuiTerminalError::Poll)?;
        assert_ne!(ready, 0, "provider interaction must wake the idle shell");
        Ok(None)
    }

    fn draw(
        &mut self,
        model: &UiModel,
    ) -> Result<Option<UiConversationViewportObservation>, TuiTerminalError> {
        if model.interaction_modal().is_some() {
            self.dialog_drawn.store(true, Ordering::SeqCst);
        }
        Ok(None)
    }

    fn restore(&mut self) -> Result<(), TuiTerminalError> {
        Ok(())
    }
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
            draw_delay: Duration::ZERO,
            draw_observations: VecDeque::new(),
            draw_positions: None,
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

    fn poll(
        &mut self,
        _executor_wake: BorrowedFd<'_>,
        _wait: Option<Duration>,
    ) -> Result<Option<TuiTerminalEvent>, TuiTerminalError> {
        self.record("poll");
        self.events
            .pop_front()
            .unwrap_or(Ok(Some(TuiTerminalEvent::Input(UiInput::Quit))))
    }

    fn draw(
        &mut self,
        model: &UiModel,
    ) -> Result<Option<UiConversationViewportObservation>, TuiTerminalError> {
        self.record("draw");
        assert!(!self.draw_panics, "scripted draw panic");
        thread::sleep(self.draw_delay);
        if let Some(positions) = &self.draw_positions {
            positions
                .lock()
                .expect("draw positions")
                .push(model.conversation_viewport_position().cloned());
        }
        Ok(self.draw_observations.pop_front().flatten())
    }

    fn restore(&mut self) -> Result<(), TuiTerminalError> {
        self.record("restore");
        Ok(())
    }
}

struct EmptyClient;

struct CountingClient {
    snapshot_loads: Arc<AtomicUsize>,
}

impl TuiClientPort for CountingClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        self.snapshot_loads.fetch_add(1, Ordering::SeqCst);
        EmptyClient.load_snapshot()
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        EmptyClient.load_conversation(row_id, cursor)
    }

    fn open_draft(
        &mut self,
        target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        EmptyClient.open_draft(target)
    }

    fn save_draft(&mut self, draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        EmptyClient.save_draft(draft)
    }

    fn submit_mailbox_command(
        &mut self,
        draft: Option<UiMailboxDraft>,
        action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        EmptyClient.submit_mailbox_command(draft, action)
    }
}

impl TuiClientPort for EmptyClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, hq_tui::UiFailure> {
        Ok(UiSnapshot {
            revision: 1,
            human_state: UiHumanState::Ready,
            inbox_rows: Vec::new(),
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: Vec::new(),
            project_rows: Vec::new(),
            direct_targets: Vec::new(),
            providers: Vec::new(),
            agents: Vec::new(),
            projects: Vec::new(),
        })
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<hq_tui::UiConversationPage, hq_tui::UiFailure> {
        Ok(hq_tui::UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
            row_id: row_id.to_owned(),
            entries: Vec::new(),
            next_cursor: None,
        })
    }

    fn open_draft(
        &mut self,
        _target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(test_draft_error())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(test_draft_error())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(test_failure())
    }
}

struct PanickingClient;

impl TuiClientPort for PanickingClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, hq_tui::UiFailure> {
        panic!("scripted client failure")
    }

    fn load_conversation(
        &mut self,
        _row_id: &str,
        _cursor: Option<String>,
    ) -> Result<hq_tui::UiConversationPage, hq_tui::UiFailure> {
        panic!("scripted client failure")
    }

    fn open_draft(
        &mut self,
        _target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        panic!("scripted client failure")
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        panic!("scripted client failure")
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        panic!("scripted client failure")
    }
}

struct IdleObserver {
    wake: mpsc::Receiver<()>,
    interrupt: IdleInterrupt,
}

struct InteractionObserver {
    wake: mpsc::Receiver<()>,
    interrupt: IdleInterrupt,
    published: bool,
}

impl TuiObservationPort for InteractionObserver {
    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        let _ = self.wake.recv();
        if self.published {
            return Vec::new();
        }
        self.published = true;
        vec![TuiClientObservation::Interactions(vec![UiInteraction {
            agent_id: [1; 32],
            agent_name: "alice".to_owned(),
            project_id: None,
            project_name: None,
            provider: "codex".to_owned(),
            session: "session".to_owned(),
            request_id: [2; 32],
            operation_id: [3; 32],
            kind: UiInteractionKind::CommandApproval,
            prompt: "List the directory?".to_owned(),
            choices: Vec::new(),
            allow_text: false,
        }])]
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.interrupt.clone())
    }
}

struct InitialObserver {
    idle: IdleObserver,
    initial: Option<UiMaterializedConversationView>,
}

impl TuiObservationPort for InitialObserver {
    fn take_initial_view(&mut self) -> Option<UiMaterializedConversationView> {
        self.initial.take()
    }

    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        self.idle.next_observations()
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        self.idle.interrupt_handle()
    }
}

#[derive(Clone)]
struct IdleInterrupt(mpsc::Sender<()>);

impl TuiObservationInterrupt for IdleInterrupt {
    fn interrupt(&self) {
        let _ = self.0.send(());
    }
}

impl TuiObservationPort for IdleObserver {
    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        let _ = self.wake.recv();
        Vec::new()
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.interrupt.clone())
    }
}

fn idle_observer() -> IdleObserver {
    let (interrupt, wake) = mpsc::channel();
    IdleObserver {
        wake,
        interrupt: IdleInterrupt(interrupt),
    }
}

fn test_failure() -> UiFailure {
    UiFailure {
        code: "unsupported_test_effect".to_owned(),
        action: "script the mailbox effect".to_owned(),
    }
}

fn test_draft_error() -> TuiDraftError {
    TuiDraftError {
        failure: test_failure(),
        current: None,
    }
}

#[derive(Clone, Copy)]
struct FixedClock;

impl TuiClock for FixedClock {
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}
