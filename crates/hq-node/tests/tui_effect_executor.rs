//! Scripted TUI client and effect-executor contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use hq_local_api::protocol::v1::{
    ActivityStatusDto, AuthoritativeSnapshotDto, ConversationEntryDto, ConversationKeyDto,
    ConversationMessageDto, ConversationPageDto, Id32, MailboxAddressDto, MessagePurposeDto,
    PresentationKindDto, SnapshotItem,
};
use hq_node::{
    TuiClientObservation, TuiClientPort, TuiClock, TuiDraftError, TuiEffectExecutor,
    TuiExecutorError, tui_conversation_page, tui_snapshot,
};
use hq_tui::{
    UiConnectionState, UiConversationEntryKind, UiConversationPage, UiEffect, UiEvent, UiFailure,
    UiInput, UiMailboxAction, UiMailboxDraft, UiMailboxDraftTarget, UiMessageState, UiModel, UiRow,
    UiRowKind, UiRowState, UiSection, UiSize, UiSnapshot, UiTechnicalSection, UiTimerKind, update,
};

type ConversationRequests = Arc<Mutex<Vec<(String, Option<String>)>>>;

#[test]
fn executor_loads_the_effects_exact_section_and_preserves_identity() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(Mutex::new(false));
    let client = ScriptedTuiClient {
        requests: Arc::clone(&requests),
        conversation_requests: Arc::new(Mutex::new(Vec::new())),
        snapshots: VecDeque::from([Ok(UiSnapshot {
            section: UiSection::Inbox,
            revision: 7,
            rows: Vec::new(),
            direct_targets: Vec::new(),
        })]),
        observations: VecDeque::new(),
        stopped: Arc::clone(&stopped),
    };
    let clock = ManualClock::default();
    let mut executor = TuiEffectExecutor::spawn(client, clock.clone()).expect("spawn executor");
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start model");
    let expected_id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("load effect");

    executor.execute(started.effects).expect("execute effects");
    let event = receive_event(&mut executor);
    assert!(matches!(
        event,
        UiEvent::SnapshotLoaded { effect_id, snapshot }
            if effect_id == expected_id
                && snapshot.section == UiSection::Inbox
                && snapshot.revision == 7
    ));
    assert_eq!(
        requests.lock().expect("requests lock").as_slice(),
        &[UiSection::Inbox]
    );
    assert!(executor.take_redraw_request());
    assert!(!executor.take_redraw_request());

    executor.shutdown().expect("joined shutdown");
    assert!(*stopped.lock().expect("stopped lock"));
}

#[test]
fn executor_coalesces_redraw_and_releases_each_timer_once() {
    let clock = ManualClock::default();
    let client = ScriptedTuiClient::empty();
    let mut executor = TuiEffectExecutor::spawn(client, clock.clone()).expect("spawn executor");
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start model");
    let timer_id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::ScheduleTimer {
                id,
                kind: UiTimerKind::PeriodicRefresh,
                ..
            } => Some(*id),
            _ => None,
        })
        .expect("timer effect");
    executor
        .execute(
            started
                .effects
                .into_iter()
                .filter(|effect| !matches!(effect, UiEffect::LoadSnapshot { .. })),
        )
        .expect("execute non-client effects");
    executor
        .execute([UiEffect::RequestRedraw, UiEffect::RequestRedraw])
        .expect("coalesce redraw");
    assert!(executor.take_redraw_request());
    assert!(!executor.take_redraw_request());
    assert!(executor.poll_event().is_none());

    clock.advance(Duration::from_secs(300));
    assert_eq!(
        executor.poll_event(),
        Some(UiEvent::TimerElapsed {
            effect_id: timer_id
        })
    );
    assert!(executor.poll_event().is_none());
    executor.shutdown().expect("joined shutdown");
}

#[test]
fn executor_loads_the_exact_conversation_row_and_preserves_effect_identity() {
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start");
    let snapshot_id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("snapshot id");
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: UiSnapshot {
                section: UiSection::Inbox,
                revision: 1,
                rows: vec![UiRow {
                    id: "thread-a".to_owned(),
                    title: "Thread A".to_owned(),
                    detail: "1 open message".to_owned(),
                    state: UiRowState::Open,
                    kind: UiRowKind::Conversation,
                }],
                direct_targets: Vec::new(),
            },
        },
    )
    .expect("snapshot applies");
    let opening = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("activate");
    let (expected_id, effect) = opening
        .effects
        .into_iter()
        .find_map(|effect| match effect {
            UiEffect::LoadConversation { id, .. } => Some((id, effect)),
            _ => None,
        })
        .expect("conversation effect");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = ScriptedTuiClient {
        requests: Arc::new(Mutex::new(Vec::new())),
        conversation_requests: Arc::clone(&requests),
        snapshots: VecDeque::new(),
        observations: VecDeque::new(),
        stopped: Arc::new(Mutex::new(false)),
    };
    let mut executor =
        TuiEffectExecutor::spawn(client, ManualClock::default()).expect("executor starts");
    executor.execute([effect]).expect("execute page effect");
    assert!(matches!(
        receive_event(&mut executor),
        UiEvent::ConversationLoaded { effect_id, page }
            if effect_id == expected_id && page.row_id == "thread-a"
    ));
    assert_eq!(
        requests.lock().expect("requests lock").as_slice(),
        &[("thread-a".to_owned(), None)]
    );
    executor.shutdown().expect("shutdown");
}

#[test]
fn executor_forwards_subscription_and_connection_observations() {
    let client = ScriptedTuiClient {
        requests: Arc::new(Mutex::new(Vec::new())),
        conversation_requests: Arc::new(Mutex::new(Vec::new())),
        snapshots: VecDeque::new(),
        observations: VecDeque::from([
            TuiClientObservation::Connection {
                generation: 3,
                state: UiConnectionState::Reconnecting,
            },
            TuiClientObservation::Invalidated { revision: 12 },
            TuiClientObservation::Failure {
                generation: 3,
                failure: UiFailure {
                    code: "local_client_unavailable".to_owned(),
                    action: "waiting to reconnect".to_owned(),
                },
            },
        ]),
        stopped: Arc::new(Mutex::new(false)),
    };
    let mut executor =
        TuiEffectExecutor::spawn(client, ManualClock::default()).expect("spawn executor");

    assert!(matches!(
        receive_event(&mut executor),
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Reconnecting
        }
    ));
    assert_eq!(
        receive_event(&mut executor),
        UiEvent::Invalidated { revision: 12 }
    );
    assert!(matches!(
        receive_event(&mut executor),
        UiEvent::ClientFailed { generation: 3, failure }
            if failure.code == "local_client_unavailable"
    ));
    executor.shutdown().expect("joined shutdown");
}

#[test]
fn executor_runs_draft_autosave_and_stable_mailbox_command_in_order() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client = MailboxTuiClient {
        calls: Arc::clone(&calls),
    };
    let clock = ManualClock::default();
    let mut executor =
        TuiEffectExecutor::spawn(client, clock.clone()).expect("spawn mailbox executor");
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start");
    let mut model = started.model;
    executor.execute(started.effects).expect("load snapshot");
    model = update(model, receive_event(&mut executor))
        .expect("snapshot event")
        .model;

    let opening = update(model, UiEvent::Input(UiInput::Character('n'))).expect("self note");
    model = opening.model;
    executor.execute(opening.effects).expect("open draft");
    model = update(model, receive_event(&mut executor))
        .expect("draft loaded")
        .model;

    let typing = update(
        model,
        UiEvent::Input(UiInput::Paste("durable note".to_owned())),
    )
    .expect("type draft");
    model = typing.model;
    executor.execute(typing.effects).expect("schedule autosave");
    clock.advance(Duration::from_millis(250));
    let elapsed = executor.poll_event().expect("autosave timer");
    let saving = update(model, elapsed).expect("emit save");
    model = saving.model;
    executor.execute(saving.effects).expect("save draft");
    model = update(model, receive_event(&mut executor))
        .expect("draft saved")
        .model;

    let submit = update(model, UiEvent::Input(UiInput::Activate)).expect("submit note");
    model = submit.model;
    executor.execute(submit.effects).expect("submit command");
    let completed = receive_event(&mut executor);
    assert!(matches!(
        completed,
        UiEvent::MailboxCommandCommitted { revision: 9, .. }
    ));
    let final_state = update(model, completed).expect("apply command receipt");
    assert!(final_state.model.mailbox_modal().is_none());
    assert_eq!(
        calls.lock().expect("calls lock").as_slice(),
        &[
            "snapshot",
            "open:self_note",
            "save:durable note",
            "submit:self_note"
        ]
    );
    executor.shutdown().expect("shutdown");
}

#[test]
fn authoritative_snapshot_mapping_is_section_bound_and_deterministic() {
    let source = AuthoritativeSnapshotDto::new(
        21,
        vec![
            SnapshotItem::Conversation {
                key: ConversationKeyDto::Thread {
                    counterparty_installation: Id32::new([1; 32]),
                    counterparty_mailbox: Id32::new([2; 32]),
                    thread: Id32::new([3; 32]),
                },
                latest_fact: Some(Id32::new([4; 32])),
                open_messages: 2,
                archived_messages: 3,
                sent_messages: 1,
            },
            SnapshotItem::Agent {
                agent_id: Id32::new([5; 32]),
                claims: vec![Id32::new([6; 32])],
                names: vec!["builder".to_owned()],
                mailboxes: vec![MailboxAddressDto {
                    installation_id: Id32::new([15; 32]),
                    mailbox_id: Id32::new([16; 32]),
                }],
                retirements: Vec::new(),
                lifecycle: "ready".to_owned(),
                runnable: true,
            },
            SnapshotItem::Project {
                project_id: Id32::new([7; 32]),
                home: Id32::new([8; 32]),
                account_id: Id32::new([9; 32]),
                mailbox_id: Id32::new([10; 32]),
                name: "release".to_owned(),
                lifecycle: "open".to_owned(),
                archived: false,
                claimable: false,
                head: Id32::new([11; 32]),
                input_sequence: 0,
            },
            SnapshotItem::IncompleteMessagesTruncated,
        ],
    )
    .expect("authoritative snapshot");

    let inbox = tui_snapshot(UiSection::Inbox, source.clone());
    assert_eq!(inbox.revision, 21);
    assert_eq!(inbox.section, UiSection::Inbox);
    assert_eq!(inbox.rows.len(), 2);
    assert_eq!(inbox.rows[0].title, "Thread 030303030303");
    assert_eq!(inbox.rows[0].detail, "2 open messages");
    assert_eq!(inbox.rows[1].state, hq_tui::UiRowState::Attention);
    assert_eq!(inbox.direct_targets.len(), 1);
    assert_eq!(inbox.direct_targets[0].label, "builder");
    assert_eq!(inbox.direct_targets[0].installation_id, [15; 32]);
    assert_eq!(inbox.direct_targets[0].mailbox_id, [16; 32]);

    let agents = tui_snapshot(UiSection::Agents, source.clone());
    assert_eq!(agents.rows.len(), 1);
    assert_eq!(agents.rows[0].title, "builder");
    assert_eq!(agents.rows[0].state, hq_tui::UiRowState::Open);

    let projects = tui_snapshot(UiSection::Projects, source.clone());
    assert_eq!(projects.rows.len(), 1);
    assert_eq!(projects.rows[0].title, "release");
    assert_eq!(projects.rows[0].state, hq_tui::UiRowState::Attention);
    assert_eq!(tui_snapshot(UiSection::Sent, source.clone()).rows.len(), 1);
    let archived = tui_snapshot(UiSection::Archived, source);
    assert_eq!(archived.rows.len(), 1);
    assert_eq!(archived.rows[0].detail, "3 archived messages");
}

#[test]
fn authoritative_snapshot_mapping_never_forwards_terminal_controls() {
    let source = AuthoritativeSnapshotDto::new(
        1,
        vec![SnapshotItem::Agent {
            agent_id: Id32::new([1; 32]),
            claims: Vec::new(),
            names: vec!["builder\u{1b}[31m".to_owned()],
            mailboxes: Vec::new(),
            retirements: Vec::new(),
            lifecycle: "ready\nspoof".to_owned(),
            runnable: true,
        }],
    )
    .expect("authoritative snapshot");

    let snapshot = tui_snapshot(UiSection::Agents, source);
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].title, "builder [31m");
    assert_eq!(snapshot.rows[0].detail, "ready spoof");
    assert!(
        snapshot.rows[0]
            .title
            .chars()
            .chain(snapshot.rows[0].detail.chars())
            .all(|character| !character.is_control())
    );
}

#[test]
fn conversation_page_mapping_preserves_reducer_order_and_typed_disclosure() {
    let page = ConversationPageDto::new(
        vec![
            ConversationEntryDto::Message(Box::new(ConversationMessageDto {
                fact_id: Id32::new([1; 32]),
                message_id: Id32::new([2; 32]),
                thread_id: Id32::new([3; 32]),
                content: "hello\nworld".to_owned(),
                sender_installation: Id32::new([4; 32]),
                sender_mailbox: Id32::new([5; 32]),
                recipient_installation: Some(Id32::new([6; 32])),
                recipient_mailbox: Some(Id32::new([7; 32])),
                purpose: MessagePurposeDto::Question,
                presentation: PresentationKindDto::Message,
                correlation_provider: Some("codex".to_owned()),
                correlation_session: Some("thread-1".to_owned()),
                correlation_operation: Some(Id32::new([8; 32])),
                project_id: None,
                open: false,
                rejected: false,
                state_frontier: vec![Id32::new([9; 32])],
                peer_received_by: vec![Id32::new([10; 32])],
                root_fact: Some(Id32::new([11; 32])),
                root_message: Some(Id32::new([12; 32])),
                ready_answer: true,
                thread_cancelled: false,
            })),
            ConversationEntryDto::Activity {
                fact_id: Id32::new([13; 32]),
                sequence: 4,
                status: ActivityStatusDto::Running,
                content: "building".to_owned(),
                truncated: false,
            },
        ],
        Some("opaque-next".to_owned()),
    )
    .expect("valid page");

    let mapped = tui_conversation_page("thread-row", page);
    assert_eq!(mapped.row_id, "thread-row");
    assert_eq!(mapped.next_cursor.as_deref(), Some("opaque-next"));
    assert_eq!(mapped.entries.len(), 2);
    assert_eq!(mapped.entries[0].kind, UiConversationEntryKind::Message);
    assert_eq!(
        mapped.entries[0].message_state,
        Some(UiMessageState::Archived)
    );
    assert_eq!(mapped.entries[0].content, "hello world");
    assert!(matches!(
        mapped.entries[0].message_target,
        Some(hq_tui::UiMessageTarget { message_id, reply_allowed: true })
            if message_id == [2; 32]
    ));
    assert!(matches!(
        mapped.entries[0].technical.as_slice(),
        [
            UiTechnicalSection::Routing { .. },
            UiTechnicalSection::Semantics { purpose, .. },
            UiTechnicalSection::Evidence { ready_answer: true, .. }
        ] if purpose == "question"
    ));
    assert_eq!(mapped.entries[1].kind, UiConversationEntryKind::Activity);
    assert_eq!(mapped.entries[1].message_state, None);
    assert_eq!(mapped.entries[1].message_target, None);
    assert!(matches!(
        mapped.entries[1].technical.as_slice(),
        [UiTechnicalSection::Activity { sequence: 4, .. }]
    ));
}

#[test]
fn worker_panics_are_joined_and_reported() {
    let mut executor =
        TuiEffectExecutor::spawn(PanickingClient, ManualClock::default()).expect("spawn executor");
    thread::sleep(Duration::from_millis(50));
    assert_eq!(executor.shutdown(), Err(TuiExecutorError::WorkerPanicked));
}

#[test]
fn shutdown_drains_saturated_worker_results_before_joining() {
    let effects = snapshot_load_effects(25);
    let mut executor = TuiEffectExecutor::spawn(ImmediateSnapshotClient, ManualClock::default())
        .expect("spawn executor");

    for effect in &effects[..8] {
        executor
            .execute([effect.clone()])
            .expect("first command batch");
    }
    thread::sleep(Duration::from_millis(50));
    for effect in &effects[8..16] {
        executor
            .execute([effect.clone()])
            .expect("second command batch");
    }
    thread::sleep(Duration::from_millis(50));
    for effect in &effects[16..] {
        if let Err(error) = executor.execute([effect.clone()]) {
            assert_eq!(error, TuiExecutorError::WorkerUnavailable);
            break;
        }
    }

    executor.shutdown().expect("saturated worker joins");
}

#[derive(Clone, Default)]
struct ManualClock {
    now: Arc<Mutex<Duration>>,
}

impl ManualClock {
    fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().expect("clock lock");
        *now = now.saturating_add(duration);
    }
}

impl TuiClock for ManualClock {
    fn now(&self) -> Duration {
        *self.now.lock().expect("clock lock")
    }
}

struct ScriptedTuiClient {
    requests: Arc<Mutex<Vec<UiSection>>>,
    conversation_requests: ConversationRequests,
    snapshots: VecDeque<Result<UiSnapshot, UiFailure>>,
    observations: VecDeque<TuiClientObservation>,
    stopped: Arc<Mutex<bool>>,
}

impl ScriptedTuiClient {
    fn empty() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            conversation_requests: Arc::new(Mutex::new(Vec::new())),
            snapshots: VecDeque::new(),
            observations: VecDeque::new(),
            stopped: Arc::new(Mutex::new(false)),
        }
    }
}

impl TuiClientPort for ScriptedTuiClient {
    fn load_snapshot(&mut self, section: UiSection) -> Result<UiSnapshot, UiFailure> {
        self.requests.lock().expect("requests lock").push(section);
        self.snapshots.pop_front().unwrap_or_else(|| {
            Err(UiFailure {
                code: "script_exhausted".to_owned(),
                action: "add a scripted snapshot".to_owned(),
            })
        })
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        self.conversation_requests
            .lock()
            .expect("conversation requests lock")
            .push((row_id.to_owned(), cursor));
        Ok(UiConversationPage {
            row_id: row_id.to_owned(),
            entries: Vec::new(),
            next_cursor: None,
        })
    }

    fn open_draft(
        &mut self,
        _target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<u64, UiFailure> {
        Err(unsupported_failure())
    }

    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation> {
        if let Some(observation) = self.observations.pop_front() {
            vec![observation]
        } else {
            thread::sleep(wait);
            Vec::new()
        }
    }
}

impl Drop for ScriptedTuiClient {
    fn drop(&mut self) {
        *self.stopped.lock().expect("stopped lock") = true;
    }
}

struct PanickingClient;

impl TuiClientPort for PanickingClient {
    fn load_snapshot(&mut self, _section: UiSection) -> Result<UiSnapshot, UiFailure> {
        panic!("scripted worker failure");
    }

    fn load_conversation(
        &mut self,
        _row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        panic!("scripted worker failure");
    }

    fn open_draft(
        &mut self,
        _target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        panic!("scripted worker failure");
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        panic!("scripted worker failure");
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<u64, UiFailure> {
        panic!("scripted worker failure");
    }

    fn poll(&mut self, _wait: Duration) -> Vec<TuiClientObservation> {
        panic!("scripted worker failure");
    }
}

struct ImmediateSnapshotClient;

impl TuiClientPort for ImmediateSnapshotClient {
    fn load_snapshot(&mut self, section: UiSection) -> Result<UiSnapshot, UiFailure> {
        Ok(UiSnapshot {
            section,
            revision: 1,
            rows: Vec::new(),
            direct_targets: Vec::new(),
        })
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
            row_id: row_id.to_owned(),
            entries: Vec::new(),
            next_cursor: None,
        })
    }

    fn open_draft(
        &mut self,
        _target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<u64, UiFailure> {
        Err(unsupported_failure())
    }

    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation> {
        thread::sleep(wait);
        Vec::new()
    }
}

struct MailboxTuiClient {
    calls: Arc<Mutex<Vec<String>>>,
}

impl TuiClientPort for MailboxTuiClient {
    fn load_snapshot(&mut self, section: UiSection) -> Result<UiSnapshot, UiFailure> {
        self.calls
            .lock()
            .expect("calls lock")
            .push("snapshot".to_owned());
        Ok(UiSnapshot {
            section,
            revision: 1,
            rows: Vec::new(),
            direct_targets: Vec::new(),
        })
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
            row_id: row_id.to_owned(),
            entries: Vec::new(),
            next_cursor: None,
        })
    }

    fn open_draft(
        &mut self,
        target: UiMailboxDraftTarget,
    ) -> Result<UiMailboxDraft, TuiDraftError> {
        assert_eq!(target, UiMailboxDraftTarget::SelfNote);
        self.calls
            .lock()
            .expect("calls lock")
            .push("open:self_note".to_owned());
        Ok(UiMailboxDraft {
            draft_id: [7; 32],
            target,
            content: String::new(),
            version: 1,
        })
    }

    fn save_draft(&mut self, draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(format!("save:{}", draft.content));
        Ok(UiMailboxDraft {
            version: draft.version + 1,
            ..draft
        })
    }

    fn submit_mailbox_command(
        &mut self,
        draft: Option<UiMailboxDraft>,
        action: UiMailboxAction,
    ) -> Result<u64, UiFailure> {
        assert!(matches!(action, UiMailboxAction::SelfNote));
        assert_eq!(
            draft.as_ref().map(|draft| draft.content.as_str()),
            Some("durable note")
        );
        self.calls
            .lock()
            .expect("calls lock")
            .push("submit:self_note".to_owned());
        Ok(9)
    }

    fn poll(&mut self, wait: Duration) -> Vec<TuiClientObservation> {
        thread::sleep(wait);
        Vec::new()
    }
}

fn snapshot_load_effects(count: usize) -> Vec<UiEffect> {
    let mut transition = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start model");
    let mut effects = Vec::with_capacity(count);
    for revision in 1..=count {
        let effect = transition
            .effects
            .iter()
            .find(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
            .cloned()
            .expect("snapshot effect");
        let UiEffect::LoadSnapshot { id, section } = effect.clone() else {
            unreachable!("matched snapshot effect")
        };
        effects.push(effect);
        let loaded = update(
            transition.model,
            UiEvent::SnapshotLoaded {
                effect_id: id,
                snapshot: UiSnapshot {
                    section,
                    revision: revision as u64,
                    rows: Vec::new(),
                    direct_targets: Vec::new(),
                },
            },
        )
        .expect("complete generated snapshot effect");
        transition = update(
            loaded.model,
            UiEvent::Invalidated {
                revision: revision as u64 + 1,
            },
        )
        .expect("generate next snapshot effect");
    }
    effects
}

fn unsupported_failure() -> UiFailure {
    UiFailure {
        code: "unsupported_test_effect".to_owned(),
        action: "add a scripted mailbox result".to_owned(),
    }
}

fn unsupported_draft() -> TuiDraftError {
    TuiDraftError {
        failure: unsupported_failure(),
        current: None,
    }
}

fn receive_event<C: TuiClock>(executor: &mut TuiEffectExecutor<C>) -> UiEvent {
    for _ in 0..200 {
        if let Some(event) = executor.poll_event() {
            return event;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("executor event did not arrive");
}
