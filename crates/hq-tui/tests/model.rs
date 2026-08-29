//! Pure TUI transition and stale-effect contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use hq_tui::{
    UiActivityStatus, UiAgent, UiAgentAction, UiAgentLifecycle, UiAgentMailbox, UiAgentModal,
    UiAgentSession, UiConnectionState, UiConversationEntry, UiConversationEntryKind,
    UiConversationPage, UiDirectTarget, UiEffect, UiEvent, UiFailure, UiFocus, UiInput,
    UiMailboxAction, UiMailboxDraft, UiMailboxDraftTarget, UiMailboxModal, UiMessageState,
    UiMessageTarget, UiModel, UiRow, UiRowKind, UiRowState, UiSection, UiSize, UiSnapshot,
    UiTechnicalSection, UiTimerKind, update,
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

fn snapshot(revision: u64, ids: &[&str]) -> UiSnapshot {
    snapshot_for(UiSection::Inbox, revision, ids)
}

fn snapshot_for(section: UiSection, revision: u64, ids: &[&str]) -> UiSnapshot {
    UiSnapshot {
        section,
        revision,
        direct_targets: Vec::new(),
        agents: Vec::new(),
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

fn loaded_agents_model(revision: u64, agents: &[UiAgent]) -> UiModel {
    let mut model = loaded_model(snapshot(0, &[]));
    for section in [UiSection::Sent, UiSection::Archived, UiSection::Agents] {
        let moved = update(model, UiEvent::Input(UiInput::Character('l'))).expect("next section");
        let id = snapshot_effect(&moved.effects);
        let source = if section == UiSection::Agents {
            agents_snapshot(revision, agents.to_owned())
        } else {
            snapshot_for(section, revision, &[])
        };
        model = update(
            moved.model,
            UiEvent::SnapshotLoaded {
                effect_id: id,
                snapshot: source,
            },
        )
        .expect("section loaded")
        .model;
    }
    model
}

fn agents_snapshot(revision: u64, agents: Vec<UiAgent>) -> UiSnapshot {
    UiSnapshot {
        section: UiSection::Agents,
        revision,
        rows: agents
            .iter()
            .map(|agent| UiRow {
                id: agent_row_id(agent.agent_id[0]),
                title: agent.names.first().cloned().unwrap_or_default(),
                detail: "active".to_owned(),
                state: UiRowState::Open,
                kind: UiRowKind::Agent,
            })
            .collect(),
        direct_targets: Vec::new(),
        agents,
    }
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
