//! Pure TUI transition and stale-effect contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use hq_tui::{
    UiActivityStatus, UiAgent, UiAgentAction, UiAgentLifecycle, UiAgentMailbox, UiAgentModal,
    UiAgentSession, UiConnectionState, UiConversationEntry, UiConversationEntryKind,
    UiConversationPage, UiDirectTarget, UiEffect, UiEvent, UiFailure, UiFocus, UiHumanState,
    UiInput, UiMailboxAction, UiMailboxDraft, UiMailboxDraftTarget, UiMailboxModal,
    UiManagedSessionAction, UiManagedSessionOutcome, UiManagedSessionResult, UiMessageState,
    UiMessageTarget, UiModel, UiProject, UiProjectAction, UiProjectAssignment,
    UiProjectExternalWarning, UiProjectModal, UiProjectOutcome, UiProjectResource,
    UiProjectResourceCheck, UiProjectResourceConflict, UiProjectResult, UiProjectThread, UiRow,
    UiRowKind, UiRowState, UiSection, UiSize, UiSnapshot, UiTechnicalSection, UiTimerKind, update,
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
    let UiEffect::LoadSnapshot { id: snapshot_id } = &transition.effects[0] else {
        panic!("first effect loads a snapshot");
    };
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
fn wide_sidebar_uses_vertical_keys_and_horizontal_keys_only_change_focus() {
    let started = update(
        UiModel::new(UiSize {
            width: 120,
            height: 30,
        }),
        UiEvent::Started,
    )
    .expect("start wide model");
    let request = snapshot_effect(&started.effects);
    let mut source = snapshot(1, &["inbox"]);
    source.sent_rows = snapshot_for(UiSection::Sent, 1, &["sent-a", "sent-b"]).sent_rows;
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: source,
        },
    )
    .expect("load complete snapshot");

    let sent = update(loaded.model, UiEvent::Input(UiInput::NextItem)).expect("down in sidebar");
    assert_eq!(sent.model.section(), UiSection::Sent);
    assert_eq!(sent.model.focus(), UiFocus::Navigation);
    assert_eq!(sent.model.selected_row(), Some("sent-a"));

    let archived =
        update(sent.model, UiEvent::Input(UiInput::Character('j'))).expect("j in sidebar");
    assert_eq!(archived.model.section(), UiSection::Archived);
    let sent =
        update(archived.model, UiEvent::Input(UiInput::Character('k'))).expect("k in sidebar");
    assert_eq!(sent.model.section(), UiSection::Sent);

    let content =
        update(sent.model, UiEvent::Input(UiInput::NextSection)).expect("right focuses content");
    assert_eq!(content.model.section(), UiSection::Sent);
    assert_eq!(content.model.focus(), UiFocus::Content);
    let second =
        update(content.model, UiEvent::Input(UiInput::NextItem)).expect("down moves content row");
    assert_eq!(second.model.selected_row(), Some("sent-b"));
    let navigation = update(second.model, UiEvent::Input(UiInput::PreviousSection))
        .expect("left returns to sidebar");
    assert_eq!(navigation.model.section(), UiSection::Sent);
    assert_eq!(navigation.model.focus(), UiFocus::Navigation);
}

#[test]
fn authoritative_refresh_retains_visible_rows_until_replacement_arrives() {
    let model = loaded_model(snapshot(1, &["retained"]));
    let refreshing =
        update(model, UiEvent::Invalidated { revision: 2 }).expect("start background refresh");
    assert!(refreshing.model.refreshing());
    assert_eq!(refreshing.model.selected_row(), Some("retained"));
    assert_eq!(
        refreshing
            .model
            .rows()
            .and_then(|rows| rows.first())
            .map(|row| row.id.as_str()),
        Some("retained")
    );
    assert!(
        refreshing
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );
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
fn section_change_uses_the_complete_in_flight_snapshot_without_another_request() {
    let started = started_model();
    let snapshot_id = snapshot_effect(&started.effects);
    let sent = update(started.model, UiEvent::Input(UiInput::NextSection))
        .expect("section changes while complete snapshot is pending");
    assert_eq!(sent.model.section(), UiSection::Sent);

    let mut complete = snapshot(4, &["inbox"]);
    complete.sent_rows = snapshot_for(UiSection::Sent, 4, &["sent"]).sent_rows;
    let loaded = update(
        sent.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: complete,
        },
    )
    .expect("complete snapshot applies to selected section");
    assert_eq!(loaded.model.selected_row(), Some("sent"));
    assert!(
        !loaded
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );

    let inbox = update(loaded.model, UiEvent::Input(UiInput::PreviousSection))
        .expect("cached inbox is immediately available");
    assert_eq!(inbox.model.section(), UiSection::Inbox);
    assert_eq!(inbox.model.selected_row(), Some("inbox"));
    assert!(
        !inbox
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );
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

#[test]
fn self_note_draft_autosaves_and_survives_resize_reconnect_and_reload() {
    let loaded = loaded_model(snapshot(1, &["thread-a"]));
    let opening =
        update(loaded, UiEvent::Input(UiInput::Character('n'))).expect("open self-note draft");
    let (open_id, target) = open_draft_effect(&opening.effects);
    assert_eq!(target, &UiMailboxDraftTarget::SelfNote);
    let draft = UiMailboxDraft {
        draft_id: [7; 32],
        target: UiMailboxDraftTarget::SelfNote,
        content: String::new(),
        version: 1,
    };
    let opened = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft,
        },
    )
    .expect("draft loaded");
    let typed = update(
        opened.model,
        UiEvent::Input(UiInput::Paste("remember this".to_owned())),
    )
    .expect("text entered");
    let autosave_id = timer_effect(&typed.effects, UiTimerKind::AutosaveDraft);
    let resized = update(
        typed.model,
        UiEvent::Resized(UiSize {
            width: 61,
            height: 16,
        }),
    )
    .expect("resize preserves editor");
    let reconnecting = update(
        resized.model,
        UiEvent::ConnectionObserved {
            generation: 3,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("reconnect state preserves editor");
    assert!(matches!(
        reconnecting.model.mailbox_modal(),
        Some(UiMailboxModal::Compose { draft, dirty: true, .. })
            if draft.content == "remember this"
    ));
    let saving = update(
        reconnecting.model,
        UiEvent::TimerElapsed {
            effect_id: autosave_id,
        },
    )
    .expect("debounce saves");
    let (save_id, saved_input) = save_draft_effect(&saving.effects);
    assert_eq!(saved_input.content, "remember this");
    let saved = update(
        saving.model,
        UiEvent::DraftSaved {
            effect_id: save_id,
            draft: UiMailboxDraft {
                version: 2,
                ..saved_input.clone()
            },
        },
    )
    .expect("save acknowledged");
    assert!(matches!(
        saved.model.mailbox_modal(),
        Some(UiMailboxModal::Compose { draft, dirty: false, submitting: false, .. })
            if draft.version == 2 && draft.content == "remember this"
    ));
}

#[test]
fn activity_never_becomes_a_reply_or_state_action_target() {
    let opened = opened_conversation(vec![
        actionable_entry("message", [1; 32]),
        entry("activity", true),
    ]);
    let activity = update(opened, UiEvent::Input(UiInput::NextItem)).expect("select activity");
    for shortcut in ['r', 'a', 'u'] {
        let inert = update(
            activity.model.clone(),
            UiEvent::Input(UiInput::Character(shortcut)),
        )
        .expect("activity shortcut is inert");
        assert!(inert.model.mailbox_modal().is_none());
        assert!(inert.effects.is_empty());
    }

    let message =
        update(activity.model, UiEvent::Input(UiInput::PreviousItem)).expect("select message");
    let reply = update(
        message.model.clone(),
        UiEvent::Input(UiInput::Character('r')),
    )
    .expect("reply opens typed target");
    assert!(matches!(
        open_draft_effect(&reply.effects).1,
        UiMailboxDraftTarget::Reply { message_id } if *message_id == [1; 32]
    ));
    let confirm = update(message.model, UiEvent::Input(UiInput::Character('a')))
        .expect("archive requires confirmation");
    assert!(matches!(
        confirm.model.mailbox_modal(),
        Some(UiMailboxModal::Confirm { action: UiMailboxAction::Archive { target_message } })
            if *target_message == [1; 32]
    ));
    let cancelled =
        update(confirm.model, UiEvent::Input(UiInput::Escape)).expect("cancel confirmation");
    assert!(cancelled.model.mailbox_modal().is_none());
    assert!(
        !cancelled
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::SubmitMailboxCommand { .. }))
    );
}

#[test]
fn summary_state_shortcuts_explain_that_an_exact_message_must_be_selected() {
    let loaded = loaded_model(snapshot(1, &["thread-a"]));

    for shortcut in ['a', 'u'] {
        let attempted = update(loaded.clone(), UiEvent::Input(UiInput::Character(shortcut)))
            .expect("summary state shortcut provides guidance");
        assert!(attempted.model.last_failure().is_none());
        assert_eq!(
            attempted.model.mailbox_hint(),
            Some("open the thread with Enter, then select the message to archive or restore")
        );
        assert_eq!(redraw_count(&attempted.effects), 1);
        assert!(attempted.model.mailbox_modal().is_none());

        let dismissed = update(attempted.model, UiEvent::Input(UiInput::NextItem))
            .expect("the next input dismisses transient guidance");
        assert!(dismissed.model.mailbox_hint().is_none());
        assert_eq!(redraw_count(&dismissed.effects), 1);
    }
}

#[test]
fn dirty_reply_saves_before_submit_and_stale_rejection_preserves_text() {
    let opened = opened_conversation(vec![actionable_entry("question", [3; 32])]);
    let opening = update(opened, UiEvent::Input(UiInput::Character('r'))).expect("reply");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let draft = UiMailboxDraft {
        draft_id: [4; 32],
        target: UiMailboxDraftTarget::Reply {
            message_id: [3; 32],
        },
        content: String::new(),
        version: 1,
    };
    let loaded = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft,
        },
    )
    .expect("draft");
    let typed = update(
        loaded.model,
        UiEvent::Input(UiInput::Paste("answer text".to_owned())),
    )
    .expect("type");
    let saving =
        update(typed.model, UiEvent::Input(UiInput::Activate)).expect("submit waits for save");
    let (save_id, save_input) = save_draft_effect(&saving.effects);
    assert!(
        !saving
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::SubmitMailboxCommand { .. }))
    );
    let submitting = update(
        saving.model,
        UiEvent::DraftSaved {
            effect_id: save_id,
            draft: UiMailboxDraft {
                version: 2,
                ..save_input.clone()
            },
        },
    )
    .expect("saved draft submits");
    let command_id = submitting
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitMailboxCommand {
                id,
                draft: Some(draft),
                action: UiMailboxAction::Reply { target_message },
            } if draft.content == "answer text" && *target_message == [3; 32] => Some(*id),
            _ => None,
        })
        .expect("typed reply command");
    let rejected = update(
        submitting.model,
        UiEvent::MailboxCommandFailed {
            effect_id: command_id,
            failure: UiFailure {
                code: "mailbox_target_stale".to_owned(),
                action: "reselect the target; the draft text is preserved".to_owned(),
            },
        },
    )
    .expect("stale rejection");
    assert!(matches!(
        rejected.model.mailbox_modal(),
        Some(UiMailboxModal::Compose { draft, submitting: false, .. })
            if draft.content == "answer text"
    ));
    assert_eq!(
        rejected
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("mailbox_target_stale")
    );
}

#[test]
fn direct_target_reselection_survives_authoritative_reorder() {
    let mut initial = snapshot(1, &["thread-a"]);
    initial.direct_targets = vec![direct_target("alpha", 1), direct_target("beta", 2)];
    let loaded = loaded_model(initial);
    let selecting =
        update(loaded, UiEvent::Input(UiInput::Character('d'))).expect("open target selector");
    let selected = update(selecting.model, UiEvent::Input(UiInput::NextItem)).expect("choose beta");
    let invalidated = update(selected.model, UiEvent::Invalidated { revision: 2 })
        .expect("reload while selecting");
    let snapshot_id = snapshot_effect(&invalidated.effects);
    let mut reordered = snapshot(2, &["thread-a"]);
    reordered.direct_targets = vec![direct_target("beta", 2), direct_target("alpha", 1)];
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: reordered,
        },
    )
    .expect("stable mailbox remains selected");
    assert!(matches!(
        reloaded.model.mailbox_modal(),
        Some(UiMailboxModal::SelectDirect { selected: Some((installation, mailbox)), .. })
            if *installation == [2; 32] && *mailbox == [12; 32]
    ));
    let opening = update(reloaded.model, UiEvent::Input(UiInput::Activate))
        .expect("open selected target draft");
    assert!(matches!(
        open_draft_effect(&opening.effects).1,
        UiMailboxDraftTarget::Direct { installation_id, mailbox_id }
            if *installation_id == [2; 32] && *mailbox_id == [12; 32]
    ));
}

#[test]
fn direct_archive_and_restore_emit_only_their_typed_commands() {
    let mut source = snapshot(1, &["thread-a"]);
    source.direct_targets = vec![direct_target("builder", 5)];
    let loaded = loaded_model(source);
    let selecting = update(loaded, UiEvent::Input(UiInput::Character('d'))).expect("direct");
    let opening = update(selecting.model, UiEvent::Input(UiInput::Activate)).expect("target");
    let (open_id, target) = open_draft_effect(&opening.effects);
    let draft = UiMailboxDraft {
        draft_id: [6; 32],
        target: target.clone(),
        content: "direct content".to_owned(),
        version: 4,
    };
    let composing = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: draft.clone(),
        },
    )
    .expect("direct draft");
    let direct = update(composing.model, UiEvent::Input(UiInput::Activate)).expect("direct submit");
    assert!(direct.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitMailboxCommand {
            draft: Some(submitted),
            action: UiMailboxAction::Direct {
                recipient_installation,
                recipient_mailbox,
            },
            ..
        } if submitted == &draft
            && *recipient_installation == [5; 32]
            && *recipient_mailbox == [15; 32]
    )));

    let open_message = opened_conversation(vec![actionable_entry("open", [8; 32])]);
    let archive_confirm =
        update(open_message, UiEvent::Input(UiInput::Character('a'))).expect("archive confirm");
    let archive =
        update(archive_confirm.model, UiEvent::Input(UiInput::Activate)).expect("archive submit");
    assert!(archive.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitMailboxCommand {
            draft: None,
            action: UiMailboxAction::Archive { target_message },
            ..
        } if *target_message == [8; 32]
    )));

    let mut archived = actionable_entry("archived", [9; 32]);
    archived.message_state = Some(UiMessageState::Archived);
    let archived_message = opened_conversation(vec![archived]);
    let restore_confirm =
        update(archived_message, UiEvent::Input(UiInput::Character('u'))).expect("restore confirm");
    let restore =
        update(restore_confirm.model, UiEvent::Input(UiInput::Activate)).expect("restore submit");
    assert!(restore.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitMailboxCommand {
            draft: None,
            action: UiMailboxAction::Restore { target_message },
            ..
        } if *target_message == [9; 32]
    )));
}

#[test]
fn escape_during_in_flight_autosave_waits_for_latest_text_before_closing() {
    let loaded = loaded_model(snapshot(1, &[]));
    let opening = update(loaded, UiEvent::Input(UiInput::Character('n'))).expect("note");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let opened = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [10; 32],
                target: UiMailboxDraftTarget::SelfNote,
                content: String::new(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let first = update(opened.model, UiEvent::Input(UiInput::Character('a'))).expect("first edit");
    let timer = timer_effect(&first.effects, UiTimerKind::AutosaveDraft);
    let saving =
        update(first.model, UiEvent::TimerElapsed { effect_id: timer }).expect("first save starts");
    let (save_id, first_input) = save_draft_effect(&saving.effects);
    let newer = update(saving.model, UiEvent::Input(UiInput::Character('b')))
        .expect("edit while save is in flight");
    let closing =
        update(newer.model, UiEvent::Input(UiInput::Escape)).expect("close waits for latest save");
    assert!(matches!(
        closing.model.mailbox_modal(),
        Some(UiMailboxModal::Compose { draft, closing: true, .. }) if draft.content == "ab"
    ));
    let follow_up = update(
        closing.model,
        UiEvent::DraftSaved {
            effect_id: save_id,
            draft: UiMailboxDraft {
                version: 2,
                ..first_input.clone()
            },
        },
    )
    .expect("old save triggers latest save");
    let (latest_id, latest) = save_draft_effect(&follow_up.effects);
    assert_eq!(latest.content, "ab");
    assert_eq!(latest.version, 2);
    let closed = update(
        follow_up.model,
        UiEvent::DraftSaved {
            effect_id: latest_id,
            draft: UiMailboxDraft {
                version: 3,
                ..latest.clone()
            },
        },
    )
    .expect("latest save closes");
    assert!(closed.model.mailbox_modal().is_none());
}

#[test]
fn optimistic_draft_conflict_preserves_local_text_and_adopts_current_version() {
    let loaded = loaded_model(snapshot(1, &[]));
    let opening = update(loaded, UiEvent::Input(UiInput::Character('n'))).expect("note");
    let (open_id, _) = open_draft_effect(&opening.effects);
    let opened = update(
        opening.model,
        UiEvent::DraftLoaded {
            effect_id: open_id,
            draft: UiMailboxDraft {
                draft_id: [11; 32],
                target: UiMailboxDraftTarget::SelfNote,
                content: "local".to_owned(),
                version: 1,
            },
        },
    )
    .expect("draft");
    let edited = update(opened.model, UiEvent::Input(UiInput::Character('!'))).expect("edit");
    let timer = timer_effect(&edited.effects, UiTimerKind::AutosaveDraft);
    let saving =
        update(edited.model, UiEvent::TimerElapsed { effect_id: timer }).expect("save starts");
    let (save_id, _) = save_draft_effect(&saving.effects);
    let conflicted = update(
        saving.model,
        UiEvent::DraftFailed {
            effect_id: save_id,
            failure: UiFailure {
                code: "draft_conflict".to_owned(),
                action: "edit the preserved text and retry against the current draft".to_owned(),
            },
            current: Some(UiMailboxDraft {
                draft_id: [11; 32],
                target: UiMailboxDraftTarget::SelfNote,
                content: "other writer".to_owned(),
                version: 7,
            }),
        },
    )
    .expect("conflict remains actionable");
    assert!(matches!(
        conflicted.model.mailbox_modal(),
        Some(UiMailboxModal::Compose { draft, dirty: true, submitting: false, closing: false })
            if draft.content == "local!" && draft.version == 7
    ));
    assert_eq!(
        conflicted
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("draft_conflict")
    );
}

#[test]
fn agent_search_and_details_keep_stable_identity_across_reload_reconnect_and_resize() {
    let model = loaded_agents_model(1, &[agent(1, "alpha"), agent(2, "beta")]);
    let searching = update(model, UiEvent::Input(UiInput::Character('/'))).expect("search");
    let matched = update(
        searching.model,
        UiEvent::Input(UiInput::Paste("beta".to_owned())),
    )
    .expect("search query");
    assert_eq!(matched.model.selected_row(), Some(agent_row_id(2).as_str()));
    let invalidated = update(matched.model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let request = snapshot_effect(&invalidated.effects);
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: agents_snapshot(2, vec![agent(2, "beta"), agent(1, "alpha")]),
        },
    )
    .expect("authoritative reorder");
    assert_eq!(
        reloaded.model.selected_row(),
        Some(agent_row_id(2).as_str())
    );
    assert!(matches!(
        reloaded.model.agent_modal(),
        Some(UiAgentModal::Search { query }) if query == "beta"
    ));
    let details = update(reloaded.model, UiEvent::Input(UiInput::Activate)).expect("inspect");
    let resized = update(
        details.model,
        UiEvent::Resized(UiSize {
            width: 62,
            height: 17,
        }),
    )
    .expect("resize");
    let reconnecting = update(
        resized.model,
        UiEvent::ConnectionObserved {
            generation: 4,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("reconnect");
    assert!(matches!(
        reconnecting.model.agent_modal(),
        Some(UiAgentModal::Details { agent, .. }) if agent.agent_id == [2; 32]
    ));
}

#[test]
fn agent_create_and_session_rename_emit_exact_typed_commands_and_preserve_failures() {
    let model = loaded_agents_model(1, &[agent(3, "builder")]);
    let create = update(model, UiEvent::Input(UiInput::Character('c'))).expect("create");
    let named = update(
        create.model,
        UiEvent::Input(UiInput::Paste("reviewer".to_owned())),
    )
    .expect("name");
    let submitted = update(named.model, UiEvent::Input(UiInput::Activate)).expect("submit");
    let (create_id, create_action) = agent_action_effect(&submitted.effects);
    assert_eq!(
        create_action,
        &UiAgentAction::Create {
            name: "reviewer".to_owned()
        }
    );
    let failed = update(
        submitted.model,
        UiEvent::AgentCommandFailed {
            effect_id: create_id,
            failure: UiFailure {
                code: "agent_command_failed".to_owned(),
                action: "correct the name and retry".to_owned(),
            },
        },
    )
    .expect("failure");
    assert!(matches!(
        failed.model.agent_modal(),
        Some(UiAgentModal::Create { name, submitting: false }) if name == "reviewer"
    ));

    let details_model = loaded_agents_model(1, &[agent(3, "builder")]);
    let details = update(details_model, UiEvent::Input(UiInput::Activate)).expect("details");
    let rename = update(details.model, UiEvent::Input(UiInput::Character('r'))).expect("rename");
    let cleared = update(rename.model, UiEvent::Input(UiInput::Backspace)).expect("clear old name");
    let renamed = update(
        cleared.model,
        UiEvent::Input(UiInput::Paste("live".to_owned())),
    )
    .expect("new name");
    let submitted =
        update(renamed.model, UiEvent::Input(UiInput::Activate)).expect("rename submit");
    assert!(matches!(
        agent_action_effect(&submitted.effects).1,
        UiAgentAction::RenameSession { agent_id, provider, session, display_name: Some(name) }
            if *agent_id == [3; 32] && provider == "codex" && session == "session-3" && name == "live"
    ));
}

#[test]
fn retirement_is_explicit_cancelable_and_force_is_part_of_the_typed_command() {
    let model = loaded_agents_model(1, &[agent(4, "worker")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let confirm = update(details.model, UiEvent::Input(UiInput::Character('x'))).expect("confirm");
    let cancelled = update(confirm.model, UiEvent::Input(UiInput::Escape)).expect("cancel");
    assert!(cancelled.model.agent_modal().is_none());
    assert!(
        !cancelled
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::SubmitAgentCommand { .. }))
    );

    let details =
        update(cancelled.model, UiEvent::Input(UiInput::Activate)).expect("details again");
    let confirm = update(details.model, UiEvent::Input(UiInput::Character('x'))).expect("confirm");
    let forced = update(confirm.model, UiEvent::Input(UiInput::Character('f'))).expect("force");
    let submitted = update(forced.model, UiEvent::Input(UiInput::Activate)).expect("retire");
    assert!(matches!(
        agent_action_effect(&submitted.effects).1,
        UiAgentAction::Retire { agent_id, force: true } if *agent_id == [4; 32]
    ));
}

#[test]
fn managed_session_start_confirms_switch_and_exact_resume_and_stop_emit_typed_commands() {
    let model = loaded_agents_model(1, &[agent(5, "runtime")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let provider =
        update(details.model, UiEvent::Input(UiInput::Character('s'))).expect("start provider");
    assert!(matches!(
        provider.model.agent_modal(),
        Some(UiAgentModal::ManagedProvider { provider, .. }) if provider == "codex"
    ));
    let confirm = update(provider.model, UiEvent::Input(UiInput::Activate)).expect("switch gate");
    assert!(matches!(
        confirm.model.agent_modal(),
        Some(UiAgentModal::ConfirmManagedSession {
            action: UiManagedSessionAction::Start { agent_id, provider }, ..
        }) if *agent_id == [5; 32] && provider == "codex"
    ));
    let started = update(confirm.model, UiEvent::Input(UiInput::Activate)).expect("start");
    assert!(matches!(
        managed_session_effect(&started.effects).1,
        UiManagedSessionAction::Start { agent_id, provider }
            if *agent_id == [5; 32] && provider == "codex"
    ));

    let model = loaded_agents_model(1, &[agent(5, "runtime")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let resumed =
        update(details.model, UiEvent::Input(UiInput::Character('e'))).expect("exact resume");
    assert!(matches!(
        managed_session_effect(&resumed.effects).1,
        UiManagedSessionAction::Resume { agent_id, provider, session }
            if *agent_id == [5; 32] && provider == "codex" && session == "session-5"
    ));

    let model = loaded_agents_model(1, &[agent(5, "runtime")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let stopped = update(details.model, UiEvent::Input(UiInput::Character('t'))).expect("stop");
    assert!(matches!(
        managed_session_effect(&stopped.effects).1,
        UiManagedSessionAction::Stop { agent_id, provider }
            if *agent_id == [5; 32] && provider == "codex"
    ));
}

#[test]
fn managed_session_switch_cancel_stale_completion_and_actionable_outcomes_are_explicit() {
    let mut target = agent(6, "switcher");
    target.sessions.push(UiAgentSession {
        provider: "codex".to_owned(),
        session: "older-session".to_owned(),
        mailbox: None,
        conflicted: false,
        selected: false,
        name_resolved: true,
        display_name: Some("older".to_owned()),
    });
    let model = loaded_agents_model(1, &[target]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let older = update(details.model, UiEvent::Input(UiInput::NextItem)).expect("older");
    let confirm = update(older.model, UiEvent::Input(UiInput::Character('e'))).expect("confirm");
    assert!(matches!(
        confirm.model.agent_modal(),
        Some(UiAgentModal::ConfirmManagedSession { .. })
    ));
    let cancelled = update(confirm.model, UiEvent::Input(UiInput::Escape)).expect("cancel");
    assert!(cancelled.model.agent_modal().is_none());

    let model = loaded_agents_model(1, &[agent(6, "switcher")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let pending = update(details.model, UiEvent::Input(UiInput::Character('e'))).expect("resume");
    let (effect_id, action) = managed_session_effect(&pending.effects);
    let reconnecting = update(
        pending.model,
        UiEvent::ConnectionObserved {
            generation: 7,
            state: UiConnectionState::Reconnecting,
        },
    )
    .expect("reconnect while managed operation is pending");
    let pending = update(
        reconnecting.model,
        UiEvent::Resized(UiSize {
            width: 61,
            height: 17,
        }),
    )
    .expect("resize while managed operation is pending");
    assert_eq!(pending.model.pending_managed_session(), Some(effect_id));
    assert!(matches!(
        pending.model.agent_modal(),
        Some(UiAgentModal::ManagingSession { .. })
    ));
    let stale_id = snapshot_effect(&started_model().effects);
    assert_ne!(stale_id, effect_id);
    let stale = update(
        pending.model.clone(),
        UiEvent::ManagedSessionCompleted {
            effect_id: stale_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [7; 32],
                outcome: UiManagedSessionOutcome::Stopped,
            },
        },
    )
    .expect("stale completion");
    assert_eq!(stale.model, pending.model);

    let rejected = update(
        pending.model,
        UiEvent::ManagedSessionCompleted {
            effect_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [8; 32],
                outcome: UiManagedSessionOutcome::Rejected {
                    category: "domain".to_owned(),
                    code: "managed_session_precondition".to_owned(),
                },
            },
        },
    )
    .expect("rejected");
    assert!(matches!(
        rejected.model.agent_modal(),
        Some(UiAgentModal::ManagedSessionOutcome {
            result: UiManagedSessionResult {
                outcome: UiManagedSessionOutcome::Rejected { code, .. }, ..
            }, ..
        }) if code == "managed_session_precondition"
    ));
    assert_eq!(
        rejected
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("managed_session_precondition")
    );
}

#[test]
fn managed_session_uncertainty_retains_operation_and_reconciliation_identity() {
    let model = loaded_agents_model(1, &[agent(6, "switcher")]);
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("details");
    let pending = update(details.model, UiEvent::Input(UiInput::Character('t'))).expect("stop");
    let (effect_id, action) = managed_session_effect(&pending.effects);
    let uncertain = update(
        pending.model,
        UiEvent::ManagedSessionCompleted {
            effect_id,
            result: UiManagedSessionResult {
                action: action.clone(),
                operation_id: [10; 32],
                outcome: UiManagedSessionOutcome::Uncertain {
                    reconciliation_id: [11; 32],
                },
            },
        },
    )
    .expect("uncertain");
    assert!(matches!(
        uncertain.model.agent_modal(),
        Some(UiAgentModal::ManagedSessionOutcome {
            result: UiManagedSessionResult {
                operation_id,
                outcome: UiManagedSessionOutcome::Uncertain {
                    reconciliation_id
                },
                ..
            },
            ..
        }) if *operation_id == [10; 32] && *reconciliation_id == [11; 32]
    ));
}

#[test]
fn mailbox_navigation_workspace_survives_visiting_agent_session_management() {
    let mut source = snapshot(1, &["thread-a"]);
    let agent_source = agents_snapshot(1, vec![agent(9, "runtime")]);
    source.agent_rows = agent_source.agent_rows;
    source.agents = agent_source.agents;
    let model = loaded_model(source);
    let opening = update(model, UiEvent::Input(UiInput::Activate)).expect("open conversation");
    let (effect_id, _, _) = conversation_effect(&opening.effects);
    let mut model = update(
        opening.model,
        UiEvent::ConversationLoaded {
            effect_id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries: vec![entry("message-a", false)],
                next_cursor: None,
            },
        },
    )
    .expect("conversation loaded")
    .model;
    assert_eq!(model.conversation_anchor(), Some("message-a"));
    for _ in 0..3 {
        model = update(model, UiEvent::Input(UiInput::Character('l')))
            .expect("next cached section")
            .model;
    }
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("agent details");
    model = update(details.model, UiEvent::Input(UiInput::Escape))
        .expect("close details")
        .model;
    for _ in 0..3 {
        model = update(model, UiEvent::Input(UiInput::Character('h')))
            .expect("previous cached section")
            .model;
    }
    assert_eq!(model.selected_row(), Some("thread-a"));
    assert_eq!(model.conversation_anchor(), Some("message-a"));
    assert!(model.conversation().is_some());
}

#[test]
fn project_search_and_details_preserve_stable_identity_across_reload_and_resize() {
    let alpha = project(1, "alpha", "/work/alpha");
    let beta = project(2, "beta", "/work/beta");
    let mut model = loaded_projects_model(4, vec![alpha, beta.clone()]);

    model = update(model, UiEvent::Input(UiInput::Character('/')))
        .expect("open search")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("beta".to_owned())))
        .expect("search")
        .model;
    assert_eq!(model.selected_row(), Some(agent_row_id(2).as_str()));
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("inspect")
        .model;
    let resized = update(
        model,
        UiEvent::Resized(UiSize {
            width: 73,
            height: 18,
        }),
    )
    .expect("resize");
    let invalidated = update(resized.model, UiEvent::Invalidated { revision: 5 }).expect("reload");
    let request = snapshot_effect(&invalidated.effects);
    let mut current = beta;
    current.name = "beta current".to_owned();
    let loaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: projects_snapshot(5, vec![current, project(1, "alpha", "/work/alpha")]),
        },
    )
    .expect("authoritative reorder");
    assert_eq!(loaded.model.selected_row(), Some(agent_row_id(2).as_str()));
    assert!(matches!(
        loaded.model.project_modal(),
        Some(UiProjectModal::Details { project, .. }) if project.name == "beta current"
    ));
}

#[test]
fn both_project_creation_modes_emit_exact_typed_commands_and_cancel_without_effects() {
    let mut model = loaded_projects_model(1, Vec::new());
    model = update(model, UiEvent::Input(UiInput::Character('c')))
        .expect("existing form")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("existing".to_owned())))
        .expect("name")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("brief")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("brief".to_owned())))
        .expect("brief text")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("path")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("/repo".to_owned())))
        .expect("path text")
        .model;
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit existing");
    let (existing_id, existing_action) = project_effect(&submitted.effects);
    assert_eq!(
        existing_action,
        UiProjectAction::CreateExisting {
            name: "existing".to_owned(),
            brief: Some("brief".to_owned()),
            path: "/repo".to_owned(),
        }
    );
    let failed = update(
        submitted.model,
        UiEvent::ProjectCommandFailed {
            effect_id: existing_id,
            failure: UiFailure {
                code: "path_changed".to_owned(),
                action: "inspect the current working tree".to_owned(),
            },
        },
    )
    .expect("recoverable failure");
    assert!(matches!(
        failed.model.project_modal(),
        Some(UiProjectModal::CreateExisting { path, submitting: false, .. }) if path == "/repo"
    ));
    let cancelled = update(failed.model, UiEvent::Input(UiInput::Escape)).expect("cancel");
    assert!(cancelled.model.project_modal().is_none());
    assert!(
        cancelled
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );

    let mut model = update(cancelled.model, UiEvent::Input(UiInput::Character('w')))
        .expect("worktree form")
        .model;
    for (index, value) in ["worktree", "", "/source", "/destination", "feature", "main"]
        .into_iter()
        .enumerate()
    {
        if !value.is_empty() {
            model = update(model, UiEvent::Input(UiInput::Paste(value.to_owned())))
                .expect("worktree field")
                .model;
        }
        if index < 5 {
            model = update(model, UiEvent::Input(UiInput::NextItem))
                .expect("next worktree field")
                .model;
        }
    }
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit worktree");
    let (_, action) = project_effect(&submitted.effects);
    assert_eq!(
        action,
        UiProjectAction::CreateWorktree {
            name: "worktree".to_owned(),
            brief: None,
            source: "/source".to_owned(),
            destination: "/destination".to_owned(),
            branch: "feature".to_owned(),
            base: Some("main".to_owned()),
        }
    );
}

#[test]
fn project_input_retains_text_on_failure_and_exposes_reconcilable_external_state() {
    let target = project(7, "target", "/target");
    let mut model = loaded_projects_model(1, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('n')))
        .expect("input form")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("ship it".to_owned())))
        .expect("input")
        .model;
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit input");
    let (first_id, action) = project_effect(&submitted.effects);
    let failed = update(
        submitted.model,
        UiEvent::ProjectCommandFailed {
            effect_id: first_id,
            failure: UiFailure {
                code: "disconnected".to_owned(),
                action: "retry the same input".to_owned(),
            },
        },
    )
    .expect("failure");
    assert!(matches!(
        failed.model.project_modal(),
        Some(UiProjectModal::SendInput { content, submitting: false, .. }) if content == "ship it"
    ));
    let retried = update(failed.model, UiEvent::Input(UiInput::Activate)).expect("retry");
    let (second_id, second_action) = project_effect(&retried.effects);
    assert_eq!(action, second_action);
    let result = UiProjectResult {
        action: second_action,
        command_id: [3; 32],
        operation_id: [4; 32],
        project_id: target.project_id,
        runtime_state: Some("uncertain".to_owned()),
        runtime_code: Some("response_lost".to_owned()),
        outcome: UiProjectOutcome::Reconcilable {
            stage: "worktree_created".to_owned(),
            category: "external_state".to_owned(),
            code: "response_lost".to_owned(),
            warning: Some(UiProjectExternalWarning {
                kind: "retained_worktree".to_owned(),
                destination: "/target".to_owned(),
                branch: "feature".to_owned(),
            }),
        },
    };
    let completed = update(
        retried.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: second_id,
            result: result.clone(),
        },
    )
    .expect("typed outcome");
    assert!(matches!(
        completed.model.project_modal(),
        Some(UiProjectModal::Outcome { result: actual }) if actual == &result
    ));
}

#[test]
fn project_progress_stale_rejection_and_mismatched_responses_remain_typed() {
    let target = project(8, "target", "/target");
    let submit_input = |content: &str| {
        let mut model = loaded_projects_model(1, vec![target.clone()]);
        model = update(model, UiEvent::Input(UiInput::Activate))
            .expect("details")
            .model;
        model = update(model, UiEvent::Input(UiInput::Character('n')))
            .expect("input form")
            .model;
        model = update(model, UiEvent::Input(UiInput::Paste(content.to_owned())))
            .expect("input")
            .model;
        update(model, UiEvent::Input(UiInput::Activate)).expect("submit input")
    };

    let running = submit_input("running");
    let (effect_id, action) = project_effect(&running.effects);
    let progress = update(
        running.model,
        UiEvent::ProjectCommandCompleted {
            effect_id,
            result: UiProjectResult {
                action,
                command_id: [1; 32],
                operation_id: [2; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::Running {
                    stage: "git_identified".to_owned(),
                },
            },
        },
    )
    .expect("progress");
    assert!(matches!(
        progress.model.project_modal(),
        Some(UiProjectModal::Outcome { result })
            if result.outcome == UiProjectOutcome::Running { stage: "git_identified".to_owned() }
    ));

    let rejected = submit_input("stale");
    let (effect_id, action) = project_effect(&rejected.effects);
    let rejected = update(
        rejected.model,
        UiEvent::ProjectCommandCompleted {
            effect_id,
            result: UiProjectResult {
                action,
                command_id: [3; 32],
                operation_id: [4; 32],
                project_id: target.project_id,
                runtime_state: Some("failed".to_owned()),
                runtime_code: Some("stale_project_head".to_owned()),
                outcome: UiProjectOutcome::Rejected {
                    category: "conflict".to_owned(),
                    code: "stale_project_head".to_owned(),
                },
            },
        },
    )
    .expect("rejected");
    assert_eq!(
        rejected
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("stale_project_head")
    );

    let mismatched = submit_input("expected");
    let (effect_id, _) = project_effect(&mismatched.effects);
    let mismatched = update(
        mismatched.model,
        UiEvent::ProjectCommandCompleted {
            effect_id,
            result: UiProjectResult {
                action: UiProjectAction::SendInput {
                    project_id: target.project_id,
                    content: "different".to_owned(),
                },
                command_id: [5; 32],
                operation_id: [6; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::InputSent {
                    message_id: [7; 32],
                },
            },
        },
    )
    .expect("mismatch rejected");
    assert_eq!(
        mismatched
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("project_response_mismatch")
    );
}

#[test]
fn resource_add_previews_authoritative_conflicts_before_mutation() {
    let target = project(9, "target", "/target");
    let mut model = loaded_projects_model(1, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('a')))
        .expect("add form")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("/shared".to_owned())))
        .expect("path")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("primary toggle")
        .model;
    let previewing = update(model, UiEvent::Input(UiInput::Activate)).expect("preview");
    let (preview_id, preview_action) = project_effect(&previewing.effects);
    assert_eq!(
        preview_action,
        UiProjectAction::PreviewAddResource {
            project_id: target.project_id,
            path: "/shared".to_owned(),
            make_primary: true,
        }
    );
    let preview = update(
        previewing.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: preview_id,
            result: UiProjectResult {
                action: preview_action,
                command_id: [4; 32],
                operation_id: [5; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::ResourcePreview {
                    display_path: "/shared".to_owned(),
                    canonical_path: "/canonical/shared".to_owned(),
                    conflicts: vec![UiProjectResourceConflict {
                        project_id: [2; 32],
                        resource_id: [3; 32],
                        display_path: "/other".to_owned(),
                        canonical_path: "/canonical".to_owned(),
                        relationship: "descendant".to_owned(),
                    }],
                },
            },
        },
    )
    .expect("preview result");
    let blocked = update(preview.model, UiEvent::Input(UiInput::Activate)).expect("blocked");
    assert!(
        blocked
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    assert_eq!(
        blocked
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("project_resource_claim_conflict")
    );
}

#[test]
fn resource_edits_force_gate_selection_and_fresh_checks_use_exact_identities() {
    let mut target = project(10, "assigned", "/first");
    target.assignment = Some(UiProjectAssignment {
        assignment_id: [40; 32],
        agent_id: [41; 32],
        provider: "codex".to_owned(),
        session: Some("session".to_owned()),
        phase: "runnable".to_owned(),
        thread_id: Some([42; 32]),
        launch_directory: Some("/first".to_owned()),
        blocked: None,
        cardinality_conflicted: false,
        runnable: true,
    });
    target.resources.push(UiProjectResource {
        resource_id: [22; 32],
        display_path: "/second".to_owned(),
        canonical_path: "/second".to_owned(),
        health: "unknown".to_owned(),
        primary: false,
        active_claim: true,
        conflicting_projects: Vec::new(),
    });
    let mut model = loaded_projects_model(1, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("select second resource")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('x')))
        .expect("remove confirmation")
        .model;
    let gated = update(model, UiEvent::Input(UiInput::Activate)).expect("force gate");
    assert_eq!(
        gated
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("project_resource_remove_force_required")
    );
    let forced = update(gated.model, UiEvent::Input(UiInput::Character('f')))
        .expect("force toggle")
        .model;
    let removing = update(forced, UiEvent::Input(UiInput::Activate)).expect("remove");
    let (_, action) = project_effect(&removing.effects);
    assert_eq!(
        action,
        UiProjectAction::RemoveResource {
            project_id: target.project_id,
            resource_id: [22; 32],
            force: true,
        }
    );

    let mut model = loaded_projects_model(2, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    let checking = update(model, UiEvent::Input(UiInput::Character('k'))).expect("exact check");
    let (check_id, check_action) = project_effect(&checking.effects);
    assert_eq!(
        check_action,
        UiProjectAction::CheckResources {
            project_id: target.project_id,
            resource_id: Some(target.resources[0].resource_id),
        }
    );
    let checked = update(
        checking.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: check_id,
            result: UiProjectResult {
                action: check_action,
                command_id: [7; 32],
                operation_id: [8; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::ResourceChecks {
                    checks: vec![UiProjectResourceCheck {
                        resource_id: target.resources[0].resource_id,
                        status: "accepted".to_owned(),
                        health: Some("healthy".to_owned()),
                        release: Some("clean".to_owned()),
                        observed_canonical_path: Some("/first".to_owned()),
                        details: None,
                        error_category: None,
                        error_code: None,
                        reconciliation_id: None,
                    }],
                },
            },
        },
    )
    .expect("fresh check");
    assert!(matches!(
        checked.model.project_modal(),
        Some(UiProjectModal::Outcome { result })
            if matches!(result.outcome, UiProjectOutcome::ResourceChecks { .. })
    ));
}

#[test]
fn project_activation_uses_exact_project_thread_and_retained_directory() {
    let mut target = project(50, "activate", "/workspace/activate");
    target.threads.push(UiProjectThread {
        agent_id: [60; 32],
        provider: "codex".to_owned(),
        session: "durable-session".to_owned(),
        thread_id: [61; 32],
    });
    let mut model = loaded_projects_model_with_agents(
        1,
        vec![target.clone()],
        vec![project_agent(60, target.home)],
    );
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('v')))
        .expect("activation")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("mode field")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("exact mode")
        .model;
    let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit");
    assert!(submitted.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::Activate {
                project_id,
                agent_id,
                provider,
                resume_session: Some(session),
                resume_thread: Some(thread),
                launch_directory,
            },
            ..
        } if *project_id == target.project_id
            && *agent_id == [60; 32]
            && provider == "codex"
            && session == "durable-session"
            && *thread == [61; 32]
            && launch_directory == "/workspace/activate"
    )));
}

#[test]
fn activation_target_and_edited_fields_survive_authoritative_reload() {
    let mut target = project(51, "retained activation", "/workspace/retained");
    target.threads.push(UiProjectThread {
        agent_id: [62; 32],
        provider: "codex".to_owned(),
        session: "retained-session".to_owned(),
        thread_id: [63; 32],
    });
    let agent = project_agent(62, target.home);
    let mut model = loaded_projects_model_with_agents(1, vec![target.clone()], vec![agent.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('v')))
        .expect("activation")
        .model;
    for _ in 0..3 {
        model = update(model, UiEvent::Input(UiInput::NextFocus))
            .expect("provider field")
            .model;
    }
    model = update(model, UiEvent::Input(UiInput::Paste("-edited".to_owned())))
        .expect("provider edit")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("directory field")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("/child".to_owned())))
        .expect("directory edit")
        .model;
    let invalidated = update(model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let effect_id = snapshot_effect(&invalidated.effects);
    let mut snapshot = projects_snapshot(2, vec![target]);
    snapshot.agents = vec![agent];
    let reloaded = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id,
            snapshot,
        },
    )
    .expect("authoritative reload")
    .model;
    let Some(UiProjectModal::Activate {
        agent_id,
        thread,
        provider,
        directory,
        ..
    }) = reloaded.project_modal()
    else {
        panic!("activation remains open");
    };
    assert_eq!(*agent_id, Some([62; 32]));
    assert_eq!(thread.as_ref().map(|value| value.thread_id), Some([63; 32]));
    assert_eq!(provider, "codex-edited");
    assert_eq!(directory, "/workspace/retained/child");
}

#[test]
fn handoff_requires_confirmation_and_keeps_force_separate() {
    let mut target = project(70, "handoff", "/workspace/handoff");
    target.assignment = Some(UiProjectAssignment {
        assignment_id: [71; 32],
        agent_id: [72; 32],
        provider: "codex".to_owned(),
        session: Some("old-session".to_owned()),
        phase: "blocked".to_owned(),
        thread_id: Some([73; 32]),
        launch_directory: Some("/workspace/handoff".to_owned()),
        blocked: Some("runtime_stop_uncertain".to_owned()),
        cardinality_conflicted: false,
        runnable: false,
    });
    target.threads.push(UiProjectThread {
        agent_id: [80; 32],
        provider: "codex".to_owned(),
        session: "target-session".to_owned(),
        thread_id: [81; 32],
    });
    let mut model = loaded_projects_model_with_agents(
        1,
        vec![target.clone()],
        vec![
            project_agent(72, target.home),
            project_agent(80, target.home),
        ],
    );
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('h')))
        .expect("handoff")
        .model;
    let blocked = update(model, UiEvent::Input(UiInput::Activate)).expect("confirmation gate");
    assert!(
        blocked
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    let mut confirmed = blocked.model;
    for _ in 0..5 {
        confirmed = update(confirmed, UiEvent::Input(UiInput::NextFocus))
            .expect("confirmation field")
            .model;
    }
    confirmed = update(confirmed, UiEvent::Input(UiInput::NextItem))
        .expect("confirm")
        .model;
    let force_field = update(confirmed, UiEvent::Input(UiInput::NextFocus))
        .expect("force field")
        .model;
    let forced = update(force_field, UiEvent::Input(UiInput::NextItem))
        .expect("force")
        .model;
    let submitted = update(forced, UiEvent::Input(UiInput::Activate)).expect("submit");
    assert!(submitted.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::Handoff {
                project_id,
                agent_id,
                thread_id,
                force_takeover: true,
                ..
            },
            ..
        } if *project_id == target.project_id && *agent_id == [80; 32] && *thread_id == [81; 32]
    )));
}

#[test]
fn project_dispatch_submits_the_exact_selected_project() {
    let target = project(90, "dispatch", "/workspace/dispatch");
    let mut model = loaded_projects_model(1, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    let submitted = update(model, UiEvent::Input(UiInput::Character('d'))).expect("dispatch");
    assert!(submitted.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::DispatchPending { project_id },
            ..
        } if *project_id == target.project_id
    )));
}

#[test]
fn resource_add_retains_input_across_reload_and_preview_failure() {
    let target = project_with_second_resource();
    let mut model = loaded_projects_model(1, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::Character('a')))
        .expect("add form")
        .model;
    model = update(model, UiEvent::Input(UiInput::Paste("/added".to_owned())))
        .expect("add path")
        .model;

    let invalidated = update(model, UiEvent::Invalidated { revision: 2 }).expect("reload");
    let snapshot_id = snapshot_effect(&invalidated.effects);
    let mut refreshed = target.clone();
    refreshed.name = "resources-current".to_owned();
    model = update(
        invalidated.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot: projects_snapshot(2, vec![refreshed]),
        },
    )
    .expect("current project")
    .model;
    assert!(matches!(
        model.project_modal(),
        Some(UiProjectModal::AddResource { project, path, .. })
            if project.name == "resources-current" && path == "/added"
    ));

    let previewing = update(model, UiEvent::Input(UiInput::Activate)).expect("preview add");
    let (preview_id, preview_action) = project_effect(&previewing.effects);
    model = update(
        previewing.model,
        UiEvent::ProjectCommandFailed {
            effect_id: preview_id,
            failure: UiFailure {
                code: "resource_inspection_failed".to_owned(),
                action: "repair the path and retry".to_owned(),
            },
        },
    )
    .expect("preview failure")
    .model;
    assert!(matches!(
        model.project_modal(),
        Some(UiProjectModal::AddResource { path, submitting: false, .. }) if path == "/added"
    ));
    assert_eq!(
        model.last_failure().map(|failure| failure.code.as_str()),
        Some("resource_inspection_failed")
    );

    let previewing = update(model, UiEvent::Input(UiInput::Activate)).expect("retry preview");
    let (preview_id, retried_action) = project_effect(&previewing.effects);
    assert_eq!(retried_action, preview_action);
    let previewed = update(
        previewing.model,
        UiEvent::ProjectCommandCompleted {
            effect_id: preview_id,
            result: UiProjectResult {
                action: retried_action,
                command_id: [40; 32],
                operation_id: [41; 32],
                project_id: target.project_id,
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::ResourcePreview {
                    display_path: "/added".to_owned(),
                    canonical_path: "/canonical/added".to_owned(),
                    conflicts: Vec::new(),
                },
            },
        },
    )
    .expect("clean preview");
    let adding = update(previewed.model, UiEvent::Input(UiInput::Activate)).expect("add");
    assert_eq!(
        project_effect(&adding.effects).1,
        UiProjectAction::AddResource {
            project_id: target.project_id,
            path: "/added".to_owned(),
            make_primary: false,
        }
    );
}

#[test]
fn resource_replace_and_primary_are_exact_and_cancelable() {
    let target = project_with_second_resource();
    let mut model = loaded_projects_model(3, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("second resource")
        .model;
    let cancelled = update(
        update(model.clone(), UiEvent::Input(UiInput::Character('e')))
            .expect("replace form")
            .model,
        UiEvent::Input(UiInput::Escape),
    )
    .expect("cancel replace");
    assert!(
        cancelled
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    assert!(cancelled.model.project_modal().is_none());

    model = update(model, UiEvent::Input(UiInput::Character('e')))
        .expect("replace form")
        .model;
    model = update(
        model,
        UiEvent::Input(UiInput::Paste("/replacement".to_owned())),
    )
    .expect("replacement path")
    .model;
    let replacing = update(model, UiEvent::Input(UiInput::Activate)).expect("replace preview");
    assert_eq!(
        project_effect(&replacing.effects).1,
        UiProjectAction::PreviewReplaceResource {
            project_id: target.project_id,
            resource_id: [33; 32],
            path: "/replacement".to_owned(),
        }
    );

    let mut model = loaded_projects_model(4, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    model = update(model, UiEvent::Input(UiInput::NextItem))
        .expect("second resource")
        .model;
    let primary_modal = update(model.clone(), UiEvent::Input(UiInput::Character('p')))
        .expect("primary confirmation");
    let cancelled =
        update(primary_modal.model, UiEvent::Input(UiInput::Escape)).expect("cancel primary");
    assert!(
        cancelled
            .effects
            .iter()
            .all(|effect| !matches!(effect, UiEffect::SubmitProjectCommand { .. }))
    );
    let primary_modal =
        update(model, UiEvent::Input(UiInput::Character('p'))).expect("primary confirmation");
    let primary =
        update(primary_modal.model, UiEvent::Input(UiInput::Activate)).expect("set primary");
    assert_eq!(
        project_effect(&primary.effects).1,
        UiProjectAction::SetPrimaryResource {
            project_id: target.project_id,
            resource_id: [33; 32],
        }
    );
}

#[test]
fn resource_check_failure_retains_exact_details_context() {
    let target = project_with_second_resource();
    let mut model = loaded_projects_model(5, vec![target.clone()]);
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("details")
        .model;
    let checking = update(model, UiEvent::Input(UiInput::Character('K'))).expect("check all");
    let (check_id, action) = project_effect(&checking.effects);
    assert_eq!(
        action,
        UiProjectAction::CheckResources {
            project_id: target.project_id,
            resource_id: None,
        }
    );
    let failed = update(
        checking.model,
        UiEvent::ProjectCommandFailed {
            effect_id: check_id,
            failure: UiFailure {
                code: "resource_check_unavailable".to_owned(),
                action: "retry after reconnect".to_owned(),
            },
        },
    )
    .expect("check failure");
    assert!(matches!(
        failed.model.project_modal(),
        Some(UiProjectModal::Details { .. })
    ));
    assert_eq!(
        failed
            .model
            .last_failure()
            .map(|failure| failure.code.as_str()),
        Some("resource_check_unavailable")
    );
}

fn snapshot(revision: u64, ids: &[&str]) -> UiSnapshot {
    snapshot_for(UiSection::Inbox, revision, ids)
}

fn snapshot_for(section: UiSection, revision: u64, ids: &[&str]) -> UiSnapshot {
    let rows = ids
        .iter()
        .map(|id| UiRow {
            id: (*id).to_owned(),
            title: format!("{id} title"),
            detail: format!("{id} detail"),
            state: UiRowState::Open,
            kind: UiRowKind::Conversation,
        })
        .collect::<Vec<_>>();
    UiSnapshot {
        revision,
        human_state: UiHumanState::Ready,
        inbox_rows: if section == UiSection::Inbox {
            rows.clone()
        } else {
            Vec::new()
        },
        sent_rows: if section == UiSection::Sent {
            rows.clone()
        } else {
            Vec::new()
        },
        archived_rows: if section == UiSection::Archived {
            rows.clone()
        } else {
            Vec::new()
        },
        agent_rows: if section == UiSection::Agents {
            rows.clone()
        } else {
            Vec::new()
        },
        project_rows: if section == UiSection::Projects {
            rows
        } else {
            Vec::new()
        },
        direct_targets: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    }
}

fn loaded_agents_model(revision: u64, agents: &[UiAgent]) -> UiModel {
    let mut model = loaded_model(agents_snapshot(revision, agents.to_owned()));
    for _ in 0..3 {
        model = update(model, UiEvent::Input(UiInput::Character('l')))
            .expect("next cached section")
            .model;
    }
    model
}

fn agents_snapshot(revision: u64, agents: Vec<UiAgent>) -> UiSnapshot {
    let rows = agents
        .iter()
        .map(|agent| UiRow {
            id: agent_row_id(agent.agent_id[0]),
            title: agent.names.first().cloned().unwrap_or_default(),
            detail: "active".to_owned(),
            state: UiRowState::Open,
            kind: UiRowKind::Agent,
        })
        .collect();
    UiSnapshot {
        revision,
        human_state: UiHumanState::Ready,
        inbox_rows: Vec::new(),
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: rows,
        project_rows: Vec::new(),
        direct_targets: Vec::new(),
        agents,
        projects: Vec::new(),
    }
}

fn loaded_projects_model(revision: u64, projects: Vec<UiProject>) -> UiModel {
    let mut model = loaded_model(projects_snapshot(revision, projects));
    for _ in 0..4 {
        model = update(model, UiEvent::Input(UiInput::Character('l')))
            .expect("next cached section")
            .model;
    }
    model
}

fn loaded_projects_model_with_agents(
    revision: u64,
    projects: Vec<UiProject>,
    agents: Vec<UiAgent>,
) -> UiModel {
    let mut snapshot = projects_snapshot(revision, projects);
    let agent_source = agents_snapshot(revision, agents);
    snapshot.agent_rows = agent_source.agent_rows;
    snapshot.agents = agent_source.agents;
    let mut model = loaded_model(snapshot);
    for _ in 0..4 {
        model = update(model, UiEvent::Input(UiInput::Character('l')))
            .expect("next cached section")
            .model;
    }
    model
}

fn project_agent(byte: u8, home: [u8; 32]) -> UiAgent {
    UiAgent {
        agent_id: [byte; 32],
        names: vec![format!("agent-{byte}")],
        mailboxes: vec![UiAgentMailbox {
            installation_id: home,
            mailbox_id: [byte.saturating_add(1); 32],
        }],
        lifecycle: UiAgentLifecycle::Active,
        runnable: true,
        sessions: vec![UiAgentSession {
            provider: "codex".to_owned(),
            session: format!("session-{byte}"),
            mailbox: None,
            conflicted: false,
            selected: true,
            name_resolved: true,
            display_name: None,
        }],
    }
}

fn projects_snapshot(revision: u64, projects: Vec<UiProject>) -> UiSnapshot {
    let rows = projects
        .iter()
        .map(|project| UiRow {
            id: agent_row_id(project.project_id[0]),
            title: project.name.clone(),
            detail: project.lifecycle.clone(),
            state: UiRowState::Open,
            kind: UiRowKind::Project,
        })
        .collect();
    UiSnapshot {
        revision,
        human_state: UiHumanState::Ready,
        inbox_rows: Vec::new(),
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: Vec::new(),
        project_rows: rows,
        direct_targets: Vec::new(),
        agents: Vec::new(),
        projects,
    }
}

fn project(byte: u8, name: &str, path: &str) -> UiProject {
    UiProject {
        project_id: [byte; 32],
        home: [9; 32],
        name: name.to_owned(),
        lifecycle: "open".to_owned(),
        archived: false,
        claimable: true,
        assignment: None,
        threads: Vec::new(),
        head: [byte.saturating_add(1); 32],
        input_sequence: 1,
        resources: vec![UiProjectResource {
            resource_id: [byte.saturating_add(2); 32],
            display_path: path.to_owned(),
            canonical_path: path.to_owned(),
            health: "clean".to_owned(),
            primary: true,
            active_claim: true,
            conflicting_projects: Vec::new(),
        }],
    }
}

fn project_with_second_resource() -> UiProject {
    let mut target = project(30, "resources", "/first");
    target.resources.push(UiProjectResource {
        resource_id: [33; 32],
        display_path: "/second".to_owned(),
        canonical_path: "/second".to_owned(),
        health: "unknown".to_owned(),
        primary: false,
        active_claim: true,
        conflicting_projects: Vec::new(),
    });
    target
}

fn project_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, UiProjectAction) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitProjectCommand { id, action } => Some((*id, action.clone())),
            _ => None,
        })
        .expect("project effect")
}

fn agent(byte: u8, name: &str) -> UiAgent {
    UiAgent {
        agent_id: [byte; 32],
        names: vec![name.to_owned()],
        mailboxes: vec![UiAgentMailbox {
            installation_id: [9; 32],
            mailbox_id: [byte; 32],
        }],
        lifecycle: UiAgentLifecycle::Active,
        runnable: true,
        sessions: vec![UiAgentSession {
            provider: "codex".to_owned(),
            session: format!("session-{byte}"),
            mailbox: None,
            conflicted: false,
            selected: true,
            name_resolved: true,
            display_name: Some("x".to_owned()),
        }],
    }
}

fn agent_row_id(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
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

fn loaded_model(snapshot: UiSnapshot) -> UiModel {
    let started = started_model();
    let id = snapshot_effect(&started.effects);
    update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: id,
            snapshot,
        },
    )
    .expect("snapshot loaded")
    .model
}

fn opened_conversation(entries: Vec<UiConversationEntry>) -> UiModel {
    let loaded = loaded_model(snapshot(1, &["thread-a"]));
    let opening = update(loaded, UiEvent::Input(UiInput::Activate)).expect("open conversation");
    let (id, _, _) = conversation_effect(&opening.effects);
    update(
        opening.model,
        UiEvent::ConversationLoaded {
            effect_id: id,
            page: UiConversationPage {
                row_id: "thread-a".to_owned(),
                entries,
                next_cursor: None,
            },
        },
    )
    .expect("conversation loaded")
    .model
}

fn snapshot_effect(effects: &[UiEffect]) -> hq_tui::EffectId {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id } => Some(*id),
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

fn open_draft_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiMailboxDraftTarget) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::OpenDraft { id, target } => Some((*id, target)),
            _ => None,
        })
        .expect("open draft effect")
}

fn save_draft_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiMailboxDraft) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SaveDraft { id, draft } => Some((*id, draft)),
            _ => None,
        })
        .expect("save draft effect")
}

fn agent_action_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiAgentAction) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitAgentCommand { id, action } => Some((*id, action)),
            _ => None,
        })
        .expect("agent command effect")
}

fn managed_session_effect(effects: &[UiEffect]) -> (hq_tui::EffectId, &UiManagedSessionAction) {
    effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitManagedSession { id, action } => Some((*id, action)),
            _ => None,
        })
        .expect("managed-session effect")
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
        message_target: None,
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

fn actionable_entry(id: &str, message_id: [u8; 32]) -> UiConversationEntry {
    UiConversationEntry {
        message_target: Some(UiMessageTarget {
            message_id,
            reply_allowed: true,
        }),
        ..entry(id, false)
    }
}

fn direct_target(label: &str, byte: u8) -> UiDirectTarget {
    UiDirectTarget {
        installation_id: [byte; 32],
        mailbox_id: [byte + 10; 32],
        label: label.to_owned(),
    }
}
