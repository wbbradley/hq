//! Scripted TUI client and effect-executor contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use hq_local_api::protocol::v1::{
    ActivityStatusDto, AuthoritativeSnapshotDto, CompletedFileChangeDto,
    CompletedItemPresentationDto, ConversationActivityDto, ConversationActivityKindDto,
    ConversationContextDto, ConversationEntryDto, ConversationKeyDto, ConversationMessageDto,
    ConversationPageDto, ConversationParticipantDto, DeviceGrantDto, Id32, MailboxAddressDto,
    MessagePurposeDto, PresentationKindDto, ProviderAvailabilityDto, ProviderCatalogDto,
    ResourceLocatorDto, ResourceSchemeDto, SnapshotItem,
};
use hq_node::{
    TuiClientObservation, TuiClientPort, TuiClock, TuiDraftError, TuiEffectExecutor,
    TuiExecutorError, TuiObservationControl, TuiObservationInterrupt, TuiObservationPort,
    tui_conversation_page, tui_snapshot, tui_snapshot_with_provider_catalog,
};
use hq_tui::{
    UiAgentAction, UiCompletedItemPresentation, UiConnectionState, UiConversationActivityKind,
    UiConversationAuthor, UiConversationEntryPresentation, UiConversationPage, UiEffect, UiEvent,
    UiFailure, UiHumanIssue, UiHumanMembershipEvidence, UiHumanMembershipStatus,
    UiHumanSelectionEvidence, UiHumanState, UiInput, UiMailboxAction, UiMailboxCommandResult,
    UiMailboxDraft, UiMailboxDraftTarget, UiManagedSessionAction, UiManagedSessionOutcome,
    UiManagedSessionResult, UiMaterializedConversationView, UiMessageDelivery, UiMessageState,
    UiModel, UiProjectAction, UiProjectExternalWarning, UiProjectOutcome, UiProjectResourceCheck,
    UiProjectResult, UiRow, UiRowKind, UiRowState, UiSize, UiSnapshot, UiTechnicalSection,
    UiTimerKind, update,
};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

type ConversationRequests = Arc<Mutex<Vec<(String, Option<String>)>>>;

#[test]
fn completed_command_mapping_is_typed_multiline_and_terminal_safe() {
    let page = ConversationPageDto::new(
        vec![ConversationEntryDto::Activity(Box::new(
            ConversationActivityDto {
                fact_id: Id32::new([1; 32]),
                activity_kind: ConversationActivityKindDto::CompletedItem,
                sequence: 2,
                source_installation: Id32::new([3; 32]),
                source_mailbox: Id32::new([4; 32]),
                provider: "provider".to_owned(),
                session: "session".to_owned(),
                operation: Id32::new([2; 32]),
                item: Some("command".to_owned()),
                logical_key: "command".to_owned(),
                runtime: "codex".to_owned(),
                occurred_at_unix_ms: 42,
                status: ActivityStatusDto::Failed {
                    reason: "command_failed".to_owned(),
                },
                content: "detail\x1b[31mred\x1b[0m".to_owned(),
                truncated: false,
                completed: Some(CompletedItemPresentationDto::Command {
                    command: "printf one\nprintf two\x1b[2J".to_owned(),
                    output: Some(
                        "one\n\x1b]8;;https://bad\x07two\x1b]8;;\x07\nthree\nfour".to_owned(),
                    ),
                    exit_code: Some(17),
                    command_truncated: false,
                    output_truncated: true,
                }),
            },
        ))],
        None,
    )
    .expect("page validates");
    let mapped = tui_conversation_page(
        "row",
        &ConversationContextDto::Personal,
        &MailboxAddressDto {
            installation_id: Id32::new([3; 32]),
            mailbox_id: Id32::new([4; 32]),
        },
        page,
    );
    assert!(matches!(
        &mapped.entries[0].presentation,
        UiConversationEntryPresentation::Activity {
            summary,
            detail,
            completed: Some(UiCompletedItemPresentation::Command {
                command,
                output: Some(output),
                exit_code: Some(17),
                output_truncated: true,
                ..
            }),
            ..
        } if summary == "Command failed"
            && detail == "detailred"
            && command == "printf one\nprintf two"
            && output == "one\ntwo\nthree\nfour"
    ));
}

#[test]
fn completed_file_tool_and_search_mapping_uses_closed_typed_summaries() {
    let activity = |seed, logical_key: &str, completed| {
        ConversationEntryDto::Activity(Box::new(ConversationActivityDto {
            fact_id: Id32::new([seed; 32]),
            activity_kind: ConversationActivityKindDto::CompletedItem,
            sequence: u64::from(seed),
            source_installation: Id32::new([4; 32]),
            source_mailbox: Id32::new([5; 32]),
            provider: "provider".to_owned(),
            session: "session".to_owned(),
            operation: Id32::new([6; 32]),
            item: Some(logical_key.to_owned()),
            logical_key: logical_key.to_owned(),
            runtime: "codex".to_owned(),
            occurred_at_unix_ms: i64::from(seed),
            status: ActivityStatusDto::Succeeded,
            content: "bounded technical detail".to_owned(),
            truncated: false,
            completed: Some(completed),
        }))
    };
    let page = ConversationPageDto::new(
        vec![
            activity(
                1,
                "file-change",
                CompletedItemPresentationDto::FileChange {
                    changes: vec![CompletedFileChangeDto {
                        path: "src/main.rs".to_owned(),
                        diff: Some("+safe".to_owned()),
                        path_truncated: false,
                        diff_truncated: false,
                    }],
                    changes_truncated: false,
                },
            ),
            activity(
                2,
                "tool",
                CompletedItemPresentationDto::Tool {
                    name: "server/tool\u{1b}[2J".to_owned(),
                    name_truncated: false,
                },
            ),
            activity(
                3,
                "search",
                CompletedItemPresentationDto::WebSearch {
                    query: "typed\nquery".to_owned(),
                    query_truncated: false,
                },
            ),
        ],
        None,
    )
    .expect("typed completed families validate");

    let mapped = tui_conversation_page(
        "row",
        &ConversationContextDto::Personal,
        &MailboxAddressDto {
            installation_id: Id32::new([4; 32]),
            mailbox_id: Id32::new([5; 32]),
        },
        page,
    );
    let summaries = mapped
        .entries
        .iter()
        .map(|entry| match &entry.presentation {
            UiConversationEntryPresentation::Activity { summary, .. } => summary.as_str(),
            UiConversationEntryPresentation::Message { .. } => panic!("expected activity"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summaries,
        [
            "Changed 1 file",
            "Tool: server/tool",
            "Web search: typed query"
        ]
    );
    assert!(
        summaries
            .iter()
            .all(|summary| *summary != "Completed an item")
    );
}

fn human_selection(
    installation: [u8; 32],
    account: [u8; 32],
    frontier: Vec<[u8; 32]>,
) -> SnapshotItem {
    SnapshotItem::AccountSelection {
        installation_id: Id32::new(installation),
        candidates: vec![Id32::new(account)],
        active: Some(Id32::new(account)),
        frontier: frontier.into_iter().map(Id32::new).collect(),
    }
}

fn human_membership(
    installation: [u8; 32],
    account: [u8; 32],
    state: &str,
    frontier: Vec<[u8; 32]>,
    active_acceptances: Vec<[u8; 32]>,
) -> SnapshotItem {
    let frontier_fact = frontier.first().copied();
    let grants = matches!(state, "pending" | "active")
        .then(|| DeviceGrantDto {
            grant_id: Id32::new([20; 32]),
            grant_fact: Id32::new(frontier_fact.unwrap_or([21; 32])),
            device: Id32::new(installation),
            signing_key: Id32::new([22; 32]),
            label: None,
            relay_hints: Vec::new(),
            frontier_member: true,
            active: state == "active",
        })
        .into_iter()
        .collect();
    SnapshotItem::Membership {
        account_id: Id32::new(account),
        device: Id32::new(installation),
        state: state.to_owned(),
        frontier: frontier.into_iter().map(Id32::new).collect(),
        grants,
        acceptances: active_acceptances.iter().copied().map(Id32::new).collect(),
        revokes: (state == "revoked")
            .then_some(frontier_fact.map(Id32::new))
            .flatten()
            .into_iter()
            .collect(),
        active_acceptances: active_acceptances.into_iter().map(Id32::new).collect(),
    }
}

#[test]
fn executor_loads_the_complete_snapshot_and_preserves_identity() {
    let requests = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(Mutex::new(false));
    let client = ScriptedTuiClient {
        requests: Arc::clone(&requests),
        conversation_requests: Arc::new(Mutex::new(Vec::new())),
        snapshots: VecDeque::from([Ok(empty_snapshot(7))]),
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
                && snapshot.revision == 7
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
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
            UiEffect::LoadSnapshot { id } => Some(*id),
            _ => None,
        })
        .expect("allocated effect identity");
    executor
        .execute([UiEffect::ScheduleTimer {
            id: timer_id,
            kind: UiTimerKind::DismissCompletion,
            after: Duration::from_secs(300),
        }])
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
    let loaded = update(
        started.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot: UiSnapshot {
                    inbox_rows: vec![UiRow {
                        id: "thread-a".to_owned(),
                        title: "Thread A".to_owned(),
                        detail: "1 open message".to_owned(),
                        state: UiRowState::Open,
                        kind: UiRowKind::Conversation,
                        conversation_target: None,
                    }],
                    ..empty_snapshot(1)
                },
                conversation: Some(UiConversationPage {
                    row_id: "thread-a".to_owned(),
                    title: "Thread A".to_owned(),
                    context: None,
                    entries: Vec::new(),
                    next_cursor: Some("older".to_owned()),
                }),
            },
        },
    )
    .expect("snapshot applies");
    let loading =
        update(loaded.model, UiEvent::Input(UiInput::LoadMore)).expect("request older page");
    let (expected_id, effect) = loading
        .effects
        .into_iter()
        .find_map(|effect| match effect {
            UiEffect::LoadConversation { id, .. } => Some((id, effect)),
            _ => None,
        })
        .expect("conversation effect");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = ScriptedTuiClient {
        requests: Arc::new(AtomicUsize::new(0)),
        conversation_requests: Arc::clone(&requests),
        snapshots: VecDeque::new(),
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
        &[("thread-a".to_owned(), Some("older".to_owned()))]
    );
    executor.shutdown().expect("shutdown");
}

#[test]
fn executor_submits_the_exact_typed_agent_command_and_preserves_effect_identity() {
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start");
    let id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("effect identity");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client = AgentTuiClient {
        calls: Arc::clone(&calls),
    };
    let mut executor =
        TuiEffectExecutor::spawn(client, ManualClock::default()).expect("executor starts");
    let action = UiAgentAction::Retire {
        agent_id: [44; 32],
        force: true,
    };
    executor
        .execute([UiEffect::SubmitAgentCommand {
            id,
            action: action.clone(),
        }])
        .expect("execute command");
    assert_eq!(
        receive_event(&mut executor),
        UiEvent::AgentCommandCommitted {
            effect_id: id,
            revision: 23,
        }
    );
    assert_eq!(calls.lock().expect("calls lock").as_slice(), &[action]);
    executor.shutdown().expect("shutdown");
}

#[test]
fn executor_submits_exact_managed_session_target_and_preserves_operation_evidence() {
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start");
    let id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("effect identity");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client = ManagedSessionTuiClient {
        calls: Arc::clone(&calls),
    };
    let mut executor =
        TuiEffectExecutor::spawn(client, ManualClock::default()).expect("executor starts");
    let action = UiManagedSessionAction::Resume {
        agent_id: [45; 32],
        provider: "codex".to_owned(),
        session: "exact-session".to_owned(),
    };
    executor
        .execute([UiEffect::SubmitManagedSession {
            id,
            action: action.clone(),
        }])
        .expect("execute managed-session command");
    assert_eq!(
        receive_event(&mut executor),
        UiEvent::ManagedSessionCompleted {
            effect_id: id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [91; 32],
                outcome: UiManagedSessionOutcome::Uncertain {
                    reconciliation_id: [92; 32],
                },
            },
        }
    );
    assert_eq!(calls.lock().expect("calls lock").as_slice(), &[action]);
    executor.shutdown().expect("shutdown");
}

#[test]
fn executor_submits_exact_project_command_and_preserves_reconciliation_evidence() {
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start");
    let id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("effect identity");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client = ProjectTuiClient {
        calls: Arc::clone(&calls),
    };
    let mut executor =
        TuiEffectExecutor::spawn(client, ManualClock::default()).expect("executor starts");
    let action = UiProjectAction::CreateWorktree {
        name: "feature".to_owned(),
        brief: None,
        source: "/source".to_owned(),
        destination: "/destination".to_owned(),
        branch: "feature".to_owned(),
        base: Some("main".to_owned()),
    };
    executor
        .execute([UiEffect::SubmitProjectCommand {
            id,
            action: action.clone(),
        }])
        .expect("execute project command");
    let result = UiProjectResult {
        action: action.clone(),
        command_id: [81; 32],
        operation_id: [82; 32],
        project_id: [83; 32],
        runtime_state: Some("uncertain".to_owned()),
        runtime_code: Some("response_lost".to_owned()),
        outcome: UiProjectOutcome::Reconcilable {
            stage: "worktree_created".to_owned(),
            category: "external_state".to_owned(),
            code: "response_lost".to_owned(),
            warning: Some(UiProjectExternalWarning {
                kind: "retained_worktree".to_owned(),
                destination: "/destination".to_owned(),
                branch: "feature".to_owned(),
            }),
        },
    };
    assert_eq!(
        receive_event(&mut executor),
        UiEvent::ProjectCommandCompleted {
            effect_id: id,
            result: result.clone(),
        }
    );
    assert_eq!(calls.lock().expect("calls lock").as_slice(), &[action]);
    executor.shutdown().expect("shutdown");
}

#[test]
fn executor_preserves_exact_resource_check_target_and_typed_failure_evidence() {
    let started = update(
        UiModel::new(UiSize {
            width: 80,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("start");
    let id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("effect identity");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client = ProjectTuiClient {
        calls: Arc::clone(&calls),
    };
    let mut executor =
        TuiEffectExecutor::spawn(client, ManualClock::default()).expect("executor starts");
    let action = UiProjectAction::CheckResources {
        project_id: [83; 32],
        resource_id: Some([84; 32]),
    };
    executor
        .execute([UiEffect::SubmitProjectCommand {
            id,
            action: action.clone(),
        }])
        .expect("execute resource check");
    assert_eq!(
        receive_event(&mut executor),
        UiEvent::ProjectCommandCompleted {
            effect_id: id,
            result: UiProjectResult {
                action: action.clone(),
                command_id: [81; 32],
                operation_id: [82; 32],
                project_id: [83; 32],
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::ResourceChecks {
                    checks: vec![UiProjectResourceCheck {
                        resource_id: [84; 32],
                        status: "rejected".to_owned(),
                        health: None,
                        release: None,
                        observed_canonical_path: None,
                        details: Some("path is unavailable".to_owned()),
                        error_category: Some("resource".to_owned()),
                        error_code: Some("path_unavailable".to_owned()),
                        reconciliation_id: None,
                    }],
                },
            },
        }
    );
    assert_eq!(calls.lock().expect("calls lock").as_slice(), &[action]);
    executor.shutdown().expect("shutdown");
}

#[test]
fn executor_forwards_subscription_and_connection_observations() {
    let client = ScriptedTuiClient {
        requests: Arc::new(AtomicUsize::new(0)),
        conversation_requests: Arc::new(Mutex::new(Vec::new())),
        snapshots: VecDeque::new(),
        stopped: Arc::new(Mutex::new(false)),
    };
    let observer = ScriptedObserver::new([
        TuiClientObservation::Connection {
            generation: 3,
            state: UiConnectionState::Reconnecting,
            cause: None,
        },
        TuiClientObservation::Invalidated { revision: 12 },
        TuiClientObservation::Failure {
            generation: 3,
            failure: UiFailure {
                code: "local_client_unavailable".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    ]);
    let mut executor =
        TuiEffectExecutor::spawn_with_observer(client, observer, ManualClock::default())
            .expect("spawn executor");

    assert!(matches!(
        receive_event(&mut executor),
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Reconnecting,
            cause: None,
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
fn blocked_command_cannot_delay_a_subscribed_invalidation() {
    let command_started = Arc::new(AtomicBool::new(false));
    let (release_command, command_release) = mpsc::channel();
    let client = BlockingSnapshotClient {
        started: Arc::clone(&command_started),
        release: command_release,
    };
    let observer = CommandGatedObserver::new(command_started, 41);
    let mut executor =
        TuiEffectExecutor::spawn_with_observer(client, observer, ManualClock::default())
            .expect("spawn split executor");

    let effect = snapshot_load_effects(1)
        .into_iter()
        .next()
        .expect("snapshot effect");
    let UiEffect::LoadSnapshot { id } = effect.clone() else {
        panic!("expected snapshot effect")
    };
    executor.execute([effect]).expect("queue blocked snapshot");
    assert_eq!(
        receive_event(&mut executor),
        UiEvent::Invalidated { revision: 41 }
    );
    assert!(
        executor.poll_event().is_none(),
        "blocked command must not have completed"
    );

    release_command.send(()).expect("release command");
    assert!(matches!(
        receive_event(&mut executor),
        UiEvent::SnapshotLoaded {
            effect_id,
            snapshot,
        } if effect_id == id && snapshot.revision == 1
    ));
    executor.shutdown().expect("joined split shutdown");
}

#[test]
fn blocked_command_cannot_delay_latest_conversation_selection_control() {
    let command_started = Arc::new(AtomicBool::new(false));
    let (release_command, command_release) = mpsc::channel();
    let client = BlockingSnapshotClient {
        started: Arc::clone(&command_started),
        release: command_release,
    };
    let selected = Arc::new(Mutex::new(TestSelection::Unchanged));
    let observer = ControlledIdleObserver::new(Arc::clone(&selected));
    let mut executor =
        TuiEffectExecutor::spawn_with_observer(client, observer, ManualClock::default())
            .expect("spawn split executor");

    let snapshot_effect = snapshot_load_effects(1)
        .into_iter()
        .next()
        .expect("snapshot effect");
    executor
        .execute([
            snapshot_effect,
            UiEffect::ObserveConversation {
                row_id: Some("thread-a".to_owned()),
            },
            UiEffect::ObserveConversation {
                row_id: Some("thread-b".to_owned()),
            },
        ])
        .expect("replace selection while command is blocked");
    assert_eq!(
        *selected.lock().expect("selected row"),
        TestSelection::Replace(Some("thread-b".to_owned()))
    );

    release_command.send(()).expect("release command");
    executor.shutdown().expect("joined split shutdown");
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

    let opening = update(model, UiEvent::Input(UiInput::Character('N'))).expect("self note");
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
#[allow(clippy::too_many_lines)]
fn authoritative_snapshot_mapping_is_complete_and_deterministic() {
    let source = AuthoritativeSnapshotDto::new(
        21,
        vec![
            SnapshotItem::Conversation {
                key: ConversationKeyDto::Thread {
                    counterparty_installation: Id32::new([1; 32]),
                    counterparty_mailbox: Id32::new([2; 32]),
                    thread: Id32::new([3; 32]),
                },
                context: ConversationContextDto::Direct {
                    participant: ConversationParticipantDto {
                        agent: Some(Id32::new([5; 32])),
                        installation: Some(Id32::new([1; 32])),
                        mailbox: Some(Id32::new([2; 32])),
                        name: Some("builder".to_owned()),
                    },
                },
                local_human: MailboxAddressDto {
                    installation_id: Id32::new([17; 32]),
                    mailbox_id: Id32::new([18; 32]),
                },
                root_message: None,
                preview: Some("Can we ship?".to_owned()),
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
                lifecycle: "active".to_owned(),
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
                input_sequence: 2,
            },
            SnapshotItem::ProjectInput {
                project_id: Id32::new([7; 32]),
                message_id: Id32::new([17; 32]),
                thread_id: Id32::new([18; 32]),
                sequence: 1,
                accepted_fact: Id32::new([19; 32]),
            },
            SnapshotItem::ProjectDispatch {
                dispatch_id: Id32::new([20; 32]),
                message_id: Id32::new([17; 32]),
                sequence: 1,
                fact_id: Id32::new([21; 32]),
                conflicted: false,
            },
            SnapshotItem::ProjectInput {
                project_id: Id32::new([7; 32]),
                message_id: Id32::new([22; 32]),
                thread_id: Id32::new([23; 32]),
                sequence: 2,
                accepted_fact: Id32::new([24; 32]),
            },
            SnapshotItem::IncompleteMessagesTruncated,
        ],
    )
    .expect("authoritative snapshot");

    let snapshot = tui_snapshot([99; 32], &source);
    assert_eq!(snapshot.revision, 21);
    assert_eq!(
        snapshot.human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::NoAccountSelected)
    );
    assert_eq!(snapshot.inbox_rows.len(), 2);
    assert_eq!(snapshot.inbox_rows[0].title, "builder");
    assert_eq!(snapshot.inbox_rows[0].detail, "Can we ship?");
    assert_eq!(snapshot.inbox_rows[1].state, hq_tui::UiRowState::Attention);
    assert_eq!(snapshot.direct_targets.len(), 1);
    assert_eq!(snapshot.direct_targets[0].label, "builder");
    assert_eq!(snapshot.direct_targets[0].installation_id, [15; 32]);
    assert_eq!(snapshot.direct_targets[0].mailbox_id, [16; 32]);

    assert_eq!(snapshot.agent_rows.len(), 1);
    assert_eq!(snapshot.agent_rows[0].title, "builder");
    assert_eq!(snapshot.agent_rows[0].detail, "unassigned");
    assert_eq!(snapshot.agent_rows[0].state, hq_tui::UiRowState::Open);
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents[0].agent_id, [5; 32]);
    assert_eq!(snapshot.agents[0].names, ["builder"]);
    assert_eq!(snapshot.agents[0].mailboxes[0].installation_id, [15; 32]);
    assert_eq!(
        snapshot.agents[0].lifecycle,
        hq_tui::UiAgentLifecycle::Active
    );

    assert_eq!(snapshot.project_rows.len(), 1);
    assert_eq!(snapshot.project_rows[0].title, "release");
    assert_eq!(
        snapshot.project_rows[0].state,
        hq_tui::UiRowState::Attention
    );
    assert_eq!(snapshot.projects.len(), 1);
    assert_eq!(snapshot.projects[0].project_id, [7; 32]);
    assert_eq!(snapshot.projects[0].home, [8; 32]);
    assert_eq!(snapshot.projects[0].name, "release");
    assert_eq!(snapshot.projects[0].head, [11; 32]);
    assert_eq!(snapshot.projects[0].pending_inputs.len(), 1);
    assert_eq!(snapshot.projects[0].pending_inputs[0].message_id, [22; 32]);
    assert_eq!(snapshot.projects[0].pending_inputs[0].thread_id, [23; 32]);
    assert_eq!(snapshot.projects[0].pending_inputs[0].sequence, 2);
    assert_eq!(snapshot.sent_rows.len(), 1);
    assert_eq!(snapshot.archived_rows.len(), 1);
    assert_eq!(snapshot.archived_rows[0].detail, "Can we ship?");
}

#[test]
fn authoritative_snapshot_preserves_project_thread_conversation_identity() {
    let source = AuthoritativeSnapshotDto::new(
        1,
        vec![SnapshotItem::Conversation {
            key: ConversationKeyDto::ProjectThread {
                project: Id32::new([0x31; 32]),
                thread: Id32::new([0x42; 32]),
            },
            context: ConversationContextDto::Project {
                project: Id32::new([0x31; 32]),
                name: Some("release".to_owned()),
                participant: Some(ConversationParticipantDto {
                    agent: Some(Id32::new([0x21; 32])),
                    installation: Some(Id32::new([0x22; 32])),
                    mailbox: Some(Id32::new([0x23; 32])),
                    name: Some("alice".to_owned()),
                }),
            },
            local_human: MailboxAddressDto {
                installation_id: Id32::new([0x24; 32]),
                mailbox_id: Id32::new([0x25; 32]),
            },
            root_message: Some(Id32::new([0x54; 32])),
            preview: Some("Let's have a conversation.".to_owned()),
            latest_fact: Some(Id32::new([0x53; 32])),
            open_messages: 1,
            archived_messages: 0,
            sent_messages: 1,
        }],
    )
    .expect("authoritative snapshot");

    let snapshot = tui_snapshot([0x64; 32], &source);
    assert_eq!(snapshot.inbox_rows.len(), 1);
    assert_eq!(
        snapshot.inbox_rows[0].id,
        format!("project:{}:{}", "31".repeat(32), "42".repeat(32))
    );
    assert_eq!(snapshot.inbox_rows[0].title, "alice");
    assert_eq!(
        snapshot.inbox_rows[0].detail,
        "release · Let's have a conversation."
    );
}

#[test]
fn provider_catalog_mapping_preserves_choices_defaults_unavailability_and_stale_defaults() {
    let snapshot = AuthoritativeSnapshotDto::new(1, Vec::new()).expect("empty snapshot");
    let catalog = ProviderCatalogDto::new(
        vec![
            ProviderAvailabilityDto::new("alpha", "Alpha", true).expect("alpha"),
            ProviderAvailabilityDto::new("codex", "Codex", true).expect("codex"),
            ProviderAvailabilityDto::new("offline", "Offline service", false).expect("offline"),
        ],
        Some("codex".to_owned()),
    )
    .expect("provider catalog");
    let mapped = tui_snapshot_with_provider_catalog([1; 32], &snapshot, &catalog);
    assert_eq!(mapped.providers.len(), 3);
    assert!(mapped.providers[0].available);
    assert!(mapped.providers[1].configured_default);
    assert!(!mapped.providers[2].available);
    assert_eq!(mapped.providers[2].name, "Offline service");

    let stale = ProviderCatalogDto::new(Vec::new(), Some("removed".to_owned()))
        .expect("stale configured provider remains valid");
    let mapped = tui_snapshot_with_provider_catalog([1; 32], &snapshot, &stale);
    assert_eq!(mapped.providers.len(), 1);
    assert_eq!(mapped.providers[0].provider, "removed");
    assert!(mapped.providers[0].configured_default);
    assert!(!mapped.providers[0].available);
}

#[test]
fn authoritative_snapshot_maps_a_new_agent_as_unassigned() {
    let source = AuthoritativeSnapshotDto::new(
        1,
        vec![SnapshotItem::Agent {
            agent_id: Id32::new([5; 32]),
            claims: vec![Id32::new([6; 32])],
            names: vec!["builder".to_owned()],
            mailboxes: vec![MailboxAddressDto {
                installation_id: Id32::new([7; 32]),
                mailbox_id: Id32::new([8; 32]),
            }],
            retirements: Vec::new(),
            lifecycle: "active".to_owned(),
            runnable: false,
        }],
    )
    .expect("authoritative snapshot");

    let snapshot = tui_snapshot([99; 32], &source);
    assert_eq!(snapshot.agent_rows.len(), 1);
    assert_eq!(snapshot.agent_rows[0].detail, "unassigned");
    assert_eq!(snapshot.agent_rows[0].state, hq_tui::UiRowState::Open);
}

#[test]
fn authoritative_snapshot_maps_current_project_assignment_states_onto_agents() {
    let mut items = vec![
        agent_snapshot_item(11, "setting-up", "active", false, false),
        agent_snapshot_item(12, "ready", "active", true, false),
        agent_snapshot_item(13, "blocked", "active", true, false),
        agent_snapshot_item(14, "conflicted", "conflicted", false, false),
        agent_snapshot_item(15, "retired", "retired", false, true),
        agent_snapshot_item(16, "double-booked", "active", true, false),
    ];
    items.extend(project_assignment_snapshot_items(
        21,
        "compiler",
        11,
        "configuring",
        false,
        None,
        false,
    ));
    items.extend(project_assignment_snapshot_items(
        22, "release", 12, "runnable", true, None, false,
    ));
    items.extend(project_assignment_snapshot_items(
        23,
        "migration",
        13,
        "blocked",
        false,
        Some("runtime_unavailable"),
        false,
    ));
    items.extend(project_assignment_snapshot_items(
        24, "client", 16, "runnable", false, None, true,
    ));

    let source = AuthoritativeSnapshotDto::new(1, items).expect("authoritative snapshot");
    let snapshot = tui_snapshot([99; 32], &source);
    let row = |name: &str| {
        snapshot
            .agent_rows
            .iter()
            .find(|row| row.title == name)
            .expect("agent row")
    };

    assert_eq!(
        row("setting-up").detail,
        "assigned to compiler · setting up"
    );
    assert_eq!(row("setting-up").state, hq_tui::UiRowState::Open);
    assert_eq!(row("ready").detail, "assigned to release · ready");
    assert_eq!(row("ready").state, hq_tui::UiRowState::Open);
    assert_eq!(row("blocked").detail, "needs attention · migration blocked");
    assert_eq!(row("blocked").state, hq_tui::UiRowState::Attention);
    assert_eq!(
        row("conflicted").detail,
        "needs attention · saved names disagree"
    );
    assert_eq!(row("conflicted").state, hq_tui::UiRowState::Attention);
    assert_eq!(
        row("double-booked").detail,
        "needs attention · assigned to more than one project"
    );
    assert_eq!(row("double-booked").state, hq_tui::UiRowState::Attention);
    assert_eq!(row("retired").detail, "retired");
    assert_eq!(row("retired").state, hq_tui::UiRowState::Archived);
}

fn agent_snapshot_item(
    byte: u8,
    name: &str,
    lifecycle: &str,
    runnable: bool,
    retired: bool,
) -> SnapshotItem {
    SnapshotItem::Agent {
        agent_id: Id32::new([byte; 32]),
        claims: vec![Id32::new([byte.saturating_add(80); 32])],
        names: vec![name.to_owned()],
        mailboxes: vec![MailboxAddressDto {
            installation_id: Id32::new([1; 32]),
            mailbox_id: Id32::new([byte.saturating_add(100); 32]),
        }],
        retirements: retired
            .then(|| Id32::new([byte.saturating_add(120); 32]))
            .into_iter()
            .collect(),
        lifecycle: lifecycle.to_owned(),
        runnable,
    }
}

fn project_assignment_snapshot_items(
    project_byte: u8,
    name: &str,
    agent_byte: u8,
    phase: &str,
    runnable: bool,
    blocked: Option<&str>,
    cardinality_conflicted: bool,
) -> Vec<SnapshotItem> {
    let assignment_id = Id32::new([project_byte.saturating_add(80); 32]);
    let runtime_fields = (phase == "runnable").then(|| {
        (
            Some("session-1".to_owned()),
            Some(Id32::new([project_byte.saturating_add(100); 32])),
            Some(
                ResourceLocatorDto::new(
                    ResourceSchemeDto::WorkingTree,
                    format!("/workspace/{name}"),
                )
                .expect("launch directory"),
            ),
        )
    });
    let (session, thread_id, launch_directory) = runtime_fields.unwrap_or((None, None, None));
    vec![
        SnapshotItem::Project {
            project_id: Id32::new([project_byte; 32]),
            home: Id32::new([1; 32]),
            account_id: Id32::new([2; 32]),
            mailbox_id: Id32::new([project_byte.saturating_add(40); 32]),
            name: name.to_owned(),
            lifecycle: "open".to_owned(),
            archived: false,
            claimable: true,
            head: Id32::new([project_byte.saturating_add(60); 32]),
            input_sequence: 0,
        },
        SnapshotItem::ProjectAssignment {
            project_id: Id32::new([project_byte; 32]),
            assignment_id,
            agent_id: Id32::new([agent_byte; 32]),
            provider: "codex".to_owned(),
            session,
            phase: phase.to_owned(),
            thread_id,
            launch_directory,
            blocked: blocked.map(str::to_owned),
            cardinality_conflicted,
            runnable,
            support: vec![assignment_id],
        },
    ]
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

    let snapshot = tui_snapshot([99; 32], &source);
    assert_eq!(snapshot.agent_rows.len(), 1);
    assert_eq!(snapshot.agent_rows[0].title, "builder [31m");
    assert_eq!(
        snapshot.agent_rows[0].detail,
        "needs attention · saved names disagree"
    );
    assert!(
        snapshot.agent_rows[0]
            .title
            .chars()
            .chain(snapshot.agent_rows[0].detail.chars())
            .all(|character| !character.is_control())
    );
}

#[test]
fn authoritative_snapshot_human_state_requires_local_selection_and_active_membership() {
    let local = [42; 32];
    let account = Id32::new([41; 32]);
    let source = AuthoritativeSnapshotDto::new(
        1,
        vec![
            SnapshotItem::AccountSelection {
                installation_id: Id32::new(local),
                candidates: vec![account],
                active: Some(account),
                frontier: vec![Id32::new([40; 32])],
            },
            SnapshotItem::Membership {
                account_id: account,
                device: Id32::new(local),
                state: "active".to_owned(),
                frontier: vec![Id32::new([43; 32])],
                grants: vec![DeviceGrantDto {
                    grant_id: Id32::new([39; 32]),
                    grant_fact: Id32::new([38; 32]),
                    device: Id32::new(local),
                    signing_key: Id32::new([37; 32]),
                    label: Some("joined machine".to_owned()),
                    relay_hints: Vec::new(),
                    frontier_member: false,
                    active: true,
                }],
                acceptances: vec![Id32::new([43; 32])],
                revokes: Vec::new(),
                active_acceptances: vec![Id32::new([43; 32])],
            },
        ],
    )
    .expect("authoritative snapshot");

    assert_eq!(
        tui_snapshot(local, &source).human_state,
        UiHumanState::Ready
    );
    assert_eq!(
        tui_snapshot([99; 32], &source).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::NoAccountSelected),
        "another machine's valid selection must not authorize this installation"
    );
}

#[test]
fn authoritative_snapshot_human_state_accepts_local_account_creator_authority() {
    let local = [42; 32];
    let account = Id32::new([41; 32]);
    let source = AuthoritativeSnapshotDto::new(
        1,
        vec![
            SnapshotItem::Account {
                account_id: account,
                root_fact: Id32::new([43; 32]),
                creator_installation: Id32::new(local),
                label: Some("personal".to_owned()),
                selected: true,
            },
            SnapshotItem::AccountSelection {
                installation_id: Id32::new(local),
                candidates: vec![account],
                active: Some(account),
                frontier: vec![Id32::new([40; 32])],
            },
        ],
    )
    .expect("authoritative snapshot");

    assert_eq!(
        tui_snapshot(local, &source).human_state,
        UiHumanState::Ready
    );
}

#[test]
fn authoritative_snapshot_distinguishes_local_human_selection_failures() {
    let local = [42; 32];
    let first = Id32::new([1; 32]);
    let second = Id32::new([2; 32]);
    let frontier = Id32::new([3; 32]);

    let absent = AuthoritativeSnapshotDto::new(1, Vec::new()).expect("empty snapshot");
    assert_eq!(
        tui_snapshot(local, &absent).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::NoAccountSelected)
    );

    let candidates = AuthoritativeSnapshotDto::new(
        1,
        vec![SnapshotItem::AccountSelection {
            installation_id: Id32::new(local),
            candidates: vec![first, second],
            active: None,
            frontier: vec![frontier],
        }],
    )
    .expect("candidate selection snapshot");
    assert_eq!(
        tui_snapshot(local, &candidates).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::SelectionCandidates {
            candidates: vec![[1; 32], [2; 32]],
            frontier: vec![[3; 32]],
        })
    );

    let records = AuthoritativeSnapshotDto::new(
        1,
        vec![
            SnapshotItem::AccountSelection {
                installation_id: Id32::new(local),
                candidates: vec![first],
                active: Some(first),
                frontier: vec![frontier],
            },
            SnapshotItem::AccountSelection {
                installation_id: Id32::new(local),
                candidates: vec![second],
                active: Some(second),
                frontier: vec![Id32::new([4; 32])],
            },
        ],
    )
    .expect("conflicting selection records");
    assert_eq!(
        tui_snapshot(local, &records).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::SelectionRecords {
            records: vec![
                UiHumanSelectionEvidence {
                    candidates: vec![[1; 32]],
                    active: Some([1; 32]),
                    frontier: vec![[3; 32]],
                },
                UiHumanSelectionEvidence {
                    candidates: vec![[2; 32]],
                    active: Some([2; 32]),
                    frontier: vec![[4; 32]],
                },
            ],
        })
    );
}

#[test]
fn authoritative_snapshot_distinguishes_local_human_authority_failures() {
    let local = [42; 32];
    let account = [7; 32];
    let selection_frontier = vec![[8; 32]];

    let without_authority = AuthoritativeSnapshotDto::new(
        1,
        vec![human_selection(local, account, selection_frontier.clone())],
    )
    .expect("selected account without local authority");
    assert_eq!(
        tui_snapshot(local, &without_authority).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::SelectedWithoutAuthority {
            account_id: account,
            selection_frontier: selection_frontier.clone(),
        })
    );

    let pending = AuthoritativeSnapshotDto::new(
        1,
        vec![
            human_selection(local, account, selection_frontier.clone()),
            human_membership(local, account, "pending", vec![[9; 32]], Vec::new()),
        ],
    )
    .expect("pending local membership");
    assert_eq!(
        tui_snapshot(local, &pending).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::MembershipPending(UiHumanMembershipEvidence {
            account_id: account,
            status: UiHumanMembershipStatus::Pending,
            frontier: vec![[9; 32]],
            active_acceptances: Vec::new(),
        }))
    );

    let revoked = AuthoritativeSnapshotDto::new(
        1,
        vec![
            human_selection(local, account, selection_frontier.clone()),
            human_membership(local, account, "revoked", vec![[10; 32]], Vec::new()),
        ],
    )
    .expect("revoked local membership");
    assert_eq!(
        tui_snapshot(local, &revoked).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::MembershipRevoked(UiHumanMembershipEvidence {
            account_id: account,
            status: UiHumanMembershipStatus::Revoked,
            frontier: vec![[10; 32]],
            active_acceptances: Vec::new(),
        }))
    );

    let conflicted = AuthoritativeSnapshotDto::new(
        1,
        vec![
            human_selection(local, account, selection_frontier),
            human_membership(
                local,
                account,
                "active",
                vec![[11; 32], [12; 32]],
                vec![[11; 32], [12; 32]],
            ),
        ],
    )
    .expect("conflicting local membership authority");
    assert!(matches!(
        tui_snapshot(local, &conflicted).human_state,
        UiHumanState::NeedsAttention(UiHumanIssue::MembershipAuthorityConflict { records })
            if records.len() == 1
                && records[0].status == UiHumanMembershipStatus::Active
                && records[0].active_acceptances == [[11; 32], [12; 32]]
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
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
            ConversationEntryDto::Activity(Box::new(ConversationActivityDto {
                fact_id: Id32::new([13; 32]),
                activity_kind: ConversationActivityKindDto::Progress,
                sequence: 4,
                source_installation: Id32::new([15; 32]),
                source_mailbox: Id32::new([16; 32]),
                provider: "provider".to_owned(),
                session: "session".to_owned(),
                operation: Id32::new([14; 32]),
                item: Some("item".to_owned()),
                logical_key: "progress".to_owned(),
                runtime: "runtime".to_owned(),
                occurred_at_unix_ms: 4,
                status: ActivityStatusDto::Running,
                content: "building".to_owned(),
                truncated: false,
                completed: None,
            })),
        ],
        Some("opaque-next".to_owned()),
    )
    .expect("valid page");

    let context = ConversationContextDto::Direct {
        participant: ConversationParticipantDto {
            agent: Some(Id32::new([14; 32])),
            installation: Some(Id32::new([6; 32])),
            mailbox: Some(Id32::new([7; 32])),
            name: Some("Alice".to_owned()),
        },
    };
    let local_human = MailboxAddressDto {
        installation_id: Id32::new([4; 32]),
        mailbox_id: Id32::new([5; 32]),
    };
    let mapped = tui_conversation_page("thread-row", &context, &local_human, page);
    assert_eq!(mapped.row_id, "thread-row");
    assert_eq!(mapped.title, "Alice");
    assert_eq!(mapped.next_cursor.as_deref(), Some("opaque-next"));
    assert_eq!(mapped.entries.len(), 2);
    assert!(matches!(
        &mapped.entries[0].presentation,
        UiConversationEntryPresentation::Message {
            author: UiConversationAuthor::You,
            body,
        } if body == "hello\nworld"
    ));
    assert_eq!(
        mapped.entries[0].message_state,
        Some(UiMessageState::Archived)
    );
    assert_eq!(
        mapped.entries[0].delivery,
        Some(UiMessageDelivery::Received)
    );
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
    assert!(matches!(
        &mapped.entries[1].presentation,
        UiConversationEntryPresentation::Activity { summary, detail, .. }
            if summary == "building" && detail == "building"
    ));
    assert_eq!(mapped.entries[1].message_state, None);
    assert_eq!(mapped.entries[1].message_target, None);
    assert!(matches!(
        mapped.entries[1].technical.as_slice(),
        [UiTechnicalSection::Activity { sequence: 4, .. }]
    ));
}

#[test]
fn conversation_message_mapping_preserves_safe_multiline_text() {
    let local_human = MailboxAddressDto {
        installation_id: Id32::new([4; 32]),
        mailbox_id: Id32::new([5; 32]),
    };
    let mut message = conversation_message(local_human.installation_id, local_human.mailbox_id);
    message.content = concat!(
        "paragraph 1\r\nparagraph 2\rlone\n",
        "\t`code\x1b[31m`\n",
        "[link](\x1b]8;;https://example.test\x07target\x1b]8;;\x07)\n",
        "C0:\0 C1:\u{0085} DEL:\u{007f}\n",
        "emoji: 👩‍💻 cafe\u{301} CJK: 界",
    )
    .to_owned();
    let page =
        ConversationPageDto::new(vec![ConversationEntryDto::Message(Box::new(message))], None)
            .expect("valid message page");

    let mapped =
        tui_conversation_page("row", &ConversationContextDto::Personal, &local_human, page);
    let UiConversationEntryPresentation::Message { body, .. } = &mapped.entries[0].presentation
    else {
        panic!("message remains typed")
    };

    assert_eq!(
        body,
        concat!(
            "paragraph 1\nparagraph 2\nlone\n",
            "    `code [31m`\n",
            "[link]( ]8;;https://example.test target ]8;; )\n",
            "C0:  C1:  DEL: \n",
            "emoji: 👩‍💻 cafe\u{301} CJK: 界",
        )
    );
    assert!(
        body.chars()
            .all(|character| { character == '\n' || !character.is_control() })
    );
}

#[test]
fn local_message_without_peer_receipt_is_presented_as_sent() {
    let local_human = MailboxAddressDto {
        installation_id: Id32::new([4; 32]),
        mailbox_id: Id32::new([5; 32]),
    };
    let page = ConversationPageDto::new(
        vec![ConversationEntryDto::Message(Box::new(
            conversation_message(local_human.installation_id, local_human.mailbox_id),
        ))],
        None,
    )
    .expect("valid message page");

    let mapped =
        tui_conversation_page("row", &ConversationContextDto::Personal, &local_human, page);

    assert_eq!(mapped.entries[0].delivery, Some(UiMessageDelivery::Sent));
}

#[test]
#[allow(clippy::too_many_lines)]
fn conversation_author_classification_uses_only_exact_mailbox_evidence() {
    let local_human = MailboxAddressDto {
        installation_id: Id32::new([4; 32]),
        mailbox_id: Id32::new([5; 32]),
    };
    let map_author = |context: ConversationContextDto, installation, mailbox| {
        let page = ConversationPageDto::new(
            vec![ConversationEntryDto::Message(Box::new(
                conversation_message(installation, mailbox),
            ))],
            None,
        )
        .expect("valid message page");
        let mapped = tui_conversation_page("row", &context, &local_human, page);
        let UiConversationEntryPresentation::Message { author, .. } =
            &mapped.entries[0].presentation
        else {
            panic!("message remains typed")
        };
        (mapped.title, mapped.context, author.clone())
    };

    let direct = ConversationContextDto::Direct {
        participant: ConversationParticipantDto {
            agent: None,
            installation: Some(Id32::new([6; 32])),
            mailbox: Some(Id32::new([7; 32])),
            name: Some("Alice".to_owned()),
        },
    };
    assert_eq!(
        map_author(direct, Id32::new([6; 32]), Id32::new([7; 32])),
        (
            "Alice".to_owned(),
            None,
            UiConversationAuthor::Participant("Alice".to_owned()),
        )
    );

    let unnamed = ConversationContextDto::Direct {
        participant: ConversationParticipantDto {
            agent: None,
            installation: Some(Id32::new([6; 32])),
            mailbox: Some(Id32::new([7; 32])),
            name: None,
        },
    };
    assert_eq!(
        map_author(unnamed, Id32::new([6; 32]), Id32::new([7; 32])).2,
        UiConversationAuthor::Participant("Other participant".to_owned())
    );

    let project = ConversationContextDto::Project {
        project: Id32::new([8; 32]),
        name: Some("Release".to_owned()),
        participant: Some(ConversationParticipantDto {
            agent: Some(Id32::new([9; 32])),
            installation: Some(Id32::new([6; 32])),
            mailbox: Some(Id32::new([7; 32])),
            name: None,
        }),
    };
    assert_eq!(
        map_author(project, Id32::new([6; 32]), Id32::new([7; 32])),
        (
            "Project agent".to_owned(),
            Some("Project · Release".to_owned()),
            UiConversationAuthor::Participant("Project agent".to_owned()),
        )
    );

    assert_eq!(
        map_author(
            ConversationContextDto::Personal,
            local_human.installation_id,
            local_human.mailbox_id,
        ),
        ("Personal notes".to_owned(), None, UiConversationAuthor::You)
    );

    let unresolved = ConversationContextDto::Direct {
        participant: ConversationParticipantDto {
            agent: None,
            installation: Some(Id32::new([6; 32])),
            mailbox: None,
            name: Some("Alice".to_owned()),
        },
    };
    assert_eq!(
        map_author(unresolved, Id32::new([6; 32]), Id32::new([7; 32])).2,
        UiConversationAuthor::Unknown
    );

    let conflicting = ConversationContextDto::Direct {
        participant: ConversationParticipantDto {
            agent: None,
            installation: Some(local_human.installation_id),
            mailbox: Some(local_human.mailbox_id),
            name: Some("Alias".to_owned()),
        },
    };
    assert_eq!(
        map_author(
            conflicting,
            local_human.installation_id,
            local_human.mailbox_id,
        )
        .2,
        UiConversationAuthor::You,
        "local-human evidence wins a contradictory display context"
    );
}

#[test]
fn every_conversation_activity_kind_and_status_remains_typed() {
    let kinds = [
        (
            ConversationActivityKindDto::Status,
            UiConversationActivityKind::Status,
        ),
        (
            ConversationActivityKindDto::AgentTurn,
            UiConversationActivityKind::AgentTurn,
        ),
        (
            ConversationActivityKindDto::Progress,
            UiConversationActivityKind::Progress,
        ),
        (
            ConversationActivityKindDto::Plan,
            UiConversationActivityKind::Plan,
        ),
        (
            ConversationActivityKindDto::Diff,
            UiConversationActivityKind::Diff,
        ),
        (
            ConversationActivityKindDto::CompletedItem,
            UiConversationActivityKind::CompletedItem,
        ),
    ];
    let statuses = [
        ActivityStatusDto::Snapshot,
        ActivityStatusDto::Running,
        ActivityStatusDto::Succeeded,
        ActivityStatusDto::Failed {
            reason: "tool_failed".to_owned(),
        },
        ActivityStatusDto::Interrupted,
    ];
    let context = ConversationContextDto::Personal;
    let local_human = MailboxAddressDto {
        installation_id: Id32::new([4; 32]),
        mailbox_id: Id32::new([5; 32]),
    };

    for (dto_kind, ui_kind) in kinds {
        for status in statuses.clone() {
            let page = ConversationPageDto::new(
                vec![ConversationEntryDto::Activity(Box::new(
                    ConversationActivityDto {
                        fact_id: Id32::new([1; 32]),
                        activity_kind: dto_kind,
                        sequence: 1,
                        source_installation: Id32::new([3; 32]),
                        source_mailbox: Id32::new([4; 32]),
                        provider: "provider".to_owned(),
                        session: "session".to_owned(),
                        operation: Id32::new([2; 32]),
                        item: None,
                        logical_key: "activity".to_owned(),
                        runtime: "runtime".to_owned(),
                        occurred_at_unix_ms: 1,
                        status,
                        content: "exact provider detail".to_owned(),
                        truncated: true,
                        completed: (dto_kind == ConversationActivityKindDto::CompletedItem)
                            .then_some(CompletedItemPresentationDto::Unknown),
                    },
                ))],
                None,
            )
            .expect("valid activity page");
            let mapped = tui_conversation_page("row", &context, &local_human, page);
            assert!(matches!(
                &mapped.entries[0].presentation,
                UiConversationEntryPresentation::Activity {
                    kind,
                    summary,
                    detail,
                    truncated: true,
                    ..
                } if *kind == ui_kind
                    && !summary.is_empty()
                    && detail == "exact provider detail"
            ));
            assert!(mapped.entries[0].message_target.is_none());
        }
    }
}

fn conversation_message(sender_installation: Id32, sender_mailbox: Id32) -> ConversationMessageDto {
    ConversationMessageDto {
        fact_id: Id32::new([1; 32]),
        message_id: Id32::new([2; 32]),
        thread_id: Id32::new([3; 32]),
        content: "hello".to_owned(),
        sender_installation,
        sender_mailbox,
        recipient_installation: None,
        recipient_mailbox: None,
        purpose: MessagePurposeDto::Asynchronous,
        presentation: PresentationKindDto::Message,
        correlation_provider: None,
        correlation_session: None,
        correlation_operation: None,
        project_id: None,
        open: true,
        rejected: false,
        state_frontier: Vec::new(),
        peer_received_by: Vec::new(),
        root_fact: None,
        root_message: None,
        ready_answer: false,
        thread_cancelled: false,
    }
}

#[test]
fn command_worker_panics_are_joined_and_reported() {
    let mut executor =
        TuiEffectExecutor::spawn(PanickingClient, ManualClock::default()).expect("spawn executor");
    executor
        .execute([snapshot_load_effects(1).remove(0)])
        .expect("queue panicking command");
    thread::sleep(Duration::from_millis(50));
    assert_eq!(executor.shutdown(), Err(TuiExecutorError::WorkerPanicked));
}

#[test]
fn observation_worker_panics_are_joined_and_reported() {
    let mut executor = TuiEffectExecutor::spawn_with_observer(
        ImmediateSnapshotClient,
        PanickingObserver::new(),
        ManualClock::default(),
    )
    .expect("spawn executor");
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

#[test]
fn shutdown_drains_saturated_observation_results_and_joins_idempotently() {
    let observer = ScriptedObserver::new(
        (1..=25).map(|revision| TuiClientObservation::Invalidated { revision }),
    );
    let mut executor = TuiEffectExecutor::spawn_with_observer(
        ImmediateSnapshotClient,
        observer,
        ManualClock::default(),
    )
    .expect("spawn observation executor");
    thread::sleep(Duration::from_millis(50));

    executor.shutdown().expect("drain and join observer");
    executor.shutdown().expect("repeat shutdown is inert");
}

#[test]
fn shutdown_preempts_queued_client_work_before_joining() {
    let calls = Arc::new(AtomicUsize::new(0));
    let client = SlowSnapshotClient {
        calls: Arc::clone(&calls),
    };
    let mut executor =
        TuiEffectExecutor::spawn(client, ManualClock::default()).expect("spawn slow executor");
    executor
        .execute(snapshot_load_effects(8))
        .expect("fill command queue");
    let observation_deadline = Instant::now() + Duration::from_secs(1);
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < observation_deadline,
            "worker did not begin queued work"
        );
        thread::yield_now();
    }

    let started = Instant::now();
    executor.shutdown().expect("queued worker joins");

    assert!(
        started.elapsed() < Duration::from_millis(400),
        "shutdown drained stale queued work before joining"
    );
    assert!(
        calls.load(Ordering::SeqCst) < 8,
        "shutdown executed every stale queued command"
    );
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

struct ScriptedObserver {
    observations: VecDeque<TuiClientObservation>,
    wake: mpsc::Receiver<()>,
    interrupt: ScriptedInterrupt,
}

struct PanickingObserver {
    interrupt: ScriptedInterrupt,
}

impl PanickingObserver {
    fn new() -> Self {
        let (interrupt, _wake) = mpsc::channel();
        Self {
            interrupt: ScriptedInterrupt(interrupt),
        }
    }
}

impl TuiObservationPort for PanickingObserver {
    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        panic!("scripted observation failure");
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.interrupt.clone())
    }
}

struct BlockingSnapshotClient {
    started: Arc<AtomicBool>,
    release: mpsc::Receiver<()>,
}

impl TuiClientPort for BlockingSnapshotClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        self.started.store(true, Ordering::SeqCst);
        self.release.recv().expect("command release");
        Ok(empty_snapshot(1))
    }

    fn load_conversation(
        &mut self,
        _row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Err(unsupported_failure())
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
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(unsupported_failure())
    }
}

struct CommandGatedObserver {
    command_started: Arc<AtomicBool>,
    revision: Option<u64>,
    wake: mpsc::Receiver<()>,
    interrupt: ScriptedInterrupt,
}

#[derive(Clone)]
struct LatestSelectionControl(Arc<Mutex<TestSelection>>);

#[derive(Debug, Eq, PartialEq)]
enum TestSelection {
    Unchanged,
    Replace(Option<String>),
}

impl TuiObservationControl for LatestSelectionControl {
    fn select_conversation(&self, row_id: Option<String>) {
        *self.0.lock().expect("selection slot") = TestSelection::Replace(row_id);
    }
}

struct ControlledIdleObserver {
    wake: mpsc::Receiver<()>,
    interrupt: ScriptedInterrupt,
    control: LatestSelectionControl,
}

impl ControlledIdleObserver {
    fn new(selected: Arc<Mutex<TestSelection>>) -> Self {
        let (interrupt, wake) = mpsc::channel();
        Self {
            wake,
            interrupt: ScriptedInterrupt(interrupt),
            control: LatestSelectionControl(selected),
        }
    }
}

impl TuiObservationPort for ControlledIdleObserver {
    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        let _ = self.wake.recv();
        Vec::new()
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.interrupt.clone())
    }

    fn control_handle(&self) -> Arc<dyn TuiObservationControl> {
        Arc::new(self.control.clone())
    }
}

impl CommandGatedObserver {
    fn new(command_started: Arc<AtomicBool>, revision: u64) -> Self {
        let (interrupt, wake) = mpsc::channel();
        Self {
            command_started,
            revision: Some(revision),
            wake,
            interrupt: ScriptedInterrupt(interrupt),
        }
    }
}

impl TuiObservationPort for CommandGatedObserver {
    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        if let Some(revision) = self.revision {
            while !self.command_started.load(Ordering::SeqCst) {
                thread::yield_now();
            }
            self.revision = None;
            vec![TuiClientObservation::Invalidated { revision }]
        } else {
            let _ = self.wake.recv();
            Vec::new()
        }
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.interrupt.clone())
    }
}

#[derive(Clone)]
struct ScriptedInterrupt(mpsc::Sender<()>);

impl ScriptedObserver {
    fn new(observations: impl IntoIterator<Item = TuiClientObservation>) -> Self {
        let (interrupt, wake) = mpsc::channel();
        Self {
            observations: observations.into_iter().collect(),
            wake,
            interrupt: ScriptedInterrupt(interrupt),
        }
    }
}

impl TuiObservationInterrupt for ScriptedInterrupt {
    fn interrupt(&self) {
        let _ = self.0.send(());
    }
}

impl TuiObservationPort for ScriptedObserver {
    fn next_observations(&mut self) -> Vec<TuiClientObservation> {
        if let Some(observation) = self.observations.pop_front() {
            vec![observation]
        } else {
            let _ = self.wake.recv();
            Vec::new()
        }
    }

    fn interrupt_handle(&self) -> Arc<dyn TuiObservationInterrupt> {
        Arc::new(self.interrupt.clone())
    }
}

struct ScriptedTuiClient {
    requests: Arc<AtomicUsize>,
    conversation_requests: ConversationRequests,
    snapshots: VecDeque<Result<UiSnapshot, UiFailure>>,
    stopped: Arc<Mutex<bool>>,
}

impl ScriptedTuiClient {
    fn empty() -> Self {
        Self {
            requests: Arc::new(AtomicUsize::new(0)),
            conversation_requests: Arc::new(Mutex::new(Vec::new())),
            snapshots: VecDeque::new(),
            stopped: Arc::new(Mutex::new(false)),
        }
    }
}

impl TuiClientPort for ScriptedTuiClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        self.requests.fetch_add(1, Ordering::SeqCst);
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
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(unsupported_failure())
    }
}

impl Drop for ScriptedTuiClient {
    fn drop(&mut self) {
        *self.stopped.lock().expect("stopped lock") = true;
    }
}

struct PanickingClient;

struct AgentTuiClient {
    calls: Arc<Mutex<Vec<UiAgentAction>>>,
}

struct ManagedSessionTuiClient {
    calls: Arc<Mutex<Vec<UiManagedSessionAction>>>,
}

struct ProjectTuiClient {
    calls: Arc<Mutex<Vec<UiProjectAction>>>,
}

impl TuiClientPort for ProjectTuiClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        Ok(empty_snapshot(1))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
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
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(unsupported_failure())
    }

    fn submit_project_command(
        &mut self,
        action: UiProjectAction,
    ) -> Result<UiProjectResult, UiFailure> {
        self.calls.lock().expect("calls lock").push(action.clone());
        let resource_check = matches!(&action, UiProjectAction::CheckResources { .. });
        let outcome = match &action {
            UiProjectAction::CheckResources { resource_id, .. } => {
                UiProjectOutcome::ResourceChecks {
                    checks: resource_id
                        .iter()
                        .map(|resource_id| UiProjectResourceCheck {
                            resource_id: *resource_id,
                            status: "rejected".to_owned(),
                            health: None,
                            release: None,
                            observed_canonical_path: None,
                            details: Some("path is unavailable".to_owned()),
                            error_category: Some("resource".to_owned()),
                            error_code: Some("path_unavailable".to_owned()),
                            reconciliation_id: None,
                        })
                        .collect(),
                }
            }
            _ => UiProjectOutcome::Reconcilable {
                stage: "worktree_created".to_owned(),
                category: "external_state".to_owned(),
                code: "response_lost".to_owned(),
                warning: Some(UiProjectExternalWarning {
                    kind: "retained_worktree".to_owned(),
                    destination: "/destination".to_owned(),
                    branch: "feature".to_owned(),
                }),
            },
        };
        Ok(UiProjectResult {
            action,
            command_id: [81; 32],
            operation_id: [82; 32],
            project_id: [83; 32],
            runtime_state: (!resource_check).then(|| "uncertain".to_owned()),
            runtime_code: (!resource_check).then(|| "response_lost".to_owned()),
            outcome,
        })
    }
}

impl TuiClientPort for ManagedSessionTuiClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        Ok(empty_snapshot(1))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
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
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(unsupported_failure())
    }

    fn submit_managed_session(
        &mut self,
        action: UiManagedSessionAction,
    ) -> Result<UiManagedSessionResult, UiFailure> {
        self.calls.lock().expect("calls lock").push(action.clone());
        Ok(UiManagedSessionResult {
            action,
            operation_id: [91; 32],
            outcome: UiManagedSessionOutcome::Uncertain {
                reconciliation_id: [92; 32],
            },
        })
    }
}

impl TuiClientPort for AgentTuiClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        Ok(empty_snapshot(1))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
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
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(unsupported_failure())
    }

    fn submit_agent_command(&mut self, action: UiAgentAction) -> Result<u64, UiFailure> {
        self.calls.lock().expect("calls lock").push(action);
        Ok(23)
    }
}

impl TuiClientPort for PanickingClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
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
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        panic!("scripted worker failure");
    }
}

struct ImmediateSnapshotClient;

struct SlowSnapshotClient {
    calls: Arc<AtomicUsize>,
}

impl TuiClientPort for SlowSnapshotClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(100));
        Ok(empty_snapshot(1))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
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
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(unsupported_failure())
    }
}

impl TuiClientPort for ImmediateSnapshotClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        Ok(empty_snapshot(1))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
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
        Err(unsupported_draft())
    }

    fn save_draft(&mut self, _draft: UiMailboxDraft) -> Result<UiMailboxDraft, TuiDraftError> {
        Err(unsupported_draft())
    }

    fn submit_mailbox_command(
        &mut self,
        _draft: Option<UiMailboxDraft>,
        _action: UiMailboxAction,
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        Err(unsupported_failure())
    }
}

struct MailboxTuiClient {
    calls: Arc<Mutex<Vec<String>>>,
}

impl TuiClientPort for MailboxTuiClient {
    fn load_snapshot(&mut self) -> Result<UiSnapshot, UiFailure> {
        self.calls
            .lock()
            .expect("calls lock")
            .push("snapshot".to_owned());
        Ok(empty_snapshot(1))
    }

    fn load_conversation(
        &mut self,
        row_id: &str,
        _cursor: Option<String>,
    ) -> Result<UiConversationPage, UiFailure> {
        Ok(UiConversationPage {
            title: "Alice".to_owned(),
            context: None,
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
    ) -> Result<UiMailboxCommandResult, UiFailure> {
        assert!(matches!(action, UiMailboxAction::SelfNote));
        assert_eq!(
            draft.as_ref().map(|draft| draft.content.as_str()),
            Some("durable note")
        );
        self.calls
            .lock()
            .expect("calls lock")
            .push("submit:self_note".to_owned());
        Ok(UiMailboxCommandResult {
            revision: 9,
            message_id: Some([7; 32]),
        })
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
        let UiEffect::LoadSnapshot { id } = effect.clone() else {
            unreachable!("matched snapshot effect")
        };
        effects.push(effect);
        let loaded = update(
            transition.model,
            UiEvent::SnapshotLoaded {
                effect_id: id,
                snapshot: empty_snapshot(revision as u64),
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

fn empty_snapshot(revision: u64) -> UiSnapshot {
    UiSnapshot {
        revision,
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
    }
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
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(event) = executor.poll_event() {
            return event;
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("executor event did not arrive");
        let ready = {
            let mut descriptors = [PollFd::new(
                executor.event_wake().as_fd(),
                PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
            )];
            poll(
                &mut descriptors,
                PollTimeout::try_from(remaining).expect("bounded test timeout"),
            )
            .expect("poll executor wake")
        };
        assert_ne!(ready, 0, "executor event did not wake its consumer");
    }
}
