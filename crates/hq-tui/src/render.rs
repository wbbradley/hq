//! Borrowed responsive Ratatui renderer.

use std::fmt::Write as _;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    UiActivityStatus, UiAgent, UiAgentLifecycle, UiAgentModal, UiConnectionState,
    UiConversationEntry, UiConversationEntryKind, UiFocus, UiHumanState, UiMailboxAction,
    UiMailboxDraftTarget, UiMailboxModal, UiManagedSessionAction, UiManagedSessionOutcome,
    UiMessageState, UiModel, UiProjectAction, UiProjectFormField, UiProjectModal, UiProjectOutcome,
    UiProjectThread, UiRow, UiRowState, UiSection, UiTechnicalSection, model::WIDE_WIDTH,
};

const MINIMUM_WIDTH: u16 = 40;
const MINIMUM_HEIGHT: u16 = 10;
const NAVIGATION_WIDTH: u16 = 24;

/// Renders the complete model without mutation or I/O.
pub fn render(frame: &mut Frame<'_>, model: &UiModel) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::new().bg(Color::Reset)), area);
    if area.width < MINIMUM_WIDTH || area.height < MINIMUM_HEIGHT {
        render_too_small(frame, model, area);
        return;
    }

    let [header, content, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(area);
    render_header(frame, model, header);
    if area.width >= WIDE_WIDTH {
        render_wide_content(frame, model, content);
    } else {
        render_compact_content(frame, model, content);
    }
    render_footer(frame, model, footer);
    render_mailbox_modal(frame, model, content);
    render_agent_modal(frame, model, content);
    render_project_modal(frame, model, content);
}

#[allow(clippy::too_many_lines)]
fn render_project_modal(frame: &mut Frame<'_>, model: &UiModel, available: Rect) {
    let Some(interaction) = model.project_modal() else {
        return;
    };
    let width = available.width.saturating_sub(4).clamp(1, 88);
    let height = available.height.saturating_sub(2).clamp(1, 22);
    let area = Rect {
        x: available.x + available.width.saturating_sub(width) / 2,
        y: available.y + available.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let (title, lines) = match interaction {
        UiProjectModal::Search { query } => (
            " Search projects ",
            vec![
                Line::from(format!("Query: {query}")),
                Line::default(),
                Line::from("Type to match names, resource paths, or stable IDs"),
                Line::from("↑/↓ cycle matches · Enter inspect · Esc keep query"),
            ],
        ),
        UiProjectModal::Details {
            project,
            selected_resource,
        } => {
            let mut lines = vec![
                Line::styled(project.name.as_str(), Style::new().fg(Color::Cyan).bold()),
                Line::from(format!("Identity: {}", short_identity(project.project_id))),
                Line::from(format!(
                    "Lifecycle: {} · archived: {} · claimable: {}",
                    project.lifecycle, project.archived, project.claimable
                )),
                Line::from(format!(
                    "Head: {} · next input: {}",
                    short_identity(project.head),
                    project.input_sequence
                )),
                Line::default(),
                Line::styled("Desired resources", Style::new().fg(Color::Cyan)),
            ];
            for resource in &project.resources {
                let selected = *selected_resource == Some(resource.resource_id);
                lines.push(Line::from(format!(
                    " {} {}{} · {} · {}",
                    if selected { '›' } else { ' ' },
                    if resource.primary { "primary " } else { "" },
                    resource.display_path,
                    resource.health,
                    if resource.active_claim {
                        "claimed"
                    } else {
                        "unclaimed"
                    }
                )));
            }
            if project.resources.is_empty() {
                lines.push(Line::from(" No desired resources"));
            }
            lines.push(Line::default());
            lines.push(Line::styled("Assignment", Style::new().fg(Color::Cyan)));
            if let Some(assignment) = &project.assignment {
                lines.push(Line::from(format!(
                    "{} · agent {} · provider {} · runnable {}",
                    assignment.phase,
                    short_identity(assignment.agent_id),
                    assignment.provider,
                    assignment.runnable
                )));
                if let Some(blocked) = &assignment.blocked {
                    lines.push(Line::styled(
                        format!("Blocked: {blocked}"),
                        Style::new().fg(Color::Yellow),
                    ));
                }
                if assignment.cardinality_conflicted {
                    lines.push(Line::styled(
                        "Assignment cardinality conflict",
                        Style::new().fg(Color::Red),
                    ));
                }
            } else {
                lines.push(Line::from("Unassigned"));
            }
            lines.push(Line::default());
            lines.push(Line::from("↑/↓ resource · a add · e replace · x remove"));
            lines.push(Line::from(
                "p primary · k check selected · K check all · n send input",
            ));
            lines.push(Line::from("v activate · d dispatch pending · h handoff"));
            lines.push(Line::from(if project.lifecycle == "closed" {
                "o reopen · z archive/unarchive"
            } else {
                "c assess and close · z archive/unarchive"
            }));
            (" Project details ", lines)
        }
        UiProjectModal::CreateExisting {
            name,
            brief,
            path,
            field,
            submitting,
        } => (
            " Create project from existing tree ",
            vec![
                project_field_line("Name", name, *field == UiProjectFormField::Name),
                project_field_line("Brief", brief, *field == UiProjectFormField::Brief),
                project_field_line("Path", path, *field == UiProjectFormField::Path),
                Line::default(),
                Line::from(if *submitting {
                    "Reconciling one stable create operation…"
                } else {
                    "↑/↓ field · Enter create · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::CreateWorktree {
            name,
            brief,
            source,
            destination,
            branch,
            base,
            field,
            submitting,
        } => (
            " Create recoverable Git worktree project ",
            vec![
                project_field_line("Name", name, *field == UiProjectFormField::Name),
                project_field_line("Brief", brief, *field == UiProjectFormField::Brief),
                project_field_line("Source", source, *field == UiProjectFormField::Source),
                project_field_line(
                    "Destination",
                    destination,
                    *field == UiProjectFormField::Destination,
                ),
                project_field_line("Branch", branch, *field == UiProjectFormField::Branch),
                project_field_line("Base", base, *field == UiProjectFormField::Base),
                Line::default(),
                Line::from(if *submitting {
                    "Reconciling worktree provisioning without removing external state…"
                } else {
                    "↑/↓ field · Enter provision · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::SendInput {
            project,
            content,
            submitting,
        } => (
            " Send project input ",
            vec![
                Line::from(format!("Project: {}", project.name)),
                project_field_line("Input", content, true),
                Line::default(),
                Line::from(if *submitting {
                    "Submitting through the ordinary local API…"
                } else {
                    "Enter send · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::AddResource {
            project,
            path,
            make_primary,
            submitting,
        } => (
            " Add desired resource ",
            vec![
                Line::from(format!("Project: {}", project.name)),
                project_field_line("Path", path, true),
                Line::from(format!("Make primary: {make_primary}")),
                Line::default(),
                Line::from(if *submitting {
                    "Inspecting canonical identity and claim conflicts…"
                } else {
                    "↑/↓ toggle primary · Enter preview · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::ReplaceResource {
            project,
            resource_id,
            path,
            submitting,
        } => (
            " Replace desired resource ",
            vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!("Replace: {}", short_identity(*resource_id))),
                project_field_line("Path", path, true),
                Line::default(),
                Line::from(if *submitting {
                    "Inspecting canonical identity and claim conflicts…"
                } else {
                    "Enter preview · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::ConfirmRemoveResource {
            project,
            resource_id,
            force,
            submitting,
        } => (
            " Confirm desired-resource removal ",
            vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!("Resource: {}", short_identity(*resource_id))),
                Line::from(format!(
                    "Assigned: {} · force: {force}",
                    project.assignment.is_some()
                )),
                Line::from("External paths, files, worktrees, and branches are retained."),
                Line::default(),
                Line::from(if *submitting {
                    "Reconciling removal…"
                } else {
                    "f toggle force · Enter remove · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::ConfirmPrimaryResource {
            project,
            resource_id,
            submitting,
        } => (
            " Confirm primary resource ",
            vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!("Resource: {}", short_identity(*resource_id))),
                Line::default(),
                Line::from(if *submitting {
                    "Reconciling primary selection…"
                } else {
                    "Enter confirm · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::Activate {
            project,
            agents,
            agent_id,
            thread,
            new_session,
            provider,
            directory,
            field,
            submitting,
        } => (
            " Activate project assignment ",
            project_activation_lines(
                project,
                agents,
                *agent_id,
                thread.as_ref(),
                *new_session,
                provider,
                directory,
                *field,
                None,
                *submitting,
            ),
        ),
        UiProjectModal::Handoff {
            project,
            agents,
            agent_id,
            thread,
            new_session,
            provider,
            directory,
            field,
            confirmed,
            force_takeover,
            submitting,
        } => (
            " Confirm project handoff ",
            project_activation_lines(
                project,
                agents,
                *agent_id,
                thread.as_ref(),
                *new_session,
                provider,
                directory,
                *field,
                Some((*confirmed, *force_takeover)),
                *submitting,
            ),
        ),
        UiProjectModal::ConfirmClose {
            project,
            checks,
            confirmed,
            force,
            submitting,
        } => {
            let mut lines = vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!(
                    "Lifecycle: {} · assigned: {}",
                    project.lifecycle,
                    project.assignment.is_some()
                )),
                Line::default(),
                Line::styled("Fresh release assessment", Style::new().fg(Color::Cyan)),
            ];
            if checks.is_empty() {
                lines.push(Line::from("No desired resources"));
            }
            for check in checks {
                lines.push(Line::from(format!(
                    "{} · {} · health={} · release={}",
                    short_identity(check.resource_id),
                    check.status,
                    check.health.as_deref().unwrap_or("unknown"),
                    check.release.as_deref().unwrap_or("unknown")
                )));
                if let Some(details) = &check.details {
                    lines.push(Line::from(format!("  {details}")));
                }
                if let (Some(category), Some(code)) = (&check.error_category, &check.error_code) {
                    lines.push(Line::styled(
                        format!("  rejected: {category}/{code}"),
                        Style::new().fg(Color::Red),
                    ));
                }
            }
            lines.push(Line::default());
            lines.push(Line::from(format!(
                "Confirmed: {confirmed} · force recovery: {force}"
            )));
            lines.push(Line::from(
                "Closing retains external paths, files, worktrees, and branches.",
            ));
            lines.push(Line::from(if *submitting {
                "Reconciling one stable close operation…"
            } else {
                "c confirm · f authorize force · Enter close · Esc cancel"
            }));
            (" Confirm project close ", lines)
        }
        UiProjectModal::ConfirmArchive {
            project,
            archived,
            submitting,
        } => (
            if *archived {
                " Confirm project archive "
            } else {
                " Confirm project unarchive "
            },
            vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!("Lifecycle: {}", project.lifecycle)),
                Line::from(if *archived {
                    "Archiving closes the project while retaining external state."
                } else {
                    "Unarchiving retains the project in its authoritative lifecycle state."
                }),
                Line::default(),
                Line::from(if *submitting {
                    "Reconciling one stable archive operation…"
                } else {
                    "Enter confirm · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::Outcome { result } => {
            let mut lines = vec![Line::from(project_action_label(&result.action))];
            lines.push(Line::from(format!(
                "Project: {} · operation: {}",
                short_identity(result.project_id),
                short_identity(result.operation_id)
            )));
            lines.push(Line::default());
            if let Some(runtime_state) = &result.runtime_state {
                lines.push(Line::from(format!(
                    "Runtime: {runtime_state}{}",
                    result
                        .runtime_code
                        .as_ref()
                        .map_or_else(String::new, |code| format!("/{code}"))
                )));
            }
            match &result.outcome {
                UiProjectOutcome::Completed { project_head } => lines.push(Line::from(format!(
                    "Completed{}",
                    project_head.map_or_else(String::new, |head| format!(
                        " at head {}",
                        short_identity(head)
                    ))
                ))),
                UiProjectOutcome::Running { stage } => {
                    lines.push(Line::from(format!("In progress: {stage}")));
                }
                UiProjectOutcome::Rejected { category, code } => lines.push(Line::styled(
                    format!("Rejected: {category}/{code}"),
                    Style::new().fg(Color::Red),
                )),
                UiProjectOutcome::Reconcilable {
                    stage,
                    category,
                    code,
                    warning,
                } => {
                    lines.push(Line::styled(
                        format!("Reconcile {stage}: {category}/{code}"),
                        Style::new().fg(Color::Yellow),
                    ));
                    if let Some(warning) = warning {
                        lines.push(Line::from(format!(
                            "Retained {}: {} ({})",
                            warning.kind, warning.destination, warning.branch
                        )));
                    }
                }
                UiProjectOutcome::InputSent { message_id } => lines.push(Line::from(format!(
                    "Input accepted as {}",
                    short_identity(*message_id)
                ))),
                UiProjectOutcome::ResourcePreview {
                    display_path,
                    canonical_path,
                    conflicts,
                } => {
                    lines.push(Line::from(format!("Display: {display_path}")));
                    lines.push(Line::from(format!("Canonical: {canonical_path}")));
                    if conflicts.is_empty() {
                        lines.push(Line::styled(
                            "No authoritative claim conflicts · Enter commit",
                            Style::new().fg(Color::Green),
                        ));
                    } else {
                        lines.push(Line::styled(
                            "Claim conflicts block mutation:",
                            Style::new().fg(Color::Red),
                        ));
                        for conflict in conflicts {
                            lines.push(Line::from(format!(
                                " {} {} · {}",
                                conflict.relationship,
                                short_identity(conflict.project_id),
                                conflict.canonical_path
                            )));
                        }
                    }
                }
                UiProjectOutcome::ResourceChecks { checks } => {
                    for check in checks {
                        lines.push(Line::from(format!(
                            "{} · {} · health={} · release={}",
                            short_identity(check.resource_id),
                            check.status,
                            check.health.as_deref().unwrap_or("unknown"),
                            check.release.as_deref().unwrap_or("unknown")
                        )));
                        if let (Some(category), Some(code)) =
                            (&check.error_category, &check.error_code)
                        {
                            lines.push(Line::styled(
                                format!("  rejected: {category}/{code}"),
                                Style::new().fg(Color::Red),
                            ));
                        }
                    }
                }
            }
            lines.push(Line::default());
            lines.push(Line::from("Esc close"));
            (" Project operation outcome ", lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn project_field_line<'value>(label: &str, value: &'value str, selected: bool) -> Line<'value> {
    Line::styled(
        format!("{} {label}: {value}", if selected { '›' } else { ' ' }),
        if selected {
            selected_style(true)
        } else {
            Style::new()
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn project_activation_lines<'value>(
    project: &'value crate::UiProject,
    agents: &'value [UiAgent],
    agent_id: Option<[u8; 32]>,
    thread: Option<&'value UiProjectThread>,
    new_session: bool,
    provider: &'value str,
    directory: &'value str,
    field: UiProjectFormField,
    handoff: Option<(bool, bool)>,
    submitting: bool,
) -> Vec<Line<'value>> {
    let agent = agent_id
        .and_then(|id| agents.iter().find(|agent| agent.agent_id == id))
        .and_then(|agent| agent.names.first().map(String::as_str))
        .unwrap_or("none");
    let thread_label = thread.map_or_else(
        || "none".to_owned(),
        |thread| {
            format!(
                "{}/{} · {}",
                thread.provider,
                thread.session,
                short_identity(thread.thread_id)
            )
        },
    );
    let mut lines = vec![
        Line::from(format!("Project: {}", project.name)),
        project_choice_line("Agent", agent, field == UiProjectFormField::Agent),
        project_choice_line(
            "Mode",
            if new_session {
                "new session"
            } else {
                "exact resume"
            },
            field == UiProjectFormField::SessionMode,
        ),
        project_choice_line("Thread", &thread_label, field == UiProjectFormField::Thread),
        project_field_line("Provider", provider, field == UiProjectFormField::Provider),
        project_field_line(
            "Directory",
            directory,
            field == UiProjectFormField::Directory,
        ),
    ];
    if let Some((confirmed, force)) = handoff {
        lines.push(project_choice_line(
            "Confirmed",
            if confirmed { "true" } else { "false" },
            field == UiProjectFormField::Confirmation,
        ));
        lines.push(project_choice_line(
            "Force takeover",
            if force { "true" } else { "false" },
            field == UiProjectFormField::Force,
        ));
        lines.push(Line::from(
            "Force revokes HQ authority; it does not prove external runtime cessation.",
        ));
    }
    lines.push(Line::default());
    lines.push(Line::from(if submitting {
        "Reconciling one stable project operation…"
    } else if handoff.is_some() {
        "Tab field · ↑/↓ change choice · Enter handoff"
    } else {
        "Tab field · ↑/↓ change choice · Enter activate"
    }));
    lines
}

fn project_choice_line(label: &str, value: &str, selected: bool) -> Line<'static> {
    Line::styled(
        format!("{} {label}: {value}", if selected { '›' } else { ' ' }),
        if selected {
            selected_style(true)
        } else {
            Style::new()
        },
    )
}

fn project_action_label(action: &UiProjectAction) -> String {
    match action {
        UiProjectAction::CreateExisting { name, .. } => format!("Create {name} from existing tree"),
        UiProjectAction::CreateWorktree { name, .. } => format!("Provision worktree for {name}"),
        UiProjectAction::SendInput { .. } => "Send project input".to_owned(),
        UiProjectAction::PreviewAddResource { .. } => {
            "Preview desired-resource addition".to_owned()
        }
        UiProjectAction::AddResource { .. } => "Add desired resource".to_owned(),
        UiProjectAction::PreviewReplaceResource { .. } => {
            "Preview desired-resource replacement".to_owned()
        }
        UiProjectAction::ReplaceResource { .. } => "Replace desired resource".to_owned(),
        UiProjectAction::RemoveResource { .. } => "Remove desired resource".to_owned(),
        UiProjectAction::SetPrimaryResource { .. } => "Select primary resource".to_owned(),
        UiProjectAction::CheckResources { .. } => "Check desired resources".to_owned(),
        UiProjectAction::Activate { .. } => "Activate project assignment".to_owned(),
        UiProjectAction::DispatchPending { .. } => "Dispatch pending project input".to_owned(),
        UiProjectAction::Handoff { .. } => "Hand off project assignment".to_owned(),
        UiProjectAction::Open { .. } => "Reopen project".to_owned(),
        UiProjectAction::PreviewClose { .. } => "Assess project close".to_owned(),
        UiProjectAction::Close { force, .. } => format!("Close project · force={force}"),
        UiProjectAction::SetArchived { archived, .. } => {
            if *archived {
                "Archive project".to_owned()
            } else {
                "Unarchive project".to_owned()
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn render_agent_modal(frame: &mut Frame<'_>, model: &UiModel, available: Rect) {
    let Some(interaction) = model.agent_modal() else {
        return;
    };
    let width = available.width.saturating_sub(4).clamp(1, 82);
    let height = available.height.clamp(1, 20);
    let area = Rect {
        x: available.x + available.width.saturating_sub(width) / 2,
        y: available.y + available.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let (title, lines) = match interaction {
        UiAgentModal::Search { query } => (
            " Search agents ",
            vec![
                Line::from(format!("Query: {query}")),
                Line::default(),
                Line::from("Type to select matching names, sessions, or IDs"),
                Line::from("↑/↓ cycle matches · Enter inspect · Esc keep query"),
            ],
        ),
        UiAgentModal::Details {
            agent,
            selected_session,
        } => {
            let mut lines = agent_summary(agent);
            lines.push(Line::default());
            lines.push(Line::styled(
                "Durable sessions",
                Style::new().fg(Color::Cyan),
            ));
            for session in &agent.sessions {
                let selected = selected_session.as_ref().is_some_and(|(provider, value)| {
                    *provider == session.provider && *value == session.session
                });
                let name = session.display_name.as_deref().unwrap_or("unnamed");
                lines.push(Line::styled(
                    format!(
                        " {} {}/{} · {}{}{}",
                        if selected { '›' } else { ' ' },
                        session.provider,
                        session.session,
                        name,
                        if session.selected { " · selected" } else { "" },
                        if session.conflicted {
                            " · conflicted"
                        } else {
                            ""
                        }
                    ),
                    if selected {
                        selected_style(true)
                    } else {
                        Style::new()
                    },
                ));
            }
            if agent.sessions.is_empty() {
                lines.push(Line::from(" No durable provider sessions"));
            }
            lines.push(Line::default());
            lines.push(Line::from(
                "↑/↓ session · s start · e exact resume · t stop",
            ));
            lines.push(Line::from("r rename/clear · x retire · Esc close"));
            (" Agent details ", lines)
        }
        UiAgentModal::Create { name, submitting } => (
            " Create named agent ",
            vec![
                Line::from(format!("Permanent lowercase name: {name}")),
                Line::default(),
                Line::from(if *submitting {
                    "Reconciling create command…"
                } else {
                    "Enter create · Esc cancel"
                }),
            ],
        ),
        UiAgentModal::RenameSession {
            provider,
            session,
            display_name,
            submitting,
            ..
        } => (
            " Rename durable session ",
            vec![
                Line::from(format!("Target: {provider}/{session}")),
                Line::from(format!("Display name: {display_name}")),
                Line::default(),
                Line::from(if *submitting {
                    "Reconciling rename command…"
                } else {
                    "Enter rename (empty clears) · Esc cancel"
                }),
            ],
        ),
        UiAgentModal::ConfirmRetire {
            agent,
            force,
            submitting,
        } => {
            let name = agent.names.first().map_or("unnamed", String::as_str);
            (
                " Confirm permanent retirement ",
                vec![
                    Line::styled(
                        format!("Retire {name}? This cannot be undone."),
                        Style::new().fg(Color::Yellow),
                    ),
                    Line::from(format!("Force assigned/runtime takeover: {force}")),
                    Line::default(),
                    Line::from(if *submitting {
                        "Reconciling retirement…"
                    } else {
                        "f toggle force · Enter confirm · Esc cancel"
                    }),
                ],
            )
        }
        UiAgentModal::ManagedProvider { provider, .. } => (
            " Start managed session ",
            vec![
                Line::from(format!("Provider namespace: {provider}")),
                Line::default(),
                Line::from("Enter continue · Esc cancel"),
            ],
        ),
        UiAgentModal::ConfirmManagedSession { action, .. } => (
            " Confirm managed-session switch ",
            vec![
                Line::styled(
                    "This target differs from the durable selected session.",
                    Style::new().fg(Color::Yellow),
                ),
                Line::from(managed_session_target(action)),
                Line::default(),
                Line::from("Runtime presence is checked by the node, not inferred here."),
                Line::from("Enter confirm · Esc cancel"),
            ],
        ),
        UiAgentModal::ManagingSession { action, .. } => (
            " Managing session ",
            vec![
                Line::from(managed_session_target(action)),
                Line::default(),
                Line::from("Reconciling one stable operation across reconnects…"),
            ],
        ),
        UiAgentModal::ManagedSessionOutcome { result, .. } => {
            let mut lines = vec![Line::from(managed_session_target(&result.action))];
            lines.push(Line::from(format!(
                "Operation: {}",
                short_identity(result.operation_id)
            )));
            lines.push(Line::default());
            match &result.outcome {
                UiManagedSessionOutcome::Ready { session } => {
                    lines.push(Line::from(format!("Ready session: {session}")));
                }
                UiManagedSessionOutcome::Stopped => {
                    lines.push(Line::from(
                        "Local runtime stopped; durable history retained.",
                    ));
                }
                UiManagedSessionOutcome::Rejected { category, code } => {
                    lines.push(Line::styled(
                        format!("Rejected: {category}/{code}"),
                        Style::new().fg(Color::Red),
                    ));
                    lines.push(Line::from("Reload and reselect an exact current target."));
                }
                UiManagedSessionOutcome::Uncertain { reconciliation_id } => {
                    lines.push(Line::styled(
                        format!("Uncertain: {}", short_identity(*reconciliation_id)),
                        Style::new().fg(Color::Yellow),
                    ));
                    lines.push(Line::from("HQ will reconcile the same stable request."));
                }
            }
            lines.push(Line::default());
            lines.push(Line::from("Esc close"));
            (" Managed-session outcome ", lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn managed_session_target(action: &UiManagedSessionAction) -> String {
    match action {
        UiManagedSessionAction::Start { provider, .. } => format!("Start fresh on {provider}"),
        UiManagedSessionAction::Resume {
            provider, session, ..
        } => format!("Resume exactly {provider}/{session}"),
        UiManagedSessionAction::Stop { provider, .. } => format!("Stop runtime on {provider}"),
    }
}

fn short_identity(identity: [u8; 32]) -> String {
    identity[..6]
        .iter()
        .fold(String::with_capacity(12), |mut rendered, byte| {
            let _ = write!(rendered, "{byte:02x}");
            rendered
        })
}

fn agent_summary(agent: &UiAgent) -> Vec<Line<'_>> {
    let lifecycle = match agent.lifecycle {
        UiAgentLifecycle::Active => "active",
        UiAgentLifecycle::Conflicted => "conflicted",
        UiAgentLifecycle::Retired => "retired",
    };
    vec![
        Line::styled(
            agent.names.first().map_or("Unnamed agent", String::as_str),
            Style::new().fg(Color::Cyan).bold(),
        ),
        Line::from(format!(
            "Identity: {:02x}{:02x}{:02x}{:02x}…",
            agent.agent_id[0], agent.agent_id[1], agent.agent_id[2], agent.agent_id[3]
        )),
        Line::from(format!(
            "Lifecycle: {lifecycle} · runnable: {} · mailboxes: {}",
            agent.runnable,
            agent.mailboxes.len()
        )),
    ]
}

#[allow(clippy::too_many_lines)]
fn render_mailbox_modal(frame: &mut Frame<'_>, model: &UiModel, available: Rect) {
    let Some(interaction) = model.mailbox_modal() else {
        return;
    };
    let width = available.width.saturating_sub(4).clamp(1, 76);
    let height = available.height.saturating_sub(2).clamp(1, 18);
    let area = Rect {
        x: available.x + available.width.saturating_sub(width) / 2,
        y: available.y + available.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    match interaction {
        UiMailboxModal::SelectDirect { targets, selected } => {
            let mut lines = vec![Line::styled(
                "Choose a resolved named agent",
                Style::new().fg(Color::Cyan),
            )];
            lines.push(Line::default());
            for target in targets {
                let is_selected = *selected == Some((target.installation_id, target.mailbox_id));
                lines.push(Line::styled(
                    format!(" {} {}", if is_selected { '›' } else { ' ' }, target.label),
                    if is_selected {
                        selected_style(true)
                    } else {
                        Style::new()
                    },
                ));
            }
            if targets.is_empty() {
                lines.push(Line::styled(
                    "No unconflicted local agent mailbox is available",
                    Style::new().fg(Color::Yellow),
                ));
            }
            lines.push(Line::default());
            lines.push(Line::from("↑/↓ select · Enter compose · Esc cancel"));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::bordered().title(" Direct message "))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        UiMailboxModal::LoadingDraft { target } => {
            frame.render_widget(
                Paragraph::new(format!("Loading {} draft…", draft_target_label(target)))
                    .block(Block::bordered().title(" Mailbox draft ")),
                area,
            );
        }
        UiMailboxModal::Compose {
            draft,
            dirty,
            submitting,
            closing,
        } => {
            let status = if *closing {
                "saving and closing"
            } else if *submitting {
                "submitting"
            } else if *dirty {
                "autosave pending"
            } else {
                "saved"
            };
            let text_area = area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 2,
            });
            frame.render_widget(
                Block::bordered().title(format!(
                    " {} · {status} · {}/{} bytes ",
                    draft_target_label(&draft.target),
                    draft.content.len(),
                    MAX_DRAFT_BYTES
                )),
                area,
            );
            frame.render_widget(
                Paragraph::new(draft.content.as_str()).wrap(Wrap { trim: false }),
                text_area,
            );
            let hint = Rect {
                y: area.y + area.height.saturating_sub(2),
                height: 1,
                x: area.x + 2,
                width: area.width.saturating_sub(4),
            };
            frame.render_widget(Paragraph::new("Enter submit · Esc save and close"), hint);
        }
        UiMailboxModal::Confirm { action } => {
            let label = match action {
                UiMailboxAction::Archive { .. } => "Archive this exact message?",
                UiMailboxAction::Restore { .. } => "Restore this exact message?",
                UiMailboxAction::Reply { .. }
                | UiMailboxAction::Direct { .. }
                | UiMailboxAction::SelfNote => "Submit this mailbox command?",
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(label),
                    Line::default(),
                    Line::from("Enter confirm · Esc cancel"),
                ])
                .block(Block::bordered().title(" Confirm mailbox action "))
                .wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

const MAX_DRAFT_BYTES: usize = 16 * 1024;

const fn draft_target_label(target: &UiMailboxDraftTarget) -> &'static str {
    match target {
        UiMailboxDraftTarget::Reply { .. } => "Reply",
        UiMailboxDraftTarget::Direct { .. } => "Direct message",
        UiMailboxDraftTarget::SelfNote => "Self-note",
    }
}

fn render_too_small(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let message = vec![
        Line::styled("HQ", Style::new().fg(Color::Cyan).bold()),
        Line::default(),
        Line::from("Terminal too small"),
        Line::from(format!(
            "Need {MINIMUM_WIDTH}x{MINIMUM_HEIGHT}; have {}x{}",
            model.viewport().width,
            model.viewport().height
        )),
        Line::default(),
        Line::from("q quit"),
    ];
    frame.render_widget(
        Paragraph::new(message)
            .block(Block::bordered().title(" HQ "))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_header(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let revision = model.snapshot().map(|snapshot| snapshot.revision);
    let status = match revision {
        Some(value) if model.refreshing() => format!("refreshing · revision {value}"),
        Some(value) => format!(
            "{} · revision {value}",
            connection_label(model.connection())
        ),
        None => connection_label(model.connection()).to_owned(),
    };
    let title = Line::from(vec![
        Span::styled(" HQ ", Style::new().fg(Color::Black).bg(Color::Cyan).bold()),
        Span::raw("  "),
        Span::styled(section_label(model.section()), Style::new().bold()),
    ]);
    let context = Line::from(vec![
        Span::styled(" local workspace ", Style::new().fg(Color::DarkGray)),
        Span::styled(status, connection_style(model.connection())),
    ]);
    frame.render_widget(
        Paragraph::new(vec![title, context]).block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn render_wide_content(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let [navigation, rows] =
        Layout::horizontal([Constraint::Length(NAVIGATION_WIDTH), Constraint::Min(1)]).areas(area);
    let navigation_lines = UiSection::ALL
        .into_iter()
        .map(|section| {
            let selected = section == model.section();
            let marker = if selected { " › " } else { "   " };
            let style = if selected {
                selected_style(model.focus() == UiFocus::Navigation)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            Line::styled(format!("{marker}{}", section_label(section)), style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(navigation_lines).block(
            Block::new()
                .borders(Borders::RIGHT)
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        navigation,
    );
    render_rows(frame, model, rows);
}

fn render_compact_content(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let [tabs, rows] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);
    let tab_line = UiSection::ALL
        .into_iter()
        .enumerate()
        .flat_map(|(index, section)| {
            let separator = (index > 0).then(|| Span::raw(" · "));
            separator.into_iter().chain(std::iter::once(Span::styled(
                section_label(section),
                if section == model.section() {
                    selected_style(model.focus() == UiFocus::Navigation)
                } else {
                    Style::new().fg(Color::DarkGray)
                },
            )))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(tab_line)).block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        tabs,
    );
    render_rows(frame, model, rows);
}

fn render_rows(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    if model.conversation().is_some() {
        if area.width >= 72 {
            let [summaries, conversation] =
                Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
                    .areas(area);
            render_summary_rows(frame, model, summaries);
            render_conversation(frame, model, conversation);
        } else {
            let [summaries, conversation] =
                Layout::vertical([Constraint::Percentage(35), Constraint::Percentage(65)])
                    .areas(area);
            render_summary_rows(frame, model, summaries);
            render_conversation(frame, model, conversation);
        }
    } else {
        render_summary_rows(frame, model, area);
    }
}

fn render_summary_rows(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let count = model.rows().map_or(0, <[UiRow]>::len);
    let mut lines = vec![
        Line::styled(
            format!(" {} · {count} items", section_label(model.section())),
            Style::new().fg(Color::Cyan).bold(),
        ),
        Line::default(),
    ];
    match model.human_state() {
        Some(UiHumanState::Unavailable) => {
            lines.push(Line::styled(
                " No active human account is currently available",
                Style::new().fg(Color::Yellow).bold(),
            ));
            lines.push(Line::from(" New: hq human create · Join: hq human join"));
            lines.push(Line::from(
                " Recover: hq relay sync / hq relay repair · Inspect: hq human show",
            ));
            lines.push(Line::default());
        }
        Some(UiHumanState::Ambiguous) => {
            lines.push(Line::styled(
                " Human account selection or authority is ambiguous",
                Style::new().fg(Color::Yellow).bold(),
            ));
            lines.push(Line::from(" Inspect and resolve with: hq human show"));
            lines.push(Line::default());
        }
        Some(UiHumanState::Ready) | None => {}
    }
    match model.rows() {
        Some([]) => {
            lines.push(Line::styled(" No items", Style::new().fg(Color::DarkGray)));
        }
        Some(rows) => {
            for row in rows {
                lines.extend(render_row(model, row));
            }
        }
        None => lines.push(Line::styled(
            " Loading authoritative snapshot…",
            Style::new().fg(Color::Yellow),
        )),
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_conversation(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let Some(conversation) = model.conversation() else {
        return;
    };
    let selected = model.conversation_anchor();
    let entry_height = if model.technical_visible() { 6 } else { 3 };
    let capacity = usize::from(area.height.saturating_sub(2))
        .checked_div(entry_height)
        .unwrap_or(0)
        .max(1);
    let selected_index = selected
        .and_then(|anchor| {
            conversation
                .entries
                .iter()
                .position(|entry| entry.id == anchor)
        })
        .unwrap_or(0);
    let start = selected_index
        .saturating_sub(capacity / 2)
        .min(conversation.entries.len().saturating_sub(capacity));
    let mut lines = Vec::new();
    for entry in conversation.entries.iter().skip(start).take(capacity) {
        lines.extend(render_conversation_entry(model, entry));
    }
    if conversation.entries.is_empty() {
        lines.push(Line::styled(
            " No conversation entries",
            Style::new().fg(Color::DarkGray),
        ));
    }
    let paging = conversation
        .next_cursor
        .as_ref()
        .map_or("complete", |_| "PageDown loads more");
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title(format!(" Conversation · {paging} "))
                    .border_style(if model.focus() == UiFocus::Conversation {
                        Style::new().fg(Color::Cyan)
                    } else {
                        Style::new().fg(Color::DarkGray)
                    }),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_conversation_entry<'entry>(
    model: &UiModel,
    entry: &'entry UiConversationEntry,
) -> Vec<Line<'entry>> {
    let selected = model.conversation_anchor() == Some(entry.id.as_str());
    let marker = if selected { " › " } else { "   " };
    let style = if selected {
        selected_style(model.focus() == UiFocus::Conversation)
    } else {
        Style::new()
    };
    let kind = match entry.kind {
        UiConversationEntryKind::Message => "message",
        UiConversationEntryKind::Activity => "activity",
    };
    let state = match entry.message_state {
        Some(UiMessageState::Open) => "open",
        Some(UiMessageState::Archived) => "archived",
        Some(UiMessageState::Rejected) => "rejected",
        None => "non-actionable",
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(marker, style),
            Span::styled(entry.summary.as_str(), style),
        ]),
        Line::from(format!("     {kind} · {state} · {}", entry.content)),
        Line::default(),
    ];
    if selected && model.technical_visible() {
        for section in &entry.technical {
            lines.push(Line::styled(
                format!("     {}", technical_summary(section)),
                Style::new().fg(Color::DarkGray),
            ));
        }
    }
    lines
}

fn technical_summary(section: &UiTechnicalSection) -> String {
    match section {
        UiTechnicalSection::Routing { sender, recipient } => format!(
            "routing sender={} recipient={}",
            short_technical(sender),
            recipient.as_deref().map_or("account", short_technical)
        ),
        UiTechnicalSection::Semantics {
            purpose,
            presentation,
            provider,
            session,
            operation,
            project,
        } => {
            format!(
                "semantics purpose={purpose} presentation={presentation} operation={} project={}",
                operation.as_deref().map_or("none", short_technical),
                project.as_deref().map_or("none", short_technical)
            ) + &provider
                .as_ref()
                .zip(session.as_ref())
                .map_or_else(String::new, |(provider, session)| {
                    format!(" provider={provider}/{session}")
                })
        }
        UiTechnicalSection::Evidence {
            message_id,
            thread_id,
            state_frontier,
            peer_received_by,
            root_fact,
            root_message,
            ready_answer,
            thread_cancelled,
        } => format!(
            "evidence message={} thread={} frontier={} receipts={} root_fact={} root={} ready={} cancelled={}",
            short_technical(message_id),
            short_technical(thread_id),
            state_frontier.len(),
            peer_received_by.len(),
            root_fact.as_deref().map_or("none", short_technical),
            root_message.as_deref().map_or("none", short_technical),
            ready_answer,
            thread_cancelled
        ),
        UiTechnicalSection::Activity {
            sequence,
            status,
            truncated,
        } => format!(
            "activity sequence={sequence} status={} truncated={truncated}",
            activity_status_label(status)
        ),
    }
}

fn activity_status_label(status: &UiActivityStatus) -> String {
    match status {
        UiActivityStatus::Snapshot => "snapshot".to_owned(),
        UiActivityStatus::Running => "running".to_owned(),
        UiActivityStatus::Succeeded => "succeeded".to_owned(),
        UiActivityStatus::Failed { reason } => format!("failed:{reason}"),
        UiActivityStatus::Interrupted => "interrupted".to_owned(),
    }
}

fn short_technical(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

fn render_row<'row>(model: &UiModel, row: &'row UiRow) -> [Line<'row>; 2] {
    let selected = model.selected_row() == Some(row.id.as_str());
    let marker = if selected { " › " } else { "   " };
    let title_style = if selected {
        selected_style(model.focus() == UiFocus::Content)
    } else {
        Style::new()
    };
    [
        Line::from(vec![
            Span::styled(marker, title_style),
            Span::styled(row.title.as_str(), title_style),
        ]),
        Line::from(vec![
            Span::raw("     "),
            Span::styled(row_state_label(row.state), row_state_style(row.state)),
            Span::raw(" · "),
            Span::styled(row.detail.as_str(), Style::new().fg(Color::DarkGray)),
        ]),
    ]
}

fn render_footer(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let content = model.last_failure().map_or_else(
        || {
            if model.focus() == UiFocus::Navigation && model.viewport().width >= WIDE_WIDTH {
                " ↑/↓ or j/k section · Enter/→/l content · q quit".to_owned()
            } else if model.focus() == UiFocus::Navigation {
                " ←/→ or h/l section · ↑/↓ or j/k select · Enter open · q quit".to_owned()
            } else if model.section() == UiSection::Agents {
                format!(
                    " ←/h navigation · / search ({}) · Enter inspect · c create · q quit",
                    if model.agent_search().is_empty() {
                        "all"
                    } else {
                        model.agent_search()
                    }
                )
            } else if model.section() == UiSection::Projects {
                format!(
                    " ←/h navigation · / search ({}) · Enter inspect · c existing · w worktree · q quit",
                    if model.project_search().is_empty() {
                        "all"
                    } else {
                        model.project_search()
                    }
                )
            } else if model.viewport().width >= WIDE_WIDTH {
                " ←/h navigation · ↑/↓ select · r reply · d direct · n note · a/u state · q quit"
                    .to_owned()
            } else {
                " ↑/↓ select · r reply · d direct · n note · a/u state · q quit".to_owned()
            }
        },
        |failure| format!(" {} · {}", failure.code, failure.action),
    );
    let style = if model.last_failure().is_some() {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(Line::styled(content, style)).block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn selected_style(focused: bool) -> Style {
    let style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    if focused {
        style.add_modifier(Modifier::REVERSED)
    } else {
        style
    }
}

const fn connection_label(state: UiConnectionState) -> &'static str {
    match state {
        UiConnectionState::Disconnected => "disconnected",
        UiConnectionState::Connecting => "connecting",
        UiConnectionState::Ready => "ready",
        UiConnectionState::Reconnecting => "reconnecting",
        UiConnectionState::Incompatible => "incompatible",
    }
}

fn connection_style(state: UiConnectionState) -> Style {
    let color = match state {
        UiConnectionState::Ready => Color::Green,
        UiConnectionState::Connecting | UiConnectionState::Reconnecting => Color::Yellow,
        UiConnectionState::Disconnected | UiConnectionState::Incompatible => Color::Red,
    };
    Style::new().fg(color)
}

const fn section_label(section: UiSection) -> &'static str {
    match section {
        UiSection::Inbox => "Inbox",
        UiSection::Sent => "Sent",
        UiSection::Archived => "Archived",
        UiSection::Agents => "Agents",
        UiSection::Projects => "Projects",
    }
}

const fn row_state_label(state: UiRowState) -> &'static str {
    match state {
        UiRowState::Open => "open",
        UiRowState::Waiting => "waiting",
        UiRowState::Archived => "archived",
        UiRowState::Attention => "attention",
    }
}

fn row_state_style(state: UiRowState) -> Style {
    let color = match state {
        UiRowState::Open => Color::Green,
        UiRowState::Waiting => Color::Yellow,
        UiRowState::Archived => Color::DarkGray,
        UiRowState::Attention => Color::Red,
    };
    Style::new().fg(color)
}
