//! Pure TUI transition and stale-effect contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use hq_tui::{
    UiActivityStatus, UiConnectionState, UiConversationEntry, UiConversationEntryKind,
    UiConversationPage, UiEffect, UiEvent, UiFailure, UiFocus, UiInput, UiMessageState, UiModel,
    UiRow, UiRowKind, UiRowState, UiSection, UiSize, UiSnapshot, UiTechnicalSection, UiTimerKind,
    update,
};

#[test]
fn startup_allocates_explicit_snapshot_tick_and_redraw_effects() {
    let transition = update(
        UiModel::new(UiSize {
            width: 120,
            height: 32,
        }),
        UiEvent::Started,
    )
    .expect("startup transition");
    assert_eq!(transition.model.connection(), UiConnectionState::Connecting);
    assert_eq!(transition.effects.len(), 3);
    let UiEffect::LoadSnapshot {
        id: snapshot_id,
        section,
    } = &transition.effects[0]
    else {
        panic!("first effect loads a snapshot");
    };
    assert_eq!(*section, UiSection::Inbox);
    let UiEffect::ScheduleTimer {
        id,
        kind: UiTimerKind::PeriodicRefresh,
        after,
    } = &transition.effects[1]
    else {
        panic!("second effect schedules periodic repair");
    };
    assert_eq!(*after, Duration::from_secs(300));
    let tick_id = *id;
    let snapshot_id = *snapshot_id;
    assert_ne!(snapshot_id, tick_id);
    assert_eq!(transition.model.pending_snapshot(), Some(snapshot_id));
    assert_eq!(transition.effects[2], UiEffect::RequestRedraw);
}

#[test]
fn stale_snapshot_success_and_failure_cannot_overwrite_newer_state() {
    let started = started_model();
    let first_id = snapshot_effect(&started.effects);
    let invalidated = update(started.model, UiEvent::Invalidated { revision: 8 })
        .expect("invalidation coalesces");
    let first_loaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: first_id,
            snapshot: snapshot(4, &["old"]),
        },
    )
    .expect("old snapshot triggers follow-up");
    let second_id = snapshot_effect(&first_loaded.effects);
    assert_ne!(first_id, second_id);

    let ready = update(
        first_loaded.model,
        UiEvent::SnapshotLoaded {
            effect_id: second_id,
            snapshot: snapshot(9, &["current"]),
        },
    )
    .expect("current snapshot applies");
    assert_eq!(ready.model.snapshot().map(|value| value.revision), Some(9));
    let stale_success = update(
        ready.model,
        UiEvent::SnapshotLoaded {
            effect_id: first_id,
            snapshot: snapshot(3, &["stale"]),
        },
    )
    .expect("stale success is inert");
    assert!(stale_success.effects.is_empty());
    assert_eq!(
        stale_success.model.snapshot().map(|value| value.revision),
        Some(9)
    );
    let stale_failure = update(
        stale_success.model,
        UiEvent::SnapshotFailed {
            effect_id: first_id,
            failure: UiFailure {
                code: "stale_failure".to_owned(),
                action: "ignore this old result".to_owned(),
            },
        },
    )
    .expect("stale failure is inert");
    assert_eq!(stale_failure.model.connection(), UiConnectionState::Ready);
    assert!(stale_failure.model.last_failure().is_none());
}

#[test]
fn invalidations_coalesce_and_one_matching_failure_schedules_one_retry() {
    let started = started_model();
    let request_id = snapshot_effect(&started.effects);
    let first =
        update(started.model, UiEvent::Invalidated { revision: 10 }).expect("first invalidation");
    assert_eq!(first.model.required_revision(), Some(10));
    assert_eq!(redraw_count(&first.effects), 1);
    let second =
        update(first.model, UiEvent::Invalidated { revision: 7 }).expect("older invalidation");
    assert_eq!(second.model.required_revision(), Some(10));
    assert!(second.effects.is_empty());
    let failed = update(
        second.model,
        UiEvent::SnapshotFailed {
            effect_id: request_id,
            failure: UiFailure {
                code: "node_unavailable".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    )
    .expect("matching failure applies");
    assert_eq!(failed.model.connection(), UiConnectionState::Reconnecting);
    assert_eq!(
        failed.model.last_failure().map(|value| value.code.as_str()),
        Some("node_unavailable")
    );
    assert!(matches!(
        failed.effects.as_slice(),
        [
            UiEffect::ScheduleTimer {
                kind: UiTimerKind::RetrySnapshot,
                after,
                ..
            },
            UiEffect::RequestRedraw
        ] if *after == Duration::from_millis(250)
    ));
}

#[test]
fn logical_selection_focus_section_resize_and_quit_are_pure_transitions() {
    let started = started_model();
    let request_id = snapshot_effect(&started.effects);
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request_id,
            snapshot: snapshot(1, &["alpha", "beta", "gamma"]),
        },
    )
    .expect("snapshot applies");
    assert_eq!(loaded.model.selected_row(), Some("alpha"));

    let down = update(loaded.model, UiEvent::Input(UiInput::NextItem)).expect("move down");
    assert_eq!(down.model.selected_row(), Some("beta"));
    let focused = update(down.model, UiEvent::Input(UiInput::NextFocus)).expect("change focus");
    assert_eq!(focused.model.focus(), UiFocus::Content);
    let section =
        update(focused.model, UiEvent::Input(UiInput::NextSection)).expect("change section");
    assert_eq!(section.model.section(), UiSection::Sent);
    let resized = update(
        section.model,
        UiEvent::Resized(UiSize {
            width: 70,
            height: 18,
        }),
    )
    .expect("resize transition");
    assert_eq!(
        resized.model.viewport(),
        UiSize {
            width: 70,
            height: 18,
        }
    );
    let quit = update(resized.model, UiEvent::Input(UiInput::Quit)).expect("quit transition");
    assert!(quit.model.should_exit());
    assert_eq!(quit.effects, vec![UiEffect::Exit]);
}

#[test]
fn reload_preserves_a_logical_selection_and_falls_back_when_it_disappears() {
    let started = started_model();
    let first_id = snapshot_effect(&started.effects);
    let first = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: first_id,
            snapshot: snapshot(1, &["alpha", "beta"]),
        },
    )
    .expect("first snapshot");
    let selected = update(first.model, UiEvent::Input(UiInput::NextItem)).expect("select beta");
    let invalidated =
        update(selected.model, UiEvent::Invalidated { revision: 2 }).expect("request reload");
    let second_id = snapshot_effect(&invalidated.effects);
    let preserved = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: second_id,
            snapshot: snapshot(2, &["gamma", "beta"]),
        },
    )
    .expect("selection preserved");
    assert_eq!(preserved.model.selected_row(), Some("beta"));

    let invalidated = update(preserved.model, UiEvent::Invalidated { revision: 3 })
        .expect("request another reload");
    let third_id = snapshot_effect(&invalidated.effects);
    let replaced = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: third_id,
            snapshot: snapshot(3, &["delta"]),
        },
    )
    .expect("missing selection falls back");
    assert_eq!(replaced.model.selected_row(), Some("delta"));
}

#[test]
fn section_change_rejects_the_old_sections_in_flight_snapshot() {
    let started = started_model();
    let inbox_id = snapshot_effect(&started.effects);
    let sent = update(started.model, UiEvent::Input(UiInput::NextSection))
        .expect("section changes while inbox is pending");
    assert_eq!(sent.model.section(), UiSection::Sent);

    let old_section = update(
        sent.model,
        UiEvent::SnapshotLoaded {
            effect_id: inbox_id,
            snapshot: snapshot_for(UiSection::Inbox, 4, &["inbox"]),
        },
    )
    .expect("old section response schedules selected section");
    assert!(old_section.model.snapshot().is_none());
    let (sent_id, requested_section) = snapshot_effect_with_section(&old_section.effects);
    assert_eq!(requested_section, UiSection::Sent);

    let current_section = update(
        old_section.model,
        UiEvent::SnapshotLoaded {
            effect_id: sent_id,
            snapshot: snapshot_for(UiSection::Sent, 4, &["sent"]),
        },
    )
    .expect("selected section applies");
    assert_eq!(
        current_section.model.snapshot().map(|value| value.section),
        Some(UiSection::Sent)
    );
    assert_eq!(current_section.model.selected_row(), Some("sent"));
}

#[test]
fn connection_observations_ignore_older_generations() {
    let started = started_model();
    let reconnecting = update(
        started.model,
        UiEvent::ConnectionObserved {
            generation: 4,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("new generation applies");
    assert_eq!(
        reconnecting.model.connection(),
        UiConnectionState::Reconnecting
    );

    let stale = update(
        reconnecting.model,
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Ready,
        },
    )
    .expect("old generation is inert");
    assert_eq!(stale.model.connection(), UiConnectionState::Reconnecting);
    assert!(stale.effects.is_empty());

    let recovered = update(
        stale.model,
        UiEvent::ConnectionObserved {
            generation: 4,
            state: UiConnectionState::Ready,
        },
    )
    .expect("current generation applies");
    assert_eq!(recovered.model.connection(), UiConnectionState::Ready);
    assert_eq!(redraw_count(&recovered.effects), 1);
}

#[test]
fn client_failures_are_scoped_to_the_current_connection_generation() {
    let started = started_model();
    let current = update(
        started.model,
        UiEvent::ClientFailed {
            generation: 4,
            failure: UiFailure {
                code: "connection_lost".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    )
    .expect("current failure applies");
    assert_eq!(current.model.connection(), UiConnectionState::Reconnecting);
    assert_eq!(
        current
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("connection_lost")
    );
    assert_eq!(redraw_count(&current.effects), 1);

    let stale = update(
        current.model,
        UiEvent::ClientFailed {
            generation: 3,
            failure: UiFailure {
                code: "stale_failure".to_owned(),
                action: "ignore old generation".to_owned(),
            },
        },
    )
    .expect("older failure is inert");
    assert_eq!(
        stale
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("connection_lost")
    );
    assert!(stale.effects.is_empty());
}

#[test]
fn stale_timer_completions_cannot_repeat_effects() {
    let started = started_model();
    let periodic_id = timer_effect(&started.effects, UiTimerKind::PeriodicRefresh);
    let elapsed = update(
        started.model,
        UiEvent::TimerElapsed {
            effect_id: periodic_id,
        },
    )
    .expect("current timer applies");
    assert!(elapsed.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::ScheduleTimer {
            kind: UiTimerKind::PeriodicRefresh,
            ..
        }
    )));

    let stale = update(
        elapsed.model,
        UiEvent::TimerElapsed {
            effect_id: periodic_id,
        },
    )
    .expect("stale timer is inert");
    assert!(stale.effects.is_empty());
}

#[test]
fn conversation_pages_preserve_reducer_order_and_use_stable_entry_anchors() {
    let started = started_model();
    let snapshot_id = snapshot_effect(&started.effects);
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: snapshot(1, &["thread-a"]),
        },
    )
    .expect("mailbox snapshot");
    let content = update(loaded.model, UiEvent::Input(UiInput::NextFocus)).expect("content focus");
    let opening = update(content.model, UiEvent::Input(UiInput::Activate)).expect("open thread");
    let (page_id, row_id, cursor) = conversation_effect(&opening.effects);
    assert_eq!(row_id, "thread-a");
    assert_eq!(cursor, None);

    let opened = update(
        opening.model,
        UiEvent::ConversationLoaded {
            effect_id: page_id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries: vec![entry("message-1", false), entry("activity-2", true)],
                next_cursor: Some("next-page".to_owned()),
            },
        },
    )
    .expect("page applies");
    assert_eq!(opened.model.focus(), UiFocus::Conversation);
    assert_eq!(opened.model.conversation_anchor(), Some("message-1"));
    assert_eq!(
        opened
            .model
            .conversation()
            .expect("conversation")
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["message-1", "activity-2"]
    );

    let moved = update(opened.model, UiEvent::Input(UiInput::NextItem)).expect("move anchor");
    assert_eq!(moved.model.conversation_anchor(), Some("activity-2"));
    let technical = update(moved.model, UiEvent::Input(UiInput::Activate)).expect("show details");
    assert!(technical.model.technical_visible());
    let resized = update(
        technical.model,
        UiEvent::Resized(UiSize {
            width: 66,
            height: 17,
        }),
    )
    .expect("resize");
    assert_eq!(resized.model.conversation_anchor(), Some("activity-2"));
    assert!(resized.model.technical_visible());

    let more = update(resized.model, UiEvent::Input(UiInput::LoadMore)).expect("load more");
    let (more_id, more_row, more_cursor) = conversation_effect(&more.effects);
    assert_eq!(more_row, "thread-a");
    assert_eq!(more_cursor, Some("next-page"));
    let appended = update(
        more.model,
        UiEvent::ConversationLoaded {
            effect_id: more_id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries: vec![entry("message-3", false)],
                next_cursor: None,
            },
        },
    )
    .expect("next page appends");
    assert_eq!(
        appended
            .model
            .conversation()
            .expect("conversation")
            .entries
            .len(),
        3
    );
    assert_eq!(appended.model.conversation_anchor(), Some("activity-2"));
}

#[test]
fn invalidation_reloads_an_open_conversation_and_ignores_its_stale_page() {
    let started = started_model();
    let snapshot_id = snapshot_effect(&started.effects);
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: snapshot(1, &["thread-a"]),
        },
    )
    .expect("snapshot");
    let opening = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("open");
    let (first_page_id, _, _) = conversation_effect(&opening.effects);
    let opened = update(
        opening.model,
        UiEvent::ConversationLoaded {
            effect_id: first_page_id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries: vec![entry("message-1", false), entry("message-2", false)],
                next_cursor: None,
            },
        },
    )
    .expect("page");
    let anchored = update(opened.model, UiEvent::Input(UiInput::NextItem)).expect("anchor second");
    let invalidated =
        update(anchored.model, UiEvent::Invalidated { revision: 2 }).expect("invalidate");
    let reload_id = snapshot_effect(&invalidated.effects);
    assert_eq!(invalidated.model.conversation_anchor(), Some("message-2"));

    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: reload_id,
            snapshot: snapshot(2, &["thread-a"]),
        },
    )
    .expect("reload snapshot");
    let (fresh_page_id, _, _) = conversation_effect(&reloaded.effects);
    let stale = update(
        reloaded.model,
        UiEvent::ConversationLoaded {
            effect_id: first_page_id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries: vec![entry("stale", false)],
                next_cursor: None,
            },
        },
    )
    .expect("stale page ignored");
    assert_eq!(stale.model.conversation_anchor(), Some("message-2"));
    let fresh = update(
        stale.model,
        UiEvent::ConversationLoaded {
            effect_id: fresh_page_id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries: vec![entry("message-0", true), entry("message-2", false)],
                next_cursor: None,
            },
        },
    )
    .expect("fresh page applies");
    assert_eq!(fresh.model.conversation_anchor(), Some("message-2"));
}

#[test]
fn reconnect_preserves_the_open_conversation_until_authoritative_repair() {
    let started = started_model();
    let snapshot_id = snapshot_effect(&started.effects);
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: snapshot(1, &["thread-a"]),
        },
    )
    .expect("snapshot");
    let opening = update(loaded.model, UiEvent::Input(UiInput::Activate)).expect("open");
    let (page_id, _, _) = conversation_effect(&opening.effects);
    let opened = update(
        opening.model,
        UiEvent::ConversationLoaded {
            effect_id: page_id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries: vec![entry("message-1", false)],
                next_cursor: None,
            },
        },
    )
    .expect("page");
    let failed = update(
        opened.model,
        UiEvent::ClientFailed {
            generation: 2,
            failure: UiFailure {
                code: "connection_lost".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    )
    .expect("disconnect");
    assert_eq!(failed.model.conversation_anchor(), Some("message-1"));
    let connected = update(
        failed.model,
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Ready,
        },
    )
    .expect("reconnect");
    let repair_id = snapshot_effect(&connected.effects);
    assert_eq!(connected.model.conversation_anchor(), Some("message-1"));
    let repaired = update(
        connected.model,
        UiEvent::SnapshotLoaded {
            effect_id: repair_id,
            snapshot: snapshot(2, &["thread-a"]),
        },
    )
    .expect("authoritative repair");
    assert!(matches!(
        conversation_effect(&repaired.effects),
        (_, "thread-a", None)
    ));
}

fn snapshot(revision: u64, ids: &[&str]) -> UiSnapshot {
    snapshot_for(UiSection::Inbox, revision, ids)
}

fn snapshot_for(section: UiSection, revision: u64, ids: &[&str]) -> UiSnapshot {
    UiSnapshot {
        section,
        revision,
        rows: ids
            .iter()
            .map(|id| UiRow {
                id: (*id).to_owned(),
                title: format!("{id} title"),
                detail: format!("{id} detail"),
                state: UiRowState::Open,
                kind: UiRowKind::Conversation,
            })
            .collect(),
    }
}

fn started_model() -> hq_tui::UiTransition {
    update(
        UiModel::new(UiSize {
            width: 90,
            height: 24,
        }),
        UiEvent::Started,
    )
    .expect("startup transition")
}

fn snapshot_effect(effects: &[UiEffect]) -> hq_tui::EffectId {
    snapshot_effect_with_section(effects).0
}

fn snapshot_effect_with_section(effects: &[UiEffect]) -> (hq_tui::EffectId, UiSection) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, section } => Some((*id, *section)),
            _ => None,
        })
        .expect("snapshot effect")
}

fn redraw_count(effects: &[UiEffect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(effect, UiEffect::RequestRedraw))
        .count()
}

fn timer_effect(effects: &[UiEffect], expected: UiTimerKind) -> hq_tui::EffectId {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::ScheduleTimer { id, kind, .. } if *kind == expected => Some(*id),
            _ => None,
        })
        .expect("timer effect")
}

fn conversation_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &str, Option<&str>) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadConversation { id, row_id, cursor } => {
                Some((*id, row_id.as_str(), cursor.as_deref()))
            }
            _ => None,
        })
        .expect("conversation effect")
}

fn entry(id: &str, activity: bool) -> UiConversationEntry {
    UiConversationEntry {
        id: id.to_owned(),
        kind: if activity {
            UiConversationEntryKind::Activity
        } else {
            UiConversationEntryKind::Message
        },
        content: format!("{id} content"),
        summary: format!("{id} summary"),
        message_state: (!activity).then_some(UiMessageState::Open),
        technical: if activity {
            vec![UiTechnicalSection::Activity {
                sequence: 2,
                status: UiActivityStatus::Running,
                truncated: false,
            }]
        } else {
            Vec::new()
        },
    }
}
