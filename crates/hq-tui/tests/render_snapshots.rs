//! Deterministic terminal-buffer snapshots for every responsive layout.

#![allow(clippy::expect_used)]

use hq_tui::{
    UiActivityStatus, UiAgent, UiAgentAssignmentPhase, UiAgentLifecycle, UiAgentMailbox,
    UiAgentProjectAssignment, UiAgentSession, UiAgentStatus, UiConversationEntry,
    UiConversationEntryKind, UiConversationPage, UiEffect, UiEvent, UiFailure, UiHumanState,
    UiInput, UiMailboxDraft, UiMailboxDraftTarget, UiMessageState, UiModel, UiProject,
    UiProjectAction, UiProjectAssignment, UiProjectExternalWarning, UiProjectOutcome,
    UiProjectResource, UiProjectResourceCheck, UiProjectResourceConflict, UiProjectResult,
    UiProjectThread, UiProvider, UiRow, UiRowKind, UiRowState, UiSection, UiSize, UiSnapshot,
    UiTechnicalSection, UiTheme, UiThemeRole, render, update,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier, Style},
};

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
fn focused_mailbox_footer_keeps_complete_actions_in_contextual_help() {
    let summary = render_text(&ready_model(UiSize {
        width: 104,
        height: 18,
    }));
    assert!(summary.contains("Enter open · n New… · d message · N note · ? help · q quit"));
    assert!(!summary.contains("archive/restore"));
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
    assert!(archived.contains("u restore · Enter info · Esc back · ? help"));

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
    assert!(confirmation.contains("conversation history stays intact"));
}

#[test]
fn guided_new_workflow_explains_each_foreign_choice_in_user_terms() {
    let model = project_model(UiSize {
        width: 104,
        height: 22,
    });
    let launcher = update(model, UiEvent::Input(UiInput::Character('n')))
        .expect("open New launcher")
        .model;
    let rendered = render_text(&launcher);
    assert!(
        rendered.contains("What would you like to do?"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Work with an agent on a project"),
        "{rendered}"
    );
    assert!(rendered.contains("Send a direct message"), "{rendered}");
    assert!(rendered.contains("Write a personal note"), "{rendered}");
    assert!(
        rendered.contains("only the choices that intent needs"),
        "{rendered}"
    );

    let projects = update(launcher, UiEvent::Input(UiInput::Activate))
        .expect("choose project work")
        .model;
    let rendered = render_text(&projects);
    assert!(
        rendered.contains("Which project is this work for?"),
        "{rendered}"
    );
    assert!(
        rendered.contains("folders or resources it owns"),
        "{rendered}"
    );
    assert!(rendered.contains("Create a project"), "{rendered}");

    let agents = update(projects, UiEvent::Input(UiInput::Activate))
        .expect("choose release")
        .model;
    let rendered = render_text(&agents);
    assert!(
        rendered.contains("Who should work on release?"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Unassigned agents are listed first"),
        "{rendered}"
    );
    assert!(rendered.contains("Create an agent"), "{rendered}");

    let provider = update(agents, UiEvent::Input(UiInput::Activate))
        .expect("choose agent")
        .model;
    let rendered = render_text(&provider);
    assert!(rendered.contains("Start project work"), "{rendered}");
    assert!(
        rendered.contains("continue the compatible saved project conversation"),
        "{rendered}"
    );
    assert!(!rendered.contains("required"), "{rendered}");
    assert!(!rendered.contains("provider namespace"), "{rendered}");
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
fn custom_normal_style_reaches_every_frame_cell() {
    let foreground = Color::Rgb(222, 211, 195);
    let background = Color::Rgb(31, 29, 27);
    let theme = UiTheme::no_color().with_style(
        UiThemeRole::Screen,
        Style::new().fg(foreground).bg(background),
    );
    let buffer = render_buffer_with_theme(
        &ready_model(UiSize {
            width: 104,
            height: 18,
        }),
        &theme,
    );
    assert!(buffer.content().iter().all(|cell| cell.fg == foreground));
    assert!(buffer.content().iter().all(|cell| cell.bg == background));
}

#[test]
fn modal_surface_and_selection_roles_survive_clear_independently() {
    let screen = Color::Rgb(20, 22, 24);
    let modal = Color::Rgb(52, 48, 44);
    let selection = Color::Rgb(214, 93, 14);
    let theme = UiTheme::no_color()
        .with_style(
            UiThemeRole::Screen,
            Style::new().fg(Color::White).bg(screen),
        )
        .with_style(
            UiThemeRole::ModalSurface,
            Style::new().fg(Color::White).bg(modal),
        )
        .with_style(
            UiThemeRole::SelectionFocused,
            Style::new()
                .fg(Color::Black)
                .bg(selection)
                .add_modifier(Modifier::BOLD),
        );
    let launcher = update(
        project_model(UiSize {
            width: 104,
            height: 22,
        }),
        UiEvent::Input(UiInput::Character('n')),
    )
    .expect("open launcher")
    .model;
    let buffer = render_buffer_with_theme(&launcher, &theme);
    assert_eq!(buffer.cell((0, 0)).expect("screen cell").bg, screen);
    let modal_text = find_text_start(&buffer, "What would you like to do?");
    assert_eq!(buffer.cell(modal_text).expect("modal text").bg, modal);
    let selected_text = find_text_start(&buffer, "Work with an agent on a project");
    let selected_cell = buffer.cell(selected_text).expect("selected text");
    assert_eq!(selected_cell.bg, selection);
    assert!(selected_cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn status_roles_are_independently_overridable() {
    let theme = UiTheme::terminal().with_style(
        UiThemeRole::ConnectionReady,
        Style::new().fg(Color::Magenta).bg(Color::LightGreen),
    );
    let buffer = render_buffer_with_theme(
        &ready_model(UiSize {
            width: 104,
            height: 18,
        }),
        &theme,
    );
    let status = buffer
        .cell(find_text_start(&buffer, "Connected"))
        .expect("connection status");
    assert_eq!(status.fg, Color::Magenta);
    assert_eq!(status.bg, Color::LightGreen);
}

#[test]
fn no_color_theme_retains_a_non_color_focus_cue() {
    let launcher = update(
        project_model(UiSize {
            width: 104,
            height: 22,
        }),
        UiEvent::Input(UiInput::Character('n')),
    )
    .expect("open launcher")
    .model;
    let buffer = render_buffer_with_theme(&launcher, &UiTheme::no_color());
    let selected = buffer
        .cell(find_text_start(&buffer, "Work with an agent on a project"))
        .expect("selected choice");
    assert_eq!(selected.fg, Color::Reset);
    assert_eq!(selected.bg, Color::Reset);
    assert!(selected.modifier.contains(Modifier::BOLD));
    assert!(selected.modifier.contains(Modifier::REVERSED));
}

#[test]
fn renderer_contains_no_concrete_color_policy() {
    let source = include_str!("../src/render.rs");
    assert!(!source.contains("Color::"));
    assert!(!source.contains("Color("));
    assert!(!source.contains("Rgb("));
    assert!(!source.contains("Indexed("));
    for role in UiThemeRole::ALL {
        assert!(
            source.contains(&format!("UiThemeRole::{role:?}")),
            "renderer must assign semantic role {} to at least one element",
            role.key()
        );
    }
}

#[test]
fn identity_only_state_renders_setup_and_recovery_actions() {
    let model = loaded_snapshot_model(
        UiSize {
            width: 104,
            height: 18,
        },
        UiSnapshot {
            human_state: UiHumanState::NeedsAttention(hq_tui::UiHumanIssue::NoAccountSelected),
            ..empty_render_snapshot(1)
        },
    );
    let rendered = render_text(&model);
    assert!(rendered.contains("No human account is selected"));
    assert!(rendered.contains("hq human create"));
    assert!(rendered.contains("press F5 to continue"));
    assert!(!rendered.contains("hq human show"));

    let help = update(model, UiEvent::Input(UiInput::Help))
        .expect("account setup help")
        .model;
    let help = render_text(&help);
    assert!(help.contains("Already have an HQ account?"), "{help}");
    assert!(
        help.contains("hq human join ABSOLUTE_INVITATION_PATH"),
        "{help}"
    );
}

#[test]
fn fresh_workspace_renders_one_ordered_onboarding_step_at_a_time() {
    let size = UiSize {
        width: 104,
        height: 24,
    };

    let empty = render_text(&loaded_snapshot_model(size, empty_render_snapshot(1)));
    assert!(empty.contains("Get started with HQ"), "{empty}");
    assert!(empty.contains("Account ready"), "{empty}");
    assert!(
        empty.contains("Current: add a project and choose the folder or resource it owns"),
        "{empty}"
    );
    assert!(empty.contains("Press n New…"), "{empty}");
    assert!(!empty.contains("provider namespace"), "{empty}");

    let mut project_only = empty_render_snapshot(2);
    project_only.projects = vec![onboarding_project()];
    project_only.project_rows = vec![UiRow {
        id: "project:release".to_owned(),
        title: "release".to_owned(),
        detail: "open".to_owned(),
        state: UiRowState::Open,
        kind: UiRowKind::Project,
    }];
    let project_only = render_text(&loaded_snapshot_model(size, project_only));
    assert!(project_only.contains("Project ready"), "{project_only}");
    assert!(
        project_only.contains("Current: create an agent to do the work"),
        "{project_only}"
    );

    let mut agent_only = empty_render_snapshot(3);
    agent_only.projects = vec![onboarding_project()];
    agent_only.agents = vec![onboarding_agent()];
    let agent_only = render_text(&loaded_snapshot_model(size, agent_only));
    assert!(agent_only.contains("Agent ready"), "{agent_only}");
    assert!(
        agent_only.contains("Current: connect an agent service"),
        "{agent_only}"
    );

    let mut ready = empty_render_snapshot(4);
    ready.projects = vec![onboarding_project()];
    ready.agents = vec![onboarding_agent()];
    ready.providers = vec![UiProvider {
        provider: "codex".to_owned(),
        name: "Codex".to_owned(),
        available: true,
        configured_default: true,
    }];
    let ready = render_text(&loaded_snapshot_model(size, ready));
    assert!(ready.contains("Agent service ready"), "{ready}");
    assert!(
        ready.contains("Current: send the first project instruction"),
        "{ready}"
    );
    assert!(
        ready.contains("HQ will ask you to choose only if more than one"),
        "{ready}"
    );
    assert!(ready.contains("service is"), "{ready}");
}

#[test]
fn f1_opens_contextual_help_while_a_foreign_dialog_is_open() {
    let launcher = update(
        project_model(UiSize {
            width: 104,
            height: 24,
        }),
        UiEvent::Input(UiInput::Character('n')),
    )
    .expect("open New launcher")
    .model;
    let helped = update(launcher, UiEvent::Input(UiInput::Help))
        .expect("open help from launcher")
        .model;
    let rendered = render_text(&helped);
    assert!(rendered.contains("Help for New…"), "{rendered}");
    assert!(
        rendered.contains("Choose the kind of thing you want to do"),
        "{rendered}"
    );
    assert!(rendered.contains("F1 / Esc — close help"), "{rendered}");
    assert!(helped.new_modal().is_some());
}

#[test]
#[allow(clippy::too_many_lines)]
fn human_account_issues_have_specific_recovery_and_technical_evidence() {
    let membership = |status| hq_tui::UiHumanMembershipEvidence {
        account_id: [7; 32],
        status,
        frontier: vec![[8; 32]],
        active_acceptances: Vec::new(),
    };
    let cases = vec![
        (
            hq_tui::UiHumanIssue::NoAccountSelected,
            "No human account is selected",
            "human_no_account_selected",
        ),
        (
            hq_tui::UiHumanIssue::SelectionCandidates {
                candidates: vec![[1; 32], [2; 32]],
                frontier: vec![[3; 32]],
            },
            "human account selection is unresolved",
            "human_selection_candidates",
        ),
        (
            hq_tui::UiHumanIssue::SelectionRecords {
                records: vec![hq_tui::UiHumanSelectionEvidence {
                    candidates: vec![[1; 32]],
                    active: Some([1; 32]),
                    frontier: vec![[3; 32]],
                }],
            },
            "conflicting account choices on this device",
            "human_selection_records_conflict",
        ),
        (
            hq_tui::UiHumanIssue::SelectedWithoutAuthority {
                account_id: [7; 32],
                selection_frontier: vec![[3; 32]],
            },
            "device is not allowed to use the selected account",
            "human_selected_without_authority",
        ),
        (
            hq_tui::UiHumanIssue::MembershipPending(membership(
                hq_tui::UiHumanMembershipStatus::Pending,
            )),
            "not finished joining",
            "human_membership_pending",
        ),
        (
            hq_tui::UiHumanIssue::MembershipRevoked(membership(
                hq_tui::UiHumanMembershipStatus::Revoked,
            )),
            "removed from the selected account",
            "human_membership_revoked",
        ),
        (
            hq_tui::UiHumanIssue::MembershipAuthorityConflict {
                records: vec![hq_tui::UiHumanMembershipEvidence {
                    active_acceptances: vec![[9; 32], [10; 32]],
                    ..membership(hq_tui::UiHumanMembershipStatus::Active)
                }],
            },
            "conflicting records for this device's account access",
            "human_membership_authority_conflict",
        ),
    ];

    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 18,
        },
    ] {
        for (issue, explanation, code) in &cases {
            let model = loaded_snapshot_model(
                size,
                UiSnapshot {
                    human_state: UiHumanState::NeedsAttention(issue.clone()),
                    ..empty_render_snapshot(42)
                },
            );
            let rendered = render_text(&model);
            assert!(rendered.contains(explanation), "{code} at {size:?}");
            assert!(!rendered.contains("selection or authority is ambiguous"));

            let help = update(model, UiEvent::Input(UiInput::Character('?')))
                .expect("open account help")
                .model;
            let technical = update(help, UiEvent::Input(UiInput::Character('t')))
                .expect("open account evidence")
                .model;
            let technical = render_text(&technical);
            assert!(technical.contains(code), "{code} at {size:?}");
        }
    }

    let candidate_model = loaded_snapshot_model(
        UiSize {
            width: 120,
            height: 24,
        },
        UiSnapshot {
            human_state: UiHumanState::NeedsAttention(hq_tui::UiHumanIssue::SelectionCandidates {
                candidates: vec![[1; 32], [2; 32]],
                frontier: vec![[3; 32]],
            }),
            ..empty_render_snapshot(42)
        },
    );
    let help = update(candidate_model, UiEvent::Input(UiInput::Character('?')))
        .expect("candidate help")
        .model;
    let technical = update(help, UiEvent::Input(UiInput::Character('t')))
        .expect("candidate evidence")
        .model;
    let rendered = render_text(&technical);
    assert!(rendered.contains(&"01".repeat(32)));
    assert!(rendered.contains(&"02".repeat(32)));
    assert!(rendered.contains(&"03".repeat(32)));
}

#[test]
fn empty_sections_and_recipient_chooser_explain_exact_next_actions() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 18,
        },
    ] {
        for (section, explanation, action) in [
            (
                UiSection::Inbox,
                "No conversations need your attention.",
                "n New… for project work, a message, or a personal note",
            ),
            (
                UiSection::Sent,
                "You have not started or replied to a conversation.",
                "n New… for project work, a message, or a personal note",
            ),
            (
                UiSection::Archived,
                "You have not put any conversations away.",
                "Browse Inbox or Sent",
            ),
            (
                UiSection::Agents,
                "No named workers yet.",
                "n New… to start guided work, or c to create an agent",
            ),
            (
                UiSection::Projects,
                "No projects yet.",
                "n New… to start guided work, or c to create a project",
            ),
        ] {
            let model = empty_section_model(size, section);
            let rendered = render_text(&model);
            assert!(rendered.contains(explanation), "{section:?} at {size:?}");
            assert!(rendered.contains(action), "{section:?} at {size:?}");
            assert!(!rendered.contains(" No items"));
            if section == UiSection::Projects {
                assert!(rendered.contains("ownership of its folders"));
                assert!(!rendered.contains("worktree"));
            }
        }

        let empty_inbox = empty_section_model(size, UiSection::Inbox);
        let chooser = update(empty_inbox, UiEvent::Input(UiInput::Character('d')))
            .expect("open empty recipient chooser")
            .model;
        let rendered = render_text(&chooser);
        assert!(rendered.contains("No reachable recipients yet."));
        assert!(rendered.contains("Create an agent from the Agents section"));
        assert!(rendered.contains("People in your HQ network can appear here"));
        assert!(rendered.contains("Esc close"));
        assert!(!rendered.contains("Enter compose"));
        assert!(!rendered.contains("↑/↓ select"));
    }
}

#[test]
fn contextual_help_covers_every_section_with_and_without_a_selection() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 18,
        },
    ] {
        for section in [
            UiSection::Inbox,
            UiSection::Sent,
            UiSection::Archived,
            UiSection::Agents,
            UiSection::Projects,
        ] {
            for selected in [false, true] {
                let context = contextual_help_model(size, section, selected);
                let rendered = render_text(&context);
                assert!(rendered.contains("Help"));
                assert!(rendered.contains("What this is"));
                assert!(
                    rendered.contains(section_help_phrase(section)),
                    "missing section phrase for {section:?} at {size:?}:\n{rendered}"
                );
                assert!(rendered.contains("Available actions"));
                assert!(rendered.contains("F1/?/Esc close help"));
                if selected {
                    assert!(rendered.contains("Selected: Example item"));
                    assert!(rendered.contains("State: needs attention"));
                } else {
                    assert!(rendered.contains("No item is selected"));
                }

                let technical = update(context, UiEvent::Input(UiInput::Character('t')))
                    .expect("technical help page")
                    .model;
                let technical = render_text(&technical);
                assert!(technical.contains("Technical details"));
                assert!(technical.contains("Connection: ready"));
                assert!(technical.contains("Recovery evidence: none"));
                if selected {
                    assert!(technical.contains("Stable item ID: example-id"));
                }
            }
        }
    }

    let context = contextual_help_model(
        UiSize {
            width: 64,
            height: 18,
        },
        UiSection::Inbox,
        true,
    );
    let failed = update(
        context,
        UiEvent::ClientFailed {
            generation: 4,
            failure: hq_tui::UiFailure {
                code: "connection_lost".to_owned(),
                action: "waiting to reconnect".to_owned(),
            },
        },
    )
    .expect("failure evidence while help is open");
    let technical = update(failed.model, UiEvent::Input(UiInput::Character('t')))
        .expect("show failure evidence")
        .model;
    let rendered = render_text(&technical);
    assert!(rendered.contains("Recovery code: connection_lost"));
    assert!(rendered.contains("Recovery action: waiting to reconnect"));
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
        .draw(|frame| render(frame, &model, &UiTheme::terminal()))
        .expect("render buffer");
    assert_eq!(model, before, "rendering must only borrow the model");
    let rendered = snapshot_text(terminal.backend().buffer());
    let message = rendered.find("question · peer").expect("message rendered");
    let activity = rendered
        .find("activity · running")
        .expect("activity rendered");
    assert!(message < activity, "reducer page order is retained");
    assert!(rendered.contains("update · information only · compiling"));
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
        let opening = update(ready, UiEvent::Input(UiInput::Character('N'))).expect("self note");
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
            .draw(|frame| render(frame, &model, &UiTheme::terminal()))
            .expect("render composer");
        assert_eq!(model, before);
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("Self-note · saved"));
        assert!(rendered.contains("Message required"));
        assert!(rendered.contains("bounded draft text│"));
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
            .draw(|frame| render(frame, &model, &UiTheme::terminal()))
            .expect("render agent details");
        assert_eq!(model, before);
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("Agent details"));
        assert!(rendered.contains("builder"));
        assert!(rendered.contains("Status: Unassigned"));
        assert!(rendered.contains("codex/session-1"));
        if size.width >= 120 {
            assert!(rendered.contains("r name/clear"));
        } else {
            assert!(
                rendered.contains("r name"),
                "agent details at {size:?}:\n{rendered}"
            );
        }
        assert!(!rendered.contains("runnable:"));
    }
}

#[test]
fn agent_form_marks_empty_requirements_without_inserting_a_pipe_character() {
    let details = agent_details_model(UiSize {
        width: 90,
        height: 18,
    });
    let agents = update(details, UiEvent::Input(UiInput::Escape))
        .expect("close details")
        .model;
    let form = update(agents, UiEvent::Input(UiInput::Character('c')))
        .expect("create agent")
        .model;
    let rendered = render_text(&form);
    assert!(rendered.contains("Name:"), "{rendered}");
    assert!(rendered.contains("(required)"), "{rendered}");
    assert!(!rendered.contains("Name: │"), "{rendered}");
    assert!(rendered.contains("such as reviewer"), "{rendered}");

    let cursor_color = Color::Magenta;
    let theme = UiTheme::terminal().with_style(
        UiThemeRole::Cursor,
        Style::new()
            .bg(cursor_color)
            .add_modifier(Modifier::REVERSED),
    );
    let buffer = render_buffer_with_theme(&form, &theme);
    let (name_x, name_y) = find_text_start(&buffer, "Name:");
    let cursor = buffer
        .cell((name_x + 5, name_y))
        .expect("trailing blank cursor cell");
    assert_eq!(cursor.symbol(), " ");
    assert_eq!(cursor.bg, cursor_color);
}

#[test]
fn assigned_agent_details_show_plain_status_and_exact_assignment_evidence() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 18,
        },
    ] {
        let assignment = UiAgentProjectAssignment {
            project_id: [4; 32],
            project_name: "release".to_owned(),
            assignment_id: [5; 32],
            provider: "codex".to_owned(),
            session: Some("session-1".to_owned()),
            phase: UiAgentAssignmentPhase::Ready,
            blocked: None,
            cardinality_conflicted: false,
        };
        let model = agent_details_model_with_status(size, UiAgentStatus::Assigned(assignment));
        let backend = TestBackend::new(size.width, size.height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &model, &UiTheme::terminal()))
            .expect("render assigned agent details");
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("Status: Assigned to release · ready"));
        assert!(rendered.contains("Project: release (040404040404)"));
        assert!(rendered.contains("Technical assignment: 050505050505"));
        assert!(rendered.contains("service codex"));
        assert!(rendered.contains("conversation session-1"));
        assert!(!rendered.contains("runnable:"));
    }
}

#[test]
fn agent_rows_show_assignment_status_without_generic_open_or_waiting_labels() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 24,
        },
    ] {
        let model = agent_status_rows_model(size);
        let backend = TestBackend::new(size.width, size.height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, &model, &UiTheme::terminal()))
            .expect("render agent statuses");
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("unassigned"));
        assert!(rendered.contains("assigned to release · setting up"));
        assert!(rendered.contains("needs attention · migration blocked"));
        assert!(rendered.contains("retired"));
        assert!(!rendered.contains("open · unassigned"));
        assert!(!rendered.contains("waiting"));
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
            .draw(|frame| render(frame, &model, &UiTheme::terminal()))
            .expect("render managed-session confirmation");
        assert_eq!(model, before);
        let rendered = snapshot_text(terminal.backend().buffer());
        assert!(rendered.contains("Switch the agent's conversation?"));
        assert!(rendered.contains("Start a new conversation using codex"));
        assert!(rendered.contains("already running"));
        assert!(!rendered.contains("runtime presence"));
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
        assert!(rendered.contains("isolated Git worktree"));
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
                model = update(model, UiEvent::Input(UiInput::NextFocus))
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
        assert!(rendered.contains("Project change"));
        assert!(rendered.contains("Technical runtime: uncertain/response_lost"));
        assert!(rendered.contains("could not confirm whether the change finished"));
        assert!(rendered.contains("response_lost"));
        assert!(
            rendered.contains("retained_worktree"),
            "worktree outcome at {size:?}:\n{rendered}"
        );
    }
}

#[test]
fn routine_project_completion_uses_the_footer_without_an_outcome_dialog() {
    let model = project_model(UiSize {
        width: 104,
        height: 20,
    });
    let details = update(model, UiEvent::Input(UiInput::Activate)).expect("project details");
    let composer = update(details.model, UiEvent::Input(UiInput::Character('n')))
        .expect("project instructions");
    let typed = update(
        composer.model,
        UiEvent::Input(UiInput::Paste("ship it".to_owned())),
    )
    .expect("instruction text");
    let pending = update(typed.model, UiEvent::Input(UiInput::Activate)).expect("send");
    let (effect_id, action) = pending
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::SubmitProjectCommand { id, action } => Some((*id, action.clone())),
            _ => None,
        })
        .expect("project effect");
    let completed = update(
        pending.model,
        UiEvent::ProjectCommandCompleted {
            effect_id,
            result: UiProjectResult {
                action,
                command_id: [11; 32],
                operation_id: [12; 32],
                project_id: [1; 32],
                runtime_state: None,
                runtime_code: None,
                outcome: UiProjectOutcome::InputSent {
                    message_id: [13; 32],
                },
            },
        },
    )
    .expect("routine completion")
    .model;
    let rendered = render_text(&completed);
    assert!(rendered.contains("Done · Instructions sent"), "{rendered}");
    assert!(!rendered.contains("Project change"), "{rendered}");
    assert!(!rendered.contains("Technical message ID"), "{rendered}");
}

#[test]
fn project_creation_chooser_leads_with_folder_ownership_and_discloses_worktrees() {
    let chooser = update(
        project_model(UiSize {
            width: 100,
            height: 22,
        }),
        UiEvent::Input(UiInput::Character('c')),
    )
    .expect("project creation chooser")
    .model;
    let rendered = render_text(&chooser);
    assert!(rendered.contains("Create project"), "{rendered}");
    assert!(rendered.contains("Use an existing folder"), "{rendered}");
    assert!(rendered.contains("recommended"), "{rendered}");
    assert!(
        rendered.contains("Create an isolated Git worktree"),
        "{rendered}"
    );
    assert!(rendered.contains("optional advanced"), "{rendered}");

    let form = update(chooser, UiEvent::Input(UiInput::Activate))
        .expect("existing folder form")
        .model;
    let form = update(
        form,
        UiEvent::Input(UiInput::Paste("/work/customer-api".to_owned())),
    )
    .expect("folder path")
    .model;
    let form = update(form, UiEvent::Input(UiInput::NextFocus))
        .expect("default project name")
        .model;
    let rendered = render_text(&form);
    assert!(rendered.contains("Name: customer-api"), "{rendered}");
    assert!(!rendered.contains("customer-api (required)"), "{rendered}");
    assert!(!rendered.contains("customer-api│"), "{rendered}");
    assert!(rendered.contains("claim this folder"), "{rendered}");
    assert!(rendered.contains("overlapping folders"), "{rendered}");
}

#[test]
fn project_form_explains_empty_fields_without_persistent_hints_or_pipe_glyphs() {
    let mut model = project_model(UiSize {
        width: 120,
        height: 24,
    })
    .with_home_directory(Some("/Users/example".to_owned()));
    model = update(model, UiEvent::Input(UiInput::Character('c')))
        .expect("creation chooser")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("existing form")
        .model;
    let empty = update(model, UiEvent::Input(UiInput::Activate))
        .expect("validate form")
        .model;
    let rendered = render_text(&empty);
    assert!(rendered.contains("Path:"), "{rendered}");
    assert!(rendered.contains("(required)"), "{rendered}");
    assert!(!rendered.contains("Path: │"), "{rendered}");
    assert!(rendered.contains("Enter a path"), "{rendered}");

    let model = update(empty, UiEvent::Input(UiInput::Paste("~/repo".to_owned())))
        .expect("path")
        .model;
    let rendered = render_text(&model);
    assert!(rendered.contains("Path: ~/repo"), "{rendered}");
    assert!(!rendered.contains("~/repo (required)"), "{rendered}");
    assert!(!rendered.contains("~/repo│"), "{rendered}");
    assert!(
        rendered.contains("Will use: /Users/example/repo"),
        "{rendered}"
    );
    assert!(rendered.contains("Tab/Shift-Tab field"), "{rendered}");

    let model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("name field")
        .model;
    let model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("brief field")
        .model;
    let model = update(
        model,
        UiEvent::Input(UiInput::Paste("Keep the release focused".to_owned())),
    )
    .expect("brief text")
    .model;
    let rendered = render_text(&model);
    assert!(
        rendered.contains("Brief: Keep the release focused"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("Keep the release focused (optional)"),
        "{rendered}"
    );
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
        assert!(rendered.contains("Folders and resources"));
        if size.height >= 24 {
            assert!(rendered.contains("check selected"));
        }

        for (key, expected) in [
            ('a', "Add a folder or resource"),
            ('e', "Change a folder or resource"),
            ('x', "Remove this project resource?"),
            ('p', "Use as the primary project resource?"),
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
                            project_id: [1; 32],
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
        assert!(rendered.contains("Check a folder or resource before adding it"));
        assert!(rendered.contains("descendant"));
        assert!(rendered.contains("Another project already owns this path"));
        assert!(rendered.contains("project ‘release’ owns /other"));
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
        assert!(rendered.contains("Set up project work"));
        if size.width >= 120 {
            assert!(rendered.contains("start a new conversation"));
        } else {
            assert!(rendered.contains("Conversation: new"));
        }
        assert!(rendered.contains("agent-5"));
        assert!(rendered.contains("Agent service: Codex · default"));
        assert!(!rendered.contains("Provider namespace"));
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
        assert!(rendered.contains("Move project work to another agent"));
        assert!(
            rendered.contains("I understand: No"),
            "handoff at {size:?}:\n{rendered}"
        );
        if size.width >= 120 {
            assert!(rendered.contains("Override safety check: No"));
            assert!(rendered.contains("may still be running elsewhere"));
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
            assert!(rendered.contains("I understand: No"));
            assert!(rendered.contains("keeps folders"));
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

#[test]
fn ordinary_surfaces_use_user_intentions_and_label_technical_evidence() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 18,
        },
    ] {
        let workspace = render_text(&ready_model(size));
        assert!(workspace.contains("Connected"));
        assert!(!workspace.contains("authoritative"));
        assert!(!workspace.contains("revision"));

        let project = update(project_model(size), UiEvent::Input(UiInput::Activate))
            .expect("project details")
            .model;
        let project_details = render_text(&project);
        assert!(project_details.contains("Folders and resources"));
        assert!(
            project_details.contains("Assigned agent"),
            "project details at {size:?}:\n{project_details}"
        );
        assert!(project_details.contains("Technical details"));
        assert!(!project_details.contains("Desired resources"));
        assert!(!project_details.contains("runnable true"));

        let activation = update(project, UiEvent::Input(UiInput::Character('v')))
            .expect("project setup")
            .model;
        let activation = render_text(&activation);
        assert!(activation.contains("Set up project work"));
        assert!(activation.contains("Conversation:"));
        assert!(activation.contains("Agent service:"));
        assert!(activation.contains("Working folder:"));
        assert!(!activation.contains("Activate project assignment"));

        let agent = agent_details_model(size);
        let agent_details = render_text(&agent);
        assert!(agent_details.contains("Saved conversations"));
        assert!(agent_details.contains("Technical details"));
        assert!(!agent_details.contains("Durable sessions"));

        let provider = update(agent, UiEvent::Input(UiInput::Character('s')))
            .expect("agent service choice")
            .model;
        let provider = render_text(&provider);
        assert!(provider.contains("Start an agent conversation"));
        assert!(provider.contains("Choose the service"));
        assert!(provider.contains("Codex · configured default"));
        assert!(provider.contains("Offline service · unavailable"));
        assert!(!provider.contains("Provider namespace"));

        let conversation = update(conversation_model(size), UiEvent::Input(UiInput::NextItem))
            .expect("select conversation update")
            .model;
        let conversation = render_text(&conversation);
        assert!(
            conversation.contains("update · information only"),
            "conversation at {size:?}:\n{conversation}"
        );
        assert!(!conversation.contains("non-actionable"));

        let human = loaded_snapshot_model(
            size,
            UiSnapshot {
                human_state: UiHumanState::NeedsAttention(
                    hq_tui::UiHumanIssue::SelectedWithoutAuthority {
                        account_id: [7; 32],
                        selection_frontier: vec![[8; 32]],
                    },
                ),
                ..empty_render_snapshot(9)
            },
        );
        let human = render_text(&human);
        assert!(human.contains("This device is not allowed to use the selected account"));
        assert!(!human.contains("authority"));
    }
}

#[test]
fn empty_and_stale_provider_catalogs_explain_why_a_conversation_cannot_start() {
    let size = UiSize {
        width: 88,
        height: 20,
    };
    let empty =
        agent_details_model_with_status_and_providers(size, UiAgentStatus::Unassigned, Vec::new());
    let empty = update(empty, UiEvent::Input(UiInput::Character('s')))
        .expect("open empty provider guidance")
        .model;
    let empty = render_text(&empty);
    assert!(empty.contains("No agent services are available on this device."));
    assert!(empty.contains("Configure or install a provider, then reload HQ."));
    assert!(empty.contains("Esc cancel"));
    assert!(!empty.contains("required"));

    let stale = agent_details_model_with_status_and_providers(
        size,
        UiAgentStatus::Unassigned,
        vec![UiProvider {
            provider: "removed".to_owned(),
            name: "Removed service".to_owned(),
            available: false,
            configured_default: true,
        }],
    );
    let stale = update(stale, UiEvent::Input(UiInput::Character('s')))
        .expect("open stale provider guidance")
        .model;
    let stale = render_text(&stale);
    assert!(stale.contains("Removed service · configured default · unavailable"));
    assert!(stale.contains("No agent services are available"));
    assert!(!stale.contains("removed"));
}

#[test]
fn failure_codes_stay_in_technical_help_while_the_footer_explains_recovery() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 18,
        },
    ] {
        let refreshing = update(ready_model(size), UiEvent::Invalidated { revision: 43 })
            .expect("request refresh");
        let effect_id = refreshing
            .effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::LoadSnapshot { id } => Some(*id),
                _ => None,
            })
            .expect("snapshot request");
        let failed = update(
            refreshing.model,
            UiEvent::SnapshotFailed {
                effect_id,
                failure: UiFailure {
                    code: "relay.transport_unavailable".to_owned(),
                    action: "Check the connection and retry".to_owned(),
                },
            },
        )
        .expect("typed failure")
        .model;

        let ordinary = render_text(&failed);
        assert!(ordinary.contains("Could not complete that action"));
        assert!(ordinary.contains("Check the connection and retry"));
        assert!(!ordinary.contains("relay.transport_unavailable"));

        let help = update(failed, UiEvent::Input(UiInput::Character('?')))
            .expect("open help")
            .model;
        let technical = update(help, UiEvent::Input(UiInput::Character('t')))
            .expect("technical help")
            .model;
        let technical = render_text(&technical);
        assert!(technical.contains("relay.transport_unavailable"));
        assert!(technical.contains("Check the connection and retry"));
    }
}

fn assert_snapshot(model: &UiModel, expected: &str) {
    let before = model.clone();
    let viewport = model.viewport();
    let backend = TestBackend::new(viewport.width, viewport.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, model, &UiTheme::terminal()))
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
        .draw(|frame| render(frame, model, &UiTheme::terminal()))
        .expect("render buffer");
    assert_eq!(model, &before);
    snapshot_text(terminal.backend().buffer())
}

fn render_buffer_with_theme(model: &UiModel, theme: &UiTheme) -> Buffer {
    let viewport = model.viewport();
    let backend = TestBackend::new(viewport.width, viewport.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, model, theme))
        .expect("render themed buffer");
    terminal.backend().buffer().clone()
}

fn find_text_start(buffer: &Buffer, needle: &str) -> (u16, u16) {
    let needle = needle
        .chars()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .enumerate()
        .find_map(|(row, cells)| {
            (0..=cells.len().saturating_sub(needle.len()))
                .find(|&column| {
                    cells[column..column + needle.len()]
                        .iter()
                        .zip(&needle)
                        .all(|(cell, expected)| cell.symbol() == expected)
                })
                .map(|column| {
                    (
                        u16::try_from(column).expect("test column fits terminal coordinates"),
                        u16::try_from(row).expect("test row fits terminal coordinates"),
                    )
                })
        })
        .expect("rendered text exists")
}

fn contextual_help_model(size: UiSize, section: UiSection, selected: bool) -> UiModel {
    let rows = selected.then(|| {
        vec![UiRow {
            id: "example-id".to_owned(),
            title: "Example item".to_owned(),
            detail: "requires a decision".to_owned(),
            state: UiRowState::Attention,
            kind: match section {
                UiSection::Agents => UiRowKind::Agent,
                UiSection::Projects => UiRowKind::Project,
                UiSection::Inbox | UiSection::Sent | UiSection::Archived => UiRowKind::Conversation,
            },
        }]
    });
    let rows_for = |candidate| {
        if candidate == section {
            rows.clone().unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    let mut model = loaded_snapshot_model(
        size,
        UiSnapshot {
            revision: 42,
            human_state: UiHumanState::Ready,
            inbox_rows: rows_for(UiSection::Inbox),
            sent_rows: rows_for(UiSection::Sent),
            archived_rows: rows_for(UiSection::Archived),
            agent_rows: rows_for(UiSection::Agents),
            project_rows: rows_for(UiSection::Projects),
            direct_targets: Vec::new(),
            providers: Vec::new(),
            agents: Vec::new(),
            projects: Vec::new(),
        },
    );
    let section_steps = match section {
        UiSection::Inbox => 0,
        UiSection::Sent => 1,
        UiSection::Archived => 2,
        UiSection::Agents => 3,
        UiSection::Projects => 4,
    };
    let section_input = if size.width >= 96 {
        UiInput::NextItem
    } else {
        UiInput::NextSection
    };
    for _ in 0..section_steps {
        model = update(model, UiEvent::Input(section_input.clone()))
            .expect("select help section")
            .model;
    }
    update(model, UiEvent::Input(UiInput::Character('?')))
        .expect("open contextual help")
        .model
}

fn empty_section_model(size: UiSize, section: UiSection) -> UiModel {
    let mut model = loaded_snapshot_model(size, empty_render_snapshot(42));
    let section_steps = match section {
        UiSection::Inbox => 0,
        UiSection::Sent => 1,
        UiSection::Archived => 2,
        UiSection::Agents => 3,
        UiSection::Projects => 4,
    };
    let section_input = if size.width >= 96 {
        UiInput::NextItem
    } else {
        UiInput::NextSection
    };
    for _ in 0..section_steps {
        model = update(model, UiEvent::Input(section_input.clone()))
            .expect("select empty section")
            .model;
    }
    update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("focus empty section")
        .model
}

const fn section_help_phrase(section: UiSection) -> &'static str {
    match section {
        UiSection::Inbox => "Inbox contains messages",
        UiSection::Sent => "Sent contains conversations",
        UiSection::Archived => "Archived contains conversations",
        UiSection::Agents => "Agents are named workers",
        UiSection::Projects => "Projects describe work",
    }
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
            providers: Vec::new(),
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
        providers: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    }
}

fn onboarding_project() -> UiProject {
    UiProject {
        project_id: [1; 32],
        home: [9; 32],
        name: "release".to_owned(),
        lifecycle: "open".to_owned(),
        archived: false,
        claimable: true,
        assignment: None,
        threads: Vec::new(),
        head: [2; 32],
        input_sequence: 0,
        resources: vec![UiProjectResource {
            resource_id: [3; 32],
            display_path: "/workspace/release".to_owned(),
            canonical_path: "/workspace/release".to_owned(),
            health: "healthy".to_owned(),
            primary: true,
            active_claim: true,
            conflicting_projects: Vec::new(),
        }],
    }
}

fn onboarding_agent() -> UiAgent {
    UiAgent {
        agent_id: [2; 32],
        names: vec!["builder".to_owned()],
        mailboxes: vec![UiAgentMailbox {
            installation_id: [9; 32],
            mailbox_id: [3; 32],
        }],
        lifecycle: UiAgentLifecycle::Active,
        runnable: false,
        status: hq_tui::UiAgentStatus::Unassigned,
        sessions: Vec::new(),
    }
}

fn render_providers() -> Vec<UiProvider> {
    vec![
        UiProvider {
            provider: "alpha".to_owned(),
            name: "Alpha".to_owned(),
            available: true,
            configured_default: false,
        },
        UiProvider {
            provider: "codex".to_owned(),
            name: "Codex".to_owned(),
            available: true,
            configured_default: true,
        },
        UiProvider {
            provider: "offline".to_owned(),
            name: "Offline service".to_owned(),
            available: false,
            configured_default: false,
        },
    ]
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
    agent_details_model_with_status(size, UiAgentStatus::Unassigned)
}

fn agent_status_rows_model(size: UiSize) -> UiModel {
    let rows = [
        ("builder", "unassigned", UiRowState::Open),
        (
            "reviewer",
            "assigned to release · setting up",
            UiRowState::Open,
        ),
        (
            "operator",
            "needs attention · migration blocked",
            UiRowState::Attention,
        ),
        ("historian", "retired", UiRowState::Archived),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (title, detail, state))| UiRow {
        id: format!("{:02x}", index + 1).repeat(32),
        title: title.to_owned(),
        detail: detail.to_owned(),
        state,
        kind: UiRowKind::Agent,
    })
    .collect();
    let mut model = loaded_snapshot_model(
        size,
        UiSnapshot {
            revision: 42,
            human_state: UiHumanState::Ready,
            inbox_rows: Vec::new(),
            sent_rows: Vec::new(),
            archived_rows: Vec::new(),
            agent_rows: rows,
            project_rows: Vec::new(),
            direct_targets: Vec::new(),
            providers: Vec::new(),
            agents: Vec::new(),
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
    update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("focus agent rows")
        .model
}

fn agent_details_model_with_status(size: UiSize, status: UiAgentStatus) -> UiModel {
    agent_details_model_with_status_and_providers(size, status, render_providers())
}

fn agent_details_model_with_status_and_providers(
    size: UiSize,
    status: UiAgentStatus,
    providers: Vec<UiProvider>,
) -> UiModel {
    let agent = UiAgent {
        agent_id: [1; 32],
        names: vec!["builder".to_owned()],
        mailboxes: vec![UiAgentMailbox {
            installation_id: [2; 32],
            mailbox_id: [3; 32],
        }],
        lifecycle: UiAgentLifecycle::Active,
        runnable: true,
        status,
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
            providers,
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
        status: UiAgentStatus::Unassigned,
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
            providers: render_providers(),
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
