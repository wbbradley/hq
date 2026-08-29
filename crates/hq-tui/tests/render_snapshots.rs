//! Deterministic terminal-buffer snapshots for every responsive layout.

#![allow(clippy::expect_used)]

use hq_tui::{
    UiActivityStatus, UiAgent, UiAgentLifecycle, UiAgentMailbox, UiAgentSession,
    UiConversationEntry, UiConversationEntryKind, UiConversationPage, UiEffect, UiEvent, UiInput,
    UiMailboxDraft, UiMailboxDraftTarget, UiMessageState, UiModel, UiRow, UiRowKind, UiRowState,
    UiSection, UiSize, UiSnapshot, UiTechnicalSection, render, update,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

#[test]
fn wide_layout_matches_snapshot_without_mutating_the_model() {
    assert_snapshot(
        &ready_model(UiSize {
            width: 104,
            height: 18,
        }),
        include_str!("snapshots/wide.txt"),
    );
}

#[test]
fn compact_layout_matches_snapshot_without_mutating_the_model() {
    assert_snapshot(
        &ready_model(UiSize {
            width: 72,
            height: 16,
        }),
        include_str!("snapshots/compact.txt"),
    );
}

#[test]
fn too_small_layout_matches_snapshot_without_mutating_the_model() {
    assert_snapshot(
        &ready_model(UiSize {
            width: 30,
            height: 8,
        }),
        include_str!("snapshots/too-small.txt"),
    );
}

#[test]
fn conversation_layout_preserves_reducer_order_and_typed_activity_state() {
    let model = conversation_model(UiSize {
        width: 120,
        height: 24,
    });
    let before = model.clone();
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &model))
        .expect("render buffer");
    assert_eq!(model, before, "rendering must only borrow the model");
    let rendered = snapshot_text(terminal.backend().buffer());
    let message = rendered.find("question · peer").expect("message rendered");
    let activity = rendered
        .find("activity · running")
        .expect("activity rendered");
    assert!(message < activity, "reducer page order is retained");
    assert!(rendered.contains("non-actionable · compiling"));
    assert!(rendered.contains("Conversation · complete"));
}

#[test]
fn mailbox_composer_is_responsive_and_rendering_only_borrows_state() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 16,
        },
    ] {
        let ready = ready_model(size);
        let opening = update(ready, UiEvent::Input(UiInput::Character('n'))).expect("self note");
        let effect_id = opening
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::OpenDraft { id, .. } => Some(*id),
                _ => None,
            })
            .expect("draft request");
        let model = update(
            opening.model,
            UiEvent::DraftLoaded {
                effect_id,
                draft: UiMailboxDraft {
                    draft_id: [1; 32],
                    target: UiMailboxDraftTarget::SelfNote,
                    content: "bounded draft text".to_owned(),
                    version: 2,
                },
            },
        )
        .expect("draft loaded")
        .model;
        let before = model.clone();
        let backend = TestBackend::new(size.width, size.height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &model))
            .expect("render composer");
        assert_eq!(model, before);
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("Self-note · saved"));
        assert!(rendered.contains("bounded draft text"));
        assert!(rendered.contains("Enter submit · Esc save and close"));
    }
}

#[test]
fn agent_inspection_is_responsive_and_rendering_only_borrows_state() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 16,
        },
    ] {
        let model = agent_details_model(size);
        let before = model.clone();
        let backend = TestBackend::new(size.width, size.height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &model))
            .expect("render agent details");
        assert_eq!(model, before);
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("Agent details"));
        assert!(rendered.contains("builder"));
        assert!(rendered.contains("codex/session-1"));
        assert!(rendered.contains("r rename/clear"));
    }
}

#[test]
fn managed_session_switch_confirmation_is_responsive_and_explicit_about_runtime_evidence() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 16,
        },
    ] {
        let details = agent_details_model(size);
        let provider =
            update(details, UiEvent::Input(UiInput::Character('s'))).expect("choose provider");
        let model = update(provider.model, UiEvent::Input(UiInput::Activate))
            .expect("switch confirmation")
            .model;
        let before = model.clone();
        let backend = TestBackend::new(size.width, size.height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &model))
            .expect("render managed-session confirmation");
        assert_eq!(model, before);
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("Confirm managed-session switch"));
        assert!(rendered.contains("Start fresh on codex"));
        assert!(rendered.contains("Runtime presence"));
        assert!(rendered.contains("inferred"));
    }
}

fn assert_snapshot(model: &UiModel, expected: &str) {
    let before = model.clone();
    let viewport = model.viewport();
    let backend = TestBackend::new(viewport.width, viewport.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, model))
        .expect("render buffer");
    assert_eq!(model, &before, "rendering must only borrow the model");
    assert_eq!(snapshot_text(terminal.backend().buffer()), expected);
}

fn snapshot_text(buffer: &Buffer) -> String {
    let width = usize::from(buffer.area.width);
    let mut output = buffer
        .content()
        .chunks(width)
        .map(|cells| {
            cells
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    output.push('\n');
    output
}

fn ready_model(size: UiSize) -> UiModel {
    let started = update(UiModel::new(size), UiEvent::Started).expect("start model");
    let request = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("snapshot request");
    let loaded = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot: UiSnapshot {
                section: hq_tui::UiSection::Inbox,
                revision: 42,
                rows: vec![
                    row("build-17", "Build release", "agent-1", UiRowState::Open),
                    row(
                        "deploy-9",
                        "Deploy production",
                        "waiting for approval",
                        UiRowState::Waiting,
                    ),
                    row(
                        "incident-4",
                        "Investigate timeout",
                        "relay needs attention",
                        UiRowState::Attention,
                    ),
                ],
                direct_targets: Vec::new(),
                agents: Vec::new(),
            },
        },
    )
    .expect("load snapshot");
    update(loaded.model, UiEvent::Input(UiInput::NextItem))
        .expect("select second row")
        .model
}

fn row(id: &str, title: &str, detail: &str, state: UiRowState) -> UiRow {
    UiRow {
        id: id.to_owned(),
        title: title.to_owned(),
        detail: detail.to_owned(),
        state,
        kind: UiRowKind::Conversation,
    }
}

fn agent_details_model(size: UiSize) -> UiModel {
    let mut model = ready_model(size);
    for section in [UiSection::Sent, UiSection::Archived, UiSection::Agents] {
        let moving = update(model, UiEvent::Input(UiInput::Character('l'))).expect("next section");
        let request = moving
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::LoadSnapshot { id, .. } => Some(*id),
                _ => None,
            })
            .expect("section snapshot");
        let agent = UiAgent {
            agent_id: [1; 32],
            names: vec!["builder".to_owned()],
            mailboxes: vec![UiAgentMailbox {
                installation_id: [2; 32],
                mailbox_id: [3; 32],
            }],
            lifecycle: UiAgentLifecycle::Active,
            runnable: true,
            sessions: vec![UiAgentSession {
                provider: "codex".to_owned(),
                session: "session-1".to_owned(),
                mailbox: None,
                conflicted: false,
                selected: true,
                name_resolved: true,
                display_name: Some("live".to_owned()),
            }],
        };
        let snapshot = UiSnapshot {
            section,
            revision: 43,
            rows: if section == UiSection::Agents {
                vec![UiRow {
                    id: "01".repeat(32),
                    title: "builder".to_owned(),
                    detail: "active".to_owned(),
                    state: UiRowState::Open,
                    kind: UiRowKind::Agent,
                }]
            } else {
                Vec::new()
            },
            direct_targets: Vec::new(),
            agents: if section == UiSection::Agents {
                vec![agent]
            } else {
                Vec::new()
            },
        };
        model = update(
            moving.model,
            UiEvent::SnapshotLoaded {
                effect_id: request,
                snapshot,
            },
        )
        .expect("section loaded")
        .model;
    }
    update(model, UiEvent::Input(UiInput::Activate))
        .expect("inspect agent")
        .model
}

fn conversation_model(size: UiSize) -> UiModel {
    let ready = ready_model(size);
    let opening = update(ready, UiEvent::Input(UiInput::Activate)).expect("open conversation");
    let effect_id = opening
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadConversation { id, .. } => Some(*id),
            _ => None,
        })
        .expect("conversation request");
    update(
        opening.model,
        UiEvent::ConversationLoaded {
            effect_id,
            page: UiConversationPage {
                row_id: "deploy-9".to_owned(),
                entries: vec![
                    UiConversationEntry {
                        id: "message-1".to_owned(),
                        kind: UiConversationEntryKind::Message,
                        content: "Can we ship?".to_owned(),
                        summary: "question · peer".to_owned(),
                        message_state: Some(UiMessageState::Open),
                        message_target: None,
                        technical: Vec::new(),
                    },
                    UiConversationEntry {
                        id: "activity-2".to_owned(),
                        kind: UiConversationEntryKind::Activity,
                        content: "compiling".to_owned(),
                        summary: "activity · running".to_owned(),
                        message_state: None,
                        message_target: None,
                        technical: vec![UiTechnicalSection::Activity {
                            sequence: 2,
                            status: UiActivityStatus::Running,
                            truncated: false,
                        }],
                    },
                ],
                next_cursor: None,
            },
        },
    )
    .expect("conversation page")
    .model
}
