//! Deterministic terminal-buffer snapshots for every responsive layout.

#![allow(clippy::expect_used)]

use hq_tui::{
    UiActivityStatus, UiAgent, UiAgentLifecycle, UiAgentMailbox, UiAgentSession,
    UiConversationEntry, UiConversationEntryKind, UiConversationPage, UiEffect, UiEvent,
    UiHumanState, UiInput, UiMailboxDraft, UiMailboxDraftTarget, UiMessageState, UiModel,
    UiProject, UiProjectAction, UiProjectAssignment, UiProjectExternalWarning, UiProjectOutcome,
    UiProjectResource, UiProjectResourceCheck, UiProjectResourceConflict, UiProjectResult,
    UiProjectThread, UiRow, UiRowKind, UiRowState, UiSize, UiSnapshot, UiTechnicalSection, render,
    update,
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
fn mailbox_footer_explains_thread_and_exact_message_actions() {
    let summary = render_text(&ready_model(UiSize {
        width: 104,
        height: 18,
    }));
    assert!(summary.contains("Enter open thread for archive/restore"));
    assert!(!summary.contains("a/u state"));

    let conversation = render_text(&conversation_model(UiSize {
        width: 104,
        height: 18,
    }));
    assert!(conversation.contains("a archive"));
    assert!(!conversation.contains("u restore"));
    assert!(!conversation.contains("a/u state"));

    let archived = render_text(&conversation_model_with_state(
        UiSize {
            width: 64,
            height: 16,
        },
        UiMessageState::Archived,
    ));
    assert!(archived.contains("u restore"));
    assert!(!archived.contains("a archive"));
    assert!(archived.contains("Enter info · Esc back · q quit"));

    let confirmation = update(
        conversation_model(UiSize {
            width: 104,
            height: 18,
        }),
        UiEvent::Input(UiInput::Character('a')),
    )
    .expect("archive confirmation")
    .model;
    let confirmation = render_text(&confirmation);
    assert!(confirmation.contains("Archive the selected message?"));
    assert!(confirmation.contains("Only this message changes state"));
    assert!(confirmation.contains("the thread and its history are kept"));
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
fn identity_only_state_renders_setup_and_recovery_actions() {
    let model = loaded_snapshot_model(
        UiSize {
            width: 104,
            height: 18,
        },
        UiSnapshot {
            human_state: UiHumanState::Unavailable,
            ..empty_render_snapshot(1)
        },
    );
    let rendered = render_text(&model);
    assert!(rendered.contains("No active human account"));
    assert!(rendered.contains("hq human create"));
    assert!(rendered.contains("hq human join"));
    assert!(rendered.contains("hq relay sync"));
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

#[test]
fn project_worktree_form_and_reconcilable_outcome_are_responsive() {
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
        let model = project_model(size);
        let form = update(model, UiEvent::Input(UiInput::Character('w')))
            .expect("worktree form")
            .model;
        let rendered = render_text(&form);
        assert!(rendered.contains("recoverable Git worktree"));
        assert!(rendered.contains("Source:"));
        assert!(rendered.contains("Destination:"));

        let mut model = form;
        for (index, value) in ["feature", "", "/source", "/destination", "branch", "main"]
            .into_iter()
            .enumerate()
        {
            if !value.is_empty() {
                model = update(model, UiEvent::Input(UiInput::Paste(value.to_owned())))
                    .expect("field text")
                    .model;
            }
            if index < 5 {
                model = update(model, UiEvent::Input(UiInput::NextItem))
                    .expect("next field")
                    .model;
            }
        }
        let submitted = update(model, UiEvent::Input(UiInput::Activate)).expect("submit");
        let (effect_id, action) = submitted
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::SubmitProjectCommand { id, action } => Some((*id, action.clone())),
                _ => None,
            })
            .expect("project effect");
        assert!(matches!(action, UiProjectAction::CreateWorktree { .. }));
        let outcome = update(
            submitted.model,
            UiEvent::ProjectCommandCompleted {
                effect_id,
                result: UiProjectResult {
                    action,
                    command_id: [2; 32],
                    operation_id: [3; 32],
                    project_id: [4; 32],
                    runtime_state: Some("uncertain".to_owned()),
                    runtime_code: Some("response_lost".to_owned()),
                    outcome: UiProjectOutcome::Reconcilable {
                        stage: "worktree_created".to_owned(),
                        category: "external_state".to_owned(),
                        code: "response_lost".to_owned(),
                        warning: Some(UiProjectExternalWarning {
                            kind: "retained_worktree".to_owned(),
                            destination: "/destination".to_owned(),
                            branch: "branch".to_owned(),
                        }),
                    },
                },
            },
        )
        .expect("typed outcome")
        .model;
        let rendered = render_text(&outcome);
        assert!(rendered.contains("Project operation outcome"));
        assert!(rendered.contains("Runtime: uncertain/response_lost"));
        assert!(rendered.contains("response_lost"));
        assert!(rendered.contains("retained_worktree"));
    }
}

#[test]
fn project_resource_forms_and_conflict_preview_are_responsive() {
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
        let details = update(project_model(size), UiEvent::Input(UiInput::Activate))
            .expect("project details")
            .model;
        let rendered = render_text(&details);
        assert!(rendered.contains("Desired resources"));
        if size.height >= 24 {
            assert!(rendered.contains("check selected"));
        }

        for (key, expected) in [
            ('a', "Add desired resource"),
            ('e', "Replace desired resource"),
            ('x', "Confirm desired-resource removal"),
            ('p', "Confirm primary resource"),
        ] {
            let modal = update(details.clone(), UiEvent::Input(UiInput::Character(key)))
                .expect("resource modal")
                .model;
            assert!(render_text(&modal).contains(expected));
        }

        let add = update(details, UiEvent::Input(UiInput::Character('a')))
            .expect("add form")
            .model;
        let add = update(add, UiEvent::Input(UiInput::Paste("/shared".to_owned())))
            .expect("resource path")
            .model;
        let previewing = update(add, UiEvent::Input(UiInput::Activate)).expect("preview");
        let (effect_id, action) = previewing
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::SubmitProjectCommand { id, action } => Some((*id, action.clone())),
                _ => None,
            })
            .expect("preview effect");
        let preview = update(
            previewing.model,
            UiEvent::ProjectCommandCompleted {
                effect_id,
                result: UiProjectResult {
                    action,
                    command_id: [5; 32],
                    operation_id: [6; 32],
                    project_id: [1; 32],
                    runtime_state: None,
                    runtime_code: None,
                    outcome: UiProjectOutcome::ResourcePreview {
                        display_path: "/shared".to_owned(),
                        canonical_path: "/canonical/shared".to_owned(),
                        conflicts: vec![UiProjectResourceConflict {
                            project_id: [7; 32],
                            resource_id: [8; 32],
                            display_path: "/other".to_owned(),
                            canonical_path: "/canonical".to_owned(),
                            relationship: "descendant".to_owned(),
                        }],
                    },
                },
            },
        )
        .expect("preview outcome")
        .model;
        let rendered = render_text(&preview);
        assert!(rendered.contains("Preview desired-resource addition"));
        assert!(rendered.contains("descendant"));
        assert!(rendered.contains("Claim conflicts block mutation"));
    }
}

#[test]
fn project_activation_form_is_responsive_and_discloses_exact_resume() {
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
        let details = update(project_model(size), UiEvent::Input(UiInput::Activate))
            .expect("project details")
            .model;
        let activation = update(details, UiEvent::Input(UiInput::Character('v')))
            .expect("activation")
            .model;
        let rendered = render_text(&activation);
        assert!(rendered.contains("Activate project assignment"));
        assert!(rendered.contains("new session"));
        assert!(rendered.contains("agent-5"));
        assert!(rendered.contains("/workspace/release"));
    }
}

#[test]
fn project_handoff_form_separates_confirmation_force_and_runtime_truth() {
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
        let details = update(
            project_model_with_assignment(size, true),
            UiEvent::Input(UiInput::Activate),
        )
        .expect("project details")
        .model;
        let handoff = update(details, UiEvent::Input(UiInput::Character('h')))
            .expect("handoff")
            .model;
        let rendered = render_text(&handoff);
        assert!(rendered.contains("Confirm project handoff"));
        assert!(rendered.contains("Confirmed: false"));
        if size.width >= 120 {
            assert!(rendered.contains("Force takeover: false"));
            assert!(rendered.contains("Force revokes HQ authority"));
            assert!(rendered.contains("does not prove external runtime cessation"));
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn project_lifecycle_controls_are_responsive_confirmed_and_force_gated() {
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
        let details = update(project_model(size), UiEvent::Input(UiInput::Activate))
            .expect("project details")
            .model;
        let previewing = update(details.clone(), UiEvent::Input(UiInput::Character('c')))
            .expect("close preview");
        let (effect_id, action) = previewing
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::SubmitProjectCommand { id, action } => Some((*id, action.clone())),
                _ => None,
            })
            .expect("preview effect");
        assert_eq!(
            action,
            UiProjectAction::PreviewClose {
                project_id: [1; 32]
            }
        );
        let assessed = update(
            previewing.model,
            UiEvent::ProjectCommandCompleted {
                effect_id,
                result: UiProjectResult {
                    action,
                    command_id: [11; 32],
                    operation_id: [12; 32],
                    project_id: [1; 32],
                    runtime_state: None,
                    runtime_code: None,
                    outcome: UiProjectOutcome::ResourceChecks {
                        checks: vec![UiProjectResourceCheck {
                            resource_id: [4; 32],
                            status: "accepted".to_owned(),
                            health: Some("healthy".to_owned()),
                            release: Some("dirty".to_owned()),
                            observed_canonical_path: Some("/workspace/release".to_owned()),
                            details: Some("working tree has changes".to_owned()),
                            error_category: None,
                            error_code: None,
                            reconciliation_id: None,
                        }],
                    },
                },
            },
        )
        .expect("typed release assessment")
        .model;
        let confirmation = update(assessed, UiEvent::Input(UiInput::Activate))
            .expect("close confirmation")
            .model;
        let rendered = render_text(&confirmation);
        assert!(rendered.contains("Confirm project close"));
        assert!(rendered.contains("release=dirty"));
        if size.width >= 120 {
            assert!(rendered.contains("Confirmed: false"));
            assert!(rendered.contains("retains external paths"));
        }
        let cancelled = update(confirmation.clone(), UiEvent::Input(UiInput::Escape))
            .expect("close cancellation");
        assert!(cancelled.model.project_modal().is_none());
        assert!(
            !cancelled
                .effects
                .iter()
                .any(|effect| matches!(effect, UiEffect::SubmitProjectCommand { .. }))
        );

        let confirmed = update(confirmation, UiEvent::Input(UiInput::Character('c')))
            .expect("separate confirmation")
            .model;
        let blocked = update(confirmed, UiEvent::Input(UiInput::Activate))
            .expect("dirty close remains blocked");
        assert!(
            !blocked
                .effects
                .iter()
                .any(|effect| matches!(effect, UiEffect::SubmitProjectCommand { .. }))
        );
        let forced = update(blocked.model, UiEvent::Input(UiInput::Character('f')))
            .expect("force recovery")
            .model;
        let closing = update(forced, UiEvent::Input(UiInput::Activate)).expect("forced close");
        assert!(closing.effects.iter().any(|effect| matches!(
            effect,
            UiEffect::SubmitProjectCommand {
                action: UiProjectAction::Close {
                    project_id,
                    force: true
                },
                ..
            } if *project_id == [1; 32]
        )));

        let archive = update(details, UiEvent::Input(UiInput::Character('z')))
            .expect("archive confirmation")
            .model;
        assert!(render_text(&archive).contains("Confirm project archive"));
        let archiving = update(archive, UiEvent::Input(UiInput::Activate)).expect("archive");
        assert!(archiving.effects.iter().any(|effect| matches!(
            effect,
            UiEffect::SubmitProjectCommand {
                action: UiProjectAction::SetArchived { archived: true, .. },
                ..
            }
        )));
    }

    let closed = update(
        project_model_with_state(
            UiSize {
                width: 100,
                height: 20,
            },
            false,
            "closed",
            true,
        ),
        UiEvent::Input(UiInput::Activate),
    )
    .expect("closed project details")
    .model;
    let opening =
        update(closed.clone(), UiEvent::Input(UiInput::Character('o'))).expect("reopen project");
    assert!(opening.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::Open { .. },
            ..
        }
    )));
    let unarchive = update(closed, UiEvent::Input(UiInput::Character('z')))
        .expect("unarchive confirmation")
        .model;
    let unarchiving = update(unarchive, UiEvent::Input(UiInput::Activate)).expect("unarchive");
    assert!(unarchiving.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::SetArchived {
                archived: false,
                ..
            },
            ..
        }
    )));
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

fn render_text(model: &UiModel) -> String {
    let before = model.clone();
    let viewport = model.viewport();
    let backend = TestBackend::new(viewport.width, viewport.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, model))
        .expect("render buffer");
    assert_eq!(model, &before);
    snapshot_text(terminal.backend().buffer())
}

fn ready_model(size: UiSize) -> UiModel {
    let model = loaded_snapshot_model(
        size,
        UiSnapshot {
            revision: 42,
            human_state: UiHumanState::Ready,
            inbox_rows: vec![
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
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: Vec::new(),
            project_rows: Vec::new(),
            direct_targets: Vec::new(),
            agents: Vec::new(),
            projects: Vec::new(),
        },
    );
    let focused = update(model, UiEvent::Input(UiInput::NextFocus)).expect("focus content");
    update(focused.model, UiEvent::Input(UiInput::NextItem))
        .expect("select second row")
        .model
}

fn loaded_snapshot_model(size: UiSize, snapshot: UiSnapshot) -> UiModel {
    let started = update(UiModel::new(size), UiEvent::Started).expect("start model");
    let request = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id } => Some(*id),
            _ => None,
        })
        .expect("snapshot request");
    update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: request,
            snapshot,
        },
    )
    .expect("load snapshot")
    .model
}

fn empty_render_snapshot(revision: u64) -> UiSnapshot {
    UiSnapshot {
        revision,
        human_state: UiHumanState::Ready,
        inbox_rows: Vec::new(),
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: Vec::new(),
        project_rows: Vec::new(),
        direct_targets: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    }
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
    let mut model = loaded_snapshot_model(
        size,
        UiSnapshot {
            revision: 43,
            human_state: UiHumanState::Ready,
            inbox_rows: Vec::new(),
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: vec![UiRow {
                id: "01".repeat(32),
                title: "builder".to_owned(),
                detail: "active".to_owned(),
                state: UiRowState::Open,
                kind: UiRowKind::Agent,
            }],
            project_rows: Vec::new(),
            direct_targets: Vec::new(),
            agents: vec![agent],
            projects: Vec::new(),
        },
    );
    let section_input = if size.width >= 96 {
        UiInput::NextItem
    } else {
        UiInput::Character('l')
    };
    for _ in 0..3 {
        model = update(model, UiEvent::Input(section_input.clone()))
            .expect("next cached section")
            .model;
    }
    model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("focus agent content")
        .model;
    update(model, UiEvent::Input(UiInput::Activate))
        .expect("inspect agent")
        .model
}

fn project_model(size: UiSize) -> UiModel {
    project_model_with_state(size, false, "open", false)
}

#[allow(clippy::too_many_lines)]
fn project_model_with_assignment(size: UiSize, assigned: bool) -> UiModel {
    project_model_with_state(size, assigned, "open", false)
}

#[allow(clippy::too_many_lines)]
fn project_model_with_state(
    size: UiSize,
    assigned: bool,
    lifecycle: &str,
    archived: bool,
) -> UiModel {
    let projects = vec![UiProject {
        project_id: [1; 32],
        home: [2; 32],
        name: "release".to_owned(),
        lifecycle: lifecycle.to_owned(),
        archived,
        claimable: true,
        assignment: assigned.then(|| UiProjectAssignment {
            assignment_id: [8; 32],
            agent_id: [5; 32],
            provider: "codex".to_owned(),
            session: Some("project-session".to_owned()),
            phase: "blocked".to_owned(),
            thread_id: Some([6; 32]),
            launch_directory: Some("/workspace/release".to_owned()),
            blocked: Some("runtime_stop_uncertain".to_owned()),
            cardinality_conflicted: false,
            runnable: false,
        }),
        threads: vec![
            UiProjectThread {
                agent_id: [5; 32],
                provider: "codex".to_owned(),
                session: "project-session".to_owned(),
                thread_id: [6; 32],
            },
            UiProjectThread {
                agent_id: [9; 32],
                provider: "codex".to_owned(),
                session: "target-session".to_owned(),
                thread_id: [10; 32],
            },
        ],
        head: [3; 32],
        input_sequence: 0,
        resources: vec![UiProjectResource {
            resource_id: [4; 32],
            display_path: "/workspace/release".to_owned(),
            canonical_path: "/workspace/release".to_owned(),
            health: "healthy".to_owned(),
            primary: true,
            active_claim: true,
            conflicting_projects: Vec::new(),
        }],
    }];
    let agents = [
        (5_u8, "agent-5", "project-session"),
        (9_u8, "agent-9", "target-session"),
    ]
    .into_iter()
    .map(|(byte, name, session)| UiAgent {
        agent_id: [byte; 32],
        names: vec![name.to_owned()],
        mailboxes: vec![UiAgentMailbox {
            installation_id: [2; 32],
            mailbox_id: [byte.saturating_add(1); 32],
        }],
        lifecycle: UiAgentLifecycle::Active,
        runnable: true,
        sessions: vec![UiAgentSession {
            provider: "codex".to_owned(),
            session: session.to_owned(),
            mailbox: None,
            conflicted: false,
            selected: true,
            name_resolved: true,
            display_name: None,
        }],
    })
    .collect();
    let mut model = loaded_snapshot_model(
        size,
        UiSnapshot {
            revision: 44,
            human_state: UiHumanState::Ready,
            inbox_rows: Vec::new(),
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: Vec::new(),
            project_rows: vec![UiRow {
                id: "01".repeat(32),
                title: "release".to_owned(),
                detail: lifecycle.to_owned(),
                state: UiRowState::Open,
                kind: UiRowKind::Project,
            }],
            direct_targets: Vec::new(),
            agents,
            projects,
        },
    );
    let section_input = if size.width >= 96 {
        UiInput::NextItem
    } else {
        UiInput::Character('l')
    };
    for _ in 0..4 {
        model = update(model, UiEvent::Input(section_input.clone()))
            .expect("next cached section")
            .model;
    }
    update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("focus project content")
        .model
}

fn conversation_model(size: UiSize) -> UiModel {
    conversation_model_with_state(size, UiMessageState::Open)
}

fn conversation_model_with_state(size: UiSize, state: UiMessageState) -> UiModel {
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
                        message_state: Some(state),
                        message_target: Some(hq_tui::UiMessageTarget {
                            message_id: [1; 32],
                            reply_allowed: true,
                        }),
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
