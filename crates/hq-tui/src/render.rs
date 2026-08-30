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
    UiActivityStatus, UiAgent, UiAgentAssignmentPhase, UiAgentAttentionReason, UiAgentModal,
    UiAgentProjectAssignment, UiAgentStatus, UiConnectionState, UiConversationEntry,
    UiConversationEntryKind, UiFocus, UiHelpPage, UiHumanIssue, UiHumanMembershipEvidence,
    UiHumanMembershipStatus, UiHumanState, UiMailboxAction, UiMailboxDraftTarget, UiMailboxModal,
    UiManagedSessionAction, UiManagedSessionOutcome, UiMessageState, UiModel, UiProjectAction,
    UiProjectCreationChoice, UiProjectFormField, UiProjectModal, UiProjectOutcome, UiProjectThread,
    UiRow, UiRowKind, UiRowState, UiSection, UiTechnicalSection, model::WIDE_WIDTH,
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
    render_help(frame, model, content);
}

fn render_help(frame: &mut Frame<'_>, model: &UiModel, available: Rect) {
    let Some(page) = model.help_page() else {
        return;
    };
    let width = available.width.saturating_sub(4).clamp(1, 88);
    let height = available.height.clamp(1, 18);
    let area = Rect {
        x: available.x + available.width.saturating_sub(width) / 2,
        y: available.y + available.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let (title, lines) = match page {
        UiHelpPage::Context => (" Help ", contextual_help_lines(model)),
        UiHelpPage::Technical => (" Technical details ", technical_help_lines(model)),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::new().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn contextual_help_lines(model: &UiModel) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled("What this is", Style::new().fg(Color::Cyan).bold()),
        Line::from(section_help_text(model.section())),
    ];
    if let Some(row) = model.selected_row_data() {
        lines.push(Line::from(format!("Selected: {}", row.title)));
        lines.push(Line::from(format!(
            "State: {} · {}",
            row_state_label(model.section(), row.state),
            row.detail
        )));
    } else {
        lines.push(Line::from("No item is selected."));
    }
    lines.push(Line::styled(
        "Available actions",
        Style::new().fg(Color::Cyan).bold(),
    ));
    lines.extend(section_help_actions(model));
    lines.push(Line::from(
        "q — quit · t — technical details · ? / Esc — close help",
    ));
    lines
}

fn technical_help_lines(model: &UiModel) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!(
            "Section: {} · Connection: {}",
            section_label(model.section()),
            connection_label(model.connection())
        )),
        Line::from(model.snapshot().map_or_else(
            || "Authoritative revision: not loaded".to_owned(),
            |snapshot| format!("Authoritative revision: {}", snapshot.revision),
        )),
    ];
    if let Some(UiHumanState::NeedsAttention(issue)) = model.human_state() {
        lines.extend(technical_human_lines(model, issue));
    } else if let Some(row) = model.selected_row_data() {
        lines.push(Line::from(format!("Stable item ID: {}", row.id)));
        lines.push(Line::from(format!(
            "Item type: {}",
            row_kind_label(row.kind)
        )));
        lines.push(Line::from(format!(
            "Presentation state: {}",
            row_state_technical_label(row.state)
        )));
        lines.push(Line::from(
            "Open the item with Enter for its complete typed evidence.",
        ));
    } else {
        lines.push(Line::from("Stable item ID: none selected"));
    }
    if let Some(failure) = model.last_failure() {
        lines.push(Line::styled(
            format!("Recovery code: {}", failure.code),
            Style::new().fg(Color::Yellow),
        ));
        lines.push(Line::from(format!("Recovery action: {}", failure.action)));
    } else {
        lines.push(Line::from("Recovery evidence: none"));
    }
    lines.push(Line::from("t — contextual help · ? / Esc — close help"));
    lines
}

fn technical_human_lines(model: &UiModel, issue: &UiHumanIssue) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        format!("Human recovery code: {}", human_issue_code(issue)),
        Style::new().fg(Color::Yellow),
    )];
    match issue {
        UiHumanIssue::NoAccountSelected => {
            lines.push(Line::from("Selection evidence: no local account selection"));
        }
        UiHumanIssue::SelectionCandidates {
            candidates,
            frontier,
        } => {
            for candidate in candidates.iter().take(3) {
                lines.push(Line::from(format!(
                    "Candidate account: {}",
                    technical_identity(model, *candidate)
                )));
            }
            push_omitted_evidence(&mut lines, candidates.len(), 3);
            lines.push(evidence_ids_line(model, "Selection frontier", frontier));
        }
        UiHumanIssue::SelectionRecords { records } => {
            for (index, record) in records.iter().take(3).enumerate() {
                lines.push(Line::from(format!(
                    "Selection record {}: active={} · {} candidates · {} frontier facts",
                    index + 1,
                    record
                        .active
                        .map_or_else(|| "none".to_owned(), |id| technical_identity(model, id)),
                    record.candidates.len(),
                    record.frontier.len()
                )));
            }
            push_omitted_evidence(&mut lines, records.len(), 3);
        }
        UiHumanIssue::SelectedWithoutAuthority {
            account_id,
            selection_frontier,
        } => {
            lines.push(Line::from(format!(
                "Selected account: {}",
                technical_identity(model, *account_id)
            )));
            lines.push(evidence_ids_line(
                model,
                "Selection frontier",
                selection_frontier,
            ));
        }
        UiHumanIssue::MembershipPending(evidence) | UiHumanIssue::MembershipRevoked(evidence) => {
            lines.extend(technical_membership_lines(model, evidence));
        }
        UiHumanIssue::MembershipAuthorityConflict { records } => {
            for evidence in records.iter().take(2) {
                lines.extend(technical_membership_lines(model, evidence));
            }
            push_omitted_evidence(&mut lines, records.len(), 2);
        }
    }
    lines
}

fn technical_membership_lines(
    model: &UiModel,
    evidence: &UiHumanMembershipEvidence,
) -> Vec<Line<'static>> {
    let status = match evidence.status {
        UiHumanMembershipStatus::Pending => "pending",
        UiHumanMembershipStatus::Active => "active",
        UiHumanMembershipStatus::Revoked => "revoked",
        UiHumanMembershipStatus::Conflicted => "conflicted",
    };
    vec![
        Line::from(format!(
            "Membership: account {} · {status}",
            technical_identity(model, evidence.account_id)
        )),
        evidence_ids_line(model, "Membership frontier", &evidence.frontier),
        evidence_ids_line(model, "Active acceptances", &evidence.active_acceptances),
    ]
}

fn technical_identity(model: &UiModel, identity: [u8; 32]) -> String {
    if model.viewport().width >= WIDE_WIDTH {
        full_identity(identity)
    } else {
        short_identity(identity)
    }
}

fn evidence_ids_line(model: &UiModel, label: &'static str, evidence: &[[u8; 32]]) -> Line<'static> {
    let shown = evidence
        .iter()
        .take(3)
        .map(|identity| technical_identity(model, *identity))
        .collect::<Vec<_>>();
    let suffix = if evidence.len() > shown.len() {
        format!(" · {} more in hq human show", evidence.len() - shown.len())
    } else {
        String::new()
    };
    Line::from(format!(
        "{label} ({}): {}{suffix}",
        evidence.len(),
        if shown.is_empty() {
            "none".to_owned()
        } else {
            shown.join(", ")
        }
    ))
}

fn push_omitted_evidence(lines: &mut Vec<Line<'static>>, count: usize, shown: usize) {
    if count > shown {
        lines.push(Line::from(format!(
            "{} more records · hq human show lists complete evidence",
            count - shown
        )));
    }
}

const fn human_issue_code(issue: &UiHumanIssue) -> &'static str {
    match issue {
        UiHumanIssue::NoAccountSelected => "human_no_account_selected",
        UiHumanIssue::SelectionCandidates { .. } => "human_selection_candidates",
        UiHumanIssue::SelectionRecords { .. } => "human_selection_records_conflict",
        UiHumanIssue::SelectedWithoutAuthority { .. } => "human_selected_without_authority",
        UiHumanIssue::MembershipPending(_) => "human_membership_pending",
        UiHumanIssue::MembershipRevoked(_) => "human_membership_revoked",
        UiHumanIssue::MembershipAuthorityConflict { .. } => "human_membership_authority_conflict",
    }
}

const fn section_help_text(section: UiSection) -> &'static str {
    match section {
        UiSection::Inbox => "Inbox contains messages and updates that need your attention.",
        UiSection::Sent => "Sent contains conversations you have started or replied to.",
        UiSection::Archived => "Archived contains conversations you have put away.",
        UiSection::Agents => "Agents are named workers you can assign and contact.",
        UiSection::Projects => "Projects describe work and the resources it owns.",
    }
}

fn section_help_actions(model: &UiModel) -> Vec<Line<'static>> {
    let mut actions = vec![
        Line::from("↑/↓ or j/k — select · ←/→ or h/l — sections"),
        Line::from("Tab / Shift-Tab — change focus"),
    ];
    match model.section() {
        UiSection::Inbox | UiSection::Sent | UiSection::Archived => {
            if model.conversation().is_some() {
                actions.push(Line::from(
                    "↑/↓ — select message · Enter — details · Esc — close conversation",
                ));
                actions.push(Line::from(
                    "r — reply · a — archive · u — restore · PgDn — load more",
                ));
            } else if model.selected_row_data().is_some() {
                actions.push(Line::from("Enter — open selected conversation"));
            }
            actions.push(Line::from(
                "d — write a direct message · n — write a personal note",
            ));
        }
        UiSection::Agents => actions.push(Line::from(if model.selected_row_data().is_some() {
            "/ — search · Enter — inspect selected agent · c — create agent"
        } else {
            "/ — search · c — create agent"
        })),
        UiSection::Projects => {
            actions.push(Line::from(if model.selected_row_data().is_some() {
                "/ — search · Enter — inspect selected project · c — create project"
            } else {
                "/ — search · c — create project"
            }));
            actions.push(Line::from(
                "w — advanced shortcut: create an isolated Git worktree",
            ));
        }
    }
    actions
}

const fn row_state_technical_label(state: UiRowState) -> &'static str {
    match state {
        UiRowState::Open => "open",
        UiRowState::Waiting => "waiting",
        UiRowState::Archived => "archived",
        UiRowState::Attention => "needs attention",
    }
}

const fn row_kind_label(kind: UiRowKind) -> &'static str {
    match kind {
        UiRowKind::Conversation => "conversation",
        UiRowKind::Agent => "agent",
        UiRowKind::Project => "project",
        UiRowKind::Diagnostic => "diagnostic",
    }
}

#[allow(clippy::too_many_lines)]
fn render_project_modal(frame: &mut Frame<'_>, model: &UiModel, available: Rect) {
    let Some(interaction) = model.project_modal() else {
        return;
    };
    let width = available.width.saturating_sub(4).clamp(1, 88);
    let height = available.height.clamp(1, 22);
    let area = Rect {
        x: available.x + available.width.saturating_sub(width) / 2,
        y: available.y + available.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let (title, lines) = match interaction {
        UiProjectModal::ChooseCreation { selected } => (
            " Create project ",
            vec![
                Line::from("How should this project's first folder be created?"),
                Line::default(),
                project_choice_line(
                    "Use an existing folder",
                    "recommended · record its ownership in HQ",
                    *selected == UiProjectCreationChoice::ExistingFolder,
                ),
                project_choice_line(
                    "Create an isolated Git worktree",
                    "optional advanced · create a branch and separate folder",
                    *selected == UiProjectCreationChoice::IsolatedWorktree,
                ),
                Line::default(),
                Line::from(
                    "HQ tracks folder ownership; it does not take over Git or filesystem maintenance.",
                ),
                Line::from("↑/↓ choose · Enter continue · Esc cancel"),
            ],
        ),
        UiProjectModal::Search { query } => (
            " Search projects ",
            vec![
                text_field_line(
                    "Query",
                    query,
                    model.search_field_cursor(query, true),
                    true,
                    "",
                ),
                Line::default(),
                Line::from("Type to match names or resource paths; technical IDs also work"),
                Line::from("↑/↓ cycle matches · Enter inspect · Esc keep query"),
            ],
        ),
        UiProjectModal::Details {
            project,
            selected_resource,
        } => {
            let mut lines = vec![
                Line::styled(project.name.as_str(), Style::new().fg(Color::Cyan).bold()),
                Line::from(format!(
                    "Status: {} · {}",
                    project_status_label(project),
                    if project.claimable {
                        "folders available"
                    } else {
                        "folder ownership needs attention"
                    }
                )),
            ];
            if model.viewport().width >= WIDE_WIDTH {
                lines.push(Line::styled(
                    "Technical details",
                    Style::new().fg(Color::DarkGray),
                ));
                lines.push(Line::from(format!(
                    "Project {} · version {} · next message {}",
                    short_identity(project.project_id),
                    short_identity(project.head),
                    project.input_sequence
                )));
            } else {
                lines.push(Line::styled(
                    format!(
                        "Technical details: project {}",
                        short_identity(project.project_id)
                    ),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::default());
            lines.push(Line::styled(
                "Folders and resources",
                Style::new().fg(Color::Cyan),
            ));
            for resource in &project.resources {
                let selected = *selected_resource == Some(resource.resource_id);
                lines.push(Line::from(format!(
                    " {} {}{} · {} · {}",
                    if selected { '›' } else { ' ' },
                    if resource.primary { "primary " } else { "" },
                    resource.display_path,
                    resource_health_label(&resource.health),
                    if resource.active_claim {
                        "owned here"
                    } else {
                        "ownership needs attention"
                    }
                )));
                if model.viewport().width >= WIDE_WIDTH {
                    lines.push(Line::from(format!(
                        "   Technical: resource {} · canonical {} · conflicts {}",
                        short_identity(resource.resource_id),
                        resource.canonical_path,
                        resource.conflicting_projects.len()
                    )));
                }
            }
            if project.resources.is_empty() {
                lines.push(Line::from(" No folders or resources recorded"));
            }
            lines.push(Line::default());
            lines.push(Line::styled("Assigned agent", Style::new().fg(Color::Cyan)));
            if let Some(assignment) = &project.assignment {
                lines.push(Line::from(format!(
                    "{} · agent {}",
                    project_assignment_status_label(assignment),
                    short_identity(assignment.agent_id)
                )));
                if model.viewport().width >= WIDE_WIDTH {
                    lines.push(Line::from(format!(
                        "Technical: assignment {} · service {} · conversation {} · thread {}",
                        short_identity(assignment.assignment_id),
                        assignment.provider,
                        assignment.session.as_deref().unwrap_or("not started"),
                        assignment
                            .thread_id
                            .map_or_else(|| "not started".to_owned(), short_identity)
                    )));
                    if let Some(folder) = &assignment.launch_directory {
                        lines.push(Line::from(format!("Working folder: {folder}")));
                    }
                }
                if let Some(blocked) = &assignment.blocked {
                    lines.push(Line::styled(
                        format!("Needs attention: {blocked}"),
                        Style::new().fg(Color::Yellow),
                    ));
                }
                if assignment.cardinality_conflicted {
                    lines.push(Line::styled(
                        "More than one agent is assigned to this project. HQ will not guess.",
                        Style::new().fg(Color::Red),
                    ));
                }
            } else {
                lines.push(Line::from("Unassigned"));
            }
            lines.push(Line::default());
            lines.push(Line::from("↑/↓ resource · a add · e replace · x remove"));
            lines.push(Line::from(
                "p primary · k check selected · K check all · n send instructions",
            ));
            lines.push(Line::from("v set up work · d send pending · h move agent"));
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
        } => {
            let mut lines = Vec::new();
            push_project_text_field(
                &mut lines,
                model,
                "Path",
                path,
                UiProjectFormField::Path,
                *field == UiProjectFormField::Path,
                true,
                "Choose the existing folder this project should own",
                true,
            );
            push_project_text_field(
                &mut lines,
                model,
                "Name",
                name,
                UiProjectFormField::Name,
                *field == UiProjectFormField::Name,
                true,
                "Example: api-redesign",
                false,
            );
            push_project_text_field(
                &mut lines,
                model,
                "Brief",
                brief,
                UiProjectFormField::Brief,
                *field == UiProjectFormField::Brief,
                false,
                "A short description helps agents understand the project",
                false,
            );
            lines.push(Line::from(
                "Ownership preview: this project will claim this folder in HQ.",
            ));
            lines.push(Line::from(
                "Other projects cannot own this folder or overlapping folders.",
            ));
            lines.push(Line::from(
                "HQ will not take over ordinary filesystem or Git maintenance.",
            ));
            lines.push(Line::default());
            lines.push(Line::from(if *submitting {
                "Creating the project safely…"
            } else {
                "Tab/Shift-Tab field · Enter create · Esc cancel"
            }));
            (" Create project from folder ", lines)
        }
        UiProjectModal::CreateWorktree {
            name,
            brief,
            source,
            destination,
            branch,
            base,
            field,
            submitting,
        } => {
            let mut lines = Vec::new();
            push_project_text_field(
                &mut lines,
                model,
                "Name",
                name,
                UiProjectFormField::Name,
                *field == UiProjectFormField::Name,
                true,
                "Example: api-redesign",
                false,
            );
            push_project_text_field(
                &mut lines,
                model,
                "Brief",
                brief,
                UiProjectFormField::Brief,
                *field == UiProjectFormField::Brief,
                false,
                "Optional project context",
                false,
            );
            push_project_text_field(
                &mut lines,
                model,
                "Source",
                source,
                UiProjectFormField::Source,
                *field == UiProjectFormField::Source,
                true,
                "Existing Git working tree",
                true,
            );
            push_project_text_field(
                &mut lines,
                model,
                "Destination",
                destination,
                UiProjectFormField::Destination,
                *field == UiProjectFormField::Destination,
                true,
                "New worktree folder",
                true,
            );
            push_project_text_field(
                &mut lines,
                model,
                "Branch",
                branch,
                UiProjectFormField::Branch,
                *field == UiProjectFormField::Branch,
                true,
                "Example: feature/api-redesign",
                false,
            );
            push_project_text_field(
                &mut lines,
                model,
                "Base",
                base,
                UiProjectFormField::Base,
                *field == UiProjectFormField::Base,
                false,
                "Optional starting revision, such as main",
                false,
            );
            lines.push(Line::from(
                "HQ creates a separate branch and folder; you keep normal Git control.",
            ));
            lines.push(Line::from(if *submitting {
                "Creating the worktree; external files will be kept if setup is interrupted…"
            } else {
                "Tab/Shift-Tab field · Enter create · Esc cancel"
            }));
            (" Create an isolated Git worktree ", lines)
        }
        UiProjectModal::SendInput {
            project,
            content,
            submitting,
        } => {
            let mut lines = vec![Line::from(format!("Project: {}", project.name))];
            push_project_text_field(
                &mut lines,
                model,
                "Instructions",
                content,
                UiProjectFormField::Content,
                true,
                true,
                "Describe the outcome you want the assigned agent to achieve",
                false,
            );
            lines.push(Line::from(
                "The assigned agent will receive these instructions once.",
            ));
            lines.push(Line::default());
            lines.push(Line::from(if *submitting {
                "Sending instructions…"
            } else {
                "Enter send · Esc cancel"
            }));
            (" Send instructions to this project ", lines)
        }
        UiProjectModal::AddResource {
            project,
            path,
            make_primary,
            submitting,
        } => {
            let mut lines = vec![Line::from(format!("Project: {}", project.name))];
            push_project_text_field(
                &mut lines,
                model,
                "Path",
                path,
                UiProjectFormField::Path,
                model.project_field_is_focused(UiProjectFormField::Path),
                true,
                "Use an absolute path, ~, or ~/…",
                true,
            );
            lines.push(project_choice_line(
                "Use as primary",
                yes_no(*make_primary),
                model.project_field_is_focused(UiProjectFormField::Primary),
            ));
            lines.push(Line::from(
                "HQ checks project ownership before saving this path.",
            ));
            lines.push(Line::default());
            lines.push(Line::from(if *submitting {
                "Checking whether another project owns this path…"
            } else {
                "Tab/Shift-Tab field · ↑/↓ change choice · Enter preview · Esc cancel"
            }));
            (" Add a folder or resource ", lines)
        }
        UiProjectModal::ReplaceResource {
            project,
            resource_id,
            path,
            submitting,
        } => {
            let mut lines = vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!("Replace: {}", short_identity(*resource_id))),
            ];
            push_project_text_field(
                &mut lines,
                model,
                "Path",
                path,
                UiProjectFormField::Path,
                true,
                true,
                "Use an absolute path, ~, or ~/…",
                true,
            );
            lines.push(Line::from(
                "HQ checks project ownership before replacing the recorded path.",
            ));
            lines.push(Line::default());
            lines.push(Line::from(if *submitting {
                "Checking whether another project owns this path…"
            } else {
                "Enter preview · Esc cancel"
            }));
            (" Change a folder or resource ", lines)
        }
        UiProjectModal::ConfirmRemoveResource {
            project,
            resource_id,
            force,
            submitting,
        } => (
            " Remove this project resource? ",
            vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!("Resource: {}", short_identity(*resource_id))),
                Line::from(if project.assignment.is_some() {
                    "An agent is assigned to this project. Removal needs extra confirmation."
                } else {
                    "No agent is assigned to this project."
                }),
                Line::from(format!("Override safety check: {}", yes_no(*force))),
                Line::from("HQ keeps the folder, files, worktree, and branch on disk."),
                Line::default(),
                Line::from(if *submitting {
                    "Removing the resource from the project…"
                } else {
                    "f toggle override · Enter remove · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::ConfirmPrimaryResource {
            project,
            resource_id,
            submitting,
        } => (
            " Use as the primary project resource? ",
            vec![
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!("Resource: {}", short_identity(*resource_id))),
                Line::from("The primary resource is the default folder for project work."),
                Line::default(),
                Line::from(if *submitting {
                    "Saving the primary resource…"
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
            " Set up project work ",
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
                model.viewport().width >= WIDE_WIDTH,
                *submitting,
                model,
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
            " Move project work to another agent ",
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
                model.viewport().width >= WIDE_WIDTH,
                *submitting,
                model,
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
                    "Status: {} · agent assigned: {}",
                    project_status_label(project),
                    yes_no(project.assignment.is_some())
                )),
                Line::default(),
                Line::styled("Folder release check", Style::new().fg(Color::Cyan)),
            ];
            if checks.is_empty() {
                lines.push(Line::from("No folders or resources are recorded."));
            }
            for check in checks {
                lines.push(Line::from(format!(
                    "{} · {}",
                    short_identity(check.resource_id),
                    resource_check_summary(check)
                )));
                lines.push(Line::from(format!(
                    "  Technical: status={} · health={} · release={}",
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
            lines.push(project_choice_line(
                "I understand",
                yes_no(*confirmed),
                model.project_field_is_focused(UiProjectFormField::Confirmation),
            ));
            if let Some(error) = model.project_field_error(UiProjectFormField::Confirmation) {
                lines.push(Line::styled(
                    format!("  {error}"),
                    Style::new().fg(Color::Red),
                ));
            }
            lines.push(project_choice_line(
                "Override safety check",
                yes_no(*force),
                model.project_field_is_focused(UiProjectFormField::Force),
            ));
            if let Some(error) = model.project_field_error(UiProjectFormField::Force) {
                lines.push(Line::styled(
                    format!("  {error}"),
                    Style::new().fg(Color::Red),
                ));
            }
            lines.push(Line::from(
                "Closing keeps folders, files, worktrees, and branches on disk.",
            ));
            lines.push(Line::from(if *submitting {
                "Closing the project safely…"
            } else {
                "Tab/Shift-Tab field · ↑/↓ change choice · Enter close · Esc cancel"
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
                Line::from(format!("Current status: {}", project_status_label(project))),
                Line::from(if *archived {
                    "Archiving closes the project but keeps its folders and history."
                } else {
                    "Unarchiving returns the project to its current open or closed status."
                }),
                Line::default(),
                Line::from(if *submitting {
                    "Saving the archive change…"
                } else {
                    "Enter confirm · Esc cancel"
                }),
            ],
        ),
        UiProjectModal::Outcome { result } => {
            let mut lines = vec![Line::from(project_action_label(&result.action))];
            lines.push(Line::from(if model.viewport().width >= WIDE_WIDTH {
                format!(
                    "Technical details: project {} · request {}",
                    short_identity(result.project_id),
                    short_identity(result.operation_id)
                )
            } else {
                format!(
                    "Technical IDs: {} / {}",
                    short_identity(result.project_id),
                    short_identity(result.operation_id)
                )
            }));
            if model.viewport().width >= WIDE_WIDTH {
                lines.push(Line::default());
            }
            if let Some(runtime_state) = &result.runtime_state {
                lines.push(Line::from(format!(
                    "Technical runtime: {runtime_state}{}",
                    result
                        .runtime_code
                        .as_ref()
                        .map_or_else(String::new, |code| format!("/{code}"))
                )));
            }
            match &result.outcome {
                UiProjectOutcome::Completed { project_head } => lines.push(Line::from(format!(
                    "Done{}",
                    project_head.map_or_else(String::new, |head| format!(
                        " · technical version {}",
                        short_identity(head)
                    ))
                ))),
                UiProjectOutcome::Running { stage } => {
                    lines.push(Line::from("HQ is still finishing this change."));
                    lines.push(Line::from(format!("Technical stage: {stage}")));
                }
                UiProjectOutcome::Rejected { category, code } => {
                    lines.push(Line::styled(
                        "HQ could not make this change.",
                        Style::new().fg(Color::Red),
                    ));
                    lines.push(Line::from(
                        "Review the technical reason, correct the problem, and try again.",
                    ));
                    lines.push(Line::from(format!("Technical reason: {category}/{code}")));
                }
                UiProjectOutcome::Reconcilable {
                    stage,
                    category,
                    code,
                    warning,
                } => {
                    lines.push(Line::styled(
                        "HQ could not confirm whether the change finished.",
                        Style::new().fg(Color::Yellow),
                    ));
                    if let Some(warning) = warning {
                        lines.push(Line::from(format!("Technical kind: {}", warning.kind)));
                        lines.push(Line::from(format!(
                            "Kept worktree {} on branch {}.",
                            warning.destination, warning.branch
                        )));
                    }
                    lines.push(Line::from(
                        "HQ will retry the same request; do not repeat it manually.",
                    ));
                    lines.push(Line::from(format!(
                        "Technical stage and reason: {stage} · {category}/{code}"
                    )));
                }
                UiProjectOutcome::InputSent { message_id } => {
                    lines.push(Line::from("Instructions sent."));
                    lines.push(Line::from(format!(
                        "Technical message ID: {}",
                        short_identity(*message_id)
                    )));
                }
                UiProjectOutcome::ResourcePreview {
                    display_path,
                    canonical_path,
                    conflicts,
                } => {
                    lines.push(Line::from(format!("Requested path: {display_path}")));
                    lines.push(Line::from(format!("Resolved path: {canonical_path}")));
                    if conflicts.is_empty() {
                        let continuation = if matches!(
                            result.action,
                            UiProjectAction::PreviewCreateExisting { .. }
                        ) {
                            "No other project owns this folder · Enter create"
                        } else {
                            "No other project owns this path · Enter add"
                        };
                        lines.push(Line::styled(continuation, Style::new().fg(Color::Green)));
                    } else {
                        let subject = if matches!(
                            result.action,
                            UiProjectAction::PreviewCreateExisting { .. }
                        ) {
                            "folder"
                        } else {
                            "path"
                        };
                        lines.push(Line::styled(
                            format!("Another project already owns this {subject}:"),
                            Style::new().fg(Color::Red),
                        ));
                        for conflict in conflicts {
                            let project = model
                                .snapshot()
                                .and_then(|snapshot| {
                                    snapshot
                                        .projects
                                        .iter()
                                        .find(|project| project.project_id == conflict.project_id)
                                })
                                .map_or_else(
                                    || format!("project {}", short_identity(conflict.project_id)),
                                    |project| format!("project ‘{}’", project.name),
                                );
                            lines.push(Line::from(format!(
                                " {project} owns {}",
                                conflict.display_path
                            )));
                            lines.push(Line::from(format!(
                                "  Technical relationship: {} · resource {}",
                                conflict.relationship,
                                short_identity(conflict.resource_id)
                            )));
                        }
                    }
                }
                UiProjectOutcome::ResourceChecks { checks } => {
                    for check in checks {
                        lines.push(Line::from(format!(
                            "{} · {}",
                            short_identity(check.resource_id),
                            resource_check_summary(check)
                        )));
                        lines.push(Line::from(format!(
                            "  Technical: status={} · health={} · release={}",
                            check.status,
                            check.health.as_deref().unwrap_or("unknown"),
                            check.release.as_deref().unwrap_or("unknown")
                        )));
                        if let (Some(category), Some(code)) =
                            (&check.error_category, &check.error_code)
                        {
                            lines.push(Line::styled(
                                format!("  Technical reason: {category}/{code}"),
                                Style::new().fg(Color::Red),
                            ));
                        }
                        if matches!(result.action, UiProjectAction::PreviewCreateExisting { .. }) {
                            lines.push(Line::from("Esc edit the folder path"));
                        }
                    }
                }
            }
            lines.push(Line::default());
            lines.push(Line::from("Esc close"));
            (" Project change ", lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn text_field_line(
    label: &str,
    value: &str,
    cursor: usize,
    selected: bool,
    requirement: &str,
) -> Line<'static> {
    let cursor = cursor.min(value.len());
    let (left, right) = value.split_at(cursor);
    let style = if selected {
        selected_style(true)
    } else {
        Style::new()
    };
    let mut spans = vec![Span::styled(
        format!("{} {label}: {left}", if selected { '›' } else { ' ' }),
        style,
    )];
    if selected {
        spans.push(Span::styled(
            "│",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(format!("{right} {requirement}"), style));
    Line::from(spans)
}

fn project_text_field_line(
    model: &UiModel,
    label: &str,
    value: &str,
    field: UiProjectFormField,
    selected: bool,
    required: bool,
) -> Line<'static> {
    text_field_line(
        label,
        value,
        model.project_field_cursor(field, value),
        selected,
        if required { "(required)" } else { "(optional)" },
    )
}

#[allow(clippy::too_many_arguments)]
fn push_project_text_field(
    lines: &mut Vec<Line<'static>>,
    model: &UiModel,
    label: &str,
    value: &str,
    field: UiProjectFormField,
    selected: bool,
    required: bool,
    guidance: &str,
    path: bool,
) {
    lines.push(project_text_field_line(
        model, label, value, field, selected, required,
    ));
    if let Some(error) = model.project_field_error(field) {
        lines.push(Line::styled(
            format!("  {error}"),
            Style::new().fg(Color::Red),
        ));
    } else if selected {
        lines.push(Line::styled(
            format!("  {guidance}"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    if path && selected && !value.is_empty() {
        let preview = match model.normalized_path_preview(value) {
            Ok(path) => format!("  Will use: {path}"),
            Err(error) => format!("  {error}"),
        };
        lines.push(Line::styled(preview, Style::new().fg(Color::DarkGray)));
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
    wide: bool,
    submitting: bool,
    model: &UiModel,
) -> Vec<Line<'value>> {
    let agent = agent_id
        .and_then(|id| agents.iter().find(|agent| agent.agent_id == id))
        .and_then(|agent| agent.names.first().map(String::as_str))
        .unwrap_or("none");
    let thread_label = thread.map_or_else(
        || "start a new conversation".to_owned(),
        |thread| {
            format!(
                "continue saved conversation · {}/{} · {}",
                thread.provider,
                thread.session,
                short_identity(thread.thread_id)
            )
        },
    );
    let mut lines = vec![Line::from(format!("Project: {}", project.name))];
    if wide {
        lines.push(Line::from(if handoff.is_some() {
            "Choose the agent that should take over this project's work."
        } else {
            "Choose an agent and conversation; HQ will connect them to this project."
        }));
    }
    lines.push(project_choice_line(
        "Agent",
        agent,
        field == UiProjectFormField::Agent,
    ));
    if let Some(error) = model.project_field_error(UiProjectFormField::Agent) {
        lines.push(Line::styled(
            format!("  {error}"),
            Style::new().fg(Color::Red),
        ));
    }
    lines.push(project_choice_line(
        "Conversation",
        if new_session {
            if wide {
                "start a new conversation"
            } else {
                "new"
            }
        } else if wide {
            "continue the selected conversation"
        } else {
            "continue"
        },
        field == UiProjectFormField::SessionMode,
    ));
    if !new_session {
        lines.push(project_choice_line(
            if wide { "Saved conversation" } else { "Saved" },
            &thread_label,
            field == UiProjectFormField::Thread,
        ));
        if let Some(error) = model.project_field_error(UiProjectFormField::Thread) {
            lines.push(Line::styled(
                format!("  {error}"),
                Style::new().fg(Color::Red),
            ));
        }
    }
    lines.push(project_text_field_line(
        model,
        "Agent service",
        provider,
        UiProjectFormField::Provider,
        field == UiProjectFormField::Provider,
        true,
    ));
    if let Some(error) = model.project_field_error(UiProjectFormField::Provider) {
        lines.push(Line::styled(
            format!("  {error}"),
            Style::new().fg(Color::Red),
        ));
    }
    lines.push(project_text_field_line(
        model,
        "Working folder",
        directory,
        UiProjectFormField::Directory,
        field == UiProjectFormField::Directory,
        true,
    ));
    if let Some(error) = model.project_field_error(UiProjectFormField::Directory) {
        lines.push(Line::styled(
            format!("  {error}"),
            Style::new().fg(Color::Red),
        ));
    } else if field == UiProjectFormField::Directory && !directory.is_empty() {
        let preview = match model.normalized_path_preview(directory) {
            Ok(path) => format!("  Will use: {path}"),
            Err(error) => format!("  {error}"),
        };
        lines.push(Line::styled(preview, Style::new().fg(Color::DarkGray)));
    }
    if let Some((confirmed, force)) = handoff {
        lines.push(project_choice_line(
            "I understand",
            yes_no(confirmed),
            field == UiProjectFormField::Confirmation,
        ));
        if let Some(error) = model.project_field_error(UiProjectFormField::Confirmation) {
            lines.push(Line::styled(
                format!("  {error}"),
                Style::new().fg(Color::Red),
            ));
        }
        lines.push(project_choice_line(
            "Override safety check",
            yes_no(force),
            field == UiProjectFormField::Force,
        ));
        lines.push(Line::from(if wide {
            "The previous agent may still be running elsewhere."
        } else {
            "The previous agent may still be running."
        }));
    }
    if wide {
        lines.push(Line::default());
    }
    lines.push(Line::from(if submitting {
        "Setting up project work safely…"
    } else if handoff.is_some() {
        if wide {
            "Tab field · ↑/↓ change choice · Enter move work"
        } else {
            "Tab field · ↑/↓ choose · Enter move"
        }
    } else if wide {
        "Tab field · ↑/↓ change choice · Enter set up work"
    } else {
        "Tab field · ↑/↓ choose · Enter set up"
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

const fn yes_no(value: bool) -> &'static str {
    if value { "Yes" } else { "No" }
}

fn project_status_label(project: &crate::UiProject) -> &'static str {
    if project.archived {
        "Archived"
    } else if project.lifecycle == "closed" {
        "Closed"
    } else if project.lifecycle == "open" {
        "Open"
    } else {
        "Needs attention"
    }
}

fn project_assignment_status_label(assignment: &crate::UiProjectAssignment) -> &'static str {
    if assignment.cardinality_conflicted {
        "Needs attention"
    } else if assignment.blocked.is_some() {
        "Blocked"
    } else if assignment.runnable {
        "Ready to work"
    } else {
        "Setting up"
    }
}

fn resource_check_summary(check: &crate::UiProjectResourceCheck) -> &'static str {
    if check.status != "accepted" {
        "HQ could not finish checking this resource"
    } else if check
        .health
        .as_deref()
        .is_some_and(|health| health != "healthy")
    {
        "The resource needs attention"
    } else {
        match check.release.as_deref() {
            Some("clean") => "Ready to release",
            Some("dirty") => "Local changes would be kept",
            Some(_) | None => "Release status needs review",
        }
    }
}

fn resource_health_label(health: &str) -> &str {
    match health {
        "healthy" => "available",
        "missing" => "not found",
        "inaccessible" => "cannot be opened",
        _ => "needs attention",
    }
}

fn project_action_label(action: &UiProjectAction) -> String {
    match action {
        UiProjectAction::PreviewCreateExisting { name, .. } => {
            format!("Check folder ownership before creating {name}")
        }
        UiProjectAction::CreateExisting { name, .. } => format!("Create {name} from a folder"),
        UiProjectAction::CreateWorktree { name, .. } => {
            format!("Create an isolated worktree for {name}")
        }
        UiProjectAction::SendInput { .. } => "Send project instructions".to_owned(),
        UiProjectAction::PreviewAddResource { .. } => {
            "Check a folder or resource before adding it".to_owned()
        }
        UiProjectAction::AddResource { .. } => "Add a folder or resource".to_owned(),
        UiProjectAction::PreviewReplaceResource { .. } => {
            "Check a replacement folder or resource".to_owned()
        }
        UiProjectAction::ReplaceResource { .. } => "Change a folder or resource".to_owned(),
        UiProjectAction::RemoveResource { .. } => "Remove a project resource".to_owned(),
        UiProjectAction::SetPrimaryResource { .. } => "Choose the primary resource".to_owned(),
        UiProjectAction::CheckResources { .. } => "Check project resources".to_owned(),
        UiProjectAction::Activate { .. } => "Set up project work".to_owned(),
        UiProjectAction::DispatchPending { .. } => "Send pending project instructions".to_owned(),
        UiProjectAction::Handoff { .. } => "Move project work to another agent".to_owned(),
        UiProjectAction::Open { .. } => "Reopen project".to_owned(),
        UiProjectAction::PreviewClose { .. } => "Assess project close".to_owned(),
        UiProjectAction::Close { force: true, .. } => {
            "Close project with safety override".to_owned()
        }
        UiProjectAction::Close { force: false, .. } => "Close project".to_owned(),
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
                text_field_line(
                    "Query",
                    query,
                    model.search_field_cursor(query, false),
                    true,
                    "",
                ),
                Line::default(),
                Line::from("Type to match agent or conversation names; technical IDs also work"),
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
                "Saved conversations",
                Style::new().fg(Color::Cyan),
            ));
            if model.viewport().width >= WIDE_WIDTH {
                lines.push(Line::from(
                    "Start a new conversation or continue one previously used by this agent.",
                ));
            }
            for session in &agent.sessions {
                let selected = selected_session.as_ref().is_some_and(|(provider, value)| {
                    *provider == session.provider && *value == session.session
                });
                let name = session.display_name.as_deref().unwrap_or("unnamed");
                lines.push(Line::styled(
                    format!(
                        " {} {}{}{} · technical {}/{}",
                        if selected { '›' } else { ' ' },
                        name,
                        if session.selected { " · current" } else { "" },
                        if session.conflicted {
                            " · needs attention"
                        } else {
                            ""
                        },
                        session.provider,
                        session.session
                    ),
                    if selected {
                        selected_style(true)
                    } else {
                        Style::new()
                    },
                ));
            }
            if agent.sessions.is_empty() {
                lines.push(Line::from(" No saved conversations yet"));
            }
            if model.viewport().width >= WIDE_WIDTH {
                lines.push(Line::default());
            }
            lines.push(Line::from(if model.viewport().width >= WIDE_WIDTH {
                "↑/↓ conversation · s start new · e continue · t stop"
            } else {
                "↑/↓ choose · s new · e continue · t stop"
            }));
            lines.push(Line::from(if model.viewport().width >= WIDE_WIDTH {
                "r name/clear · x retire agent · Esc close"
            } else {
                "r name · x retire · Esc close"
            }));
            (" Agent details ", lines)
        }
        UiAgentModal::Create { name, submitting } => {
            let mut lines = vec![text_field_line(
                "Name",
                name,
                model.agent_field_cursor(name),
                true,
                "(required)",
            )];
            if let Some(error) = model.agent_field_error() {
                lines.push(Line::styled(
                    format!("  {error}"),
                    Style::new().fg(Color::Red),
                ));
            } else {
                lines.push(Line::from(
                    "Use a permanent lowercase name without spaces, such as reviewer.",
                ));
            }
            lines.push(Line::from(
                "You can assign this agent to projects and message it directly.",
            ));
            lines.push(Line::default());
            lines.push(Line::from(if *submitting {
                "Creating the agent…"
            } else {
                "Enter create · Esc cancel"
            }));
            (" Create agent ", lines)
        }
        UiAgentModal::RenameSession {
            provider,
            session,
            display_name,
            submitting,
            ..
        } => (
            " Name saved conversation ",
            vec![
                Line::from(format!("Technical conversation: {provider}/{session}")),
                text_field_line(
                    "Name",
                    display_name,
                    model.session_field_cursor(display_name),
                    true,
                    "(optional)",
                ),
                Line::from("A recognizable name, such as release-review; leave empty to clear."),
                Line::default(),
                Line::from(if *submitting {
                    "Saving the conversation name…"
                } else {
                    "Enter save · Esc cancel"
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
                    Line::from(format!(
                        "Override active-work safety check: {}",
                        yes_no(*force)
                    )),
                    Line::from("An override does not stop an agent running outside HQ."),
                    Line::default(),
                    Line::from(if *submitting {
                        "Retiring the agent…"
                    } else {
                        "f toggle override · Enter confirm · Esc cancel"
                    }),
                ],
            )
        }
        UiAgentModal::ManagedProvider { provider, .. } => (
            " Start an agent conversation ",
            vec![
                text_field_line(
                    "Agent service",
                    provider,
                    model.provider_field_cursor(provider),
                    true,
                    "(required)",
                ),
                Line::from("This service will run the agent's conversation."),
                Line::from(
                    model
                        .provider_field_error()
                        .unwrap_or("Choose one of the services available on this device."),
                ),
                Line::default(),
                Line::from("Enter continue · Esc cancel"),
            ],
        ),
        UiAgentModal::ConfirmManagedSession { action, .. } => (
            " Switch the agent's conversation? ",
            vec![
                Line::styled(
                    "This differs from the agent's currently saved conversation.",
                    Style::new().fg(Color::Yellow),
                ),
                Line::from(managed_session_target(action)),
                Line::default(),
                Line::from("HQ will check whether the agent is already running before switching."),
                Line::from("Enter confirm · Esc cancel"),
            ],
        ),
        UiAgentModal::ManagingSession { action, .. } => (
            " Updating agent conversation ",
            vec![
                Line::from(managed_session_target(action)),
                Line::default(),
                Line::from("HQ is confirming the request and will keep checking after reconnects…"),
            ],
        ),
        UiAgentModal::ManagedSessionOutcome { result, .. } => {
            let mut lines = vec![Line::from(managed_session_target(&result.action))];
            lines.push(Line::from(format!(
                "Technical request: {}",
                short_identity(result.operation_id)
            )));
            lines.push(Line::default());
            match &result.outcome {
                UiManagedSessionOutcome::Ready { session } => {
                    lines.push(Line::from("The agent conversation is ready."));
                    lines.push(Line::from(format!("Technical session: {session}")));
                }
                UiManagedSessionOutcome::Stopped => {
                    lines.push(Line::from(
                        "The agent stopped on this device. Its saved conversation was kept.",
                    ));
                }
                UiManagedSessionOutcome::Rejected { category, code } => {
                    lines.push(Line::styled(
                        "HQ could not change the agent conversation.",
                        Style::new().fg(Color::Red),
                    ));
                    lines.push(Line::from(format!("Technical reason: {category}/{code}")));
                    lines.push(Line::from(
                        "Reload the agent, choose a current conversation, and try again.",
                    ));
                }
                UiManagedSessionOutcome::Uncertain { reconciliation_id } => {
                    lines.push(Line::styled(
                        "HQ could not confirm whether the change finished.",
                        Style::new().fg(Color::Yellow),
                    ));
                    lines.push(Line::from(format!(
                        "Technical recovery ID: {}",
                        short_identity(*reconciliation_id)
                    )));
                    lines.push(Line::from("HQ will keep checking the same request."));
                }
            }
            lines.push(Line::default());
            lines.push(Line::from("Esc close"));
            (" Agent conversation change ", lines)
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
        UiManagedSessionAction::Start { provider, .. } => {
            format!("Start a new conversation using {provider}")
        }
        UiManagedSessionAction::Resume {
            provider, session, ..
        } => format!("Continue saved conversation {provider}/{session}"),
        UiManagedSessionAction::Stop { provider, .. } => {
            format!("Stop the agent running through {provider}")
        }
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

fn full_identity(identity: [u8; 32]) -> String {
    identity
        .iter()
        .fold(String::with_capacity(64), |mut rendered, byte| {
            let _ = write!(rendered, "{byte:02x}");
            rendered
        })
}

fn agent_summary(agent: &UiAgent) -> Vec<Line<'_>> {
    let mut lines = vec![
        Line::styled(
            agent.names.first().map_or("Unnamed agent", String::as_str),
            Style::new().fg(Color::Cyan).bold(),
        ),
        Line::from(format!("Status: {}", agent_status_label(&agent.status))),
        Line::styled("Technical details", Style::new().fg(Color::DarkGray)),
        Line::from(format!(
            "Agent ID {:02x}{:02x}{:02x}{:02x}… · mailboxes {}",
            agent.agent_id[0],
            agent.agent_id[1],
            agent.agent_id[2],
            agent.agent_id[3],
            agent.mailboxes.len()
        )),
    ];
    match &agent.status {
        UiAgentStatus::Assigned(assignment) => {
            lines.extend(agent_assignment_evidence(assignment));
        }
        UiAgentStatus::NeedsAttention { assignments, .. } => {
            for assignment in assignments {
                lines.extend(agent_assignment_evidence(assignment));
            }
        }
        UiAgentStatus::Unassigned | UiAgentStatus::Retired => {}
    }
    lines
}

fn agent_status_label(status: &UiAgentStatus) -> String {
    match status {
        UiAgentStatus::Unassigned => "Unassigned".to_owned(),
        UiAgentStatus::Assigned(assignment) => format!(
            "Assigned to {} · {}",
            assignment.project_name,
            agent_assignment_phase_label(assignment.phase)
        ),
        UiAgentStatus::NeedsAttention { reason, .. } => format!(
            "Needs attention · {}",
            match reason {
                UiAgentAttentionReason::IdentityConflict => "saved names disagree",
                UiAgentAttentionReason::AssignmentConflict => "assigned to more than one project",
                UiAgentAttentionReason::AssignmentBlocked => "project setup is blocked",
            }
        ),
        UiAgentStatus::Retired => "Retired".to_owned(),
    }
}

fn agent_assignment_phase_label(phase: UiAgentAssignmentPhase) -> &'static str {
    match phase {
        UiAgentAssignmentPhase::SettingUp => "setting up",
        UiAgentAssignmentPhase::Ready => "ready",
        UiAgentAssignmentPhase::Blocked => "blocked",
    }
}

fn agent_assignment_evidence(assignment: &UiAgentProjectAssignment) -> Vec<Line<'_>> {
    let mut lines = vec![
        Line::from(format!(
            "Project: {} ({})",
            assignment.project_name,
            short_identity(assignment.project_id)
        )),
        Line::from(format!(
            "Technical assignment: {} · service {} · conversation {}",
            short_identity(assignment.assignment_id),
            assignment.provider,
            assignment.session.as_deref().unwrap_or("session pending")
        )),
    ];
    if let Some(blocked) = &assignment.blocked {
        lines.push(Line::from(format!("Blocked: {blocked}")));
    }
    lines
}

#[allow(clippy::too_many_lines)]
fn render_mailbox_modal(frame: &mut Frame<'_>, model: &UiModel, available: Rect) {
    let Some(interaction) = model.mailbox_modal() else {
        return;
    };
    let width = available.width.saturating_sub(4).clamp(1, 76);
    let height = available.height.clamp(1, 18);
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
                "Choose who to message",
                Style::new().fg(Color::Cyan),
            )];
            if !targets.is_empty() {
                lines.push(Line::from(
                    "HQ lists agents—and, in the future, people—you can reach directly.",
                ));
            }
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
                    "No reachable recipients yet.",
                    Style::new().fg(Color::Yellow).bold(),
                ));
                lines.push(Line::from(
                    "Create an agent from the Agents section, then return here.",
                ));
                lines.push(Line::from(
                    "People in your HQ network can appear here when they are reachable.",
                ));
            }
            lines.push(Line::default());
            lines.push(Line::from(if targets.is_empty() {
                "Esc close"
            } else {
                "↑/↓ select · Enter compose · Esc cancel"
            }));
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
                    .block(Block::bordered().title(" Preparing message ")),
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
                "sending"
            } else if *dirty {
                "saving"
            } else {
                "saved"
            };
            let text_area = area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 2,
            });
            frame.render_widget(
                Block::bordered()
                    .border_style(Style::new().fg(Color::Cyan))
                    .title(format!(
                        " {} · {status} · Message required · {}/{} bytes ",
                        draft_target_label(&draft.target),
                        draft.content.len(),
                        MAX_DRAFT_BYTES
                    )),
                area,
            );
            let cursor = model.message_field_cursor(&draft.content);
            let mut content = draft.content.clone();
            content.insert(cursor, '│');
            frame.render_widget(
                Paragraph::new(content).wrap(Wrap { trim: false }),
                text_area,
            );
            let hint = Rect {
                y: area.y + area.height.saturating_sub(2),
                height: 1,
                x: area.x + 2,
                width: area.width.saturating_sub(4),
            };
            let hint_text = model
                .message_field_error()
                .unwrap_or("Enter submit · Esc save and close");
            frame.render_widget(
                Paragraph::new(hint_text).style(if model.message_field_error().is_some() {
                    Style::new().fg(Color::Red)
                } else {
                    Style::new()
                }),
                hint,
            );
        }
        UiMailboxModal::Confirm { action } => {
            let (label, explanation) = match action {
                UiMailboxAction::Archive { .. } => (
                    "Archive the selected message?",
                    Some("Only this message changes state. The conversation history stays intact."),
                ),
                UiMailboxAction::Restore { .. } => (
                    "Restore the selected message?",
                    Some(
                        "Only this message returns to open views; the rest of the conversation is unchanged.",
                    ),
                ),
                UiMailboxAction::Reply { .. } | UiMailboxAction::Direct { .. } => (
                    "Send this message?",
                    Some("The recipient will see it in their HQ conversation."),
                ),
                UiMailboxAction::SelfNote => (
                    "Save this personal note?",
                    Some("Only you will receive this note."),
                ),
            };
            let mut lines = vec![Line::from(label), Line::default()];
            if let Some(explanation) = explanation {
                lines.push(Line::from(explanation));
                lines.push(Line::default());
            }
            lines.push(Line::from("Enter confirm · Esc cancel"));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::bordered().title(" Confirm message change "))
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
    let status = if model.refreshing() {
        "Updating…"
    } else {
        workspace_connection_label(model.connection())
    };
    let title = Line::from(vec![
        Span::styled(" HQ ", Style::new().fg(Color::Black).bg(Color::Cyan).bold()),
        Span::raw("  "),
        Span::styled(section_label(model.section()), Style::new().bold()),
    ]);
    let context = Line::from(vec![
        Span::styled(" this device ", Style::new().fg(Color::DarkGray)),
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
            format!(
                " {} · {count} {}",
                section_label(model.section()),
                section_item_label(model.section(), count)
            ),
            Style::new().fg(Color::Cyan).bold(),
        ),
        Line::default(),
    ];
    match model.human_state() {
        Some(UiHumanState::NeedsAttention(issue)) => {
            lines.extend(human_issue_lines(issue));
            lines.push(Line::default());
        }
        Some(UiHumanState::Ready) | None => {}
    }
    match model.rows() {
        Some([]) => lines.extend(empty_section_lines(model.section())),
        Some(rows) => {
            for row in rows {
                lines.extend(render_row(model, row));
            }
        }
        None => lines.push(Line::styled(
            " Loading your workspace…",
            Style::new().fg(Color::Yellow),
        )),
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn human_issue_lines(issue: &UiHumanIssue) -> Vec<Line<'static>> {
    let heading = Style::new().fg(Color::Yellow).bold();
    match issue {
        UiHumanIssue::NoAccountSelected => vec![
            Line::styled(" No human account is selected on this device.", heading),
            Line::from(" Create one: hq human create"),
            Line::from(" Join one: hq human join ABSOLUTE_INVITATION_PATH"),
        ],
        UiHumanIssue::SelectionCandidates { .. } => vec![
            Line::styled(
                " The human account selection is unresolved; HQ will not guess.",
                heading,
            ),
            Line::from(" Choose one: hq human select ACCOUNT_ID"),
            Line::from(" Press ? then t for the candidate account IDs."),
        ],
        UiHumanIssue::SelectionRecords { .. } => vec![
            Line::styled(
                " HQ found conflicting account choices on this device.",
                heading,
            ),
            Line::from(" Try restoring shared records: hq relay sync · hq relay repair"),
            Line::from(" Press ? then t for the conflicting records."),
        ],
        UiHumanIssue::SelectedWithoutAuthority { .. } => vec![
            Line::styled(
                " This device is not allowed to use the selected account.",
                heading,
            ),
            Line::from(" Try restoring shared records: hq relay sync · hq relay repair"),
            Line::from(" New device? Rejoin with: hq human join ABSOLUTE_INVITATION_PATH"),
        ],
        UiHumanIssue::MembershipPending(_) => vec![
            Line::styled(
                " This device has not finished joining the account.",
                heading,
            ),
            Line::from(" Finish joining: hq human join ABSOLUTE_INVITATION_PATH"),
        ],
        UiHumanIssue::MembershipRevoked(_) => vec![
            Line::styled(
                " This device was removed from the selected account.",
                heading,
            ),
            Line::from(" Ask the account owner for a new invitation, then run hq human join."),
        ],
        UiHumanIssue::MembershipAuthorityConflict { .. } => vec![
            Line::styled(
                " HQ found conflicting records for this device's account access.",
                heading,
            ),
            Line::from(" Try restoring shared records: hq relay sync · hq relay repair"),
            Line::from(" Press ? then t for the exact membership evidence."),
        ],
    }
}

fn empty_section_lines(section: UiSection) -> Vec<Line<'static>> {
    let heading = Style::new().fg(Color::Cyan).bold();
    match section {
        UiSection::Inbox => vec![
            Line::styled(" No conversations need your attention.", heading),
            Line::from(" Start one now: d message · n note"),
        ],
        UiSection::Sent => vec![
            Line::styled(
                " You have not started or replied to a conversation.",
                heading,
            ),
            Line::from(" Start one now: d message · n note"),
        ],
        UiSection::Archived => vec![
            Line::styled(" You have not put any conversations away.", heading),
            Line::from(" Browse Inbox or Sent to find an active conversation."),
        ],
        UiSection::Agents => vec![
            Line::styled(" No named workers yet.", heading),
            Line::from(" Press c create an agent you can assign and contact."),
        ],
        UiSection::Projects => vec![
            Line::styled(" No projects yet.", heading),
            Line::from(" A project records work and ownership of its folders and resources."),
            Line::from(" Press c create a project and choose its first folder."),
        ],
    }
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
        UiConversationEntryKind::Activity => "update",
    };
    let state = match entry.message_state {
        Some(UiMessageState::Open) => "open",
        Some(UiMessageState::Archived) => "archived",
        Some(UiMessageState::Rejected) => "rejected",
        None => "information only",
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
    let detail = if row.kind == UiRowKind::Agent {
        Line::from(vec![
            Span::raw("     "),
            Span::styled(row.detail.as_str(), row_state_style(row.state)),
        ])
    } else {
        Line::from(vec![
            Span::raw("     "),
            Span::styled(
                row_state_label(model.section(), row.state),
                row_state_style(row.state),
            ),
            Span::raw(" · "),
            Span::styled(row.detail.as_str(), Style::new().fg(Color::DarkGray)),
        ])
    };
    [
        Line::from(vec![
            Span::styled(marker, title_style),
            Span::styled(row.title.as_str(), title_style),
        ]),
        detail,
    ]
}

fn render_footer(frame: &mut Frame<'_>, model: &UiModel, area: Rect) {
    let content = if let Some(page) = model.help_page() {
        match page {
            UiHelpPage::Context => " t technical details · ?/Esc close help".to_owned(),
            UiHelpPage::Technical => " t contextual help · ?/Esc close help".to_owned(),
        }
    } else if let Some(failure) = model.last_failure() {
        format!(
            " Could not complete that action · {} · ? details",
            failure.action
        )
    } else if let Some(hint) = model.transient_help() {
        format!(" Hint · {hint}")
    } else if model.focus() == UiFocus::Navigation && model.viewport().width >= WIDE_WIDTH {
        " ↑/↓ section · Enter content · ? help · q quit".to_owned()
    } else if model.focus() == UiFocus::Navigation {
        " ←/→ section · Enter content · ? help · q quit".to_owned()
    } else if matches!(model.section(), UiSection::Agents | UiSection::Projects) {
        if model.selected_row_data().is_some() {
            " Enter inspect · c create · / search · ? help · q quit".to_owned()
        } else {
            " c create · / search · ? help · q quit".to_owned()
        }
    } else if model.focus() == UiFocus::Conversation {
        conversation_footer(model)
    } else if model.selected_row_data().is_none() {
        " d message · n note · ? help · q quit".to_owned()
    } else if model.viewport().width >= WIDE_WIDTH {
        " Enter open · d message · n note · ? help · q quit".to_owned()
    } else {
        " Enter open · ? help · q quit".to_owned()
    };
    let style = if model.last_failure().is_some() && model.help_page().is_none() {
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

fn conversation_footer(model: &UiModel) -> String {
    let selected = model.conversation().and_then(|conversation| {
        let anchor = model.conversation_anchor()?;
        conversation.entries.iter().find(|entry| entry.id == anchor)
    });
    let mut controls = vec![if model.viewport().width >= WIDE_WIDTH {
        "↑/↓ message"
    } else {
        "↑/↓ msg"
    }];
    if selected
        .and_then(|entry| entry.message_target)
        .is_some_and(|target| target.reply_allowed)
    {
        controls.push("r reply");
    }
    match selected.and_then(|entry| entry.message_target.zip(entry.message_state)) {
        Some((_, UiMessageState::Open)) => controls.push("a archive"),
        Some((_, UiMessageState::Archived)) => controls.push("u restore"),
        Some((_, UiMessageState::Rejected)) | None => {}
    }
    controls.push(if model.viewport().width >= WIDE_WIDTH {
        "Enter details"
    } else {
        "Enter info"
    });
    controls.push(if model.viewport().width >= WIDE_WIDTH {
        "Esc close"
    } else {
        "Esc back"
    });
    controls.push("? help");
    format!(" {}", controls.join(" · "))
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

const fn workspace_connection_label(state: UiConnectionState) -> &'static str {
    match state {
        UiConnectionState::Disconnected => "Offline",
        UiConnectionState::Connecting => "Connecting…",
        UiConnectionState::Ready => "Connected",
        UiConnectionState::Reconnecting => "Reconnecting…",
        UiConnectionState::Incompatible => "Update required",
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

const fn row_state_label(section: UiSection, state: UiRowState) -> &'static str {
    match (section, state) {
        (UiSection::Sent, UiRowState::Waiting) => "sent",
        (_, UiRowState::Open) => "open",
        (_, UiRowState::Waiting) => "waiting",
        (_, UiRowState::Archived) => "archived",
        (_, UiRowState::Attention) => "needs attention",
    }
}

const fn section_item_label(section: UiSection, count: usize) -> &'static str {
    match (section, count) {
        (UiSection::Inbox | UiSection::Sent | UiSection::Archived, 1) => "conversation",
        (UiSection::Inbox | UiSection::Sent | UiSection::Archived, _) => "conversations",
        (UiSection::Agents, 1) => "agent",
        (UiSection::Agents, _) => "agents",
        (UiSection::Projects, 1) => "project",
        (UiSection::Projects, _) => "projects",
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
