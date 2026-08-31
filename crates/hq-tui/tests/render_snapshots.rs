//! Deterministic terminal-buffer snapshots for every responsive layout.

#![allow(clippy::expect_used, clippy::panic)]

use hq_tui::{
    UiActivityStatus, UiAgent, UiAgentAssignmentPhase, UiAgentLifecycle, UiAgentMailbox,
    UiAgentProjectAssignment, UiAgentSession, UiAgentStatus, UiConversationActivityKind,
    UiConversationAuthor, UiConversationEntry, UiConversationEntryPresentation, UiConversationPage,
    UiEffect, UiEvent, UiFailure, UiHumanState, UiInput, UiMailboxDraft, UiMailboxDraftTarget,
    UiMessageDelivery, UiMessageState, UiModel, UiProject, UiProjectAction, UiProjectAssignment,
    UiProjectExternalWarning, UiProjectFolderAction, UiProjectManagementAction, UiProjectOutcome,
    UiProjectResource, UiProjectResourceCheck, UiProjectResourceConflict, UiProjectResult,
    UiProjectSummaryFocus, UiProjectThread, UiProvider, UiRow, UiRowKind, UiRowState, UiSection,
    UiSize, UiSnapshot, UiTechnicalSection, UiTheme, UiThemeRole, render, update,
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
            height: 24,
        },
        UiMessageState::Archived,
    ));
    assert!(archived.contains("u restore"));
    assert!(!archived.contains("a archive"));
    assert!(archived.contains("u restore · Enter info · h/← Inbox · ? help"));

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
fn locally_authored_messages_show_pending_sent_and_received_progress() {
    for (delivery, label) in [
        (UiMessageDelivery::Pending, "You · Pending"),
        (UiMessageDelivery::Sent, "You · Sent"),
        (UiMessageDelivery::Received, "You · Received"),
    ] {
        let rendered = render_text(&conversation_model_with_delivery(
            UiSize {
                width: 104,
                height: 18,
            },
            delivery,
        ));
        assert!(rendered.contains(label), "{rendered}");
    }
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
    assert!(!rendered.contains("Start project work"), "{rendered}");
    assert!(
        rendered.contains("Preparing the project conversation"),
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
fn pane_titles_distinguish_active_focus_from_selected_context() {
    let focused = Style::new()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);
    let unfocused = Style::new().fg(Color::Blue);
    let theme = UiTheme::terminal()
        .with_style(UiThemeRole::PaneTitleFocused, focused)
        .with_style(UiThemeRole::PaneTitleUnfocused, unfocused);

    let inbox = render_buffer_with_theme(
        &ready_model(UiSize {
            width: 104,
            height: 18,
        }),
        &theme,
    );
    let inbox_title = inbox
        .cell(find_text_start(&inbox, "Inbox · 3 conversations"))
        .expect("focused Inbox title");
    assert_eq!(inbox_title.fg, Color::LightCyan);
    assert!(inbox_title.modifier.contains(Modifier::BOLD));

    let conversation = render_buffer_with_theme(
        &conversation_model(UiSize {
            width: 104,
            height: 18,
        }),
        &theme,
    );
    let inbox_context = conversation
        .cell(find_text_start(&conversation, "Inbox · 3 conversations"))
        .expect("unfocused Inbox title");
    let conversation_title = conversation
        .cell(find_text_start(&conversation, "Alice"))
        .expect("focused conversation title");
    assert_eq!(inbox_context.fg, Color::Blue);
    assert_eq!(conversation_title.fg, Color::LightCyan);
    assert!(conversation_title.modifier.contains(Modifier::BOLD));

    let no_color = render_buffer_with_theme(
        &ready_model(UiSize {
            width: 104,
            height: 18,
        }),
        &UiTheme::no_color(),
    );
    let no_color_title = no_color
        .cell(find_text_start(&no_color, "Inbox · 3 conversations"))
        .expect("no-color focused Inbox title");
    assert!(no_color_title.modifier.contains(Modifier::BOLD));
    assert!(no_color_title.modifier.contains(Modifier::UNDERLINED));
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
    assert!(contains_visible_words_in_order(
        &rendered,
        "No human account is selected"
    ));
    assert!(contains_visible_words_in_order(
        &rendered,
        "hq human create"
    ));
    assert!(
        contains_visible_words_in_order(&rendered, "press F5 to continue"),
        "{rendered}"
    );
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
        contains_visible_words_in_order(
            &empty,
            "Current: add a project and choose the folder or resource it owns",
        ),
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
        conversation_target: None,
    }];
    let project_only = render_text(&loaded_snapshot_model(size, project_only));
    assert!(project_only.contains("Project ready"), "{project_only}");
    assert!(
        contains_visible_words_in_order(&project_only, "Current: create an agent to do the work"),
        "{project_only}"
    );

    let mut agent_only = empty_render_snapshot(3);
    agent_only.projects = vec![onboarding_project()];
    agent_only.agents = vec![onboarding_agent()];
    let agent_only = render_text(&loaded_snapshot_model(size, agent_only));
    assert!(agent_only.contains("Agent ready"), "{agent_only}");
    assert!(
        contains_visible_words_in_order(&agent_only, "Current: connect an agent service"),
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
        contains_visible_words_in_order(&ready, "Current: open the first project conversation"),
        "{ready}"
    );
    assert!(
        contains_visible_words_in_order(
            &ready,
            "HQ will ask you to choose only if more than one service is available",
        ),
        "{ready}"
    );
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
            assert!(
                contains_visible_words_in_order(&rendered, explanation),
                "{code} at {size:?}:\n{rendered}"
            );
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
            if section == UiSection::Inbox && size.width < 96 {
                assert!(rendered.contains("Get started with HQ"), "{rendered}");
                assert!(
                    contains_visible_words_in_order(&rendered, "Press n New"),
                    "{rendered}"
                );
            } else {
                assert!(
                    contains_visible_words_in_order(&rendered, explanation),
                    "{section:?} at {size:?}:\n{rendered}"
                );
                assert!(
                    contains_visible_words_in_order(&rendered, action),
                    "{section:?} at {size:?}:\n{rendered}"
                );
            }
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
fn conversation_layout_renders_typed_activity_after_its_earlier_message() {
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
    let message = rendered.find("Alice").expect("author rendered");
    let activity = rendered
        .find("● Work in progress…")
        .expect("activity rendered");
    assert!(message < activity, "earlier message precedes its activity");
    assert!(rendered.contains("Can we ship?"));
    for obsolete in [
        "Conversation · complete",
        "question · peer",
        "message · open",
        "update · information only",
    ] {
        assert!(!rendered.contains(obsolete), "obsolete label: {obsolete}");
    }
}

#[test]
fn compact_conversation_focus_keeps_only_the_selected_inbox_summary() {
    let rendered = render_text(&conversation_model(UiSize {
        width: 72,
        height: 20,
    }));
    assert!(rendered.contains("Deploy production"), "{rendered}");
    assert!(rendered.contains("waiting for approval"), "{rendered}");
    assert!(!rendered.contains("Build release"), "{rendered}");
}

#[test]
fn conversation_messages_start_at_the_pane_edge_and_selection_fills_the_row() {
    let model = conversation_model(UiSize {
        width: 120,
        height: 24,
    });
    let theme = UiTheme::terminal();
    let buffer = render_buffer_with_theme(&model, &theme);
    let (body_x, body_y) = find_text_start(&buffer, "Can we ship?");
    let divider_x = (0..body_x)
        .rev()
        .find(|x| {
            buffer
                .cell((*x, body_y))
                .is_some_and(|cell| cell.symbol() == "│")
        })
        .expect("Conversation divider");
    assert_eq!(body_x, divider_x + 1, "message body has no renderer indent");

    let selected = theme.style(UiThemeRole::ConversationSelectionFocused);
    for y in [body_y - 1, body_y, body_y + 1] {
        let final_cell = buffer.cell((119, y)).expect("full selected row");
        assert!(
            final_cell.modifier.contains(Modifier::REVERSED),
            "selection reaches the last pane cell at row {y}"
        );
        assert!(
            final_cell.modifier.contains(selected.add_modifier),
            "selection uses the focused conversation role"
        );
    }
    let author = buffer.cell((body_x, body_y - 1)).expect("author cell");
    assert_eq!(
        Some(author.fg),
        theme.style(UiThemeRole::ConversationAuthorParticipant).fg
    );
}

#[test]
fn markdown_messages_render_safely_across_widths_without_parsing_activity() {
    let message = concat!(
        "# Release\n\n",
        "**Ready** with [notes](https://example.test/release).\n\n",
        "| Item | Description |\n| --- | --- |\n| build | a deliberately wide value |",
    );
    for size in [
        UiSize {
            width: 64,
            height: 28,
        },
        UiSize {
            width: 104,
            height: 28,
        },
        UiSize {
            width: 120,
            height: 28,
        },
    ] {
        let model = conversation_model_with_content(
            size,
            UiMessageState::Open,
            None,
            message,
            "**Work** `remains raw`",
        );
        let theme = UiTheme::terminal();
        let buffer = render_buffer_with_theme(&model, &theme);
        let rendered = snapshot_text(&buffer);

        assert!(rendered.contains("# Release"), "{size:?}:\n{rendered}");
        assert!(rendered.contains("Ready"), "{size:?}:\n{rendered}");
        assert!(!rendered.contains("**Ready**"), "{size:?}:\n{rendered}");
        assert!(
            rendered.contains("notes (https://example.test/release)"),
            "{size:?}:\n{rendered}"
        );
        assert!(
            rendered.contains("● **Work** `remains raw`"),
            "{size:?}:\n{rendered}"
        );

        let (ready_x, ready_y) = find_text_start(&buffer, "Ready");
        let ready = buffer
            .cell((ready_x, ready_y))
            .expect("styled message cell");
        assert!(ready.modifier.contains(Modifier::BOLD));
        assert!(ready.modifier.contains(Modifier::REVERSED));
    }

    let narrow = conversation_model_with_content(
        UiSize {
            width: 64,
            height: 28,
        },
        UiMessageState::Open,
        None,
        message,
        "activity",
    );
    let anchor = narrow.conversation_anchor().map(str::to_owned);
    let resized = update(
        narrow,
        UiEvent::Resized(UiSize {
            width: 120,
            height: 28,
        }),
    )
    .expect("resize conversation")
    .model;
    assert_eq!(resized.conversation_anchor(), anchor.as_deref());
    assert!(render_text(&resized).contains("a deliberately wide value"));
}

#[test]
fn technical_details_are_in_pane_and_keep_exact_activity_content() {
    for size in [
        UiSize {
            width: 120,
            height: 24,
        },
        UiSize {
            width: 64,
            height: 20,
        },
    ] {
        let activity = update(conversation_model(size), UiEvent::Input(UiInput::NextItem))
            .expect("select activity");
        let details = update(activity.model, UiEvent::Input(UiInput::Activate))
            .expect("open technical details")
            .model;
        let rendered = render_text(&details);
        assert!(rendered.contains("Activity details"), "{rendered}");
        assert!(rendered.contains("compiling"), "{rendered}");
        assert!(rendered.contains("sequence: 2"), "{rendered}");
        assert!(rendered.contains("h/← close details"), "{rendered}");
        assert!(!rendered.contains("activity sequence="), "{rendered}");
    }
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
            height: 24,
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
                    content: "bounded **draft** text".to_owned(),
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
        assert!(
            rendered.contains("bounded **draft** text"),
            "{size:?}:\n{rendered}"
        );
        assert!(rendered.contains('│'));
        assert!(rendered.contains("Enter send · Esc save and close"));
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
        .cell((name_x + 6, name_y))
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
            height: 24,
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
        assert!(rendered.contains("retained this request for recovery"));
        assert!(
            rendered
                .find("could not confirm whether the change finished")
                .expect("human explanation")
                < rendered
                    .find("Technical details:")
                    .or_else(|| rendered.find("Technical IDs:"))
                    .expect("technical details at the bottom"),
            "technical details should follow the human explanation:\n{rendered}"
        );
        assert!(rendered.contains("response_lost"));
        assert!(
            rendered.contains("retained_worktree"),
            "worktree outcome at {size:?}:\n{rendered}"
        );
    }
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
    assert!(rendered.contains("optional"), "{rendered}");
    assert!(rendered.contains("advanced"), "{rendered}");

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
    assert!(
        rendered.contains("this project will claim this folder"),
        "{rendered}"
    );
    assert!(rendered.contains("in HQ."), "{rendered}");
    assert!(rendered.contains("or overlapping"), "{rendered}");
    assert!(rendered.contains("folders."), "{rendered}");
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
fn one_line_fields_fill_the_dialog_and_float_empty_requirements() {
    let mut model = project_model(UiSize {
        width: 120,
        height: 24,
    });
    model = update(model, UiEvent::Input(UiInput::Character('c')))
        .expect("project creation chooser")
        .model;
    model = update(model, UiEvent::Input(UiInput::Activate))
        .expect("existing-folder form")
        .model;

    let unfocused = Color::DarkGray;
    let focused = Color::Blue;
    let caret = Color::White;
    let theme = UiTheme::terminal()
        .with_style(UiThemeRole::InputField, Style::new().bg(unfocused))
        .with_style(UiThemeRole::InputFieldFocused, Style::new().bg(focused))
        .with_style(UiThemeRole::Cursor, Style::new().bg(caret));
    let buffer = render_buffer_with_theme(&model, &theme);
    let (path_x, path_y) = find_text_start(&buffer, "Path:");
    let gap = buffer.cell((path_x + 5, path_y)).expect("label gap");
    assert_eq!(gap.symbol(), " ");
    assert_ne!(gap.bg, focused);
    let input_start = path_x + 6;
    assert_eq!(
        buffer.cell((input_start, path_y)).expect("caret cell").bg,
        caret
    );
    let (required_x, required_y) = find_text_start(&buffer, "(required)");
    assert_eq!(required_y, path_y);
    let right_border = required_x + u16::try_from("(required)".len()).expect("short hint width");
    for x in input_start + 1..right_border {
        let cell = buffer.cell((x, path_y)).expect("focused field cell");
        assert_eq!(cell.bg, focused, "field cell {x} was not focused");
    }

    let unfocused_model = update(model, UiEvent::Input(UiInput::NextFocus))
        .expect("move to project name")
        .model;
    let buffer = render_buffer_with_theme(&unfocused_model, &theme);
    let (path_x, path_y) = find_text_start(&buffer, "Path:");
    let input_start = path_x + 6;
    assert_eq!(
        buffer
            .cell((input_start, path_y))
            .expect("unfocused input surface")
            .bg,
        unfocused
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
        let folders =
            open_project_management_action(project_model(size), UiProjectManagementAction::Folders);
        let rendered = render_text(&folders);
        assert!(rendered.contains("Folders"));
        assert!(rendered.contains("Add folder"));
        if size.width >= 120 {
            assert!(rendered.contains("Check folder now"));
        }

        let add = open_project_folder_action(project_model(size), UiProjectFolderAction::AddFolder);
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
        let activation = open_project_management_action(
            project_model(size),
            UiProjectManagementAction::AssignAgent,
        );
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
        let handoff = open_project_management_action(
            project_model_with_assignment(size, true),
            UiProjectManagementAction::ChangeAssignedAgent,
        );
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
        let previewing = trigger_project_management_action(
            project_model(size),
            UiProjectManagementAction::CloseProject,
        );
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
        assert!(cancelled.model.project_interaction().is_none());
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

        let archive = open_project_management_action(
            project_model(size),
            UiProjectManagementAction::ArchiveProject,
        );
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

    let closed_model = project_model_with_state(
        UiSize {
            width: 100,
            height: 20,
        },
        false,
        "closed",
        false,
    );
    let opening = trigger_project_management_action(
        closed_model.clone(),
        UiProjectManagementAction::ReopenProject,
    );
    assert!(opening.effects.iter().any(|effect| matches!(
        effect,
        UiEffect::SubmitProjectCommand {
            action: UiProjectAction::Open { .. },
            ..
        }
    )));
    let archived_model = project_model_with_state(
        UiSize {
            width: 100,
            height: 20,
        },
        false,
        "closed",
        true,
    );
    let unarchive = open_project_management_action(
        archived_model,
        UiProjectManagementAction::RestoreArchivedProject,
    );
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

        let project = update(
            project_model(size),
            UiEvent::Input(UiInput::MoveCursorRight),
        )
        .expect("project summary")
        .model;
        let project_details = render_text(&project);
        assert!(project_details.contains("Folders"));
        assert!(
            project_details.contains("Agent"),
            "project details at {size:?}:\n{project_details}"
        );
        assert!(project_details.contains("Manage project"));
        assert!(!project_details.contains("Desired resources"));
        assert!(!project_details.contains("runnable true"));

        let activation = open_project_management_action(
            project_model(size),
            UiProjectManagementAction::AssignAgent,
        );
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
            conversation.contains("● Work in progress…"),
            "conversation at {size:?}:\n{conversation}"
        );
        assert!(!conversation.contains("update · information only"));
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
        assert!(contains_visible_words_in_order(
            &human,
            "This device is not allowed to use the selected account"
        ));
        assert!(!human.contains("authority"));
    }
}

#[test]
fn projects_workspace_uses_persistent_wide_summary_and_compact_one_level_detail() {
    let wide = render_text(&project_model(UiSize {
        width: 120,
        height: 24,
    }));
    assert!(wide.contains("Projects · 1 project"));
    assert!(wide.contains("Project · release"));
    assert!(wide.contains("Start conversation"));
    assert!(wide.contains("Agent · No agent assigned"));
    assert!(wide.contains("Folders · 1"));
    assert!(wide.contains("Manage project…"));
    assert!(!wide.contains("Project details"));
    assert!(!wide.contains("a add · e replace"));

    let compact_model = project_model(UiSize {
        width: 64,
        height: 18,
    });
    let compact_list = render_text(&compact_model);
    assert!(compact_list.contains("Projects · 1 project"));
    assert!(!compact_list.contains("Project · release"));
    let compact_detail = update(compact_model, UiEvent::Input(UiInput::MoveCursorRight))
        .expect("open compact project detail")
        .model;
    let no_color_buffer = render_buffer_with_theme(&compact_detail, &UiTheme::no_color());
    let no_color = snapshot_text(&no_color_buffer);
    let compact_detail = render_text(&compact_detail);
    assert!(compact_detail.contains("Projects / release"));
    assert!(no_color.contains('›'), "no-color project focus: {no_color}");
    assert!(
        compact_detail.contains("h/← Projects"),
        "compact project detail:\n{compact_detail}"
    );
    assert!(!compact_detail.contains("Project details"));

    let filtered = update(
        project_model(UiSize {
            width: 120,
            height: 24,
        }),
        UiEvent::Input(UiInput::Activate),
    )
    .expect("start project conversation")
    .model;
    let filtered = render_text(&filtered);
    assert!(
        filtered.contains("Project: release")
            && filtered.contains("Esc clear")
            && filtered.contains("filter"),
        "filtered Inbox:\n{filtered}"
    );
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

fn visible_words(rendered: &str) -> String {
    rendered
        .chars()
        .map(|character| match character {
            '│' | '─' | '┌' | '┐' | '└' | '┘' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_visible_words_in_order(rendered: &str, expected: &str) -> bool {
    let visible = visible_words(rendered);
    let visible_words = visible
        .split_whitespace()
        .map(canonical_visible_word)
        .collect::<Vec<_>>();
    let mut visible_words = visible_words.iter();
    expected
        .split_whitespace()
        .map(canonical_visible_word)
        .all(|expected_word| visible_words.any(|visible_word| visible_word == &expected_word))
}

fn canonical_visible_word(word: &str) -> String {
    word.trim_matches(|character: char| !character.is_alphanumeric())
        .to_owned()
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
            conversation_target: None,
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

fn ready_transition(size: UiSize) -> hq_tui::UiTransition {
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
    update(focused.model, UiEvent::Input(UiInput::NextItem)).expect("select second row")
}

fn ready_model(size: UiSize) -> UiModel {
    ready_transition(size).model
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
        pending_inputs: Vec::new(),
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
        conversation_target: None,
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
        conversation_target: None,
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
                conversation_target: None,
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

fn open_project_management(mut model: UiModel) -> UiModel {
    model = update(model, UiEvent::Input(UiInput::MoveCursorRight))
        .expect("open project summary")
        .model;
    for _ in 0..5 {
        if model.project_summary_focus() == Some(UiProjectSummaryFocus::Manage) {
            break;
        }
        model = update(model, UiEvent::Input(UiInput::NextItem))
            .expect("choose summary card")
            .model;
    }
    update(model, UiEvent::Input(UiInput::Activate))
        .expect("open project management")
        .model
}

fn select_project_management_action(model: UiModel, action: UiProjectManagementAction) -> UiModel {
    let mut model = open_project_management(model);
    for _ in 0..8 {
        if model.project_management_action() == Some(action) {
            return model;
        }
        model = update(model, UiEvent::Input(UiInput::NextItem))
            .expect("choose project action")
            .model;
    }
    panic!("project action unavailable: {action:?}");
}

fn trigger_project_management_action(
    model: UiModel,
    action: UiProjectManagementAction,
) -> hq_tui::UiTransition {
    update(
        select_project_management_action(model, action),
        UiEvent::Input(UiInput::Activate),
    )
    .expect("activate project action")
}

fn open_project_management_action(model: UiModel, action: UiProjectManagementAction) -> UiModel {
    trigger_project_management_action(model, action).model
}

fn trigger_project_folder_action(
    model: UiModel,
    action: UiProjectFolderAction,
) -> hq_tui::UiTransition {
    let mut model = open_project_management_action(model, UiProjectManagementAction::Folders);
    for _ in 0..8 {
        if model.project_folder_action() == Some(action) {
            return update(model, UiEvent::Input(UiInput::Activate))
                .expect("activate folder action");
        }
        model = update(model, UiEvent::Input(UiInput::NextItem))
            .expect("choose folder action")
            .model;
    }
    panic!("folder action unavailable: {action:?}");
}

fn open_project_folder_action(model: UiModel, action: UiProjectFolderAction) -> UiModel {
    trigger_project_folder_action(model, action).model
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
        pending_inputs: Vec::new(),
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
                conversation_target: None,
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
    conversation_model_with_state_and_delivery(size, state, None)
}

fn conversation_model_with_delivery(size: UiSize, delivery: UiMessageDelivery) -> UiModel {
    conversation_model_with_state_and_delivery(size, UiMessageState::Open, Some(delivery))
}

fn conversation_model_with_state_and_delivery(
    size: UiSize,
    state: UiMessageState,
    delivery: Option<UiMessageDelivery>,
) -> UiModel {
    conversation_model_with_content(size, state, delivery, "Can we ship?", "Work in progress…")
}

fn conversation_model_with_content(
    size: UiSize,
    state: UiMessageState,
    delivery: Option<UiMessageDelivery>,
    body: &str,
    activity_summary: &str,
) -> UiModel {
    let ready = ready_transition(size);
    let effect_id = ready
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadConversation { id, .. } => Some(*id),
            _ => None,
        })
        .expect("conversation request");
    let opening =
        update(ready.model, UiEvent::Input(UiInput::Activate)).expect("open conversation");
    let opened = update(
        opening.model,
        UiEvent::ConversationLoaded {
            effect_id,
            page: UiConversationPage {
                title: "Alice".to_owned(),
                context: None,
                row_id: "deploy-9".to_owned(),
                entries: vec![
                    UiConversationEntry {
                        id: "message-1".to_owned(),
                        presentation: UiConversationEntryPresentation::Message {
                            author: delivery.map_or_else(
                                || UiConversationAuthor::Participant("Alice".to_owned()),
                                |_| UiConversationAuthor::You,
                            ),
                            body: body.to_owned(),
                        },
                        message_state: Some(state),
                        delivery,
                        message_target: Some(hq_tui::UiMessageTarget {
                            message_id: [1; 32],
                            reply_allowed: true,
                        }),
                        technical: Vec::new(),
                    },
                    UiConversationEntry {
                        id: "activity-2".to_owned(),
                        presentation: UiConversationEntryPresentation::Activity {
                            kind: UiConversationActivityKind::Progress,
                            status: UiActivityStatus::Running,
                            summary: activity_summary.to_owned(),
                            detail: "compiling".to_owned(),
                            truncated: false,
                        },
                        message_state: None,
                        delivery: None,
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
    .expect("conversation page");
    update(opened.model, UiEvent::Input(UiInput::PreviousItem))
        .expect("select message")
        .model
}
